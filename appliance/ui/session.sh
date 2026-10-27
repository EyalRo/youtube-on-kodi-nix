#!/bin/sh
# Runs the UI; when it asks (exit status 10), hands the display to `play`, then starts it again.
#
# Between processes the kernel shows the fbdev buffer, which is the text console unless something
# else was written there. So tty1 goes to graphics mode once, and the splash is painted before
# every handoff: it is what the screen shows while the next process starts.
state="${UI_STATE:-/tmp/appliance-ui}"
here="$(dirname "$(readlink -f "$0")")"
splash() { "$here/fbsplash" "${UI_SPLASH:-$here/../share/appliance-ui/logo-1400x300.gray}"; }
"$here/fbsplash" --vt-graphics
splash
# A fresh start (boot, or the service restarting) opens on the home screen's first tile. The focus and
# page files are only for coming back from playing something, which stays inside the loop below.
rm -f "$state/focus" "$state/detail"
while :; do
    rm -f "$state/launch"
    appliance-ui
    rc=$?
    if [ "$rc" -eq 10 ] && [ -s "$state/launch" ]; then
        "${UI_PLAY:-play}" "$(cat "$state/launch")"
        splash
        continue
    fi
    exit "$rc"
done
