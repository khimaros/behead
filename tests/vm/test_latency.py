"""touch to picture round trip in the vm, over tcp, for the pipe demo and the
wayland kiosk. prints the figures; the limit only catches regressions. the
rpi device tests measure the same on hardware."""

import statistics
import sys
import unittest

import desktop
import latency
from desktop import tcp

class LatencyTest(desktop.DesktopTest):
    """subclasses name the video command and the median it must beat; the
    demo draws the markers"""
    NAME, LIMIT = "", 0

    @classmethod
    def started(cls):
        return len(cls.headunit.frames) > 0

    def test_touch_to_picture(self):
        trips = latency.measure(self.headunit)
        self.assertNotIn(None, trips, "a tap never showed")
        print("\n" + latency.summary(self.NAME, trips), file=sys.stderr, flush=True)
        self.assertLess(statistics.median(trips), self.LIMIT)


class PipeDemoLatency(LatencyTest):
    """behead-demo through ffmpeg's x264, the rpi image's default. one frame,
    over tcp as over usb"""
    NAME, LIMIT = "pipe demo", 0.05

    @classmethod
    def video_cmd(cls):
        return tcp.DEMO_CMD


class WaylandKioskLatency(LatencyTest):
    """the desktop path: uinput, sway, a wayland client, wf-recorder"""
    NAME, LIMIT = "wayland kiosk", 0.5

    @classmethod
    def video_cmd(cls):
        return f"{desktop.SESSIONS / 'kiosk.sh'} {{width}} {{height}} {{fps}} {tcp.DEMO} --wayland"


del LatencyTest

if __name__ == "__main__":
    unittest.main()
