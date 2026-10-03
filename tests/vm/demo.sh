#!/bin/bash
# interactive demo, run inside the test vm: the server on the usb gadget and
# openauto as the headunit, with openauto's screen served to a browser.
# touch and button events from openauto are printed here. ctrl-c stops it.
set -euo pipefail

REPO=/incant
SERVER=$REPO/target/debug/behead
# the default video command runs behead-demo from the path
export PATH=$REPO/target/debug:$PATH
CONTAINER=behead-openauto
WORK=$(mktemp -d)
# `make demo` has checked that SESSION names a session
[ -z "${SESSION:-}" ] || VIDEO_CMD="$REPO/sessions/$SESSION.sh {width} {height} {fps}"
VIDEO_CMD=${VIDEO_CMD:-$(sed -n 's/^VIDEO_CMD="\(.*\)"$/\1/p' "$REPO/rpi/overlay/etc/behead/behead.env")}
ADDRESS=$(ip -4 -o addr show scope global | awk '$2 != "podman0" {sub("/.*", "", $4); print $4; exit}')

cleanup() {
    podman rm -f "$CONTAINER" >/dev/null 2>&1 || true
    [ -n "${SERVER_PID:-}" ] && kill -TERM "$SERVER_PID" 2>/dev/null && wait "$SERVER_PID" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

modprobe -a dummy_hcd libcomposite uinput
podman rm -f "$CONTAINER" >/dev/null 2>&1 || true
"$SERVER" teardown
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj /O=demo \
    -keyout "$WORK/phone.key" -out "$WORK/phone.crt" >/dev/null 2>&1

# --uinput lets a desktop session receive openauto's clicks
"$SERVER" --usb auto --cert "$WORK/phone.crt" --key "$WORK/phone.key" --video-cmd "$VIDEO_CMD" --uinput &
SERVER_PID=$!
sleep 1

# openauto in a container misses the re-enumeration after the accessory
# switch, so do the switch first, the way the tests do
python3 -c "
import sys, usb.util
sys.path[:0] = ['$REPO/tests/e2e', '$REPO/tests/vm']
import test_usb
usb.util.dispose_resources(test_usb.switch_to_accessory())"

podman run -d --name "$CONTAINER" --privileged --net=host -e DISPLAY=:99 -e VNC=1 \
    -v /dev/bus/usb:/dev/bus/usb -v /run/udev:/run/udev:ro "$CONTAINER" >/dev/null

echo
echo "openauto is up. open in a browser on the laptop:"
echo "    http://$ADDRESS:6080/vnc.html?autoconnect=1&resize=scale"
echo "or point a vnc viewer at $ADDRESS:5900. ctrl-c here to stop."
echo
wait "$SERVER_PID"
