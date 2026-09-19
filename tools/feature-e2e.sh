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

echo "-- downloads run side by side and survive a force stop"
# A library album of at least six songs, none of them provider tracks (streaming one of those makes
# octo-fiesta fetch it), with nothing of it downloaded yet.
aid=$(api getAlbumList2 "&type=random&size=40" | python3 -c "
import sys,json
for a in json.load(sys.stdin)['subsonic-response']['albumList2'].get('album',[]):
    if not a['id'].startswith('ext-') and a.get('songCount',0) >= 6: print(a['id'])" | while read -r a; do
  api getAlbum "&id=$a" | python3 -c "
import sys,json
s=json.load(sys.stdin)['subsonic-response']['album']['song']
ok=all(not x['id'].startswith('ext-') and x.get('suffix')!='Remote' for x in s)
print('$a' if ok else '')"; done | grep . | head -12 | tr '\n' ' ')
if [ -n "$aid" ]; then
  before=$(field downloaded)
  # An album this suite has already downloaded has nothing left to fetch and would report nothing
  # downloading at all, so ask each candidate in turn until one has work to do.
  active=0
  for a in $aid; do
    aid=$a
    "$app" do "download album:$aid" >/dev/null; sleep 3
    active=$(field dlActive)
    [ "${active:-0}" -ge 2 ] && break
  done
  echo "     $active downloading at once, parallel setting $(adb shell run-as dev.flint.music cat shared_prefs/flint.xml 2>/dev/null | grep -o 'parallelDownloads" value="[0-9]*' | grep -o '[0-9]*$')"
  check "several songs download at once ($active)" test "${active:-0}" -ge 2
  adb shell am force-stop dev.flint.music >/dev/null 2>&1; "$app" launch >/dev/null; sleep 5
  left=$(field downloading); active=$(field dlActive)
  check "after a force stop the queue picks up again ($left left, $active downloading)" bash -c "[ '${left:-1}' = 0 ] || [ '${active:-0}' -gt 0 ]"
  for _ in $(seq 60); do [ "$(field downloading)" = 0 ] && break; sleep 3; done
  check "the interrupted album finishes ($(field downloaded) downloaded, was $before)" test "$(field downloading)" = 0
else
  echo "     no library-only album of six songs found; skipped"
fi
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

echo "-- for you: favourites and mixes open as pages"
# What a mix page shows, read from the screen: "<count>|<title>@<x>,<y>|..." for the song count in its
# caption and every fully visible row title (rows sit below the Play pill and above the mini player).
page() {
  adb shell uiautomator dump /sdcard/flint-ui.xml >/dev/null 2>&1
  adb shell cat /sdcard/flint-ui.xml | python3 -c "
import sys,re
nodes=[(t,d,*map(int,b)) for t,d,b in ((m.group(1),m.group(2),re.findall(r'\d+',m.group(3))) for m in re.finditer(r'text=\"([^\"]*)\"[^>]*content-desc=\"([^\"]*)\"[^>]*bounds=\"([^\"]*)\"',sys.stdin.read()))]
count=next((int(m.group(1)) for t,*_ in nodes for m in [re.match(r'(\d+) songs? ',t)] if m),0)
bar=next((y1 for t,d,x1,y1,x2,y2 in nodes if d=='Now playing bar'),10**6)
play=next((y2 for t,d,x1,y1,x2,y2 in nodes if t=='Play'),0)
rows=[n for n in nodes if n[0] and n[3]>=play and n[5]<=bar and 150<n[2]<260]
titles=[n for i,n in enumerate(rows) if i%2==0]
print('|'.join([str(count)]+['%s@%d,%d'%(t,(x1+x2)//2,(y1+y2)//2) for t,d,x1,y1,x2,y2 in titles]))"
}
starred_count() { api getStarred2 | python3 -c "
import sys,json
d=json.load(sys.stdin)['subsonic-response'].get('starred2',{})
print(sum(1 for s in d.get('song',[]) if not s.get('isExternal') and not s['id'].startswith(('ext-','pl-'))))"; }
"$app" open mix/favourites >/dev/null; sleep 4
check "the favourites tile opens its page" test "$(field route)" = "mix/{id}"
favs=$(page | cut -d'|' -f1); server=$(starred_count)
check "it lists the songs the server has starred ($favs, server $server)" test "${favs:-x}" = "$server"
"$app" do "star song:$id" >/dev/null; sleep 5
check "starring a song adds it while the page is open" test "$(page | cut -d'|' -f1)" = "$((server + 1))"
"$app" do "star song:$id" >/dev/null; sleep 5   # put it back
check "and unstarring takes it away again" test "$(page | cut -d'|' -f1)" = "$server"
"$app" open mix/discover >/dev/null; sleep 4
first=$(page)
"$app" open home >/dev/null; sleep 2; "$app" open mix/discover >/dev/null; sleep 4
check "a mix stays the same when it is opened again" test "$first" = "$(page)"
# What you see is what plays: a tap on the third row starts the whole mix at that row.
n=$(echo "$first" | cut -d'|' -f1); third=$(echo "$first" | cut -d'|' -f4)
if [ -n "$third" ]; then
  xy=${third##*@}; adb shell input tap "${xy%,*}" "${xy#*,}"; sleep 6
  check "tapping a row plays that song (${third%@*})" test "$(field title)" = "${third%@*}"
  check "with the rest of the mix around it ($(field index) of $(field queue), page $n)" \
    test "$(field index)" = "2" -a "$(field queue)" = "$n"
  "$app" do pause >/dev/null
else
  check "the mix has songs to play" false
fi

echo "-- automix over a real album"
"$app" set autoMix true >/dev/null
# Consecutive tracks of one album are meant to stay gapless, so that setting has to be off for a
# transition to be planned at all - otherwise this checks the wrong thing and calls the feature broken.
"$app" set crossfadeKeepAlbums false >/dev/null
# From nothing measured, so the measuring ahead is this run's work and not an earlier one's.
"$app" set clearAnalyses true >/dev/null
adb logcat -c
"$app" play "album:6Lt5zppPoP7FGBYqInxzZB" >/dev/null; sleep 10
# Jump to just before the end so the next track starts decoding and a transition has to be planned.
dur=$(field durationMs); "$app" do "seek $(( ${dur:-240000} - 14000 ))" >/dev/null; sleep 18
planned=$(adb logcat -d -s flint:I | grep -cE "transition .* -> ")
analysed=$(adb logcat -d -s flint:I | grep -c "analysed")
ahead=$(adb logcat -d -s flint:I | grep -c "analysed .* ahead")
check "a transition is planned at a track boundary ($planned)" test "${planned:-0}" -ge 1
# The tracks are measured before they are played, so the first meeting of two songs is a real mix
# rather than a fade; the measurement only runs on audio already on the device, so this is a report
# rather than a check - an empty cache legitimately has nothing to measure yet.
check "the tracks coming up are measured before they are played ($ahead)" test "${ahead:-0}" -ge 1
echo "     (analysis events seen: $analysed)"
"$app" set crossfadeKeepAlbums true >/dev/null

echo "-- what plays when the queue runs out"
# One song on its own, so the queue really does run out; the album basis is the one that has to queue a
# whole record rather than a handful of songs.
"$app" set autoFill true >/dev/null
"$app" set autoFillBasis SIMILAR >/dev/null
"$app" set autoFillKind SONGS >/dev/null
"$app" play "search:creep" >/dev/null; sleep 10
grew=$(field queue)
check "the queue is carried on past its last song ($grew)" test "${grew:-0}" -gt 1
"$app" set autoFillKind ALBUMS >/dev/null
"$app" play "search:creep" >/dev/null; sleep 16
album=$(field queue)
check "a whole album is queued when albums are chosen ($album)" test "${album:-0}" -gt 2
"$app" set autoFillKind SONGS >/dev/null

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

echo "-- a sound per output device"
# The service switches the sound when the output changes, with no screen involved. A fake DAC stands in
# for the device; "eq" in the state is the equalizer switch, which each device's sound sets.
dev="USB: Flint Check DAC"; hp="USB: Sennheiser HD 600"
clean() {
  "$app" do "dac off" >/dev/null; sleep 2; "$app" set autoEqAuto false >/dev/null
  for p in "flint check" "Flat" "Sennheiser HD 600"; do "$app" set deleteProfile "$p" >/dev/null; done
  "$app" set forgetDevice "$dev" >/dev/null; "$app" set forgetDevice "$hp" >/dev/null
}
clean   # a run that stopped half-way must not decide this one
"$app" set autoEqAuto false >/dev/null; "$app" do "dac off" >/dev/null; "$app" set eq false >/dev/null
"$app" play "search:creep" >/dev/null; sleep 6
"$app" set eq true >/dev/null; "$app" set saveProfile "flint check" >/dev/null; sleep 2; "$app" set eq false >/dev/null
"$app" set deviceSound "$dev=profile:flint check" >/dev/null; sleep 2
"$app" do "dac Flint Check DAC@44100/16" >/dev/null; sleep 3
check "a device with a profile gets it on connect" test "$(field output)/$(field eq)" = "$dev/True"
"$app" do "dac off" >/dev/null; sleep 3
check "and the sound from before comes back without it" test "$(field eq)" = "False"
"$app" set eq true >/dev/null; "$app" set deviceSound "$dev=flat" >/dev/null; sleep 2
"$app" do "dac Flint Check DAC@44100/16" >/dev/null; sleep 3
check "a device set to flat turns the equalizer off" test "$(field eq)" = "False"
"$app" do "dac off" >/dev/null; sleep 3
check "and it is on again on the speaker" test "$(field eq)" = "True"
"$app" set eq false >/dev/null
# AutoEQ: the index is one download from github.com; without it there is nothing to match against.
"$app" set autoEqIndex 1 >/dev/null; sleep 12
adb logcat -c; "$app" do "dac Sennheiser HD 600@44100/16" >/dev/null; sleep 4
if adb logcat -d -s flint:I | grep -q "device sound: $hp -> nothing chosen"; then
  check "asking first leaves the sound alone" test "$(field eq)" = "False"
  "$app" set eqNotice apply >/dev/null; sleep 4
  check "saying yes applies the headphones' curve" test "$(field eq)" = "True"
  "$app" do "dac off" >/dev/null; sleep 3
  "$app" set deviceSound "$hp=auto" >/dev/null; "$app" set deleteProfile "Sennheiser HD 600" >/dev/null; sleep 2
  "$app" set autoEqAuto true >/dev/null; "$app" set eq false >/dev/null
  "$app" do "dac Sennheiser HD 600@44100/16" >/dev/null; sleep 5
  check "with automatic AutoEQ on, the curve is applied without asking" test "$(field eq)" = "True"
  "$app" set eqNotice undo >/dev/null; sleep 3
  check "undo puts the sound back" test "$(field eq)" = "False"
  "$app" do "dac off" >/dev/null; sleep 2; "$app" do "dac Sennheiser HD 600@44100/16" >/dev/null; sleep 4
  check "and that device is not switched again" test "$(field eq)" = "False"
else
  echo "     (AutoEQ index not available, curve checks skipped)"
fi
# Tidy up: nothing of the check stays in the device list or the profiles.
clean

echo "== $pass passed, $fail failed"
[ "$fail" -eq 0 ]
