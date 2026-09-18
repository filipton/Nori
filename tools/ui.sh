#!/usr/bin/env bash
# Tiny UI driver over uiautomator, resolution independent. Honours ANDROID_SERIAL.
#   tools/ui.sh tap "Text or content-desc"      taps the first node with exactly that label
#   tools/ui.sh tapn "Label" 2                  the n-th match (1-based)
#   tools/ui.sh has "Label"                     exit 0 when on screen
#   tools/ui.sh texts                           everything readable on screen
# uiautomator occasionally returns "null root node" right after an activity change; try again.
dump() {
  for _ in 1 2 3; do
    adb shell uiautomator dump /sdcard/ui.xml >/dev/null 2>&1
    local x; x=$(adb shell cat /sdcard/ui.xml 2>/dev/null)
    case "$x" in *"<hierarchy"*) printf '%s' "$x"; return 0 ;; esac
    sleep 1
  done
  return 1
}
centre() { python3 -c "
import sys,re
x=sys.stdin.read(); t=sys.argv[1]; n=int(sys.argv[2])
m=list(re.finditer(r'<node[^>]*?(?:text|content-desc)=\"'+re.escape(t)+r'\"[^>]*?bounds=\"\[(\d+),(\d+)\]\[(\d+),(\d+)\]\"',x))
if len(m)>=n:
    a,b,c,d=map(int,m[n-1].groups()); print((a+c)//2,(b+d)//2)
" "$1" "${2:-1}"; }
# The on-screen keyboard is the enemy of UI automation: it covers the bottom of the screen, so taps
# meant for the app land on keys, and a fling across it is glide typing, which quietly fills whatever
# text field has focus. `input text` injects key events and needs no IME at all, so a scripted run
# simply turns the keyboard off.
#   tools/ui.sh kb off   disables every enabled IME (remembering them)
#   tools/ui.sh kb on    puts them back
kb() {
  local store=/sdcard/flint-imes
  case "$2" in
    off) adb shell "ime list -s | tr -d '\r' > $store; for i in \$(cat $store); do ime disable \$i; done" >/dev/null 2>&1 ;;
    on)  adb shell "for i in \$(cat $store 2>/dev/null); do ime enable \$i; done; ime set \$(head -1 $store 2>/dev/null)" >/dev/null 2>&1 ;;
    *) echo "usage: ui.sh kb off|on" >&2; return 2 ;;
  esac
}

case "$1" in
  kb) kb "$@" ;;
  tap|tapn) read -r x y < <(dump | centre "$2" "${3:-1}"); [ -n "${x:-}" ] && adb shell input tap "$x" "$y" || { echo "not on screen: $2" >&2; exit 1; } ;;
  has) [ -n "$(dump | centre "$2" 1)" ] ;;
  texts) dump | grep -oE '(text|content-desc)="[^"]+"' | sed -E 's/^[a-z-]+="//; s/"$//' ;;
  *) echo "usage: ui.sh tap|tapn|has|texts|kb" >&2; exit 2 ;;
esac
