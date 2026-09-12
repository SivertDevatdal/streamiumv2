# Streamium

A fast, cross-platform media player for live TV, on-demand video, personal
media libraries and internet radio. One Rust core, native shells on every
platform, hardware decoding everywhere.

Streamium ships **no content**. Users add their own sources: playlists and
servers they are entitled to use, files they own, media servers and tuners
they run. See [docs/COMPLIANCE.md](docs/COMPLIANCE.md) for the product rules
that keep it a general-purpose player.

## Status

| Layer | State |
|---|---|
| `crates/streamium-core` (playlists, Xtream, XMLTV/EPG, catalog) | Implemented, unit-tested |
| `crates/streamium-mpegts` (transport-stream demuxer) | Implemented, tested with a synthetic muxer |
| `crates/streamium-ffi` (Swift/Kotlin bindings via UniFFI) | Implemented, Swift bindings generate |
| `apple/` (SwiftUI app for iOS, iPadOS, macOS + playback engines) | Scaffolded, needs an Xcode build pass |
| Android, Windows, Linux shells | Planned, see [docs/ROADMAP.md](docs/ROADMAP.md) |

## Repository layout

```
crates/streamium-core     sans-IO domain logic (no networking, no threads)
crates/streamium-mpegts   MPEG-TS demuxer feeding platform hardware decoders
crates/streamium-ffi      UniFFI surface: the one crate that crosses languages
crates/uniffi-bindgen     binary that generates Swift/Kotlin bindings
apple/StreamiumKit        Swift package: generated bindings + playback engines
apple/Streamium           SwiftUI application (iOS/iPadOS/macOS targets)
apple/scripts             build-xcframework.sh
docs/                     architecture, playback, compliance, roadmap, building
```

## Quick start

```sh
# Core: build and test on any OS
cargo test --workspace

# Apple: produce the XCFramework + Swift bindings, then open the project
./apple/scripts/build-xcframework.sh
cd apple && xcodegen generate && open Streamium.xcodeproj
```

Full instructions: [docs/BUILDING.md](docs/BUILDING.md).

## Documents

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): why Rust and not assembly, the layering, threading and data flow.
- [docs/PLAYBACK.md](docs/PLAYBACK.md): engine selection, the transport-stream pipeline, latency strategy.
- [docs/COMPLIANCE.md](docs/COMPLIANCE.md): App Store and legal posture, the legitimate feature set.
- [docs/ROADMAP.md](docs/ROADMAP.md): phased plan from iOS/macOS to every platform.
