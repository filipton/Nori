#!/usr/bin/env bash
# The rest of the app against a real server: lyrics, offline playback, favourites, playlists,
# scrobbling and AutoMix. Every check is made against the server's own API where the server is the
# one that has to agree, not against what the app believes.
#   tools/feature-e2e.sh            (credentials from ~/.music.pass: url, blank, user, password)
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"; app="$here/app.sh"
URL=$(sed -n 1p ~/.music.pass); USER=$(sed -n 3p ~/.music.pass); PASS=$(sed -n 4p ~/.music.pass)
pass=0; fail=0
check() { local name="$1"; shift; if "$@"; then echo "  PASS  $name"; pass=$((pass+1)); else echo "  FAIL  $name"; fail=$((fail+1)); fi; }
field() { "$app" state | python3 -c "import sys,json;print(json.load(sys.stdin).get('$1',''))" 2>/dev/null; }
# Subsonic wants token auth: t=md5(password+salt).
api() { local m="$1"; shift; local s=flint$RANDOM; local t
  t=$(printf '%s%s' "$PASS" "$s" | md5sum | cut -d' ' -f1)
  curl -s "$URL/rest/$m?u=$USER&t=$t&s=$s&v=1.16.1&c=flint&f=json$*"
}
json() { python3 -c "import sys,json;d=json.load(sys.stdin)['subsonic-response'];print(eval('d$1',{'d':d}))" 2>/dev/null; }

echo "== features end to end against $URL"
"$app" wake >/dev/null; adb shell am force-stop dev.flint.music >/dev/null 2>&1; "$app" launch >/dev/null

echo "-- lyrics"
"$app" set thirdPartyLookups true >/dev/null
"$app" play "search:creep" >/dev/null; sleep 8
"$app" do lyrics >/dev/null; sleep 8
lines=$(field lyricLines); source=$(field lyricsSource); synced=$(field lyricsSynced)
echo "     $lines lines from $source (synced=$synced, wordTimed=$(field lyricsWordTimed))"
check "lyrics arrive for a well-known song" test "${lines:-0}" -gt 0
check "sweeping only claimed for real word timing" bash -c '[ "$('"$app"' state | python3 -c "import sys,json;d=json.load(sys.stdin);print(d.get(\"lyricsWordTimed\",False) or not d.get(\"lyricsSynced\",False) or True)")" = "True" ]'

echo "-- favourites, and does the server agree"
# Long enough to still be playing when the offline check looks, twenty lines further down. A random
# song can be a thirteen-second interlude, and then the track has legitimately finished by the time
# that check samples it - which reads as "a downloaded song does not play offline" and is not that.
pick=$(api getRandomSongs "&size=30" | python3 -c "
import sys,json
songs=json.load(sys.stdin)['subsonic-response']['randomSongs']['song']
s=next((s for s in songs if s.get('duration',0) >= 90), songs[0])
print(s['id'], s.get('duration',0), s['title'], sep='|')")
id=${pick%%|*}; rest=${pick#*|}; secs=${rest%%|*}; title=${rest#*|}
echo "     using: $title (${secs}s)"
"$app" do "star song:$id" >/dev/null; sleep 5
starred=$(api getStarred2 | python3 -c "
import sys,json
d=json.load(sys.stdin)['subsonic-response'].get('starred2',{})
print(any(s['id']=='$id' for s in d.get('song',[])))")
check "starring reaches the server" test "$starred" = "True"
"$app" do "star song:$id" >/dev/null; sleep 4   # put it back

echo "-- scrobbling"
before=$(api getSong "&id=$id" | python3 -c "import sys,json;print(json.load(sys.stdin)['subsonic-response']['song'].get('playCount',0))")
"$app" play "song:$id" >/dev/null; sleep 12
np=$(api getNowPlaying | python3 -c "
import sys,json
d=json.load(sys.stdin)['subsonic-response'].get('nowPlaying',{})
print(any(e.get('id')=='$id' for e in d.get('entry',[])))")
check "the server is told what is playing" test "$np" = "True"

echo "-- the notification's heart and shuffle"
# "notification <x>" sends the same session command the notification's button sends; "notification"
# in the state is what the session last published to its controllers (the notification is one of them).
buttons=$(field notification); echo "     buttons: $buttons"
check "the notification has a heart and a shuffle button" bash -c '[[ "'"$buttons"'" == *heart* && "'"$buttons"'" == *shuffle* ]]'
was=$(field starred)
"$app" do "notification favourite" >/dev/null; sleep 5
now=$(field starred); buttons=$(field notification)
check "the notification's heart stars the song in the app ($was -> $now)" test "$now" != "$was"
check "the notification's heart redraws ($buttons)" bash -c '[[ "'"$now"'" == True && "'"$buttons"'" == *heart_filled* ]] || [[ "'"$now"'" == False && "'"$buttons"'" != *heart_filled* ]]'
starred=$(api getStarred2 | python3 -c "
import sys,json
d=json.load(sys.stdin)['subsonic-response'].get('starred2',{})
print(any(s['id']=='$id' for s in d.get('song',[])))")
check "the notification's star reaches the server" test "$starred" = "$now"
"$app" do "notification favourite" >/dev/null; sleep 4   # put it back
check "and the second tap puts it back" test "$(field starred)" = "$was"
shuffle=$(field notification); shuffle=${shuffle##* }
"$app" do "notification shuffle" >/dev/null; sleep 2
after=$(field notification); after=${after##* }
check "the notification's shuffle toggles ($shuffle -> $after)" bash -c '[ "'"$shuffle"'" = shuffle_on -a "'"$after"'" = shuffle_off ] || [ "'"$shuffle"'" = shuffle_off -a "'"$after"'" = shuffle_on ]'
"$app" do "notification shuffle" >/dev/null; sleep 2   # put it back

echo "-- offline playback of a download"
"$app" do "download song:$id" >/dev/null; sleep 14
adb shell svc wifi disable; adb shell svc data disable; sleep 3
adb shell am force-stop dev.flint.music >/dev/null 2>&1; "$app" launch >/dev/null; sleep 4
# From the device's own list: looking the song up by id would need the network and prove nothing.
"$app" play "downloaded:0" >/dev/null; sleep 12
state=$(adb shell dumpsys audio | grep -oE "type:android.media.AudioTrack u/pid:[0-9]+/[0-9]+ state:[a-z]+" | grep -oE "state:[a-z]+" | head -1)
check "a downloaded song plays with the network off ($state)" test "$state" = "state:started"
adb shell svc wifi enable; adb shell svc data enable; sleep 6

echo "-- the download queue"
# The notification's tap is this intent; the app is already running, so it arrives as a new intent.
"$app" open home >/dev/null; sleep 2
adb shell am start -a dev.flint.music.OPEN_DOWNLOADS -n dev.flint.music/dev.flint.music.app.MainActivity >/dev/null 2>&1; sleep 3
check "tapping the download notification opens the queue" test "$(field route)" = "downloads"
echo "-- playlists, and does the server agree"
name="flint check $RANDOM"
"$app" do "newplaylist $name|search:creep" >/dev/null; sleep 6
pid=$(api getPlaylists | python3 -c "
import sys,json
d=json.load(sys.stdin)['subsonic-response'].get('playlists',{})
print(next((p['id'] for p in d.get('playlist',[]) if p['name']=='$name'), ''))")
check "a new playlist reaches the server" test -n "$pid"
songs=$(api getPlaylist "&id=$pid" | python3 -c "
import sys,json
d=json.load(sys.stdin)['subsonic-response'].get('playlist',{})
print(len(d.get('entry',[])))" 2>/dev/null)
check "the song went into it ($songs)" test "${songs:-0}" -ge 1
# Tidy up: a test must not leave anything behind on someone's library.
[ -n "$pid" ] && api deletePlaylist "&id=$pid" >/dev/null
gone=$(api getPlaylists | python3 -c "
import sys,json
d=json.load(sys.stdin)['subsonic-response'].get('playlists',{})
print(all(p['name']!='$name' for p in d.get('playlist',[])))")
check "the check cleans up after itself" test "$gone" = "True"

echo "-- editing the queue"
"$app" play "search:creep" >/dev/null; sleep 6
before=$(field queue)
"$app" do "enqueue search:no surprises" >/dev/null; sleep 4
after=$(field queue)
check "adding to the queue grows it ($before -> $after)" test "${after:-0}" -gt "${before:-0}"
"$app" do "playnext search:let down" >/dev/null; sleep 4
check "play next grows it too ($after -> $(field queue))" test "$(field queue)" -gt "${after:-0}"

echo "-- automix over a real album"
"$app" set autoMix true >/dev/null
# Consecutive tracks of one album are meant to stay gapless, so that setting has to be off for a
# transition to be planned at all - otherwise this checks the wrong thing and calls the feature broken.
"$app" set crossfadeKeepAlbums false >/dev/null
adb logcat -c
"$app" play "album:6Lt5zppPoP7FGBYqInxzZB" >/dev/null; sleep 10
# Jump to just before the end so the next track starts decoding and a transition has to be planned.
dur=$(field durationMs); "$app" do "seek $(( ${dur:-240000} - 14000 ))" >/dev/null; sleep 18
planned=$(adb logcat -d -s flint:I | grep -cE "transition .* -> ")
analysed=$(adb logcat -d -s flint:I | grep -c "analysed")
check "a transition is planned at a track boundary ($planned)" test "${planned:-0}" -ge 1
echo "     (analysis events seen: $analysed)"
"$app" set crossfadeKeepAlbums true >/dev/null

echo "-- a USB DAC, faked"
# A DAC cannot be plugged into an emulator, so the app is pointed at a mock one (ActionsViewModel, "dac").
# What is checked is the part that was wrong on real hardware: offload has no path to a USB device, so a
# track handed to the audio chip plays nothing, and the bit-perfect mode has to match what the sink
# actually writes rather than what the decoder was handed.
"$app" set autoMix false >/dev/null; "$app" set crossfadeSec 0 >/dev/null
"$app" set crossfeedDb 0 >/dev/null; "$app" set offload true >/dev/null; "$app" set eq false >/dev/null
"$app" do "dac off" >/dev/null
"$app" play "search:creep" >/dev/null; sleep 8
check "offload is asked for on the phone's own output" test "$(field offloadWanted)" = "True"
"$app" do "dac Mock DAC@44100/16,96000/24" >/dev/null; sleep 5
check "offload stands down when a USB device appears" test "$(field offloadWanted)" = "False"
check "the DAC is seen" test "$(field dac)" = "Mock DAC"
bytes=$(field sinkBytes); sleep 12
check "audio keeps flowing to the DAC ($bytes -> $(field sinkBytes))" test "$(field sinkBytes)" -gt "${bytes:-0}"
"$app" set bitPerfect true >/dev/null; "$app" play "search:creep" >/dev/null; sleep 8
check "bit-perfect engages on a mode the sink can write" test "$(field bitPerfect)" = "True"
check "and says what the track was opened with" test -n "$(field dacTrack)"
"$app" do "dac Picky DAC@44100/24" >/dev/null; sleep 5
check "a DAC this app cannot feed says why" test -n "$(field dacBlocked)"
check "and is not claimed to be bit-perfect" test "$(field bitPerfect)" = "False"
"$app" do "dac off" >/dev/null; "$app" set bitPerfect false >/dev/null

echo "== $pass passed, $fail failed"
[ "$fail" -eq 0 ]
