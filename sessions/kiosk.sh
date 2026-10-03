#!/bin/sh
# one wayland application full screen on the headunit: a headless sway at
# the negotiated mode, captured to annex-b h.264 on stdout. use it as
#
#   behead --uinput --video-cmd 'sessions/kiosk.sh {width} {height} {fps} APP [ARGS]...'
#
# touch and buttons reach the application through the --uinput devices,
# which sway's libinput backend picks up. renders on the cpu, so it needs no
# gpu, and opens input devices without a login session, so it runs as root.
set -eu
. "$(dirname "$0")/lib.sh"

width=$1 height=$2 fps=$3
shift 3

cat > "$runtime/config" <<EOF
output HEADLESS-1 mode ${width}x${height}@${fps}Hz
input type:touch map_to_output HEADLESS-1
xwayland disable
default_border none
seat * hide_cursor 100
for_window [all] fullscreen enable
exec $*
EOF

start_session_bus
run sway --config "$runtime/config"
wait_for_display
capture HEADLESS-1 "$width" "$height" "$fps"
