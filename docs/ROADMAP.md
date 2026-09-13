# Roadmap

## Phase 0: foundation (this commit)

- Rust workspace: core (playlists, Xtream, XMLTV/EPG, catalog), MPEG-TS
  demuxer, UniFFI bridge, bindgen tool.
- Apple scaffolding: XCFramework build script, Swift package with the
  playback engines, SwiftUI app skeleton, XcodeGen project spec.
- CI: Rust checks on Linux, Apple build on macOS runners.

## Phase 1: iOS + macOS release candidate

- [x] Demuxer verified against real ffmpeg-produced streams: H.264/AAC,
      HEVC/AC-3, MPEG-2/MP2 and a two-service multiplex all demux to
      bit-identical decoded frames and audio samples, and survive
      mid-broadcast tune-in, packet loss, garbage and truncation.
- [x] Swift bitstream parsers checked against the same real elementary
      streams (frame counts, sample rates, channel counts including 5.1).
- [ ] First Xcode build pass over the Swift code; fix compile errors.
      Needs macOS; nothing in this repository has compiled Swift yet.
- [ ] TransportStreamEngine end-to-end on a device, using the generated
      streams served over HTTP.
- [ ] AVPlayerEngine with headers, PiP, AirPlay, background audio.
- [ ] Sources: M3U (remote/local), Xtream; Keychain storage; refresh on
      launch and on a schedule.
- [ ] Channel list with groups, search, favourites, channel numbers; guide
      now/next in the list; full grid view.
- [ ] Catch-up for Xtream (`timeshift.php`) and playlist `catchup=` schemes.
- [ ] Personal library: Files/iCloud/document picker, resume positions.
- [ ] Legal documents, first-run notice, App Review demo source.
- [ ] Performance targets measured: cold start < 2 s, TS channel change < 1 s.

## Phase 2: depth

- [ ] SQLite persistence in the core (rusqlite bundled) for large catalogs
      and seven-day guides.
- [ ] Series browsing for Xtream (seasons, episodes, watched state).
- [ ] Jellyfin/Emby client; HDHomeRun discovery and lineup; Tvheadend.
- [ ] Internet radio directory and podcast subscriptions.
- [ ] Parental PIN, profiles, iCloud sync of settings via the user's account.
- [ ] tvOS target (same Swift package, Siri Remote navigation).

## Phase 3: fallback engine

- [ ] Optional FFmpeg-based engine (dynamic LGPL framework) for MKV/AVI,
      DTS, RTSP and UDP multicast, with VideoToolbox hwaccel.
- [ ] Subtitle rendering: DVB bitmap, teletext, SRT/WebVTT sidecars.
- [ ] SMB/WebDAV browsing.

## Phase 4: every platform

- [ ] Android (Jetpack Compose, ExoPlayer + MediaCodec path from the
      shared demuxer).
- [ ] Windows and Linux shells in Rust (Slint or egui), Media Foundation and
      VA-API decoders, same core crates linked directly.
- [ ] Shared UI test fixtures: the same playlist and EPG samples drive UI
      tests on all platforms.
