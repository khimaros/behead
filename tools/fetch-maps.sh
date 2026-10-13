#!/bin/sh
# the parts of the map a device shows with no network: fetched here by gnome
# maps' own downloader (tools/fetch-map-areas.js), in gnome maps' flatpak for
# this machine, and left in DEST as the two files it keeps them in.
#
#   tools/fetch-maps.sh DEST 'NAME=WEST,SOUTH,EAST,NORTH'...
#
# sessions/phosh.sh puts them where the device's gnome maps reads its own. the
# areas fetched before are kept in a cache and brought up to date, so a build
# fetches again only what is new. needs flatpak and the network.
set -eu

MAPS=org.gnome.Maps
CACHE=${BEHEAD_MAPS_CACHE:-$HOME/.cache/behead/maps}
# where a flatpak application keeps its data, under the home it is run with
DATA=.var/app/$MAPS/data/gnome-maps
HERE=$(cd "$(dirname "$0")" && pwd)

[ $# -ge 2 ] || { echo "usage: fetch-maps.sh DEST NAME=WEST,SOUTH,EAST,NORTH..." >&2; exit 2; }
dest=$1
shift

arch=$(flatpak --default-arch)
"$HERE/fetch-flatpak.sh" "$arch" - "$MAPS"
mkdir -p "$CACHE"
installation="${BEHEAD_FLATPAK_CACHE:-$HOME/.cache/behead/flatpak}/$arch"
FLATPAK_USER_DIR="$installation" HOME="$CACHE" \
    flatpak --user run --command=gjs --filesystem="$HERE":ro "$MAPS" -m "$HERE/fetch-map-areas.js" "$@" >&2
mkdir -p "$dest"
cp "$CACHE/$DATA/downloads.db" "$CACHE/$DATA/downloads.json" "$dest/"
echo "maps: $(du -sh "$dest" | cut -f1) in $dest"
