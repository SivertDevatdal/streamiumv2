#!/usr/bin/env bash
# One command to go from a fresh clone to a running app on a Mac.
#
#   ./apple/scripts/bootstrap-mac.sh            # build everything, open Xcode
#   ./apple/scripts/bootstrap-mac.sh --run      # build and launch the Mac app
#   ./apple/scripts/bootstrap-mac.sh --mac-only # skip the iOS slices (fastest)
#
# It installs nothing without telling you first.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APPLE="$ROOT/apple"
RUN_APP=0
OPEN_XCODE=1

for arg in "$@"; do
  case "$arg" in
    --run) RUN_APP=1; OPEN_XCODE=0 ;;
    --no-open) OPEN_XCODE=0 ;;
    --mac-only) export SKIP_IOS=1 ;;
    --debug) export PROFILE=debug ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }
die() { printf '\n\033[31merror:\033[0m %s\n' "$1" >&2; exit 1; }

# ---- prerequisites --------------------------------------------------------
step "Checking prerequisites"
[ "$(uname -s)" = "Darwin" ] || die "This script must run on macOS."

command -v xcodebuild >/dev/null || die "Xcode is not installed. Install it from the App Store, then run: sudo xcode-select -s /Applications/Xcode.app"
if ! xcodebuild -version >/dev/null 2>&1; then
  die "xcodebuild is not usable. Open Xcode once to finish installation, then run: sudo xcode-select -s /Applications/Xcode.app"
fi
echo "    $(xcodebuild -version | head -1)"

if ! command -v cargo >/dev/null; then
  die "Rust is not installed. Install it with:
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  then open a new terminal and run this script again."
fi
echo "    $(cargo --version)"

if ! command -v xcodegen >/dev/null; then
  if command -v brew >/dev/null; then
    step "Installing XcodeGen (generates the Xcode project)"
    brew install xcodegen
  else
    die "XcodeGen is missing and Homebrew is not installed.
Install Homebrew from https://brew.sh then run: brew install xcodegen"
  fi
fi
echo "    xcodegen $(xcodegen --version 2>/dev/null | head -1)"

# ---- build ----------------------------------------------------------------
step "Building the Rust core and Swift bindings"
"$APPLE/scripts/build-xcframework.sh"

step "Generating Streamium.xcodeproj"
(cd "$APPLE" && xcodegen generate)

# ---- compile --------------------------------------------------------------
step "Building the macOS app"
mkdir -p "$APPLE/build"
set +e
xcodebuild \
  -project "$APPLE/Streamium.xcodeproj" \
  -scheme Streamium-macOS \
  -configuration Debug \
  -derivedDataPath "$APPLE/build/DerivedData" \
  CODE_SIGN_IDENTITY="-" \
  CODE_SIGNING_REQUIRED=NO \
  CODE_SIGNING_ALLOWED=YES \
  build 2>&1 | tee "$APPLE/build/xcodebuild.log" | \
  grep -E "error:|warning:|BUILD (SUCCEEDED|FAILED)" | head -60
STATUS=${PIPESTATUS[0]}
set -e

if [ "$STATUS" -ne 0 ]; then
  echo
  echo "The build failed. The errors above are the place to start;"
  echo "the full log is at apple/build/xcodebuild.log"
  echo
  echo "This is the first time these Swift sources have been compiled,"
  echo "so some errors are expected. Paste them back and they can be fixed."
  exit 1
fi

APP="$APPLE/build/DerivedData/Build/Products/Debug/Streamium.app"
step "Built $APP"

if [ "$RUN_APP" = "1" ]; then
  step "Launching Streamium"
  open "$APP"
elif [ "$OPEN_XCODE" = "1" ]; then
  step "Opening Xcode"
  echo "    Pick the Streamium-macOS scheme and press Run."
  open "$APPLE/Streamium.xcodeproj"
fi

cat <<'NEXT'

Next, inside the app:
  1. Go to Sources, press +, choose "Xtream account".
  2. Enter your server address, username and password, then Add.
     The address is the part before /player_api.php, for example
     http://example.com:8080 -- pasting the full player_api.php URL works too.
  3. Channels appear under Live TV. Tap one to play.
     The programme guide keeps downloading in the background.

If a channel does not start, long-press the player to show the diagnostics
overlay: it reports the detected format, time to first frame, buffer depth,
codecs and byte count.
NEXT
