#!/usr/bin/env bash
# Generates the real transport streams used by the streamium-mpegts
# integration tests. They are built with ffmpeg rather than committed, so the
# repository stays small and the fixtures stay reproducible.
#
#   ./scripts/make-test-streams.sh [output_dir]
#
# Default output: crates/streamium-mpegts/tests/fixtures (git-ignored).
# The tests skip themselves when the directory is absent, so ffmpeg is only
# needed to exercise the real-stream suite.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/crates/streamium-mpegts/tests/fixtures}"

if ! command -v ffmpeg >/dev/null; then
  echo "ffmpeg is required (apt-get install ffmpeg / brew install ffmpeg)" >&2
  exit 1
fi

mkdir -p "$OUT"
DURATION="${DURATION:-3}"
SIZE="${SIZE:-320x240}"
FPS="${FPS:-25}"
# One key frame per second, so a mid-stream join finds one quickly.
GOP=$FPS

echo "==> H.264 + AAC (the common IPTV combination)"
ffmpeg -v error -y \
  -f lavfi -i "testsrc2=size=$SIZE:rate=$FPS:duration=$DURATION" \
  -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=$DURATION" \
  -c:v libx264 -preset ultrafast -g $GOP -pix_fmt yuv420p -b:v 300k \
  -c:a aac -b:a 64k -ar 48000 -ac 2 \
  -muxrate 1500k -f mpegts "$OUT/h264_aac.ts"

echo "==> HEVC + AC-3"
ffmpeg -v error -y \
  -f lavfi -i "testsrc2=size=$SIZE:rate=$FPS:duration=$DURATION" \
  -f lavfi -i "sine=frequency=660:sample_rate=48000:duration=$DURATION" \
  -c:v libx265 -preset ultrafast -x265-params "log-level=error:keyint=$GOP:min-keyint=$GOP" \
  -pix_fmt yuv420p -b:v 300k \
  -c:a ac3 -b:a 192k -ar 48000 -ac 2 \
  -f mpegts "$OUT/hevc_ac3.ts"

echo "==> MPEG-2 video + MP2 audio (older DVB and satellite feeds)"
ffmpeg -v error -y \
  -f lavfi -i "testsrc2=size=$SIZE:rate=$FPS:duration=$DURATION" \
  -f lavfi -i "sine=frequency=880:sample_rate=48000:duration=$DURATION" \
  -c:v mpeg2video -g $GOP -b:v 600k \
  -c:a mp2 -b:a 192k -ar 48000 -ac 2 \
  -f mpegts "$OUT/mpeg2_mp2.ts"

echo "==> multi-program stream (two services in one mux, as a DVB transponder carries)"
ffmpeg -v error -y \
  -f lavfi -i "testsrc2=size=$SIZE:rate=$FPS:duration=$DURATION" \
  -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=$DURATION" \
  -f lavfi -i "testsrc2=size=$SIZE:rate=$FPS:duration=$DURATION" \
  -f lavfi -i "sine=frequency=880:sample_rate=48000:duration=$DURATION" \
  -map 0:v -map 1:a -map 2:v -map 3:a \
  -c:v libx264 -preset ultrafast -g $GOP -pix_fmt yuv420p -b:v 200k \
  -c:a aac -b:a 64k -ar 48000 -ac 2 \
  -program "title=One:program_num=1:st=0:st=1" \
  -program "title=Two:program_num=2:st=2:st=3" \
  -f mpegts "$OUT/multiprogram.ts" 2>/dev/null || {
    echo "   (multi-program generation unsupported by this ffmpeg; tests skip it)"
    rm -f "$OUT/multiprogram.ts"
  }

# Elementary streams, demuxed by ffmpeg itself. These are independent of our
# demuxer, so the Swift bitstream parsers are checked against a real encoder's
# output rather than against our own code.
echo "==> elementary streams for the bitstream parsers"
ffmpeg -v error -y -i "$OUT/h264_aac.ts"  -map 0:a -c copy -f adts "$OUT/audio_aac.adts"
ffmpeg -v error -y -i "$OUT/hevc_ac3.ts"  -map 0:a -c copy -f ac3  "$OUT/audio_ac3.ac3"
ffmpeg -v error -y -i "$OUT/mpeg2_mp2.ts" -map 0:a -c copy -f mp2  "$OUT/audio_mp2.mp2"
ffmpeg -v error -y -i "$OUT/h264_aac.ts"  -map 0:v -c copy -f h264 "$OUT/video_h264.h264"
ffmpeg -v error -y -i "$OUT/hevc_ac3.ts"  -map 0:v -c copy -f hevc "$OUT/video_hevc.hevc"

echo "==> surround and mono AC-3 (channel counts depend on bit fields whose position varies)"
ffmpeg -v error -y -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=2" \
  -af "pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0" \
  -c:a ac3 -b:a 448k -f ac3 "$OUT/audio_ac3_51.ac3"
ffmpeg -v error -y -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=2" \
  -c:a ac3 -b:a 192k -ac 1 -f ac3 "$OUT/audio_ac3_mono.ac3"

ls -la "$OUT"
echo "==> fixtures written to $OUT"
