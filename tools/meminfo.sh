#!/usr/bin/env bash
# One line of `dumpsys meminfo`'s App Summary, in MB, for the app as it is now:
#   tools/meminfo.sh <label> [package=dev.nori.music]
# label, total PSS, Java heap, native heap, code, stack, graphics, private other, system, the PSS of
# libnorimusic.so's mappings (debug builds only: read through run-as) and the thread count.
# Set ANDROID_SERIAL when more than one device is attached.
set -uo pipefail
pkg=${2:-dev.nori.music}
m=$(adb shell dumpsys meminfo "$pkg")
g() { echo "$m" | grep -E "^ *$1:" | head -1 | awk -F: '{print $2}' | awk '{printf "%.1f", $1/1024}'; }
tot=$(echo "$m" | grep -E "^ *TOTAL PSS:" | awk '{printf "%.1f", $3/1024}')
so=$(adb shell "run-as $pkg sh -c 'cat /proc/\$(pidof $pkg)/smaps'" 2>/dev/null |
  awk '/libnorimusic/{f=1;next} /^[0-9a-f]+-/{f=0} f&&/^Pss:/{s+=$2} END{printf "%.1f", s/1024}')
threads=$(adb shell "ls /proc/\$(pidof $pkg)/task | wc -l" | tr -d '\r')
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$tot" "$(g 'Java Heap')" "$(g 'Native Heap')" "$(g 'Code')" \
  "$(g 'Stack')" "$(g 'Graphics')" "$(g 'Private Other')" "$(g 'System')" "$so" "$threads"
