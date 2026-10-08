# shared by the session scripts, which source it: a private runtime
# directory, the processes a session starts, and h.264 capture of a wlroots
# output on stdout. everything started with run() stops with the script.
. "$(dirname "$0")/home.sh"
. "$(dirname "$0")/encoder.sh"

# the format of the car's media channel
SOUND_RATE=48000 SOUND_CHANNELS=2

# BEHEAD_RUNTIME_DIR names the runtime directory, for whatever outside the
# session has to reach its bus or sockets. it is removed with the session
runtime=${BEHEAD_RUNTIME_DIR:-$(mktemp -d)}
mkdir -p -m 700 "$runtime"
children= recorder=
# wf-recorder hangs, ignoring SIGTERM, once its compositor is gone, and has
# nothing worth flushing by then. errexit applies inside traps too, so a
# process that already exited must not end the cleanup early
trap 'kill -KILL $recorder 2>/dev/null || true; kill $children 2>/dev/null || true; rm -rf "$runtime"' EXIT
trap 'exit 1' INT TERM HUP
export XDG_RUNTIME_DIR="$runtime"
# waydroid binds a pulseaudio socket into android whether one exists or not,
# and android fails to start without it. this stand-in lets it start; its
# sound goes nowhere until a session runs a sound server
mkdir -p "$runtime/pulse"
touch "$runtime/pulse/native"
# render on the cpu, which works everywhere, unless BEHEAD_RENDERER names
# another of wlroots' renderers: gles2 for a gpu. open input devices without
# a login session
export WLR_BACKENDS=headless,libinput WLR_RENDERER="${BEHEAD_RENDERER:-pixman}" WLR_LIBINPUT_NO_DEVICES=1 \
    LIBSEAT_BACKEND=noop

# start a process in the background, its output on stderr
run() {
    "$@" >&2 &
    children="$children $!"
}

# a session bus of the session's own, which phosh and waydroid need
start_session_bus() {
    run dbus-daemon --session --nofork --nopidfile --address="unix:path=$runtime/bus"
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus"
}

# a sound server of the session's own, for applications that play through
# pulseaudio: a pulseaudio whose only sink is the playback end of an alsa
# loopback. audio.sh records the other end for the car, at the format the
# car's media channel has, which the loopback makes both ends agree on.
# pipewire would need a session manager beside it to connect an application
# to the sink. needs the snd-aloop module
start_sound() {
    modprobe snd-aloop
    rm -f "$runtime/pulse/native"
    run pulseaudio -n --daemonize=no --exit-idle-time=-1 \
        --load="module-native-protocol-unix auth-anonymous=1" \
        --load="module-alsa-sink device=hw:CARD=Loopback,DEV=0 format=s16le rate=$SOUND_RATE channels=$SOUND_CHANNELS"
}

# the server writes the car's touch, button and sensor lines to the script's
# stdin. call a function with each line's words, in the background
on_line() {
    exec 4<&0
    (set -f; while read -r line; do "$1" $line || true; done <&4) &
    children="$children $!"
}

# wait until the compositor started last has a wayland socket, and use it
wait_for_display() {
    compositor=${children##* }
    while ! socket=$(ls "$runtime" | grep -x 'wayland-[0-9]*'); do
        kill -0 "$compositor"
        sleep 0.1
    done
    export WAYLAND_DISPLAY="$socket"
}

# record with one encoder until the recorder ends: record ENCODER OUTPUT WIDTH HEIGHT FPS
record() {
    # video on stdout, everything wf-recorder says on stderr. capture every
    # frame at a constant rate, still screen or not, because decoders such as
    # openauto's show a picture only once more follow it
    # shellcheck disable=SC2046
    wf-recorder --output "$2" --overwrite --no-damage --framerate "$5" --muxer h264 --file /dev/fd/3 \
        $(recorder_options "$1" "$3" "$4" "$5") -p g="$5" 3>&1 >&2 &
    recorder=$!
    wait "$recorder" || true
}

# record an output to stdout until the server goes away: capture OUTPUT WIDTH HEIGHT FPS
capture() {
    # wf-recorder notices neither the server going away, while the pipe has
    # room, nor its compositor dying, after which it hangs. end the session
    # when either does
    (while kill -0 "$PPID" 2>/dev/null && kill -0 "$compositor" 2>/dev/null; do sleep 1; done; kill $$) &
    children="$children $!"
    chosen=$(encoder "$2" "$3" "$4")
    record "$chosen" "$@"
    # a hardware encoder that would not start, or gave up, leaves the
    # session without a picture. software always works
    if [ "$chosen" != x264 ] && kill -0 "$compositor" 2>/dev/null; then
        echo "encoder $chosen ended, falling back to x264" >&2
        record x264 "$@"
    fi
}
