#!/usr/bin/env bash
# Frames of opening a page, and of scrolling it once open:
#   tools/open-bench.sh <package> "<label>" [times=10] [flings=0]
# Put the app on a list that shows <label> first: Library > Playlists for the thousand-song playlist
# tools/dev-server.sh seeds ("Nori Bench 1000"), an album grid, a home shelf. Each round taps it, waits
# for the page to settle (the slide, the songs arriving), reads the frames of that open alone, then, with
# [flings] > 0, flings the page down and back up that many times and reads those frames; then goes back
# and waits for the list to settle. Per round and in total: frames drawn, janky frames (a missed deadline
# on Android 12 and later) and the slowest percentiles gfxinfo keeps. Honours ANDROID_SERIAL.
#
# Judge a perf or release build (tools/apk.sh, ./gradlew :app:assemblePerf): a debug build composes
# several times slower and is janky anyway. The perf build's Performance page counts the same frames
# (JankStats) for the stretch the rounds run in.
set -uo pipefail
pkg=${1:?package}; label=${2:?label}; n=${3:-10}; flings=${4:-0}
here=$(dirname "$0")

read -r x y < <("$here/ui.sh" where "$label") || exit 1
# The screen's size, for flings in the middle of it whatever the phone.
read -r w h < <(adb shell wm size | tr -d '\r' | grep -oE '[0-9]+x[0-9]+' | tail -1 | tr x ' ')
fx=$((w / 2)); low=$((h * 4 / 5)); high=$((h / 4))

# One gfxinfo reading, from its summary (the first of each line): "frames janky deadline p90 p99".
frames() {
  adb shell dumpsys gfxinfo "$pkg" | tr -d '\r' | awk '
    /Total frames rendered:/ && t == "" { t = $NF }
    /^Janky frames:/ && j == "" { j = $3 }
    /Number Frame deadline missed:/ && d == "" { d = $NF }
    /^90th percentile:/ && p90 == "" { p90 = $NF }
    /^99th percentile:/ && p99 == "" { p99 = $NF }
    END { print t + 0, j + 0, d + 0, p90, p99 }'
}

sum_open=(0 0 0); sum_scroll=(0 0 0)
add() { local -n s=$1; s[0]=$((s[0] + $2)); s[1]=$((s[1] + $3)); s[2]=$((s[2] + $4)); }
printf '%-6s %-7s %7s %6s %9s %6s %6s\n' round what frames janky deadline p90 p99
for i in $(seq "$n"); do
  adb shell dumpsys gfxinfo "$pkg" reset >/dev/null
  adb shell input tap "$x" "$y"
  sleep 1.5 # the slide (200 ms), the answer, the songs arriving (320 ms), with room to spare
  read -r t j d p90 p99 < <(frames)
  printf '%-6s %-7s %7s %6s %9s %6s %6s\n' "$i" open "$t" "$j" "$d" "$p90" "$p99"
  add sum_open "$t" "$j" "$d"
  if [ "$flings" -gt 0 ]; then
    adb shell dumpsys gfxinfo "$pkg" reset >/dev/null
    for _ in $(seq "$flings"); do adb shell input swipe "$fx" "$low" "$fx" "$high" 120; sleep 0.7; done
    for _ in $(seq "$flings"); do adb shell input swipe "$fx" "$high" "$fx" "$low" 120; sleep 0.7; done
    read -r t j d p90 p99 < <(frames)
    printf '%-6s %-7s %7s %6s %9s %6s %6s\n' "$i" scroll "$t" "$j" "$d" "$p90" "$p99"
    add sum_scroll "$t" "$j" "$d"
  fi
  adb shell input keyevent BACK
  sleep 1.2 # the pop and the list settling before the next round is measured
done
printf '%-6s %-7s %7s %6s %9s\n' total open "${sum_open[0]}" "${sum_open[1]}" "${sum_open[2]}"
[ "$flings" -gt 0 ] && printf '%-6s %-7s %7s %6s %9s\n' total scroll "${sum_scroll[0]}" "${sum_scroll[1]}" "${sum_scroll[2]}"
exit 0
