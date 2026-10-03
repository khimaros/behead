#!/bin/sh
# the demo as a wayland client in the kiosk, which shows that touch and
# buttons make it through a compositor. use it as
#
#   behead --uinput --video-cmd 'sessions/wayland.sh {width} {height} {fps}'
exec "$(dirname "$0")/kiosk.sh" "$@" behead-demo --wayland
