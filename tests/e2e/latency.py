"""touch to picture round trip, shared by the vm and device tests: how long
after the car sends a touch the picture showing it arrives back at the car.
the video source must be behead-demo, which draws a marker under each
finger, and must stream steadily. covers the server, the video command and
the transport, but not the car's own decoding and display."""

import pathlib
import statistics
import subprocess
import tempfile
import time

import fakehu

TAPS = 10
# far enough apart that one tap's marker never covers the next spot
SPOTS = [(150 + 55 * index, 140 + 25 * index) for index in range(TAPS)]
# long enough for the slowest path to show the marker
HOLD_DOWN = 1.0
BETWEEN_TAPS = 0.3
# the demo's marker for the first finger is red
MARKER_RED, MARKER_GREEN = 180, 120
WIDTH, HEIGHT = 800, 480
FRAME_BYTES = WIDTH * HEIGHT * 3


def pause(headunit, seconds):
    end = time.monotonic() + seconds
    headunit.run_until(lambda: time.monotonic() >= end)


def decoded(headunit):
    with tempfile.TemporaryDirectory() as directory:
        stream = pathlib.Path(directory) / "latency.h264"
        stream.write_bytes(headunit.config[0] + b"".join(data for _, data in headunit.frames))
        raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", stream, "-fps_mode", "passthrough",
                              "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], check=True, capture_output=True).stdout
    return [raw[at:at + FRAME_BYTES] for at in range(0, len(raw), FRAME_BYTES)]


def has_marker(frame, x, y):
    red, green, _ = frame[(y * WIDTH + x) * 3:(y * WIDTH + x) * 3 + 3]
    return red > MARKER_RED and green < MARKER_GREEN


def measure(headunit):
    """tap each spot in turn, then find the first picture showing each tap.
    returns the round trips in seconds, None where a tap never showed"""
    touched = []
    for x, y in SPOTS:
        touched.append(time.monotonic())
        headunit.touch(fakehu.TOUCH_DOWN, [(0, x, y)])
        pause(headunit, HOLD_DOWN)
        headunit.touch(fakehu.TOUCH_UP, [(0, x, y)])
        pause(headunit, BETWEEN_TAPS)
    frames, arrivals = decoded(headunit), headunit.arrivals
    assert len(frames) == len(arrivals), f"{len(frames)} of {len(arrivals)} pictures decode"
    return [next((arrived - sent for frame, arrived in zip(frames, arrivals) if arrived > sent and has_marker(frame, x, y)),
                 None) for (x, y), sent in zip(SPOTS, touched)]


def summary(name, trips):
    milliseconds = sorted(trip * 1000 for trip in trips)
    return (f"{name}: touch to picture over {len(trips)} taps: median {statistics.median(milliseconds):.0f} ms, "
            f"p90 {milliseconds[int(len(trips) * 0.9) - 1]:.0f} ms, min {milliseconds[0]:.0f} ms, "
            f"max {milliseconds[-1]:.0f} ms")
