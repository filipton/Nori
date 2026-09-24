#!/usr/bin/env bash
# Measures what a music app costs while it plays. Start playback in the app first, then:
#   tools/bench.sh <package> [seconds=120] [screen=off|on]
# Reports CPU time, wakeups (voluntary context switches), memory and held wake locks for every
# process of the package. Works on an emulator or a phone over adb; debuggable not required for
# CPU and memory, but per-thread detail needs a rooted/emulator adb.
set -uo pipefail
pkg=$1; secs=${2:-120}; screen=${3:-off}

pids() { adb shell "pgrep -f '^$pkg'" | tr -d '\r' | tr '\n' ' '; }
# utime+stime ticks and voluntary context switches, one line per thread ("tid ticks switches"). Totals are
# taken over the threads alive at both ends of the window: a thread that exits in between would otherwise
# take its whole lifetime's count out of the second sum, and a quiet app read as negative.
sample() {
  adb shell "for p in $(pids); do for d in /proc/\$p/task/*; do
      set -- \$(cat \$d/stat 2>/dev/null | sed 's/.*) //'); t=\$((\${12:-0} + \${13:-0}));
      v=\$(grep -s voluntary_ctxt \$d/status | head -1 | tr -dc 0-9); echo \${d##*/} \$t \${v:-0}; done; done" | tr -d '\r' | sort
}
threads() {
  adb shell "for p in $(pids); do for d in /proc/\$p/task/*; do set -- \$(cat \$d/stat 2>/dev/null | sed 's/.*) //'); v=\$(grep -s voluntary_ctxt \$d/status | head -1 | tr -dc 0-9); echo \$(cat \$d/comm | tr ' ' '_'):\${d##*/} \$((\${12:-0} + \${13:-0})) \${v:-0}; done; done" | tr -d '\r' | sort
}

state=$(adb shell dumpsys media_session | grep -A8 "package=$pkg" | grep -oE "\{state=[A-Z_0-9]+" | head -1 | tr -d "{" | sed -E "s/=3$/=PLAYING/; s/=2$/=PAUSED/" || true)
echo "package: $pkg   session: ${state:-none}   window: ${secs}s   screen: $screen"
if [ "$screen" = off ]; then adb shell input keyevent 223; else adb shell input keyevent 224; adb shell svc power stayon true; fi
sleep 15
sample > /tmp/bench-sa.$$; threads > /tmp/bench-a.$$
# Once a second, on the device: how many times did any thread of the app go to sleep and get woken again?
# A second with next to none is a second the CPU was free to stay in deep idle.
quiet=$(adb shell "prev=; q=0; i=0; while [ \$i -lt $secs ]; do w=0; for p in $(pids); do for d in /proc/\$p/task/*; do v=\$(grep -s voluntary_ctxt \$d/status | head -1 | tr -dc 0-9); w=\$((w + \${v:-0})); done; done; [ -n \"\$prev\" ] && [ \$((w - prev)) -lt 12 ] && q=\$((q + 1)); prev=\$w; i=\$((i + 1)); sleep 1; done; echo \$q" | tr -d '\r')
sample > /tmp/bench-sb.$$; threads > /tmp/bench-b.$$
set -- $(join /tmp/bench-sa.$$ /tmp/bench-sb.$$ | awk '{t += $4 - $2; w += $5 - $3} END {print t+0, w+0}')
ticks=$1; wake=$2
echo "cpu:      $((ticks * 10)) ms  = $(echo "scale=2; $ticks / $secs" | bc)% of one core"
echo "wakeups:  $(echo "scale=1; $wake / $secs" | bc) per second"
echo "quiet:    $quiet of $secs seconds with (almost) no wakeups"
echo "memory:   $(adb shell dumpsys meminfo $pkg | grep -E 'TOTAL PSS' | awk '{print int($3/1024)" MB PSS"}')"
uid=$(adb shell dumpsys package $pkg | grep -m1 -oE 'userId=[0-9]+' | cut -d= -f2 | tr -d '\r')
echo "wakelocks: $(adb shell dumpsys power | grep -E '^ +[A-Z_]+_WAKE_LOCK' | grep "uid=$uid" | awk '{print $2}' | tr '\n' ' ' || true)"
echo "busiest threads (ms):"
join /tmp/bench-a.$$ /tmp/bench-b.$$ 2>/dev/null | awk '$4-$2>0 {sub(/:[0-9]+$/,"",$1); print "  " ($4-$2)*10, $1}' | sort -rn | head -8
echo "busiest threads (wakeups/s):"
join /tmp/bench-a.$$ /tmp/bench-b.$$ 2>/dev/null | awk -v s=$secs '$5-$3>0 {sub(/:[0-9]+$/,"",$1); printf "  %.1f %s\n", ($5-$3)/s, $1}' | sort -rn | head -8
rm -f /tmp/bench-a.$$ /tmp/bench-b.$$ /tmp/bench-sa.$$ /tmp/bench-sb.$$
after=$(adb shell dumpsys media_session | grep -A8 "package=$pkg" | grep -oE "\{state=[A-Z_0-9]+" | head -1 | tr -d "{" | sed -E "s/=3$/=PLAYING/; s/=2$/=PAUSED/" || true)
echo "session after: ${after:-none}"
[ "$screen" = on ] && adb shell svc power stayon false || true
