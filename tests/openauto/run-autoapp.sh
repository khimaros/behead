#!/bin/sh
# start a virtual display and openauto on it. ximagesink draws without gl,
# which xvfb lacks. with VNC=1 the display is also served over vnc (5900)
# and through novnc to a browser (6080), for interactive use.
set -e
Xvfb "$DISPLAY" -screen 0 "${SCREEN:-800x480x24}" &
while ! xdotool getdisplaygeometry >/dev/null 2>&1; do sleep 0.1; done
if [ "${VNC:-0}" = 1 ]; then
    x11vnc -display "$DISPLAY" -forever -shared -nopw -quiet -rfbport 5900 &
    websockify --web /usr/share/novnc 6080 localhost:5900 >/dev/null 2>&1 &
fi
export QT_QPA_PLATFORM=xcb
export QT_GSTREAMER_PLAYBIN_VIDEOSINK=ximagesink QT_GSTREAMER_WIDGET_VIDEOSINK=ximagesink
exec autoapp
