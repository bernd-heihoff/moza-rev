#!/usr/bin/env bash

set -u

BRIDGE="/home/bernd/moza-rev/tools/acr-ams2-bridge/acr_to_ams2.exe"
LAUNCHER="/home/bernd/.local/bin/protontricks-launch"
LOG="/tmp/acr-bridge.log"

bridge_pid=""

cleanup() {
    if [[ -n "$bridge_pid" ]]; then
        kill -TERM "$bridge_pid" 2>/dev/null || true
        wait "$bridge_pid" 2>/dev/null || true
    fi
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

{
    echo
    echo "============================================================"
    echo "ACR launch: $(date --iso-8601=seconds)"
    echo "============================================================"
} >"$LOG"

#
# Start bridge first.
#
# The bridge itself waits until Local\acpmf_physics exists.
#
"$LAUNCHER" \
    --no-bwrap \
    --appid 3917090 \
    "$BRIDGE" >>"$LOG" 2>&1 &

bridge_pid=$!

echo "Bridge launcher PID: $bridge_pid" >>"$LOG"

#
# Run Steam's original launch command normally, in the foreground.
#
# Steam's reaper/waitforexitandrun machinery is responsible for
# tracking the actual game. We don't try to second-guess it.
#
"$@"
game_status=$?

echo "Steam launch command exited: $game_status" >>"$LOG"

exit "$game_status"
