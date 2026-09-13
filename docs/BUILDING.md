# Building

## Prerequisites

- Rust stable (the `rust-toolchain.toml` pins it; rustup installs it).
- For Apple targets: macOS 14+, Xcode 15.4+, and
  `brew install xcodegen`. Optional: `brew install swift-format` for
  nicely formatted generated bindings.

## Core (any OS)

```sh
cargo test --workspace          # unit + integration tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Real-stream test fixtures

Some tests run against transport streams produced by a real encoder rather
than by hand. They are generated rather than committed, so the repository
stays small, and every test that needs them skips when they are absent.

```sh
./scripts/make-test-streams.sh   # requires ffmpeg; writes to
                                 # crates/streamium-mpegts/tests/fixtures
cargo test --workspace           # the real-stream suite now runs
./scripts/verify-demuxer.sh      # decode the demuxer's output and compare it
                                 # frame by frame with the original stream
```

`make-test-streams.sh` produces H.264/AAC, HEVC/AC-3 and MPEG-2/MP2 streams, a
two-service multiplex, and the matching elementary streams (including 5.1 and
mono AC-3) that the Swift bitstream tests read. Override the defaults with
`DURATION`, `SIZE` and `FPS`.

The Swift tests find the fixtures relative to the repository. Point them
somewhere else with `STREAMIUM_FIXTURES=/path/to/fixtures swift test`.

Debugging a stream by hand:

```sh
cargo run -p streamium-mpegts --example tsdump -- stream.ts out_dir 1500
```

It prints the programs, codecs, access unit counts and timing, and writes each
elementary stream to `out_dir` so an external decoder can check it.

## Generate bindings only (any OS)

```sh
cargo build -p streamium-ffi
cargo run -p uniffi-bindgen -- generate \
    --library target/debug/libstreamium.so \
    --language swift --out-dir out/swift
cargo run -p uniffi-bindgen -- generate \
    --library target/debug/libstreamium.so \
    --language kotlin --out-dir out/kotlin
```

On macOS the library is `libstreamium.dylib`.

## Apple app

> The Apple sources have not been compiled yet: whoever runs the bootstrap
> script first is the first to build them. Expect to fix compile errors.

One command does everything:

```sh
./apple/scripts/bootstrap-mac.sh          # build, generate, compile, open Xcode
./apple/scripts/bootstrap-mac.sh --run    # ... and launch the Mac app instead
./apple/scripts/bootstrap-mac.sh --mac-only --debug   # fastest iteration
```

It checks that Xcode, Rust and XcodeGen are present and says what to install if
not, builds the XCFramework, generates the project, and compiles the macOS
target. The full compiler output goes to `apple/build/xcodebuild.log`.

The individual steps, if you prefer to run them yourself:

```sh
./apple/scripts/build-xcframework.sh     # Rust → StreamiumCoreFFI.xcframework + Swift bindings
cd apple
xcodegen generate                        # project.yml → Streamium.xcodeproj
open Streamium.xcodeproj
```

`build-xcframework.sh` compiles the FFI crate for each slice, merges them with
`lipo`, wraps them with `xcodebuild -create-xcframework`, and drops the
generated `StreamiumCore.swift` into the package. Both outputs are git-ignored;
the script is the source of truth.

By default it builds only this machine's architecture, plus the iOS device and
the matching simulator, because building the other architecture doubles the
time and is never needed to run locally.

Environment variables:

- `PROFILE=debug` builds unoptimised Rust for faster iteration
  (default `release`).
- `FULL=1` builds both architectures for macOS and the simulator, which is what
  a distributable build needs.
- `SKIP_IOS=1` builds the macOS slice only.

## Staying up to date while developing

```sh
./apple/scripts/watch-mac.sh                 # watch, rebuild, relaunch
./apple/scripts/watch-mac.sh --once          # a single pull, build and run
./apple/scripts/watch-mac.sh --no-restart    # build without disturbing playback
INTERVAL=15 ./apple/scripts/watch-mac.sh     # poll more often (default 30s)
```

The watcher only does the expensive work when it has to: the Rust core and
bindings are rebuilt when anything under `crates/` or a manifest changes, the
Xcode project is regenerated only when `apple/project.yml` changes, and a
Swift-only change goes straight to `xcodebuild`.

A failed build does not stop the watch. It prints the compiler errors, keeps
the full log at `apple/build/xcodebuild.log`, and carries on waiting for the
next commit. If the working tree has local edits the pull is skipped rather
than risking them.

## Testing without a provider

`scripts/mock-provider.py` is a local stand-in for an IPTV service. It speaks
enough of the Xtream Codes protocol to exercise the whole app and serves real
generated video, so playback can actually be seen.

```sh
./scripts/mock-provider.py          # http://localhost:8080, needs ffmpeg
```

Then add a source in the app:

| Type | Value |
|---|---|
| Xtream account | server `http://localhost:8080`, any username and password |
| Playlist (M3U) | `http://localhost:8080/get.php?username=demo&password=demo` |

It provides three live channels, each a different test pattern and audio pitch
so a wrong stream is obvious, in both transport stream and HLS form, plus a
movie and a twelve hour guide. That covers both playback engines and both
source types.

This is also the demo source to give App Review, which needs a working account
that is not a third-party IPTV service. See
[COMPLIANCE.md](COMPLIANCE.md).

## Diagnosing a provider

`streamium-probe` runs the same core the app runs, against a real provider,
and reports what the app would see. It is the fastest way to tell whether a
problem is in the provider, the parsing or the player.

```sh
cargo run -p streamium-probe -- xtream http://host:8080 USERNAME PASSWORD
cargo run -p streamium-probe -- playlist http://host/list.m3u
cargo run -p streamium-probe -- stream http://host/live/u/p/101.ts 10
```

It reports every HTTP call with status and timing, the account state, category
and channel counts, how many channels match a guide entry, and then reads a
live channel and demuxes it, finishing with whether video and audio were both
recovered. Credentials are masked in the output, so a report is safe to paste
into a bug report.

Networking is delegated to `curl`, which keeps the core crates sans-IO.

## Connecting a provider

Streamium ships no sources. In the app, **Sources → +**:

- **Playlist (M3U)**: the playlist URL, and optionally an XMLTV guide URL. If
  the playlist advertises one in its `#EXTM3U` header, that is used
  automatically.
- **Xtream account**: the server address, username and password. The address is
  the part before `/player_api.php`; pasting the full URL also works.

After adding, the source row shows what the provider reported: account status,
connection count, expiry, and whether transport streams or HLS are in use. If
the account is provisioned for HLS only, Streamium switches to HLS by itself
rather than failing with an opaque error.

Live channels default to transport streams, which give the lowest latency and
use the demuxer in this repository. **Sources → Playback** switches to HLS if a
provider needs it; refresh the source afterwards so the URLs are rebuilt.

The channel list appears as soon as the playlist or provider listing is parsed.
Guides are downloaded afterwards, in the background, because a provider's XMLTV
can run to hundreds of megabytes and the channels must not wait for it.

## Adding an FFI function

1. Implement the logic in `streamium-core` or `streamium-mpegts` with tests.
2. Expose it in `crates/streamium-ffi/src/lib.rs` with `#[uniffi::export]`.
3. `cargo test -p streamium-ffi`, then rerun `build-xcframework.sh`; the
   Swift side picks up the new symbol on the next build.
