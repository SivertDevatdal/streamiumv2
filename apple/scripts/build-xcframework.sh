#!/usr/bin/env bash
# Builds the Rust FFI crate for every Apple slice, generates the Swift
# bindings, and assembles StreamiumCoreFFI.xcframework for the Swift package.
#
#   PROFILE=debug ./apple/scripts/build-xcframework.sh     # faster local builds
#   SKIP_TARGETS="x86_64-apple-ios x86_64-apple-darwin" ... # skip slices
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KIT="$ROOT/apple/StreamiumKit"
BUILD="$ROOT/apple/build"
PROFILE="${PROFILE:-release}"
SKIP_TARGETS="${SKIP_TARGETS:-}"
LIB_NAME="libstreamium.a"
CARGO_PROFILE_FLAG=""
[[ "$PROFILE" == "release" ]] && CARGO_PROFILE_FLAG="--release"

export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

IOS_DEVICE=(aarch64-apple-ios)
IOS_SIM=(aarch64-apple-ios-sim x86_64-apple-ios)
MACOS=(aarch64-apple-darwin x86_64-apple-darwin)

skip() { [[ " $SKIP_TARGETS " == *" $1 "* ]]; }

build_target() {
  local t="$1"
  if skip "$t"; then echo "skip $t"; return; fi
  echo "==> cargo build ($PROFILE) for $t"
  rustup target add "$t" >/dev/null
  (cd "$ROOT" && cargo build -p streamium-ffi --lib $CARGO_PROFILE_FLAG --target "$t")
}

for t in "${IOS_DEVICE[@]}" "${IOS_SIM[@]}" "${MACOS[@]}"; do build_target "$t"; done

# ---- bindings -------------------------------------------------------------
HOST_TARGET="aarch64-apple-darwin"
[[ "$(uname -m)" == "x86_64" ]] && HOST_TARGET="x86_64-apple-darwin"
skip "$HOST_TARGET" && { echo "host target $HOST_TARGET must not be skipped"; exit 1; }
HOST_LIB="$ROOT/target/$HOST_TARGET/$PROFILE/libstreamium.dylib"

rm -rf "$BUILD"
mkdir -p "$BUILD/bindings" "$BUILD/include" "$BUILD/sim" "$BUILD/mac"
echo "==> generating Swift bindings from $HOST_LIB"
(cd "$ROOT" && cargo run -q -p uniffi-bindgen -- generate --library "$HOST_LIB" --language swift --out-dir "$BUILD/bindings")

cp "$BUILD/bindings/StreamiumCoreFFI.h" "$BUILD/include/"
cp "$BUILD/bindings/StreamiumCoreFFI.modulemap" "$BUILD/include/module.modulemap"

# ---- fat slices -----------------------------------------------------------
lipo_merge() {
  local out="$1"; shift
  local inputs=()
  for t in "$@"; do
    skip "$t" || inputs+=("$ROOT/target/$t/$PROFILE/$LIB_NAME")
  done
  if [[ ${#inputs[@]} -eq 1 ]]; then cp "${inputs[0]}" "$out"; else lipo -create "${inputs[@]}" -output "$out"; fi
}
lipo_merge "$BUILD/sim/$LIB_NAME" "${IOS_SIM[@]}"
lipo_merge "$BUILD/mac/$LIB_NAME" "${MACOS[@]}"

# ---- xcframework ----------------------------------------------------------
XC_ARGS=()
if ! skip "${IOS_DEVICE[0]}"; then
  XC_ARGS+=(-library "$ROOT/target/${IOS_DEVICE[0]}/$PROFILE/$LIB_NAME" -headers "$BUILD/include")
fi
XC_ARGS+=(-library "$BUILD/sim/$LIB_NAME" -headers "$BUILD/include")
XC_ARGS+=(-library "$BUILD/mac/$LIB_NAME" -headers "$BUILD/include")

FRAMEWORK_DIR="$KIT/Frameworks"
mkdir -p "$FRAMEWORK_DIR"
rm -rf "$FRAMEWORK_DIR/StreamiumCoreFFI.xcframework"
echo "==> creating xcframework"
xcodebuild -create-xcframework "${XC_ARGS[@]}" -output "$FRAMEWORK_DIR/StreamiumCoreFFI.xcframework"

mkdir -p "$KIT/Sources/StreamiumCore"
cp "$BUILD/bindings/StreamiumCore.swift" "$KIT/Sources/StreamiumCore/StreamiumCore.swift"

echo "==> done"
echo "    $FRAMEWORK_DIR/StreamiumCoreFFI.xcframework"
echo "    $KIT/Sources/StreamiumCore/StreamiumCore.swift"
