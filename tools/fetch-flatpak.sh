#!/bin/sh
# applications from flathub, for a machine of another kind than this one,
# with the runtimes they need: installed into a cache here, which flatpak
# does without running any of it, and copied to where an image will carry it.
#
#   tools/fetch-flatpak.sh ARCH DEST APPLICATION...
#
# DEST is a whole flatpak installation, the kind a user has, for the sessions
# to name in FLATPAK_USER_DIR: an image is built with no network, and a
# device has none to fetch a gigabyte over. `-` for DEST leaves it in the
# cache, as tools/fetch-maps.sh wants its own machine's copy. what flathub
# calls stable today is what is fetched: nothing is pinned.
set -eu

CACHE=${BEHEAD_FLATPAK_CACHE:-$HOME/.cache/behead/flatpak}
FLATHUB=https://dl.flathub.org/repo/flathub.flatpakrepo

[ $# -ge 3 ] || { echo "usage: fetch-flatpak.sh ARCH DEST APPLICATION..." >&2; exit 2; }
arch=$1 dest=$2
shift 2

export FLATPAK_USER_DIR="$CACHE/$arch"
mkdir -p "$FLATPAK_USER_DIR"
flatpak --user remote-add --if-not-exists flathub "$FLATHUB"
flatpak --user install --or-update --arch="$arch" --noninteractive -y flathub "$@" >&2
[ "$dest" != - ] || exit 0
rm -rf "$dest"
mkdir -p "$(dirname "$dest")"
# -a keeps the hard links between the store and what is checked out of it,
# without which the copy is twice the size
cp -a --reflink=auto "$FLATPAK_USER_DIR" "$dest"
# the cache keeps whatever was ever asked of it. the copy has what was asked
# this time, and the runtimes that go with it
export FLATPAK_USER_DIR="$dest"
for installed in $(flatpak --user list --app --arch="$arch" --columns=application); do
    case " $* " in *" $installed "*) ;; *) flatpak --user uninstall --arch="$arch" --noninteractive -y "$installed" >&2 ;; esac
done
flatpak --user uninstall --unused --arch="$arch" --noninteractive -y >&2
echo "$*: for $arch in $dest ($(du -sh "$dest" | cut -f1))"
