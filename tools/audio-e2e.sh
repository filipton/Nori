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
# The sink holds the outgoing track's ending and mixes the next one into it, and says so while the mix
# is being heard - not while it is being made, which is a buffer ahead of the ear.
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
adb shell am force-stop dev.nori.music >/dev/null 2>&1
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
  adb logcat -v time -s nori:I > "$watching" 2>/dev/null &
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
  # The bar shows what is heard. The player itself counts the held ending as played the moment it
  # is decoded (so the next track arrives in time), and the bar used to show that: a jump of the
  # whole crossfade the moment the hold began, then the next song at 0:00 while this one still
  # played alone. Now it walks steadily up to the point the mix starts and only then changes song.
  steady=1; last=""; seen=""
  for _ in $(seq 8); do
    t=$(field title); p=$(field positionMs)
    [ "$t" = "$leaving" ] || break
    if [ -n "$last" ] && { [ "${p:-0}" -lt "$last" ] || [ $((p - last)) -gt 4000 ]; }; then steady=0; fi
    last=$p; seen="$seen $p"
    sleep 1
  done
  check "the bar walks steadily through the held ending ($seen)" test "$steady" = 1 -a -n "$last" -a "$last" -gt $((dur - 22000))
  check "the sink reaches the mix" waitfor_mix
  # The whole point, and the thing that was broken: the next track's samples have to arrive while there
  # is still sound in the sink to mix them into. When they were late the crossfade played after a hole
  # as long as itself - the last twelve seconds of the song, silent.
  check "the next track arrives in time to be mixed" waitfor "mixing: the next track arrived" 60
  check "the ending is not let go for want of it" never "letting the ending play"
  check "the next track plays out of the mix" next_track_plays "$leaving"
  # A scrub into the mix stays on the song to hear the ending: the tail belongs to the song, so
  # going there replays it instead of jumping to the next one (eight seconds from the end is
  # mid-mix for a twelve-second crossfade).
  "$app" do "playnext $song" >/dev/null
  # The plan past the new song is remade asynchronously; the seek below needs it there.
  watch_from_now
  check "a crossfade is planned past the new song too" waitfor "transition .*: [A-Z_]+ [0-9]+ ms at" 15
  leaving2=$(field title); d2=$(field durationMs)
  if [ "${d2:-0}" -gt 60000 ]; then
    # The arrival below must be this seek's, not the earlier boundary's.
    watch_from_now
    "$app" do "seek $((d2 - 8000))" >/dev/null
    heard=""
    for _ in $(seq 10); do
      t=$(field title); p=$(field positionMs)
      if [ "$t" = "$leaving2" ] && [ "${p:-0}" -ge $((d2 - 8000)) ]; then heard="$t@${p}"; break; fi
      # Fast decode can finish the pipeline in milliseconds: the item flips while the deep
      # buffer still plays the tail out. The two checks below prove the mix either way.
      if [ "$t" != "$leaving2" ] && [ -n "$t" ]; then heard="early-flip:$t@${p}"; break; fi
      sleep 1
    done
    check "a scrub into the mix stays to hear the ending ($heard)" test -n "$heard"
    # The tail it points at still gets its mix: a hold beginning seconds in still fires.
    check "the mix still fires after the late seek" waitfor "mixing: the next track arrived" 60
    check "the next track plays out of that mix" next_track_plays "$leaving2"
  fi
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

echo "-- tuning borrows the shallow buffer and returns it"
# The equalizer screen trades the deep buffer for instant response; leaving it schedules the
# deep buffer's return at the next boundary. Without that the pipeline stays half a second deep
# and every transition bows out for want of runway.
"$app" set eq true >/dev/null; sleep 2
watch_from_now
"$app" do "tuning on" >/dev/null; sleep 4
check "still playing after tuning cuts in" playing_audio
shallow=$(grep -oE "buffer=[0-9]+" "$watching" | tail -1 | grep -oE "[0-9]+")
"$app" do "tuning off" >/dev/null; sleep 2
"$app" do "playnext $other" >/dev/null; sleep 2
"$app" do next >/dev/null; sleep 6
deep=$(grep -oE "buffer=[0-9]+" "$watching" | tail -1 | grep -oE "[0-9]+")
check "tuning takes the shallow buffer ($shallow)" bash -c "[ '${shallow:-0}' -gt 0 ]"
check "the deep buffer is back after the next boundary ($deep)" bash -c "[ '${deep:-0}' -gt '${shallow:-0}' ]"
check "the deep buffer swap happens at the boundary" waitfor "chain swap at the boundary" 15
check "still playing after the deep swap" playing_audio

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

echo "-- seek after a restart"
# Pause, kill, reopen, seek while paused, play: the seek has to win over the restored position.
# The watchdog once anchored on the idle position (0) and read the restored one as "moved by
# someone else", so the dropped seek was never re-asked and play started from the old spot.
"$app" play "$song" >/dev/null; sleep 4
"$app" do "seek 10000" >/dev/null; sleep 2
"$app" do pause >/dev/null; sleep 2
adb shell am force-stop dev.nori.music >/dev/null 2>&1
"$app" launch >/dev/null
# Cold boot: wait for the queue to be back before touching it.
for _ in $(seq 40); do t=$(field title); [ -n "$t" ] && break; sleep 2; done
"$app" do "seek 30000" >/dev/null; sleep 4
b=$(field positionMs)
check "a seek while paused after a restart sticks ($b)" bash -c "[ '${b:-0}' -ge 27000 ] && [ '${b:-0}' -le 33000 ]"
"$app" do resume >/dev/null; sleep 6
c=$(field positionMs)
check "play resumes from the seek ($c)" bash -c "[ '${c:-0}' -ge 29000 ]"

echo "-- errors"
errs=$(adb logcat -d | grep -c "ExoPlayerImplInternal: Playback error")
check "no playback errors in logcat ($errs)" test "$errs" -eq 0
echo "== $pass passed, $fail failed"
[ "$fail" -eq 0 ]
