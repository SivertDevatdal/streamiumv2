#!/usr/bin/env bash
# End-to-end proof that the demuxer's output is not merely well-formed but
# bit-for-bit correct: every elementary stream it extracts is decoded by
# ffmpeg and compared frame by frame against the same stream decoded straight
# from the original transport stream.
#
#   ./scripts/verify-demuxer.sh
#
# Requires ffmpeg and the fixtures from scripts/make-test-streams.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURES="$ROOT/crates/streamium-mpegts/tests/fixtures"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# macOS ships `md5`, Linux ships `md5sum`.
if ! command -v md5sum >/dev/null && command -v md5 >/dev/null; then
  md5sum() { md5 -q "$@" | sed 's/$/  -/'; }
fi

if ! command -v ffmpeg >/dev/null; then
  echo "ffmpeg is required" >&2
  exit 1
fi
if [ ! -f "$FIXTURES/h264_aac.ts" ]; then
  echo "fixtures missing; run scripts/make-test-streams.sh first" >&2
  exit 1
fi

cargo build --release -p streamium-mpegts --example tsdump
TSDUMP="$ROOT/target/release/examples/tsdump"

failures=0
check() {
  local name="$1" video_ext="$2"
  local ts="$FIXTURES/$name.ts"
  local out="$WORK/$name"
  mkdir -p "$out"

  echo "==> $name"
  # Feed the demuxer in small pieces, the way a socket delivers them.
  "$TSDUMP" "$ts" "$out" 1500 > "$out/report.txt"
  if grep -qE "^(discontinuities: [1-9]|sync losses: [1-9]|crc errors: [1-9])" "$out/report.txt"; then
    echo "   FAIL: clean stream reported errors"
    grep -E "^(discontinuities|sync losses|crc errors)" "$out/report.txt" | sed 's/^/     /'
    failures=$((failures + 1))
  fi

  # Pick the extracted streams by extension without relying on glob expansion
  # succeeding, which `set -e` would otherwise treat as fatal.
  local video="" audio="" f
  for f in "$out"/*; do
    case "$f" in
      *."$video_ext") [ -z "$video" ] && video="$f" ;;
      *.aac|*.ac3|*.mp2) [ -z "$audio" ] && audio="$f" ;;
    esac
  done
  if [ -z "$video" ] || [ -z "$audio" ]; then
    echo "   FAIL: demuxer produced no $( [ -z "$video" ] && echo video || echo audio ) stream"
    failures=$((failures + 1))
    return
  fi

  # Video: compare the md5 of every decoded frame.
  ffmpeg -v error -y -i "$ts"    -map 0:v -pix_fmt yuv420p -f framemd5 "$out/orig.md5"
  ffmpeg -v error -y -i "$video"          -pix_fmt yuv420p -f framemd5 "$out/demux.md5"
  local a b
  a="$(grep -v '^#' "$out/orig.md5"  | awk '{print $NF}')"
  b="$(grep -v '^#' "$out/demux.md5" | awk '{print $NF}')"
  if [ "$a" = "$b" ] && [ -n "$a" ]; then
    echo "   video: $(grep -vc '^#' "$out/demux.md5") frames, every frame identical"
  else
    echo "   FAIL: decoded video differs from the source"
    failures=$((failures + 1))
  fi

  # Audio: compare decoded PCM over the common prefix.
  ffmpeg -v error -y -i "$ts"    -map 0:a -f s16le -ar 48000 -ac 2 "$out/orig.pcm"
  ffmpeg -v error -y -i "$audio"          -f s16le -ar 48000 -ac 2 "$out/demux.pcm"
  local n
  n=$(( $(stat -c%s "$out/orig.pcm" 2>/dev/null || stat -f%z "$out/orig.pcm") ))
  local m
  m=$(( $(stat -c%s "$out/demux.pcm" 2>/dev/null || stat -f%z "$out/demux.pcm") ))
  [ "$m" -lt "$n" ] && n="$m"
  if [ "$n" -gt 0 ] \
     && [ "$(head -c "$n" "$out/orig.pcm" | md5sum | cut -d' ' -f1)" \
        = "$(head -c "$n" "$out/demux.pcm" | md5sum | cut -d' ' -f1)" ]; then
    echo "   audio: $n bytes of PCM identical"
  else
    echo "   FAIL: decoded audio differs from the source"
    failures=$((failures + 1))
  fi
}

check h264_aac  h264
check hevc_ac3  hevc
check mpeg2_mp2 m2v

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "all streams demuxed to bit-identical audio and video"
