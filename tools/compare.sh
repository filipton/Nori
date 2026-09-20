#!/usr/bin/env bash
# Side-by-side cost of music apps on a real phone (USB or wireless adb). For each package you are
# asked to start the same track in that app; it is then measured with the screen off and with the
# player on screen. Besides tools/bench.sh numbers this records Android's own per-app battery
# estimate: the phone is told it is unplugged for the window so batterystats keeps counting.
#
#   tools/compare.sh [seconds=300] [packages...]
#   default packages: nori, Navic, Musly, Symfonium
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
secs=300
[[ "${1:-}" =~ ^[0-9]+$ ]] && { secs=$1; shift; }
pkgs=("$@"); [ ${#pkgs[@]} -eq 0 ] && pkgs=(dev.nori.music paige.navic com.devid.musly app.symfonik.music.player)
out="bench-$(date +%Y%m%d-%H%M).txt"
trap 'adb shell dumpsys battery reset >/dev/null' EXIT

for pkg in "${pkgs[@]}"; do
  adb shell pm path "$pkg" >/dev/null 2>&1 || { echo "skip $pkg (not installed)"; continue; }
  for p in "${pkgs[@]}"; do [ "$p" != "$pkg" ] && adb shell am force-stop "$p"; done
  for screen in off on; do
    echo; read -r -p ">> $pkg: start the test track, $([ $screen = on ] && echo 'leave the PLAYER SCREEN open' || echo 'any screen'), then press enter "
    uid=$(adb shell dumpsys package "$pkg" | grep -m1 -oE 'userId=[0-9]+' | cut -d= -f2 | tr -d '\r')
    adb shell dumpsys batterystats --reset >/dev/null; adb shell dumpsys battery unplug >/dev/null
    { echo "===== $pkg, screen $screen"; "$here/bench.sh" "$pkg" "$secs" "$screen"
      echo "battery estimate (mAh, whole uid, incl. the 15 s settle):"
      adb shell dumpsys batterystats "$pkg" | grep -E "^\s+(UID|Uid) u0a$((uid - 10000)):" | head -2
      adb shell dumpsys batterystats "$pkg" | grep -E "Computed drain|Screen:|Capacity:" | head -3
    } | tee -a "$out"
    adb shell dumpsys battery reset >/dev/null
  done
done
echo; echo "saved to $out"
