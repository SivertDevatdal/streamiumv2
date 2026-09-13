#!/usr/bin/env bash
# Verifies that a built app bundle actually allows plain HTTP. Many IPTV
# providers and nearly every home media server speak http://, and App
# Transport Security fails them at runtime with a message that does not say
# which Info.plist key is at fault. Checking after the build turns that into
# something visible at build time.
#
#   ./apple/scripts/check-app-plist.sh /path/to/Streamium.app
set -uo pipefail

APP="${1:-}"
PLIST="$APP/Contents/Info.plist"
[ -f "$PLIST" ] || { echo "no Info.plist at $PLIST" >&2; exit 1; }

read_key() { /usr/libexec/PlistBuddy -c "Print :NSAppTransportSecurity:$1" "$PLIST" 2>/dev/null; }

arbitrary="$(read_key NSAllowsArbitraryLoads)"
problems=0

if [ "$arbitrary" != "true" ]; then
  printf '\033[31mwarning:\033[0m NSAllowsArbitraryLoads is not set; providers on http:// will fail\n'
  problems=1
fi

# These silently disable NSAllowsArbitraryLoads on macOS 10.12+ and iOS 10+.
for key in NSAllowsArbitraryLoadsForMedia NSAllowsArbitraryLoadsInWebContent NSAllowsLocalNetworking; do
  if [ -n "$(read_key "$key")" ]; then
    printf '\033[31mwarning:\033[0m %s is present, which makes the system ignore NSAllowsArbitraryLoads\n' "$key"
    problems=1
  fi
done

if [ "$problems" -eq 0 ]; then
  echo "    App Transport Security: plain HTTP allowed"
fi
exit 0
