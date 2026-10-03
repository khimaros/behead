#!/bin/sh
# kodi on the headunit, full screen in the kiosk. use it as
#
#   behead --uinput --video-cmd 'sessions/kodi.sh {width} {height} {fps}'
#
# with BEHEAD_MEDIA set, a kodi that has no sources yet gets that
# directory's videos and music as its sources. sources set up in kodi are
# never replaced. kodi-library.py then puts what the two hold into kodi's
# library, the videos as movies named by their files. needs python3.
#
# for sound, a kodi that has no settings yet plays into one end of an alsa
# loopback, and `--audio-cmd 'sessions/audio.sh {rate} {channels}'` records
# the other for the car. kodi keeps the device it finds at its first start,
# so the loopback has to be there by then. needs the snd-aloop module.
#
# a kodi starting for the first time would ask whether to enable each add-on
# the system's packages brought. kodi-addons.py switches them on beforehand.
set -eu
. "$(dirname "$0")/home.sh"

userdata="$HOME/.kodi/userdata"
sources="$userdata/sources.xml"
settings="$userdata/guisettings.xml"
# kodi's name for the loopback's playback end
AUDIO_DEVICE="ALSA:@:CARD=Loopback,DEV=0|Loopback (@:CARD=Loopback,DEV=0)"

mkdir -p "$userdata"
modprobe snd-aloop 2> /dev/null || true
[ -e "$settings" ] || cat > "$settings" <<EOF
<settings version="2">
    <setting id="audiooutput.audiodevice">$AUDIO_DEVICE</setting>
</settings>
EOF
python3 "$(dirname "$0")/kodi-addons.py" "$userdata" >&2

# one of kodi's source lists: source_list KIND DIRECTORY
source_list() {
    cat <<EOF
    <$1>
        <default pathversion="1"></default>
        <source>
            <name>$(basename "$2")</name>
            <path pathversion="1">$2/</path>
            <allowsharing>true</allowsharing>
        </source>
    </$1>
EOF
}

if [ -n "${BEHEAD_MEDIA:-}" ]; then
    mkdir -p "$BEHEAD_MEDIA/videos" "$BEHEAD_MEDIA/music"
    if [ ! -e "$sources" ]; then
        { echo "<sources>"; source_list video "$BEHEAD_MEDIA/videos"; source_list music "$BEHEAD_MEDIA/music"
          echo "</sources>"; } > "$sources"
    fi
    # its output must stay out of the video on stdout
    python3 "$(dirname "$0")/kodi-library.py" "$BEHEAD_MEDIA" "$userdata" $$ < /dev/null >&2 &
fi

exec "$(dirname "$0")/kiosk.sh" "$@" kodi --windowing=wayland
