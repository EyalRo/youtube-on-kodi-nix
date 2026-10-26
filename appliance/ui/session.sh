#!/bin/sh
# Runs the UI; when it asks (exit status 10), hands the display to `play`, then starts it again.
state="${UI_STATE:-/tmp/appliance-ui}"
while :; do
    rm -f "$state/launch"
    appliance-ui
    rc=$?
    if [ "$rc" -eq 10 ] && [ -s "$state/launch" ]; then
        "${UI_PLAY:-play}" "$(cat "$state/launch")"
        continue
    fi
    exit "$rc"
done
