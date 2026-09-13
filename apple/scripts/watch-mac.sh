#!/usr/bin/env bash
# Watches this branch and rebuilds the app whenever new commits are pushed.
# Leave it running in a terminal; it pulls, rebuilds only what changed, and
# relaunches the app.
#
#   ./apple/scripts/watch-mac.sh                 # watch and relaunch on change
#   ./apple/scripts/watch-mac.sh --once          # pull, build and run once
#   ./apple/scripts/watch-mac.sh --no-restart    # build, but leave the running app alone
#   INTERVAL=15 ./apple/scripts/watch-mac.sh     # check every 15s (default 30)
#
# Your sources and settings live outside the app bundle, so they survive a
# rebuild: you do not re-enter your provider details each time.
#
# Ctrl-C stops it. Local edits are never discarded: if the working tree is
# dirty the pull is skipped and it says so.
set -uo pipefail   # deliberately not -e: a failed build must not stop the watch

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APPLE="$ROOT/apple"
APP="$APPLE/build/DerivedData/Build/Products/Debug/Streamium.app"
INTERVAL="${INTERVAL:-30}"
ONCE=0
RESTART=1

for arg in "$@"; do
  case "$arg" in
    --once) ONCE=1 ;;
    --no-restart) RESTART=0 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

cd "$ROOT"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"

bold() { printf '\033[1m%s\033[0m\n' "$1"; }
red()  { printf '\033[31m%s\033[0m\n' "$1"; }
dim()  { printf '\033[2m%s\033[0m\n' "$1"; }

rebuild() {
  local changed="$1"

  if grep -qE '^(crates/|Cargo\.toml|Cargo\.lock|rust-toolchain\.toml)' <<<"$changed"; then
    bold "Rust changed, rebuilding the core and bindings"
    "$APPLE/scripts/build-xcframework.sh" || { red "core build failed"; return 1; }
  fi

  if grep -qE '^apple/project\.yml' <<<"$changed" || [ ! -d "$APPLE/Streamium.xcodeproj" ]; then
    bold "Project definition changed, regenerating Streamium.xcodeproj"
    (cd "$APPLE" && xcodegen generate) || { red "xcodegen failed"; return 1; }
  fi

  bold "Building the app"
  mkdir -p "$APPLE/build"
  xcodebuild \
    -project "$APPLE/Streamium.xcodeproj" \
    -scheme Streamium-macOS \
    -configuration Debug \
    -derivedDataPath "$APPLE/build/DerivedData" \
    CODE_SIGN_IDENTITY="-" \
    CODE_SIGNING_REQUIRED=NO \
    CODE_SIGNING_ALLOWED=YES \
    build > "$APPLE/build/xcodebuild.log" 2>&1
  local status=$?

  if [ "$status" -ne 0 ]; then
    red "Build failed. Errors:"
    grep -E "error:" "$APPLE/build/xcodebuild.log" | head -25
    dim "Full log: apple/build/xcodebuild.log"
    return 1
  fi

  if [ "$RESTART" = "1" ]; then
    if pgrep -f "Streamium.app/Contents/MacOS/Streamium" >/dev/null; then
      dim "Restarting Streamium"
      pkill -f "Streamium.app/Contents/MacOS/Streamium" 2>/dev/null
      sleep 1
    fi
    open "$APP"
  else
    dim "Built. The running app was left alone; quit and reopen to pick it up."
  fi
  bold "Up to date at $(git log --oneline -n 1)"
  return 0
}

pull_if_clean() {
  if [ -n "$(git status --porcelain)" ]; then
    red "Local changes present, skipping the pull so nothing is lost:"
    git status --short | head -10
    return 1
  fi
  git merge --ff-only "origin/$BRANCH" --quiet
}

# ---- first pass -----------------------------------------------------------
bold "Watching $BRANCH for updates (checking every ${INTERVAL}s)"
dim  "Ctrl-C to stop. Your sources and settings are kept across rebuilds."

git fetch --quiet origin "$BRANCH" 2>/dev/null
LOCAL="$(git rev-parse HEAD)"
REMOTE="$(git rev-parse "origin/$BRANCH" 2>/dev/null || echo "$LOCAL")"

if [ "$ONCE" = "1" ] || [ ! -d "$APP" ]; then
  CHANGED=""
  if [ "$LOCAL" != "$REMOTE" ]; then
    CHANGED="$(git diff --name-only "$LOCAL" "$REMOTE")"
    pull_if_clean && LOCAL="$(git rev-parse HEAD)"
  fi
  # Nothing built yet means everything is new.
  [ -d "$APP" ] || CHANGED="crates/ apple/project.yml"
  rebuild "$CHANGED"
  [ "$ONCE" = "1" ] && exit $?
fi

# ---- watch loop -----------------------------------------------------------
while true; do
  sleep "$INTERVAL"
  git fetch --quiet origin "$BRANCH" 2>/dev/null || continue
  REMOTE="$(git rev-parse "origin/$BRANCH" 2>/dev/null)" || continue
  LOCAL="$(git rev-parse HEAD)"
  [ "$LOCAL" = "$REMOTE" ] && continue

  echo
  bold "New commits:"
  git log --oneline "$LOCAL..$REMOTE" | sed 's/^/    /'
  CHANGED="$(git diff --name-only "$LOCAL" "$REMOTE")"
  pull_if_clean || continue
  rebuild "$CHANGED"
done
