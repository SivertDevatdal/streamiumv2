# Streamium

A cross-platform IPTV and media player. One Rust core shared by every
platform, native shells on top, hardware decoding everywhere.

## Layout

```
crates/streamium-core     sans-IO domain logic: M3U, Xtream, XMLTV/EPG, catalog
crates/streamium-mpegts   MPEG-TS demuxer feeding platform hardware decoders
crates/streamium-ffi      UniFFI surface: the only crate that crosses languages
crates/streamium-probe    diagnostic CLI for a real provider
crates/uniffi-bindgen     generates the Swift/Kotlin bindings
apple/StreamiumKit        Swift package: generated bindings + playback engines
apple/Streamium           SwiftUI app (iOS/iPadOS and macOS targets)
apple/scripts             bootstrap-mac.sh, build-xcframework.sh, watch-mac.sh
scripts/                  test fixtures, demuxer verification, mock provider
docs/                     architecture, playback, compliance, roadmap, building
```

## Rules that matter

- **The core is sans-IO.** `streamium-core` and `streamium-mpegts` never open a
  socket, touch the filesystem, or spawn a thread. The host fetches bytes and
  hands them in. Do not add an HTTP client or async runtime to them. The probe
  shells out to `curl` precisely to preserve this.
- **Ship no content.** No bundled playlists, servers, channels or guide data,
  and no discovery of third-party IPTV sources. Every source comes from user
  input. See `docs/COMPLIANCE.md` before adding anything that looks like a
  directory, search or recommendation of providers.
- **Hardware decode only.** The demuxer produces access units for
  VideoToolbox/MediaCodec. Do not add a software decoder to the default path.
- **Credentials stay on device.** Keychain only. Never log a source URL,
  password, channel name or programme title.

## Commands

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

./scripts/make-test-streams.sh     # real ffmpeg fixtures (gitignored)
./scripts/verify-demuxer.sh        # decode the demuxer output, compare frames
./scripts/mock-provider.py         # local stand-in for an IPTV service

./apple/scripts/bootstrap-mac.sh --run    # build and launch the Mac app
./apple/scripts/watch-mac.sh              # rebuild and relaunch on each push
cargo run -p streamium-probe -- xtream http://host:8080 USER PASS
```

Tests that need fixtures skip themselves when they are absent, so ffmpeg is
optional.

## State of things

The Rust side is tested: 58 tests, and the demuxer is verified against real
ffmpeg-produced streams down to bit-identical decoded frames and audio.

The Swift side compiles and the macOS app runs, but it is young. It has never
been exercised against a real provider. The playback engines in
`apple/StreamiumKit/Sources/StreamiumKit/Playback` are the least proven code
in the repository.

## Conventions

- Comments explain why, not what. Do not narrate the code.
- Errors surfaced to the user say what failed and what to do about it.
- No em-dashes in user-visible strings or docs.
