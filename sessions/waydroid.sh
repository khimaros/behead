#!/bin/sh
# android on the headunit: waydroid's full ui in the kiosk session, so the
# car's touch and buttons reach android apps through the --uinput devices.
# use it as
#
#   behead --uinput --video-cmd 'sessions/waydroid.sh {width} {height} {fps}'
#
# where waydroid has no state yet, as on a read-only root after each boot,
# this sets it up: `waydroid init`, which downloads android's images unless
# they are in /usr/share/waydroid-extra/images, and android rendering on the
# cpu, as the session's compositor does.
#
# apks in BEHEAD_APKS, /usr/share/behead/apks unless set, are installed when
# android does not have them yet. they are named as f-droid names them,
# PACKAGE_VERSIONCODE.apk, which is how tools/fetch-fdroid.py leaves them.
# tools/preinstall-apks.py saves that step by putting them in android's image.
set -eu
. "$(dirname "$0")/home.sh"

CONFIG=/var/lib/waydroid/waydroid.cfg
# android's shell user, the one adb commands run as
SHELL_UID=2000

# the debian kernel builds binder without binderfs, so ask for the devices
[ -e /dev/binder ] || modprobe binder_linux devices=binder,hwbinder,vndbinder
if [ ! -e "$CONFIG" ]; then
    waydroid init >&2
    # waydroid picks the gpu it finds. without BEHEAD_RENDERER the session
    # renders on the cpu, and android has to as well. a fresh config ends
    # with an empty [properties] section
    if [ -z "${BEHEAD_RENDERER:-}" ]; then
        printf 'ro.hardware.gralloc = default\nro.hardware.egl = swiftshader\n' >> "$CONFIG"
        waydroid upgrade --offline >&2
    fi
    systemctl restart waydroid-container
fi
systemctl start waydroid-container

apks=${BEHEAD_APKS:-/usr/share/behead/apks}
# `waydroid app` finds the session over the session's bus, which lives in
# the runtime directory, so name the directory here rather than in the kiosk
export BEHEAD_RUNTIME_DIR="${BEHEAD_RUNTIME_DIR:-$(mktemp -d)}"
bus="unix:path=$BEHEAD_RUNTIME_DIR/bus"

wait_for_android() {
    until [ "$(waydroid shell -- getprop sys.boot_completed 2> /dev/null)" = 1 ]; do sleep 2; done
}

# android's location service takes commands from its shell user, not from
# root, whose commands it drops without a word
as_shell() {
    waydroid shell --uid "$SHELL_UID" -- "$@" < /dev/null > /dev/null
}

# make the car android's gps receiver: a test provider in place of the gps
# android does not have here, which the shell user may feed once allowed
provide_gps() {
    as_shell cmd appops set com.android.shell android:mock_location allow
    as_shell cmd location providers add-test-provider gps
    as_shell cmd location providers set-test-provider-enabled gps true
}

# hand android a fix: locate LATITUDE LONGITUDE ACCURACY, the last one `-`
# when the car left it out. the command takes no speed, bearing or altitude
locate() {
    [ -n "$gps" ] || provide_gps
    gps=1
    case $3 in -) accuracy= ;; *) accuracy="--accuracy $3" ;; esac
    as_shell cmd location providers set-test-provider-location gps --location "$1,$2" $accuracy
}

# follow the car's night mode and location with android's, from the sensor
# lines the server writes to stdin. one can arrive before android has started
follow_sensors() {
    gps= started=
    while read -r kind value more accuracy _; do
        case $kind in night | location) [ -n "$started" ] || wait_for_android ;; *) continue ;; esac
        started=1
        case $kind:$value in
            night:1) as_shell cmd uimode night yes ;;
            night:*) as_shell cmd uimode night no ;;
            location:-) ;;
            location:*) locate "$value" "$more" "$accuracy" ;;
        esac || true
    done
}

install_apps() {
    wait_for_android
    for apk in "$apks"/*.apk; do
        [ -e "$apk" ] || continue
        package=$(basename "$apk" .apk)
        package=${package%_[0-9]*}
        DBUS_SESSION_BUS_ADDRESS=$bus waydroid app list | grep -qx "packageName: $package" \
            || DBUS_SESSION_BUS_ADDRESS=$bus waydroid app install "$apk" >&2 || true
    done
}

exec 4<&0
follow_sensors <&4 &
sensors=$!
install_apps &
installer=$!

# android outlives its window, so stop it with the session
trap 'kill $sensors $installer 2> /dev/null || true; waydroid session stop > /dev/null 2>&1 || true' EXIT
trap 'exit 1' INT TERM HUP

"$(dirname "$0")/kiosk.sh" "$1" "$2" "$3" waydroid show-full-ui
