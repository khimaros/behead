"""laptop as headunit, against a real device on a usb cable. the device runs
the behead service; this plays the car through pyusb. needs access to the
usb device nodes: see 70-behead.rules."""

import pathlib
import statistics
import sys
import tempfile
import unittest

for tier in ("e2e", "vm"):
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / tier))
import fakehu
import latency
import test_session as tcp
import test_usb as usb_tier

FRAME_COUNT = 150
FRAME_RATE = 30
RATE_TOLERANCE = 3
# no gap between two frames reaching the car may exceed this many frame periods
MAX_GAP_PERIODS = 2
# nor may frames bunch up: this share of the gaps must be at least half a period
MIN_SPACED_SHARE = 0.95
STREAM_TIMEOUT = 30
MICROSECONDS = 1_000_000
# a regression guard; the figure is printed
LATENCY_LIMIT = 0.5


class DeviceTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.dir = pathlib.Path(cls.tmp.name)
        cls.car = tcp.make_v1_cert(cls.dir, "car")

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def connect(self, **options):
        link = usb_tier.UsbLink(usb_tier.switch_to_accessory())
        self.addCleanup(link.close)
        headunit = fakehu.FakeHeadunit(link, *self.car, **options)
        headunit.handshake()
        return headunit

    def stream(self, **options):
        headunit = self.connect(**options)
        headunit.run_until(lambda: len(headunit.frames) >= FRAME_COUNT, timeout=STREAM_TIMEOUT)
        return headunit

    def test_streams_decodable_video_at_a_stable_rate(self):
        headunit = self.stream()
        timestamps = [timestamp for timestamp, _ in headunit.frames]
        rate = (len(timestamps) - 1) * MICROSECONDS / (timestamps[-1] - timestamps[0])
        self.assertAlmostEqual(rate, FRAME_RATE, delta=RATE_TOLERANCE)

        period = 1 / FRAME_RATE
        gaps = [later - earlier for earlier, later in zip(headunit.arrivals, headunit.arrivals[1:])]
        self.assertLess(max(gaps), MAX_GAP_PERIODS * period, "a frame arrived late")
        spaced = sum(gap >= period / 2 for gap in gaps) / len(gaps)
        self.assertGreaterEqual(spaced, MIN_SPACED_SHARE, "frames arrived in bursts")

        received = self.dir / "received.h264"
        received.write_bytes(headunit.config[0] + b"".join(data for _, data in headunit.frames))
        self.assertEqual(tcp.count_frames(received), len(headunit.frames))

    def test_serves_720p(self):
        headunit = self.stream(resolution=fakehu.RESOLUTION_1280X720)
        self.assertGreaterEqual(len(headunit.frames), FRAME_COUNT)

    def test_reconnects_after_a_bus_reset(self):
        self.stream().sock.close()
        self.stream()

    def test_touch_to_picture(self):
        """needs the image's default video command, the pipe demo"""
        trips = latency.measure(self.stream())
        self.assertNotIn(None, trips, "a tap never showed")
        print("\n" + latency.summary("device", trips), file=sys.stderr, flush=True)
        self.assertLess(statistics.median(trips), LATENCY_LIMIT)


if __name__ == "__main__":
    unittest.main()
