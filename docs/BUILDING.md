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

> The Apple sources have not been compiled yet: this repository's CI is the
> first thing to build them. Expect to fix compile errors on the first run.


```sh
./apple/scripts/build-xcframework.sh     # Rust → StreamiumCoreFFI.xcframework + Swift bindings
cd apple
xcodegen generate                        # project.yml → Streamium.xcodeproj
open Streamium.xcodeproj
```

The script builds the FFI crate for device (arm64), simulator (arm64 and
x86-64) and macOS (arm64 and x86-64), merges the slices with `lipo`, wraps
them with `xcodebuild -create-xcframework`, and drops the generated
`StreamiumCore.swift` into the package. Both outputs are git-ignored; the
script is the source of truth.

Environment variables:

- `PROFILE=debug` builds unoptimised Rust for faster iteration
  (default `release`).
- `SKIP_TARGETS="x86_64-apple-ios x86_64-apple-darwin"` skips slices you do
  not need locally.

## Adding an FFI function

1. Implement the logic in `streamium-core` or `streamium-mpegts` with tests.
2. Expose it in `crates/streamium-ffi/src/lib.rs` with `#[uniffi::export]`.
3. `cargo test -p streamium-ffi`, then rerun `build-xcframework.sh`; the
   Swift side picks up the new symbol on the next build.
