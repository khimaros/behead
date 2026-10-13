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
KEYBOARD=squeekboard
# the style applications are asked for. dark, which is easier on the eyes in
# a car, unless BEHEAD_COLOR_SCHEME says light, or car: dark until the car
# says it is day, and then as the car says
DARK=prefer-dark LIGHT=default
# the gtk theme that goes with each. newer applications follow the style
# alone; the keyboard on the screen, and gtk 3, go by the theme's name
DARK_THEME=Adwaita-dark LIGHT_THEME=Adwaita
# and the map comaps draws, which follows neither: it reads one of these
# names from its settings when it starts
DARK_MAP=MapStyleDefaultDark LIGHT_MAP=MapStyleDefaultLight
# the pointer's theme, where the names of the usual one's pictures are, and
# an xcursor file of one clear pixel: a header and a list of one picture,
# then the picture, each number four bytes, the low one first
POINTER_THEME=behead-hidden
POINTER_NAMES=/usr/share/icons/Adwaita/cursors
CLEAR_CURSOR='Xcur\020\0\0\0\0\0\001\0\001\0\0\0\002\0\375\377\001\0\0\0\034\0\0\0'
CLEAR_CURSOR="$CLEAR_CURSOR"'\044\0\0\0\002\0\375\377\001\0\0\0\001\0\0\0\001\0\0\0\001\0\0\0\0\0\0\0\0\0\0\0\001\0\0\0\0\0\0\0'
LOCATION_APPLICATIONS="org.gnome.Maps app.comaps.comaps"
# what an image brings beside its packages (tools/fetch-flatpak.sh,
# tools/fetch-maps.sh and tools/fetch-comaps.py): applications as a flatpak
# installation, the parts of the map gnome maps is to show with no network,
# and the regions comaps shows, searches and routes on
FLATPAKS=${BEHEAD_FLATPAKS:-/usr/share/behead/flatpak}
MAPS=${BEHEAD_MAPS:-/usr/share/behead/maps}
COMAPS=${BEHEAD_COMAPS:-/usr/share/behead/comaps}
# where comaps, as a flatpak, keeps its regions, a directory for each version
# of the data, and its settings
COMAPS_HOME=.var/app/app.comaps.comaps
COMAPS_DATA=$COMAPS_HOME/data/CoMaps
COMAPS_SETTINGS=$COMAPS_HOME/config/CoMaps/settings.ini
COMAPS_ENTRY=app.comaps.comaps.desktop
# the music player, a flatpak too, and where it keeps its settings
GAPLESS=com.github.neithern.g4music
GAPLESS_SETTINGS=.var/app/$GAPLESS/config/glib-2.0/settings/keyfile
# where gnome maps, as a flatpak, keeps the areas it has fetched, and beside
# them the list the image's came with
MAPS_DATA=.var/app/org.gnome.Maps/data/gnome-maps
MAPS_FROM_IMAGE=.from-image
scheme=${BEHEAD_COLOR_SCHEME:-dark}
case $scheme in
    dark | car) style=$DARK theme=$DARK_THEME map=$DARK_MAP ;;
    light) style=$LIGHT theme=$LIGHT_THEME map=$LIGHT_MAP ;;
    *) echo "BEHEAD_COLOR_SCHEME is $scheme: dark, light or car" >&2; exit 1 ;;
esac

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

[org/gnome/desktop/a11y/applications]
screen-keyboard-enabled=true

[org/gnome/desktop/interface]
color-scheme='$style'
gtk-theme='$theme'
cursor-theme='$POINTER_THEME'

[org/gnome/shell/keybindings]
toggle-overview=['XF86HomePage']
EOF

cat > "$runtime/phoc.ini" <<EOF
[core]
xwayland=false

[output:HEADLESS-1]
mode = ${width}x${height}
scale = 1
EOF

# follow the car's night mode with the style applications ask for, when asked to
sensor() {
    [ "$1" = night ] && [ "$scheme" = car ] || return 0
    case $2 in 1) style=$DARK theme=$DARK_THEME ;; *) style=$LIGHT theme=$LIGHT_THEME ;; esac
    gsettings set org.gnome.desktop.interface color-scheme "$style"
    gsettings set org.gnome.desktop.interface gtk-theme "$theme"
}

# the sandboxed applications that may know where the car is unasked. the
# desktop portal would otherwise have whoever is driving answer for each, and
# gnome maps shows no window until somebody has
allow_location() {
    command -v flatpak > /dev/null || return 0
    for application in $LOCATION_APPLICATIONS; do
        flatpak permission-set location location "$application" EXACT 0 || true
    done
}

# the shell lists and starts the image's flatpak applications as it does the
# packaged ones
use_flatpaks() {
    [ -d "$FLATPAKS" ] || return 0
    export FLATPAK_USER_DIR="$FLATPAKS"
    export XDG_DATA_DIRS="$FLATPAKS/exports/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
}

# hand gnome maps the image's areas as if it had fetched them itself. once
# for each list an image comes with: after that they are gnome maps' own, to
# bring up to date when there is a network and to add to
install_maps() {
    [ -f "$MAPS/downloads.json" ] || return 0
    data="$HOME/$MAPS_DATA"
    ! cmp -s "$MAPS/downloads.json" "$data/$MAPS_FROM_IMAGE" || return 0
    mkdir -p "$data"
    cp "$MAPS/downloads.db" "$MAPS/downloads.json" "$data/"
    cp "$MAPS/downloads.json" "$data/$MAPS_FROM_IMAGE"
}

# hand comaps the image's regions. copied, because a sandbox sees nothing of
# the image's own directories, and comaps removes a region it cannot read.
# -u, so that a region is copied once, and again only when an image brings a
# newer one. and its settings are told three things, beside whatever else is
# in them: its first start's two questions are answered, the licences and a
# page that offers to download what is already there, and its map is the
# session's style, as it was when the session started
install_comaps() {
    [ -d "$COMAPS" ] || return 0
    for regions in "$COMAPS"/*/; do
        [ -d "$regions" ] || continue
        data="$HOME/$COMAPS_DATA/$(basename "$regions")"
        mkdir -p "$data"
        cp -u "$regions"*.mwm "$data/"
    done
    settings="$HOME/$COMAPS_SETTINGS"
    mkdir -p "$(dirname "$settings")"
    {
        grep -v -e '^EulaAccepted=' -e '^ShowWelcome=' -e '^MapStyleKeyV1=' "$settings" 2> /dev/null || true
        printf 'EulaAccepted=true\nShowWelcome=false\nMapStyleKeyV1=%s\n' "$map"
    } > "$settings.new"
    mv "$settings.new" "$settings"
    fit_comaps "$(dirname "$settings")/behead.qss"
}

# comaps is made for a mouse: a button's list opens for the small arrow
# beside it, and the buttons a screen this size has no room for are behind a
# strip at the foot of the others that is hard to see, let alone touch. a qt
# style sheet at SHEET has a list open for a finger that stays on its button,
# and makes the strip a button. comaps takes it only from an icon of the
# session's own, in front of the one flatpak made: its arguments have to end
# in "--" for qt to see what follows
fit_comaps() {
    entry="$FLATPAKS/exports/share/applications/$COMAPS_ENTRY"
    [ -f "$entry" ] || return 0
    cat > "$1" <<EOF
QToolButton[popupMode="1"] { qproperty-popupMode: DelayedPopup; }
QToolBarExtension { background: #777777; margin-top: -24px; min-height: 36px; }
EOF
    mkdir -p "$HOME/.local/share/applications"
    sed "s|^Exec=.*|& -- -stylesheet=$1|" "$entry" > "$HOME/.local/share/applications/$COMAPS_ENTRY"
}

# a car has nothing to move a pointer with, and the knob as a wheel
# (--knob scroll) still puts one on the screen. so the pointer's pictures
# are a theme of this session's own: one clear dot, under every name the
# usual theme has, or whoever asks for a name it lacks is given the usual one
hide_pointer() {
    cursors="$HOME/.local/share/icons/$POINTER_THEME/cursors"
    mkdir -p "$cursors"
    # shellcheck disable=SC2059
    printf "$CLEAR_CURSOR" > "$cursors/default"
    for cursor in "$POINTER_NAMES"/*; do
        [ -e "$cursors/$(basename "$cursor")" ] || ln -s default "$cursors/$(basename "$cursor")"
    done
    export XCURSOR_THEME="$POINTER_THEME"
}

hide_pointer
# gapless plays the music under BEHEAD_MEDIA, where kodi and the launcher
# look. as a flatpak it sees ~/Music and no more, so it is let into that
# directory, to read, and told to look there: once, after which where it
# looks is its own setting to change
share_music() {
    [ -n "${BEHEAD_MEDIA:-}" ] && [ -d "$FLATPAKS/app/$GAPLESS" ] || return 0
    flatpak override --user "--filesystem=$BEHEAD_MEDIA/music:ro" "$GAPLESS" || true
    settings="$HOME/$GAPLESS_SETTINGS"
    [ ! -f "$settings" ] || return 0
    mkdir -p "$(dirname "$settings")"
    printf "[com/github/neithern/g4music]\nmusic-dir='file://%s'\n" "$BEHEAD_MEDIA/music" > "$settings"
}

use_flatpaks
install_maps
install_comaps
share_music
start_session_bus
allow_location
start_sound
# phosh locks at startup unless a display manager, which sets GDMSESSION,
# already let the user in
run phoc --shell --config "$runtime/phoc.ini" --exec "env GDMSESSION=phosh $PHOSH"
wait_for_display
# a car has no keyboard to type with. gnome-session would start the one on
# screen, and there is none here, so it is started beside the shell, which
# shows it (screen-keyboard-enabled above) when something wants text
if command -v "$KEYBOARD" > /dev/null; then run "$KEYBOARD"; fi
on_line sensor
capture HEADLESS-1 "$width" "$height" "$fps"
