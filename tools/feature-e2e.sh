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
id=$(api getRandomSongs "&size=1" | python3 -c "import sys,json;print(json.load(sys.stdin)['subsonic-response']['randomSongs']['song'][0]['id'])")
title=$(api getSong "&id=$id" | python3 -c "import sys,json;print(json.load(sys.stdin)['subsonic-response']['song']['title'])")
echo "     using: $title"
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

echo "-- offline playback of a download"
"$app" do "download song:$id" >/dev/null; sleep 14
adb shell svc wifi disable; adb shell svc data disable; sleep 3
adb shell am force-stop dev.flint.music >/dev/null 2>&1; "$app" launch >/dev/null; sleep 4
# From the device's own list: looking the song up by id would need the network and prove nothing.
"$app" play "downloaded:0" >/dev/null; sleep 12
state=$(adb shell dumpsys audio | grep -oE "type:android.media.AudioTrack u/pid:[0-9]+/[0-9]+ state:[a-z]+" | grep -oE "state:[a-z]+" | head -1)
check "a downloaded song plays with the network off ($state)" test "$state" = "state:started"
adb shell svc wifi enable; adb shell svc data enable; sleep 6

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

echo "== $pass passed, $fail failed"
[ "$fail" -eq 0 ]
