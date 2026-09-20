#!/usr/bin/env bash
# Audio, end to end, the way a person uses it: play, pause and resume from the notification, the lock
# screen and the media keys, and change every processing switch while the music runs. Each step asserts
# that the playhead is still moving afterwards and that nothing errored - the class of bug where a sink
# quietly stops accepting audio shows up exactly here.
#   tools/audio-e2e.sh [song-search-text]
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"; app="$here/app.sh"
song=${1:-"search:paranoid android"}
# A second song, off a different album: transitions between two tracks of one album are meant to be gapless.
other=${2:-"search:nothing else matters"}
pass=0; fail=0
state() { "$app" state; }
field() { state | python3 -c "import sys,json;print(json.load(sys.stdin).get('$1',''))" 2>/dev/null; }
# Is audio actually flowing? Three things can answer, and only one of them can be trusted on its own.
# The media session's position is deliberately not updated periodically here (it would cost wakeups),
# and a controller in the background reports a stale one - a test watching either can pass in silence.
# The bytes this app hands to the AudioTrack are honest but arrive in ten-second bursts, so a short
# window sees nothing. What the system itself says about our AudioTrack is instant and background-safe.
track_state() {
  adb shell dumpsys audio | grep -oE "type:android.media.AudioTrack u/pid:[0-9]+/[0-9]+ state:[a-z]+" |
    grep -oE "state:[a-z]+" | sed 's/state://' | head -1
}
playing_audio() { [ "$(track_state)" = "started" ]; }
# For the checks that matter most, also prove the bursts keep coming over a full buffer cycle.
moving() {
  playing_audio || return 1
  local a b; a=$(field sinkBytes 2>/dev/null); sleep 13; b=$(field sinkBytes 2>/dev/null)
  [ -n "$a" ] && [ -n "$b" ] && [ "$b" -gt "$a" ]
}
stopped() { [ "$(track_state)" != "started" ]; }
# The sink holds the outgoing track's ending and mixes the next one into it, and says so while it does.
# It starts as the decoder passes the planned point, which is a buffer ahead of what is being heard.
waitfor_mix() { for _ in $(seq 60); do mixing && return 0; sleep 1; done; return 1; }
next_track_plays() { # next_track_plays <title it is leaving>
  for _ in $(seq 60); do [ "$(field title)" != "$1" ] && { sleep 3; playing_audio; return; }; sleep 1; done
  return 1
}
check() { # check <name> <command...>
  local name="$1"; shift
  if "$@"; then echo "  PASS  $name"; pass=$((pass+1)); else echo "  FAIL  $name"; fail=$((fail+1)); fi
}
key() { adb shell input keyevent "$1"; sleep 2; }

echo "== audio end to end"
adb shell am force-stop dev.flint.music >/dev/null 2>&1
"$app" wake >/dev/null; "$app" launch >/dev/null
"$app" play "$song" >/dev/null; sleep 6
check "plays a song" moving

echo "-- transport from outside the app"
key KEYCODE_MEDIA_PAUSE; check "media key pause stops the playhead" stopped
key KEYCODE_MEDIA_PLAY; check "media key resume" playing_audio
adb shell input keyevent KEYCODE_HOME; sleep 1
key KEYCODE_MEDIA_PAUSE; sleep 20; key KEYCODE_MEDIA_PLAY
check "resume after 20 s paused in the background" moving
adb shell input keyevent KEYCODE_SLEEP; sleep 3; key KEYCODE_MEDIA_PAUSE; sleep 5; key KEYCODE_MEDIA_PLAY; sleep 2
check "pause and resume with the screen off" moving
adb shell input keyevent KEYCODE_WAKEUP; adb shell wm dismiss-keyguard >/dev/null 2>&1; sleep 2
"$app" launch >/dev/null; sleep 2   # back in the foreground: the settings below need the app's own hooks

echo "-- processing changed while it plays"
for setting in "eq true" "eq false" "limiter true" "mono true" "mono false" "limiter false" "autoMix true" "offload false" "offload true"; do
  "$app" set $setting >/dev/null; sleep 5
  check "still playing after $setting" playing_audio
  # Bytes flowing is not sound: a limiter that pulled every sample down 90 dB passed the line above in
  # silence. At its -1 dB default, mastered music needs a few dB at most.
  if [ "$setting" = "limiter true" ]; then
    gr=$(field gainReductionDb)
    check "limiter only catches peaks (${gr} dB)" python3 -c "import sys; sys.exit(0 if float('$gr' or 99) < 6 else 1)"
  fi
done

echo "-- transitions between tracks"
# Nothing here can listen to a crossfade, and every other signal is the same whether one happened or
# not: the bytes keep flowing either way. The planner says what it decided, one line per boundary, and
# the sink says while it is mixing - between them they cover the whole path from the setting to the
# audio. This section exists because all of it was broken while the checks above passed: a plan was
# asked for once, on a track's first decoded buffer, when the queue the planner reads was often still
# empty, and never asked for again - so crossfade and AutoMix did nothing at all for whole queues.
# Read from a running capture rather than from `adb logcat -d`: every call to app.sh clears the log
# buffer (it has to, to read its own answer back), so a line printed a second ago is often already gone.
watching=$(mktemp)
watcher=""
watch_from_now() {
  [ -n "$watcher" ] && kill "$watcher" 2>/dev/null
  adb logcat -c
  adb logcat -v time -s flint:I > "$watching" 2>/dev/null &
  watcher=$!
  sleep 0.5
}
trap '[ -n "$watcher" ] && kill $watcher 2>/dev/null; rm -f "$watching"' EXIT
logged() { grep -qE "$1" "$watching"; }
never() { ! logged "$1"; }
waitfor() { local n=$2; for _ in $(seq "$n"); do logged "$1" && return 0; sleep 1; done; return 1; }
mixing() { [ "$(field mixing)" = "True" ]; }

watch_from_now
"$app" set autoMix false >/dev/null
"$app" set crossfadeSec 12 >/dev/null
# Two songs off different albums: "keep albums gapless" deliberately runs an album straight on, so an
# album pair proves nothing either way.
"$app" play "$song" >/dev/null; sleep 4
"$app" do "playnext $other" >/dev/null
check "a crossfade is planned for the next boundary" waitfor "transition .*: [A-Z_]+ [0-9]+ ms at" 10

# And it is planned for the song already playing: turning the setting on and waiting for this song to
# end is how anyone tries the feature out.
dur=$(field durationMs)
if [ "${dur:-0}" -gt 40000 ]; then
  leaving=$(field title)
  "$app" do "seek $((dur - 24000))" >/dev/null
  check "the sink reaches the mix" waitfor_mix
  # The whole point, and the thing that was broken: the next track's samples have to arrive while there
  # is still sound in the sink to mix them into. When they were late the crossfade played after a hole
  # as long as itself - the last twelve seconds of the song, silent.
  check "the next track arrives in time to be mixed" waitfor "mixing: the next track arrived" 60
  check "the ending is not let go for want of it" never "letting the ending play"
  check "the next track plays out of the mix" next_track_plays "$leaving"
fi

watch_from_now
"$app" set crossfadeSec 0 >/dev/null
check "with it off, the planner says so rather than going quiet" waitfor "planFor: off .*crossfadeSec=0" 10

echo "-- a chain rebuild waits for the boundary"
# Taking the equalizer out of the chain needs a sink rebuild, which used to cut the song
# mid-track. Now it waits for the next boundary instead - and the swap itself is silent.
# First a boundary to drain whatever the processing loop left pending, so the waits below
# can only be satisfied by the toggles that follow.
"$app" do "playnext $other" >/dev/null; sleep 2
"$app" do next >/dev/null; sleep 6
watch_from_now
"$app" set eq true >/dev/null; sleep 3
"$app" set eq false >/dev/null; sleep 3
check "taking the EQ out waits for the boundary" waitfor "chain swap deferred" 10
check "still playing after the EQ leaves" playing_audio
"$app" do "playnext $other" >/dev/null; sleep 2
"$app" do next >/dev/null; sleep 6
check "the swap happens at the boundary" waitfor "chain swap at the boundary" 15
check "still playing after the swap" playing_audio

echo "-- AutoMix"
watch_from_now
"$app" set autoMix true >/dev/null
check "measuring starts when AutoMix is switched on" waitfor "measuring ahead:" 20
# A track can only be measured from bytes already on the device; the log says when that is why.
check "the tracks coming up are measured" waitfor "analysed [^ ]+ ahead: [0-9]|not on the device yet" 120
check "the mix is planned from what was measured" waitfor "transition .*: [A-Z_]+ [0-9]+ ms at" 30
"$app" set autoMix false >/dev/null

echo "-- skipping and seeking"
"$app" do next >/dev/null; sleep 5; check "next track plays" playing_audio
"$app" do previous >/dev/null; sleep 5; check "previous track plays" playing_audio

echo "-- errors"
errs=$(adb logcat -d | grep -c "ExoPlayerImplInternal: Playback error")
check "no playback errors in logcat ($errs)" test "$errs" -eq 0
echo "== $pass passed, $fail failed"
[ "$fail" -eq 0 ]
