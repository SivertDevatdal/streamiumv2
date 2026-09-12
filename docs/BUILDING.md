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
