#!/bin/sh
# what a session plays, as pcm on stdout. use it as
#
#   behead --audio-cmd 'sessions/audio.sh {rate} {channels}'
#
# beside a session that plays into one end of an alsa loopback, as kodi.sh
# has kodi do; this records the other end. needs the snd-aloop kernel module
# and arecord, from alsa-utils.
set -eu

# the session loads the module too, but the car may ask for sound first
modprobe snd-aloop
exec arecord -q -D hw:CARD=Loopback,DEV=1 -f S16_LE -r "$1" -c "$2" -t raw
