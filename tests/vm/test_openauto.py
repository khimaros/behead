"""an independent headunit: openauto, in a container inside the test vm,
drives the server over the usb gadget. every other test checks the server
against our own reading of the protocol; this one checks it against someone
else's. needs the image built by provision.sh."""

import os
import pathlib
import re
import signal
import subprocess
import sys
import threading
import time
import unittest

import usb.util

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "e2e"))
import test_session as tcp
import test_usb as usb_tier

ROOT = pathlib.Path(__file__).resolve().parents[2]
ENV_FILE = ROOT / "rpi/overlay/etc/behead/behead.env"
SCREENSHOT = ROOT / "target/openauto-screen.png"
IMAGE = CONTAINER = "behead-openauto"
DISPLAY = ":99"
WIDTH, HEIGHT = 800, 480
SESSION_TIMEOUT = 90
EVENT_TIMEOUT = 10
FRAME_BYTES = WIDTH * HEIGHT * 3
CLICK = (400, 240)
HOLD = (600, 300)
# time for a touch to reach the demo and its frame to be encoded and drawn
MARKER_DELAY = 1
DRAG = ((100, 100), (700, 380))
# openauto's keyboard mapping (enabled in tests/openauto/openauto.ini) and
# the android auto button codes it sends
KEYS = {"Return": 23, "Left": 21, "Right": 22, "Up": 19, "Down": 20, "Escape": 4, "h": 3}
# openauto scales its window to the 800x480 touch space; allow for rounding
CLICK_SLACK = 8


def video_command():
    """the rpi image's video command, so this also exercises the shipped default"""
    line = next(l for l in ENV_FILE.read_text().splitlines() if l.startswith("VIDEO_CMD="))
    return line.split("=", 1)[1].strip('"')


def podman(*args, **kwargs):
    return subprocess.run(["podman", *args], check=True, capture_output=True, **kwargs).stdout


class Lines:
    """collects a pipe's lines on a thread, so tests can wait for one"""

    def __init__(self, pipe):
        self.lines, self.changed = [], threading.Condition()
        threading.Thread(target=self.pump, args=(pipe,), daemon=True).start()

    def pump(self, pipe):
        for line in pipe:
            with self.changed:
                self.lines.append(line.decode(errors="replace").rstrip("\n"))
                self.changed.notify_all()

    def wait(self, pattern, timeout=EVENT_TIMEOUT):
        with self.changed:
            found = self.changed.wait_for(lambda: [l for l in self.lines if re.search(pattern, l)], timeout)
        assert found, f"never saw {pattern!r} in:\n" + "\n".join(self.lines)
        return found


def grab():
    return podman("exec", CONTAINER, "ffmpeg", "-loglevel", "error", "-f", "x11grab", "-video_size",
                  f"{WIDTH}x{HEIGHT}", "-i", DISPLAY, "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-")


class OpenautoTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        tcp.SessionTest.setUpClass.__func__(cls)
        subprocess.run(["modprobe", "-a", *usb_tier.MODULES], check=True)
        subprocess.run(["podman", "rm", "-f", CONTAINER], capture_output=True)
        subprocess.run([tcp.SERVER, "teardown"], check=True)
        cls.server = subprocess.Popen(
            [tcp.SERVER, "--usb", "auto", "--cert", cls.phone[0], "--key", cls.phone[1], "--video-cmd", video_command()],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            env={**os.environ, "PATH": f"{tcp.DEMO.parent}:{os.environ['PATH']}"})
        # class cleanups run even when the rest of this setup fails
        cls.addClassCleanup(cls.stop)
        cls.log, cls.input = Lines(cls.server.stderr), Lines(cls.server.stdout)
        cls.log.wait("usb gadget ready")
        # openauto in a container misses the re-enumeration after the switch
        # (found with AACS), so switch first and let it find the accessory
        usb.util.dispose_resources(usb_tier.switch_to_accessory())
        podman("run", "-d", "--name", CONTAINER, "--privileged", "--net=host", "-e", f"DISPLAY={DISPLAY}",
               "-e", f"SCREEN={WIDTH}x{HEIGHT}x24", "-v", "/dev/bus/usb:/dev/bus/usb", "-v", "/run/udev:/run/udev:ro",
               IMAGE)
        cls.log.wait(r"video started", timeout=SESSION_TIMEOUT)

    @classmethod
    def stop(cls):
        """keep openauto's log for debugging, then stop both ends"""
        logs = subprocess.run(["podman", "logs", CONTAINER], capture_output=True, text=True)
        (ROOT / "target/openauto.log").write_text(logs.stdout + logs.stderr)
        subprocess.run(["podman", "rm", "-f", CONTAINER], capture_output=True)
        cls.server.send_signal(signal.SIGTERM)
        cls.server.wait(timeout=EVENT_TIMEOUT)
        tcp.SessionTest.tearDownClass.__func__(cls)

    def test_negotiates_a_video_session(self):
        self.log.wait("headunit authenticated")
        self.log.wait(r"video started \d+x\d+@\d+")

    def test_reports_openautos_sensors(self):
        """openauto answers a sensor's start request with a first reading"""
        self.input.wait(r"^driving 0$")
        self.input.wait(r"^night [01]$")

    def test_draws_the_moving_video(self):
        first = grab()
        time.sleep(1)
        second = grab()
        self.assertEqual(len(second), FRAME_BYTES)
        self.assertNotEqual(first, second, "the picture is not moving")

    def test_a_held_touch_shows_under_the_finger(self):
        """the round trip: openauto's touch reaches the demo, whose marker
        comes back as video and is drawn by openauto where the finger is"""
        x, y = HOLD
        podman("exec", CONTAINER, "xdotool", "mousemove", str(x), str(y), "mousedown", "1")
        try:
            self.input.wait(r"^touch down ")
            time.sleep(MARKER_DELAY)
            screen = grab()
            offset = (y * WIDTH + x) * 3
            red, green, _ = screen[offset:offset + 3]
            self.assertGreater(red, 180, "no marker under the finger")
            self.assertLess(green, 120, "no marker under the finger")
            podman("exec", CONTAINER, "ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-video_size",
                   f"{WIDTH}x{HEIGHT}", "-i", DISPLAY, "-frames:v", "1", "/tmp/screen.png")
            podman("cp", f"{CONTAINER}:/tmp/screen.png", str(SCREENSHOT))
        finally:
            podman("exec", CONTAINER, "xdotool", "mouseup", "1")

    def assert_touch_near(self, action, x, y):
        line = self.input.wait(rf"^touch {action} ")[-1]
        px, py = map(int, line.split()[-1].split(":")[1].split(","))
        self.assertLessEqual(abs(px - x), CLICK_SLACK, line)
        self.assertLessEqual(abs(py - y), CLICK_SLACK, line)

    def test_clicks_become_touch_events(self):
        x, y = CLICK
        podman("exec", CONTAINER, "xdotool", "mousemove", str(x), str(y), "click", "1")
        self.assert_touch_near("down", x, y)
        self.assert_touch_near("up", x, y)

    def test_keys_become_button_events(self):
        for key, code in KEYS.items():
            podman("exec", CONTAINER, "xdotool", "key", key)
            self.input.wait(rf"^button {code} down$")
            self.input.wait(rf"^button {code} up$")

    def test_drags_become_touch_moves(self):
        (x, y), (end_x, end_y) = DRAG
        podman("exec", CONTAINER, "xdotool", "mousemove", str(x), str(y), "mousedown", "1",
               "mousemove", str((x + end_x) // 2), str((y + end_y) // 2), "mousemove", str(end_x), str(end_y),
               "mouseup", "1")
        self.assert_touch_near("move", end_x, end_y)
        self.assert_touch_near("up", end_x, end_y)


if __name__ == "__main__":
    unittest.main()
