#!/bin/sh
# the launcher: a home screen made for the car, with a music player. it
# needs no compositor, for it draws its own pictures, which ffmpeg encodes.
# use it as
#
#   behead --video-cmd 'sessions/launcher.sh {width} {height} {fps}' \
#       --audio-cmd 'sessions/launcher.sh --audio {rate} {channels}'
#
# it plays the music under BEHEAD_MEDIA/music, where kodi.sh looks too, or
# without BEHEAD_MEDIA under ~/Music.
set -eu
. "$(dirname "$0")/encoder.sh"

# what the launcher plays as, which is what cars take as media audio
SOUND_FORMAT="48000 2"
# the launcher writes what it plays to this pipe, and the audio command
# reads it. whichever of the two the server starts first makes it. on the pi
# it is in the service's runtime directory
pipe=${BEHEAD_LAUNCHER_PIPE:-${RUNTIME_DIRECTORY:-${XDG_RUNTIME_DIR:-/tmp}}/behead-launcher-audio}
mkfifo "$pipe" 2>/dev/null || [ -p "$pipe" ]

if [ "$1" = --audio ]; then
    # no sound is better than sound at the wrong speed
    [ "$2 $3" = "$SOUND_FORMAT" ] || { echo "the launcher plays at $SOUND_FORMAT, not $2 $3" >&2; exit 1; }
    exec cat "$pipe"
fi

music=${BEHEAD_MEDIA:+$BEHEAD_MEDIA/music}
music=${music:-$HOME/Music}
mkdir -p "$music"
# ffmpeg's arguments for the encoder: the pi's own for a picture large
# enough to gain from it, as for the sessions that record a compositor
case $(encoder "$1" "$2" "$3") in
    v4l2m2m) encode="-c:v h264_v4l2m2m -b:v $(hardware_bitrate "$1" "$2" "$3") -profile:v $BASELINE_PROFILE" ;;
    *) encode="-c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline" ;;
esac
# shellcheck disable=SC2086
behead-launcher --width "$1" --height "$2" --fps "$3" --music "$music" --audio "$pipe" |
    ffmpeg -loglevel error -f rawvideo -pix_fmt rgb24 -s "$1x$2" -r "$3" -i - -pix_fmt yuv420p $encode -g "$3" -f h264 -
