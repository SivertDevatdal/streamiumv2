# Playback

## Engine selection

| Source format | Apple (iOS/macOS/tvOS) | Android | Windows / Linux |
|---|---|---|---|
| HLS (`.m3u8`) | AVPlayer (native, hardware, AirPlay, PiP) | ExoPlayer | Rust shell + platform decoder (phase 4) |
| Progressive MP4/MOV/M4A/MP3 | AVPlayer | ExoPlayer | same |
| MPEG-TS over HTTP (`.ts`, Xtream live) | **TransportStreamEngine** (Rust demuxer → VideoToolbox) | Rust demuxer → MediaCodec | Rust demuxer → MF / VA-API |
| MKV / AVI / WebM, DTS, exotic codecs | Fallback engine (phase 3) | ExoPlayer handles most | fallback engine |
| RTSP / UDP multicast | Fallback engine (phase 3) | fallback engine | fallback engine |
| DASH (`.mpd`) | Fallback engine (phase 3) | ExoPlayer | fallback engine |

The URL extension gives a first guess (`inferStreamFormat`). When it is
`unknown`, the host fetches the first kilobyte and sniffs: `0x47` every 188
bytes is a transport stream, `#EXTM3U` is HLS, `ftyp` is MP4. The probe
result picks the engine before the real connection is opened, so an engine
switch never costs a visible delay.

## Apple: TransportStreamEngine

This is the engine that makes Streamium faster than players built on
ffmpeg for the most common IPTV format.

```
URLSession (streaming data task, custom User-Agent/headers)
   │ Data chunks
   ▼
TransportStreamDemuxer.push(bytes:)        (Rust, on the demux queue)
   │ .programFound / .accessUnit / .pcr / .discontinuity
   ▼
BitstreamConverter (Swift)
   ├─ H.264/HEVC Annex B  → length-prefixed NAL units + CMVideoFormatDescription from SPS/PPS(/VPS)
   ├─ AAC ADTS            → raw AAC frames + AudioSpecificConfig magic cookie
   ├─ AC-3 / E-AC-3       → sync frames, passed through (CoreAudio decodes)
   └─ MPEG-1/2 Layer II   → frames, passed through
   │ CMSampleBuffer (PTS/DTS in 90 kHz)
   ▼
AVSampleBufferRenderSynchronizer
   ├─ AVSampleBufferDisplayLayer.sampleBufferRenderer   (VideoToolbox hardware decode)
   └─ AVSampleBufferAudioRenderer                       (CoreAudio, AirPlay-capable)
```

### Clocking

- The audio renderer is the master clock; the synchronizer's timebase is set
  from the first audio PTS. Video buffers are scheduled against it.
- PTS values are 33-bit and wrap every 26.5 hours; `PtsUnwrapper` extends
  them to 64 bits so timestamps stay monotonic.
- Live buffer target: start at 1.5 s of audio buffered, grow towards 4 s if
  rebuffers occur, shrink back after 60 s of stable playback. On a
  well-behaved source that is roughly one second less latency than HLS.

### Discontinuities

Continuity-counter gaps, PCR jumps larger than one second and PMT version
changes all produce `.discontinuity`. The engine flushes both renderers,
resets the timebase, and resumes on the next key frame. The demuxer
co-operates: it withholds video until an IDR/IRAP frame, both at the start of
a stream and again after any gap, because frames that reference data which
never arrived decode to visible artefacts. Channel changes reuse the same
path: tear down the data task, `resetTiming()`, open the next URL.

This behaviour is covered by tests against real streams. Cutting a run of
bytes out of a transport stream makes the demuxer report the gap and then
deliver only whole, decodable runs: ffmpeg decodes the result with no errors,
where an ungated demuxer yields frames the decoder has to discard.

### Why not decode in Rust?

We could call VideoToolbox from Rust through FFI. We do not, because the
Swift side must own the `CMSampleBuffer` lifetimes, AirPlay, Picture in
Picture and the audio session anyway, and those APIs are far more pleasant
from Swift. The split is: Rust does everything that is the same on every
platform; Swift does what is Apple-specific.

## Apple: AVPlayerEngine

HLS, MP4, audio-only streams and Apple's own features (AirPlay, PiP,
FairPlay, subtitle rendering, audio track selection) go through `AVPlayer`
with an `AVPlayerLayer`. Custom headers are passed through
`AVURLAssetHTTPHeaderFieldsKey`; the User-Agent through the same mechanism.

## Fallback engine (phase 3)

For containers and codecs the platform cannot decode (MKV with DTS, AVI,
RTSP cameras, UDP multicast), a separate engine built on FFmpeg's libavformat
and libavcodec, linked dynamically as an LGPL framework. It uses VideoToolbox
through FFmpeg's hwaccel where possible. It is optional at build time so the
default app binary stays lean and free of LGPL obligations until the feature
ships.

## Android

ExoPlayer (Media3) covers HLS, DASH and progressive with hardware decoding.
For transport streams the same Rust demuxer feeds `MediaCodec` directly
through a custom `Renderer`, giving Android the same low-latency path.

## Diagnostics

Every engine exposes `PlaybackStats`: time to first frame, buffered
duration, dropped frames, rebuffer count, current bitrate estimate, codec
names. A debug overlay (long-press on the player) shows them live.
