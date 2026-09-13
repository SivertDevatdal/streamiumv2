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
| `crates/streamium-mpegts` (transport-stream demuxer) | Implemented; verified against real streams down to identical decoded frames |
| `crates/streamium-ffi` (Swift/Kotlin bindings via UniFFI) | Implemented, Swift bindings generate |
| `apple/` (SwiftUI app for iOS, iPadOS, macOS + playback engines) | Written, tests included; **not yet compiled** (needs macOS) |
| `crates/streamium-desktop` (Windows/Linux shell) | Implemented and building on a Windows runner; playback is handed to mpv/VLC |
| Android shell | Planned, see [docs/ROADMAP.md](docs/ROADMAP.md) |

The demuxer is checked against transport streams produced by ffmpeg in three
codec combinations (H.264/AAC, HEVC/AC-3, MPEG-2/MP2). Every elementary
stream it extracts decodes to **bit-identical video frames and audio samples**
compared with decoding the original stream directly. It is also exercised for
mid-broadcast tune-in, packet loss, garbage prefixes, truncation and arbitrary
network chunk sizes. Run `./scripts/verify-demuxer.sh` to reproduce.

## Repository layout

```
crates/streamium-core     sans-IO domain logic (no networking, no threads)
crates/streamium-mpegts   MPEG-TS demuxer feeding platform hardware decoders
crates/streamium-ffi      UniFFI surface: the one crate that crosses languages
crates/uniffi-bindgen     binary that generates Swift/Kotlin bindings
crates/streamium-desktop  Windows/Linux shell (egui) linking the core directly
apple/StreamiumKit        Swift package: generated bindings + playback engines
apple/Streamium           SwiftUI application (iOS/iPadOS/macOS targets)
apple/scripts             build-xcframework.sh
docs/                     architecture, playback, compliance, roadmap, building
```

## Quick start

### Run it on a Mac

```sh
./apple/scripts/bootstrap-mac.sh --run
```

That builds the Rust core for this machine, generates the Swift bindings and
the XCFramework, creates the Xcode project, compiles the macOS app and
launches it. Add `--mac-only` to skip the iOS slices for the fastest first
build, or `--debug` for an unoptimised Rust build.

It needs Xcode, Rust (`rustup`) and XcodeGen; it checks for each and tells you
what to install. Nothing is installed without asking, apart from XcodeGen via
Homebrew if you have it.

Then, in the app: **Sources → + → Xtream account**, enter your server address,
username and password. Channels appear under **Live TV**; the guide keeps
downloading in the background. Long-press the player for a diagnostics overlay
showing the detected format, time to first frame, buffer depth and codecs.

### Run it on Windows or Linux

```sh
cargo run -p streamium-desktop --release
```

The desktop shell links the core crates directly — no bindings, no FFI. It
browses Xtream accounts, M3U playlists and local folders, downloads and
indexes the XMLTV guide, and hands playback to mpv or VLC. **Analyse stream**
runs `streamium-mpegts` over the first seconds of a channel and reports the
programs, codecs, key-frame latency and errors it found, which is how a
tester on another machine can tell us something useful about a stream we
cannot reach.

On Windows nothing else is required. On Linux, install the windowing headers
first: `libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev
libxrandr-dev libxi-dev libgl1-mesa-dev`.

A ready-made `streamium.exe` is attached to every CI run as the
`streamium-windows-x86_64` artifact. [docs/WINDOWS.md](docs/WINDOWS.md) is
written for someone testing that build.

### Everything else

```sh
# Core: build and test on any OS
cargo test --workspace

# Optional: generate real transport streams and prove the demuxer against them
./scripts/make-test-streams.sh     # needs ffmpeg
cargo test --workspace             # the real-stream suite now runs too
./scripts/verify-demuxer.sh        # decodes the output and compares frames

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
- [docs/WINDOWS.md](docs/WINDOWS.md): installing, testing and reporting on the Windows build.
