"""laptop as headunit, against an esp32-p4 running the firmware on a usb
cable. the firmware shows the launcher, so the device tests that need the
demo open it first, and the music is whatever the card in the board holds"""

import os
import statistics
import sys
import time
import unittest

# first: it puts the other tiers' modules on the path
import test_device
import fakehu
import latency

# the middle of the launcher's tiles: the music player, and the demo
MUSIC_TILE, INPUTS_TILE = (200, 240), (600, 240)
# android's key to play, which unlike play-pause never stops what plays
PLAY, PAUSE = 126, 127
# how often a car leaves and comes back before the music is tried
RETURNS = 2
# long enough for the launcher to change screens, and for a track to start
SETTLE = 0.5
LISTEN = 3
BYTES_PER_SECOND = fakehu.MEDIA_RATE * fakehu.MEDIA_CHANNELS * fakehu.SAMPLE_BITS // 8
RATE_TOLERANCE = 0.1
# seconds to play on for, from `make test-device SOAK=`, checked a window
# at a time. the firmware sends sound in packets of this length
SOAK = float(os.environ.get("BEHEAD_SOAK") or 0)
SOAK_WINDOW = 5
PACKET_SECONDS = 0.04


class FirmwareTest(test_device.DeviceTest):
    def test_touch_to_picture(self):
        headunit = self.stream()
        # the launcher is on whatever screen its last car left it
        headunit.button(fakehu.HOME, True)
        headunit.button(fakehu.HOME, False)
        headunit.touch(fakehu.TOUCH_DOWN, [(0, *INPUTS_TILE)])
        headunit.touch(fakehu.TOUCH_UP, [(0, *INPUTS_TILE)])
        latency.pause(headunit, SETTLE)
        trips = latency.measure(headunit)
        self.assertNotIn(None, trips, "a tap never showed")
        print("\n" + latency.summary("device", trips), file=sys.stderr, flush=True)
        self.assertLess(statistics.median(trips), test_device.LATENCY_LIMIT)

    def listener(self):
        """a car that has begun to get sound, silent or not, and pictures"""
        headunit = self.connect(keycodes=(fakehu.HOME, fakehu.BACK, PLAY, PAUSE))
        headunit.run_until(lambda: headunit.audio and headunit.frames, timeout=test_device.STREAM_TIMEOUT)
        return headunit

    def assert_plays(self, headunit):
        """the car's play key starts a track, or goes on with the one the
        last car left. sound and pictures then both keep their pace"""
        headunit.button(PLAY, True)
        headunit.button(PLAY, False)
        latency.pause(headunit, SETTLE)
        packets, pictures = len(headunit.audio), len(headunit.frames)
        latency.pause(headunit, LISTEN)
        pcm = b"".join(data for _, data in headunit.audio[packets:])
        rate = (len(headunit.frames) - pictures) / LISTEN
        # the launcher goes on playing for the next car, which the tests
        # after this one are, and they are about pictures alone
        headunit.button(PAUSE, True)
        headunit.button(PAUSE, False)
        latency.pause(headunit, SETTLE)
        if not any(pcm):
            self.skipTest("nothing played: no card in the board, or no music on it")
        self.assertAlmostEqual(len(pcm) / BYTES_PER_SECOND, LISTEN, delta=LISTEN * RATE_TOLERANCE)
        self.assertAlmostEqual(rate, test_device.FRAME_RATE, delta=test_device.RATE_TOLERANCE)

    def test_plays_the_cards_music_beside_the_picture(self):
        self.assert_plays(self.listener())

    def test_plays_for_a_car_that_comes_back(self):
        """sound is not the first connection's alone"""
        for _ in range(RETURNS):
            self.listener().sock.close()
        self.assert_plays(self.listener())

    @unittest.skipUnless(SOAK, "takes as long as asked: make test-device DEVICE=esp32 SOAK=600")
    def test_plays_on_without_a_stall(self):
        """a transfer the usb driver loses stops sound and pictures for good,
        with nothing in the log. it showed anywhere from 20 seconds to four
        minutes into a track, which no other test here plays for"""
        headunit = self.listener()
        # on the music screen, as someone listening is, whatever screen
        # the last car left
        headunit.button(fakehu.HOME, True)
        headunit.button(fakehu.HOME, False)
        headunit.touch(fakehu.TOUCH_DOWN, [(0, *MUSIC_TILE)])
        headunit.touch(fakehu.TOUCH_UP, [(0, *MUSIC_TILE)])
        headunit.button(PLAY, True)
        headunit.button(PLAY, False)
        latency.pause(headunit, SETTLE)
        end = time.monotonic() + SOAK
        while time.monotonic() < end:
            packets, pictures = len(headunit.audio), len(headunit.frames)
            latency.pause(headunit, SOAK_WINDOW)
            sound = (len(headunit.audio) - packets) * PACKET_SECONDS / SOAK_WINDOW
            rate = (len(headunit.frames) - pictures) / SOAK_WINDOW
            left = f"{end - time.monotonic():.0f} s before the end"
            self.assertAlmostEqual(sound, 1, delta=RATE_TOLERANCE, msg=f"sound fell behind {left}")
            self.assertAlmostEqual(rate, test_device.FRAME_RATE, delta=test_device.RATE_TOLERANCE, msg=left)
            # hours of it would not fit in memory
            for kept in (headunit.audio, headunit.frames, headunit.arrivals, headunit.log):
                del kept[:]
        headunit.button(PAUSE, True)
        headunit.button(PAUSE, False)
        latency.pause(headunit, SETTLE)


if __name__ == "__main__":
    unittest.main()
