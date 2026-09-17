#!/usr/bin/env bash
# Scroll jank of whatever list is on screen: tools/scroll.sh <package> [flings=12]
pkg=$1; n=${2:-12}
adb shell dumpsys gfxinfo "$pkg" reset >/dev/null
for i in $(seq "$n"); do adb shell input swipe 540 1900 540 500 120; sleep 0.7; done
for i in $(seq "$n"); do adb shell input swipe 540 600 540 2000 120; sleep 0.7; done
adb shell dumpsys gfxinfo "$pkg" | grep -E "Total frames|Janky frames:|50th|90th|95th|99th|Number Slow UI|Number Slow bitmap"
