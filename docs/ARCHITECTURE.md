# Architecture

## Goals

1. **Fast.** Cold start to first frame under two seconds on a mid-range phone.
   Channel change under one second on MPEG-TS sources. No garbage collector
   pauses, no interpreted layers in the playback path.
2. **Smooth.** Every frame decoded by the platform's hardware decoder and
   presented by the platform's compositor. Audio-master clock, adaptive
   live buffer, clean recovery from discontinuities.
3. **One implementation.** The logic that parses playlists, talks to
   servers, indexes the guide and demuxes streams is written once and runs
   bit-for-bit identically on iOS, iPadOS, macOS, tvOS, Android, Windows and
   Linux.
4. **Native feel.** Each platform gets its own UI toolkit and its own
   conventions (SwiftUI on Apple, Jetpack Compose on Android, and so on).

## Why Rust, not assembly

The request was for assembly "or something close to it" so the code is
robust, quick and portable. Assembly gives the opposite of portability:
arm64 and x86-64 have different instruction sets, and every OS has its own
calling conventions, so an assembly program would need to be rewritten per
CPU and per platform, and it offers no protection against memory bugs.

What the request is really asking for is a language that compiles to native
machine code, has no runtime or garbage collector, and gives full control
over memory layout. Rust is that language:

- It compiles ahead of time to the same machine code a C or assembly
  programmer would target, with LLVM performing register allocation,
  vectorisation and inlining.
- It has no runtime. The core links as a plain static library into Swift,
  Kotlin or C++ hosts.
- The borrow checker eliminates the memory-safety bugs (use-after-free,
  data races, buffer overruns) that make hand-written low-level code
  fragile. That is where the robustness comes from.
- One `cargo build --target` per platform produces the identical library
  for every OS listed above.

Where hand-written assembly genuinely matters, in video and audio codecs,
we do not write it: the platform decoders (VideoToolbox, MediaCodec, Media
Foundation, VA-API) already run on dedicated silicon, which no software
decoder can match for speed or battery life.

## Layers

```
┌──────────────────────────────────────────────────────────────────────┐
│  UI shell (per platform)                                             │
│  SwiftUI · Jetpack Compose · WinUI/Slint · GTK/Slint                 │
├──────────────────────────────────────────────────────────────────────┤
│  Playback engines (per platform, thin)                               │
│  AVPlayer / AVSampleBuffer* · ExoPlayer / MediaCodec · MF · GStreamer│
├──────────────────────────────────────────────────────────────────────┤
│  streamium-ffi  (UniFFI: Swift, Kotlin; C ABI for everything else)   │
├──────────────────────────────────────────────────────────────────────┤
│  streamium-core (sans-IO)          │  streamium-mpegts                │
│  playlists · Xtream · XMLTV · EPG  │  TS → access units for decoders │
│  catalog · search · favourites     │                                  │
└──────────────────────────────────────────────────────────────────────┘
```

### The core is sans-IO

`streamium-core` never opens a socket, reads a file, or spawns a thread. The
host fetches bytes with its native HTTP stack (`URLSession`, `OkHttp`,
`WinHTTP`) and hands them to the core, which parses, indexes and answers
questions. Reasons:

- Native networking gives correct behaviour for proxies, VPN profiles,
  captive portals, background transfers and App Transport Security for free.
- The core has no TLS stack or async runtime to cross-compile, so the static
  library is small and links into anything.
- Every function is deterministic and unit-testable on any machine, which
  is why the whole core is tested on Linux in CI even though the first
  targets are Apple platforms.

### The demuxer feeds hardware decoders

Most IPTV live streams are raw MPEG transport streams over HTTP. Apple's and
Google's built-in players do not accept those, which is why every existing
IPTV app bundles ffmpeg or VLC and decodes in software or through a second
pipeline. `streamium-mpegts` instead splits the transport stream into
elementary-stream access units (H.264/HEVC frames, AAC/AC-3 audio frames)
with timestamps, and the host feeds those directly into the platform's
hardware decoder and renderer. No ffmpeg, no software decode, lowest
possible latency. See [PLAYBACK.md](PLAYBACK.md).

### One FFI crate

`streamium-ffi` is the only crate that knows about the language boundary. It
defines plain records (Swift structs, Kotlin data classes), a few objects
that own state (`ChannelCatalog`, `EpgGuide`, `TransportStreamDemuxer`,
`XtreamEndpoints`) and free functions. The objects are internally
synchronised, so the host may call them from any thread or queue. Errors are
a flat enum. Release builds abort on panic, so a bug in the core can never
unwind into Swift or Kotlin and corrupt state.

## Data flow examples

**Adding an M3U source**

1. UI collects the playlist URL and optional EPG URL.
2. Host downloads the playlist with `URLSession`.
3. `parsePlaylist(text:)` returns channels, EPG URLs found in the header and
   warnings for unparseable lines.
4. Host adds the channels to a `ChannelCatalog` and persists `toJson()`.
5. Host downloads each EPG URL (gzip or plain) and calls
   `EpgGuide.loadXmltv(bytes:)`; the guide merges them.
6. For each channel, `EpgGuide.resolve(tvgId:name:)` maps it to a guide
   channel so now/next can be shown in the list.

**Playing a transport-stream channel**

1. `inferStreamFormat(url:)` says `.mpegTs` (or `.unknown`, in which case the
   host sniffs the first bytes).
2. Host opens a streaming `URLSession` data task with the channel's
   User-Agent and headers.
3. Every received chunk goes to `TransportStreamDemuxer.push(bytes:)` on a
   dedicated serial queue.
4. `.programFound` tells the host which PIDs carry video and audio and in
   which codec. It creates the format descriptions and renderers.
5. `.accessUnit` events become `CMSampleBuffer`s enqueued into
   `AVSampleBufferDisplayLayer` and `AVSampleBufferAudioRenderer`, both
   driven by one `AVSampleBufferRenderSynchronizer`.
6. `.discontinuity` flushes the renderers and waits for the next key frame.

## Threading model

- UI thread: SwiftUI/Compose state only.
- Network delegate queue: receives bytes, passes them to the demux queue.
- Demux queue (one per player): runs the Rust demuxer and bitstream
  conversion, enqueues sample buffers. This is the only latency-critical
  CPU work and it is a few percent of one core.
- Indexing queue: playlist and EPG parsing for large sources (a 300 MB
  XMLTV file parses in a few seconds without blocking the UI).

The Rust objects are thread-safe; the host guarantees ordering by using
serial queues per player.

## Persistence

Phase 1 stores sources, favourites and the catalog as JSON produced by the
core, and the EPG as the downloaded XMLTV bytes (re-parsed on launch in the
background). Phase 2 moves the catalog and guide into SQLite inside the core
(via `rusqlite` with the bundled engine, which cross-compiles to every
target) so that seven-day guides for 10 000+ channels query instantly
without holding everything in RAM.

## Performance principles

- Hardware decode always. A software decoder is a last resort for codecs the
  platform lacks, and it lives in a separate, optional engine.
- Zero-copy where it matters. Bytes are copied once from the socket buffer
  into the demuxer and once into the `CMBlockBuffer`; sample data is never
  duplicated again.
- No allocation on the per-packet path: the demuxer reuses buffers per PID.
- Parse lazily. Channel lists render from the catalog immediately; logos,
  guide data and metadata arrive afterwards.
- Measure. Every engine reports time-to-first-frame, buffer depth, dropped
  frames and rebuffer count, shown in a debug overlay.
