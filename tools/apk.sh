#!/usr/bin/env bash
# Builds a release APK you can put on a phone.
#   tools/apk.sh                 arm64 (every phone made in the last decade)
#   tools/apk.sh x86_64          for an emulator
#   tools/apk.sh arm64-v8a,x86_64   both ABIs in one (bigger) APK
#   tools/apk.sh --install       build arm64 and push it to the connected device
#
# The APK is signed with the Android debug key, which is fine for your own phone: it installs and
# updates normally, but it cannot be published, and switching later to a real key means uninstalling
# first. The Rust core is cross-compiled for exactly the ABIs asked for, so nothing unused is shipped.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"; root="$(cd "$here/.." && pwd)"
install=false
abis=arm64-v8a
for arg in "$@"; do
  case "$arg" in
    --install) install=true ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *) abis="$arg" ;;
  esac
done
version=$(grep -oE 'versionName = "[^"]+"' "$root/app/build.gradle.kts" | head -1 | cut -d'"' -f2)
out="$root/build/nori-music-$version-${abis//,/+}.apk"

echo "building $version for $abis …"
(cd "$root" && ./gradlew :app:assembleRelease -PrustTargets="$abis" -q)
mkdir -p "$root/build"
cp "$root/app/build/outputs/apk/release/app-release.apk" "$out"

size=$(du -h "$out" | cut -f1)
echo "$out  ($size)"
unzip -l "$out" | grep -oE 'lib/[a-z0-9_-]+/' | sort -u | sed 's/^/  contains /'
if $install; then
  adb install -r "$out" && echo "installed on $(adb shell getprop ro.product.model | tr -d '\r')"
fi
