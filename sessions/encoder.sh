# sourced by lib.sh: which h.264 encoder a session records with, and the
# settings each one takes

# what the hardware encoder is given per pixel and second, in hundredths of
# a bit: 4 mbit/s at 800x480 and 30 frames
HARDWARE_CENTIBITS=35
# baseline, as ffmpeg numbers h.264 profiles
BASELINE_PROFILE=66

# pictures of fewer pixels than this stay with x264. measured on a pi 4 at
# 30 frames: at 800x480 the hardware saves a tenth of a core (37% against
# 48%) and costs two frames of touch to picture latency (170 ms against
# 103 ms), at 1280x720 it saves a third (63% against 95%, which is all one
# x264 thread has)
HARDWARE_FROM_PIXELS=500000

# the encoder to use at WIDTH HEIGHT FPS: the one BEHEAD_ENCODER names, x264
# or v4l2m2m, or else the hardware encoder for a picture large enough to
# gain from it, where a short trial encode works, as on a pi 4, and x264 in
# software everywhere else
encoder() {
    case ${BEHEAD_ENCODER:-} in x264 | v4l2m2m) echo "$BEHEAD_ENCODER"; return ;; esac
    if [ $(( $1 * $2 )) -lt "$HARDWARE_FROM_PIXELS" ]; then
        echo x264
    elif ffmpeg -loglevel quiet -f lavfi -i "color=size=$1x$2:rate=$3" -frames:v 1 -pix_fmt yuv420p \
        -c:v h264_v4l2m2m -f null - < /dev/null > /dev/null 2>&1; then
        echo v4l2m2m
    else
        echo x264
    fi
}

# the lowest h.264 level whose macroblock rate fits the mode. wf-recorder
# hands x264 a microsecond timebase, which x264 would take for the frame
# rate and so claim level 6.2.
h264_level() {
    macroblocks=$(( ($1 + 15) / 16 * (($2 + 15) / 16) * $3 ))
    if [ "$macroblocks" -le 108000 ]; then echo 31
    elif [ "$macroblocks" -le 216000 ]; then echo 32
    elif [ "$macroblocks" -le 245760 ]; then echo 40
    else echo 42
    fi
}

# what the hardware encoder is given per second at WIDTH HEIGHT FPS. it has
# no quality setting to go by
hardware_bitrate() {
    echo $(( $1 * $2 * $3 * HARDWARE_CENTIBITS / 100 ))
}

# wf-recorder's arguments for an encoder: recorder_options ENCODER WIDTH HEIGHT FPS.
# both take yuv420p: the pi's encoder accepts nv12 too, which is cheaper to
# make, but its picture comes out green from halfway down
recorder_options() {
    case $1 in
        v4l2m2m) echo "--codec h264_v4l2m2m --pixel-format yuv420p -p b=$(hardware_bitrate "$2" "$3" "$4")" \
            "-p profile=$BASELINE_PROFILE" ;;
        *) echo "--codec libx264 --pixel-format yuv420p -p preset=ultrafast -p tune=zerolatency -p profile=baseline" \
            "-p level=$(h264_level "$2" "$3" "$4")" ;;
    esac
}
