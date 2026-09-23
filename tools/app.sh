#!/usr/bin/env bash
# Drives a debug build of nori directly, instead of tapping screen coordinates:
#   tools/app.sh open settings/sound      navigate to a route
#   tools/app.sh play "search:noise 1"    play a song by id, album, or first search hit
#   tools/app.sh set limiter true         flip one setting
#   tools/app.sh state                    one JSON line: route, playback, key settings
#   tools/app.sh engine rust|exoplayer    the player the service builds (set playbackEngine), app restarted
# Needs the app running (tools/app.sh launch). Answers come back from logcat, tag noritest.
set -uo pipefail
pkg=${NORI_PKG:-dev.nori.music}
send() {
  adb logcat -c
  # Quote for the shell ON THE DEVICE: adb hands it a command line, so an unquoted | or space there
  # becomes a pipe or an argument break and the extra arrives mangled (or not at all).
  local cmdline="am broadcast -n $pkg/dev.nori.music.app.TestBridge -a dev.nori.music.TEST --es cmd '$1'"
  [ -n "${2:-}" ] && cmdline="$cmdline --es arg '$2'"
  [ -n "${3:-}" ] && cmdline="$cmdline --es value '$3'"
  adb shell "$cmdline" >/dev/null 2>&1
  for _ in $(seq 20); do
    local line; line=$(adb logcat -d -s noritest:I | tail -1 | sed -E 's/^.*noritest: //')
    [ -n "$line" ] && { echo "$line"; return 0; }
    sleep 0.25
  done
  echo "no answer (is a debug build running?)" >&2; return 1
}
case "${1:-}" in
  launch) adb shell monkey -p $pkg -c android.intent.category.LAUNCHER 1 >/dev/null 2>&1; sleep 4; send state ;;
  open|play|state|set|login|do) send "$@" ;;
  # The service reads the setting when it starts: the app is stopped and launched again with it.
  engine) send set playbackEngine "${2:?rust or exoplayer}" && adb shell am force-stop $pkg && "$0" launch ;;
  wake) adb shell input keyevent 224 >/dev/null; adb shell wm dismiss-keyguard >/dev/null 2>&1; sleep 1 ;;
  *) echo "usage: app.sh launch|open <route>|play <ref>|do <action>|login <url|user|pass>|set <name> <value>|engine rust|exoplayer|state|wake" >&2; exit 2 ;;
esac
