#!/usr/bin/env bash
# Local Navidrome with generated, tagged test music. Emulator reaches it at http://10.0.2.2:4533
# First login (admin/admin) is created automatically through the API.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)/.dev"
music="$root/music"
mkdir -p "$music" "$root/data"
if [ -z "$(ls -A "$music")" ]; then
  n=0
  for artist in "Alpha Waves" "Beta Band" "Gamma Ray Trio"; do
    for album in "First Light" "Second Wind"; do
      d="$music/$artist/$album"; mkdir -p "$d"
      ffmpeg -loglevel error -f lavfi -i "color=c=0x$(printf '%02x%02x%02x' $((n*40%256)) $((n*90%256)) $((n*150%256))):s=600x600" -frames:v 1 "$d/cover.jpg"
      for t in 1 2 3 4; do
        n=$((n+1)); ext=mp3; [ $((n%3)) = 0 ] && ext=flac
        ffmpeg -loglevel error -f lavfi -i "sine=frequency=$((200+n*25)):duration=$((40+n*3))" \
          -metadata title="Track $t of $album" -metadata artist="$artist" -metadata album_artist="$artist" \
          -metadata album="$album" -metadata track="$t" -metadata date="$((2000+n))" -metadata genre="Test" \
          "$d/0$t - Track $t.$ext"
      done
    done
  done
fi
docker rm -f flint-navidrome >/dev/null 2>&1 || true
docker run -d --name flint-navidrome --user "$(id -u):$(id -g)" -p 4533:4533 \
  -e ND_SCANNER_SCHEDULE=@every\ 1m -e ND_LOGLEVEL=info \
  -v "$music:/music:ro" -v "$root/data:/data" deluan/navidrome:latest >/dev/null
for _ in $(seq 30); do curl -sf localhost:4533/ping >/dev/null && break; sleep 1; done
curl -sf -X POST localhost:4533/auth/createAdmin -H 'content-type: application/json' \
  -d '{"username":"admin","password":"admin"}' >/dev/null || true
echo "navidrome: http://localhost:4533  (admin/admin)"
