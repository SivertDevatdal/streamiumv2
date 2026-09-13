#!/usr/bin/env bash
# Builds the Rust FFI crate for each Apple slice, generates the Swift
# bindings, and assembles StreamiumCoreFFI.xcframework for the Swift package.
#
#   ./apple/scripts/build-xcframework.sh
#
# Environment:
#   PROFILE=debug     unoptimised Rust, much faster to build (default release)
#   FULL=1            build both architectures for macOS and the simulator,
#                     which is what a distributable build needs. By default
#                     only this machine's architecture is built.
#   SKIP_IOS=1        macOS only, for the quickest possible first run.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KIT="$ROOT/apple/StreamiumKit"
BUILD="$ROOT/apple/build"
PROFILE="${PROFILE:-release}"
LIB_NAME="libstreamium.a"

CARGO_PROFILE_FLAG=""
[ "$PROFILE" = "release" ] && CARGO_PROFILE_FLAG="--release"

export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "This script needs macOS and Xcode." >&2
  exit 1
fi

# Match this machine unless a distributable build was asked for: building the
# other architecture doubles the time and is never needed to run locally.
if [ "$(uname -m)" = "arm64" ]; then
  MAC_TARGETS=(aarch64-apple-darwin)
  SIM_TARGETS=(aarch64-apple-ios-sim)
  HOST_TARGET=aarch64-apple-darwin
else
  MAC_TARGETS=(x86_64-apple-darwin)
  SIM_TARGETS=(x86_64-apple-ios)
  HOST_TARGET=x86_64-apple-darwin
fi
if [ "${FULL:-0}" = "1" ]; then
  MAC_TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
  SIM_TARGETS=(aarch64-apple-ios-sim x86_64-apple-ios)
fi
IOS_TARGETS=(aarch64-apple-ios)
if [ "${SKIP_IOS:-0}" = "1" ]; then
  SIM_TARGETS=()
  IOS_TARGETS=()
fi

ALL_TARGETS=("${MAC_TARGETS[@]}" ${SIM_TARGETS[@]+"${SIM_TARGETS[@]}"} ${IOS_TARGETS[@]+"${IOS_TARGETS[@]}"})

echo "==> building streamium-ffi ($PROFILE) for: ${ALL_TARGETS[*]}"
for target in "${ALL_TARGETS[@]}"; do
  rustup target add "$target" >/dev/null 2>&1 || true
  echo "    $target"
  (cd "$ROOT" && cargo build -p streamium-ffi --lib $CARGO_PROFILE_FLAG --target "$target")
done

rm -rf "$BUILD"
mkdir -p "$BUILD/bindings" "$BUILD/include"

# ---- Swift bindings -------------------------------------------------------
HOST_LIB="$ROOT/target/$HOST_TARGET/$PROFILE/libstreamium.dylib"
if [ ! -f "$HOST_LIB" ]; then
  echo "expected $HOST_LIB to exist" >&2
  exit 1
fi
echo "==> generating Swift bindings"
(cd "$ROOT" && cargo run -q -p uniffi-bindgen -- \
  generate --library "$HOST_LIB" --language swift --out-dir "$BUILD/bindings")

cp "$BUILD/bindings/StreamiumCoreFFI.h" "$BUILD/include/"
cp "$BUILD/bindings/StreamiumCoreFFI.modulemap" "$BUILD/include/module.modulemap"

# ---- fat slices -----------------------------------------------------------
merge() {
  local out="$1"; shift
  mkdir -p "$(dirname "$out")"
  local inputs=()
  local target
  for target in "$@"; do inputs+=("$ROOT/target/$target/$PROFILE/$LIB_NAME"); done
  if [ "${#inputs[@]}" -eq 1 ]; then
    cp "${inputs[0]}" "$out"
  else
    lipo -create "${inputs[@]}" -output "$out"
  fi
}

XC_ARGS=()
merge "$BUILD/mac/$LIB_NAME" "${MAC_TARGETS[@]}"
XC_ARGS+=(-library "$BUILD/mac/$LIB_NAME" -headers "$BUILD/include")

if [ "${#SIM_TARGETS[@]}" -gt 0 ]; then
  merge "$BUILD/sim/$LIB_NAME" "${SIM_TARGETS[@]}"
  XC_ARGS+=(-library "$BUILD/sim/$LIB_NAME" -headers "$BUILD/include")
fi
if [ "${#IOS_TARGETS[@]}" -gt 0 ]; then
  merge "$BUILD/ios/$LIB_NAME" "${IOS_TARGETS[@]}"
  XC_ARGS+=(-library "$BUILD/ios/$LIB_NAME" -headers "$BUILD/include")
fi

echo "==> creating xcframework"
mkdir -p "$KIT/Frameworks"
rm -rf "$KIT/Frameworks/StreamiumCoreFFI.xcframework"
xcodebuild -create-xcframework "${XC_ARGS[@]}" \
  -output "$KIT/Frameworks/StreamiumCoreFFI.xcframework" >/dev/null

mkdir -p "$KIT/Sources/StreamiumCore"
cp "$BUILD/bindings/StreamiumCore.swift" "$KIT/Sources/StreamiumCore/StreamiumCore.swift"

echo "==> done"
echo "    $KIT/Frameworks/StreamiumCoreFFI.xcframework"
echo "    $KIT/Sources/StreamiumCore/StreamiumCore.swift"
