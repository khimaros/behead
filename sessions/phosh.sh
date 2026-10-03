#!/bin/sh
# the phosh mobile shell on the headunit: a headless phoc at the negotiated
# mode, captured to annex-b h.264 on stdout. use it as
#
#   behead --uinput --video-cmd 'sessions/phosh.sh {width} {height} {fps}'
#
# like kiosk.sh, input arrives through the --uinput devices and everything
# renders on the cpu unless BEHEAD_RENDERER says otherwise. phosh runs
# without gnome-session, on a session bus of its own, beside a pulseaudio
# that `--audio-cmd 'sessions/audio.sh {rate} {channels}'` records for the car.
set -eu
. "$(dirname "$0")/lib.sh"

width=$1 height=$2 fps=$3
PHOSH=${PHOSH:-/usr/libexec/phosh}

# a car screen should neither lock nor blank. with location on, phosh lets
# applications ask geoclue where the car is (see geoclue.conf)
export GSETTINGS_BACKEND=keyfile XDG_CONFIG_HOME="$runtime/config"
mkdir -p "$XDG_CONFIG_HOME/glib-2.0/settings"
cat > "$XDG_CONFIG_HOME/glib-2.0/settings/keyfile" <<EOF
[org/gnome/desktop/screensaver]
lock-enabled=false

[org/gnome/desktop/session]
idle-delay=uint32 0

[org/gnome/system/location]
enabled=true
EOF

cat > "$runtime/phoc.ini" <<EOF
[core]
xwayland=false

[output:HEADLESS-1]
mode = ${width}x${height}
scale = 1
EOF

# follow the car's night mode with the dark style applications ask for
sensor() {
    [ "$1" = night ] || return 0
    case $2 in 1) style=prefer-dark ;; *) style=default ;; esac
    gsettings set org.gnome.desktop.interface color-scheme "$style"
}

start_session_bus
start_sound
# phosh locks at startup unless a display manager, which sets GDMSESSION,
# already let the user in
run phoc --shell --config "$runtime/phoc.ini" --exec "env GDMSESSION=phosh $PHOSH"
wait_for_display
on_line sensor
capture HEADLESS-1 "$width" "$height" "$fps"
