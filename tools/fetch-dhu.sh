#!/bin/sh
# install Google's DHU from the Android SDK, for `make demo-dhu` and `make
# test-dhu`. it links libc++, which Debian ships apart from the compiler:
# when the system lacks it, the Debian packages are unpacked into LIBS
# instead of installed.
#
#   tools/fetch-dhu.sh DHU_DIR LIBS
set -eu
DHU=$1/desktop-head-unit
LIBS=$2

# sdkmanager asks to accept Google's license, so this stays interactive
[ -x "$DHU" ] || sdkmanager "extras;google;auto"
missing() { LD_LIBRARY_PATH=$LIBS ldd "$DHU" | grep 'not found'; }
if missing > /dev/null; then
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    (cd "$work" && apt-get download libc++1 libc++abi1)
    for deb in "$work"/*.deb; do dpkg-deb -x "$deb" "$work/root"; done
    mkdir -p "$LIBS"
    cp -a "$work"/root/usr/lib/*/libc++*.so.* "$LIBS"/
fi
if missing; then
    echo "the DHU still lacks the libraries above" >&2
    exit 1
fi
LD_LIBRARY_PATH=$LIBS "$DHU" --version
