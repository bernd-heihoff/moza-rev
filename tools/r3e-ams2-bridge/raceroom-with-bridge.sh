#!/usr/bin/env bash

set -u

BRIDGE="/home/bernd/moza-rev/tools/r3e-ams2-bridge/r3e_to_ams2.exe"
LAUNCHER="/home/bernd/.local/bin/protontricks-launch"
LOG="/tmp/r3e-bridge-auto.log"

bridge_pid=""

stop_bridge() {
    if [[ -n "$bridge_pid" ]]; then
        kill "$bridge_pid" 2>/dev/null || true
        wait "$bridge_pid" 2>/dev/null || true
    fi

    # Ensure the Windows-side process is also gone.
    pkill -TERM -f '[r]3e_to_ams2\.exe' 2>/dev/null || true
}

trap stop_bridge EXIT INT TERM

{
    printf '\n============================================================\n'
    printf 'RaceRoom bridge launch: %s\n' "$(date --iso-8601=seconds)"
    printf '============================================================\n'
} > "$LOG"

if [[ ! -x "$LAUNCHER" ]]; then
    printf 'Missing launcher: %s\n' "$LAUNCHER" >> "$LOG"
    exit 1
fi

if [[ ! -f "$BRIDGE" ]]; then
    printf 'Missing bridge: %s\n' "$BRIDGE" >> "$LOG"
    exit 1
fi

printf 'Starting RaceRoom command\n' >> "$LOG"

"$@" &
game_pid=$!

# Let Proton begin creating the RaceRoom prefix processes.
sleep 3

while kill -0 "$game_pid" 2>/dev/null; do
    printf '\nLaunching telemetry bridge at %s\n' \
        "$(date --iso-8601=seconds)" >> "$LOG"

    "$LAUNCHER" \
        --no-bwrap \
        --appid 211500 \
        "$BRIDGE" >> "$LOG" 2>&1 &

    bridge_pid=$!

    # Wait until either the bridge or RaceRoom exits.
    while kill -0 "$game_pid" 2>/dev/null &&
          kill -0 "$bridge_pid" 2>/dev/null
    do
        sleep 1
    done

    if ! kill -0 "$game_pid" 2>/dev/null; then
        break
    fi

    # The mapping may not have existed yet. Retry after a short pause.
    wait "$bridge_pid" 2>/dev/null || true
    bridge_pid=""

    printf 'Bridge exited; retrying in two seconds\n' >> "$LOG"
    sleep 2
done

wait "$game_pid"
game_status=$?

printf '\nRaceRoom exited with status %d\n' "$game_status" >> "$LOG"

stop_bridge
trap - EXIT INT TERM

exit "$game_status"
