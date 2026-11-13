#!/bin/bash
# interactive demo on this machine: the server over TCP, and Google's DHU
# as the car, in a window to drive with the mouse. its console here takes
# commands too: `help` lists them. touch and button
# events reaching the server go to target/demo-dhu.log. `quit` or ctrl-c
# stops both.
set -euo pipefail

REPO=$(cd "$(dirname "$0")/../.." && pwd)
SERVER=$REPO/target/debug/behead
# the default video command runs behead-demo from the path
export PATH=$REPO/target/debug:$PATH
VIDEO_CMD=${VIDEO_CMD:-$(sed -n 's/^VIDEO_CMD="\(.*\)"$/\1/p' "$REPO/rpi/overlay/etc/behead/behead.env")}
CERT=$CERT_CACHE/carservice
LOG=$REPO/target/demo-dhu.log
PORT=5277

[ -x "$BEHEAD_DHU_DIR/desktop-head-unit" ] || { echo "no DHU: run \`make dhu\`" >&2; exit 1; }
[ -f "$CERT.key" ] || { echo "it needs a Google signed certificate in $CERT_CACHE: see docs/certificates.md" >&2; exit 1; }

cleanup() {
    # the server ends by this signal, which is no failure of the demo
    kill -TERM "$SERVER_PID" 2>/dev/null && wait "$SERVER_PID" 2>/dev/null || true
}

"$SERVER" --listen "127.0.0.1:$PORT" --cert "$CERT.crt" --key "$CERT.key" --video-cmd "$VIDEO_CMD" > "$LOG" 2>&1 &
SERVER_PID=$!
trap cleanup EXIT
sleep 1
echo "server log: $LOG"
cd "$BEHEAD_DHU_DIR"
LD_LIBRARY_PATH=$BEHEAD_DHU_LIBS ./desktop-head-unit --config="$REPO/tests/dhu/headunit.ini" --adb="localhost:$PORT"
