"""end-to-end: the browser page behind HEADED=1 as a headunit's controls.
what the page asks for over http has to come out as input for a headunit"""

import atexit
import socket
import statistics
import subprocess
import time
import unittest
import urllib.request

import fakehu
import viewer

# what linux lets a pipe hold, which one write to it cannot exceed
PIPE_HOLDS = 65536
# a picture of noise, whose keyframe is many times that, the rows at its
# foot that a decoder short of bytes has to make up, and how far apart
# noise's greys are when it does not: made up rows are all but one grey
NOISE_WIDTH, NOISE_HEIGHT, NOISE_ROWS, NOISE_DEVIATION = 1280, 720, 64, 30
NOISE_SIZE = f"{NOISE_WIDTH}x{NOISE_HEIGHT}"


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class ViewerControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.port = free_port()
        cls.viewer = viewer.Viewer(cls.port)
        # nobody is watching this one
        atexit.unregister(cls.viewer.linger)
        cls.addClassCleanup(cls.viewer.decoder.kill)

    def ask(self, path):
        with urllib.request.urlopen(f"http://127.0.0.1:{self.port}{path}", timeout=fakehu.TIMEOUT) as response:
            return response.status

    def test_the_mouse_is_a_finger(self):
        self.ask("/touch?action=down&x=12&y=34")
        self.assertEqual(self.viewer.touches.get(timeout=fakehu.TIMEOUT), ("down", 12, 34))

    def test_keys_are_the_cars_buttons_and_the_wheel_its_knob(self):
        for path in (f"/button?code={fakehu.DPAD_LEFT}&pressed=1", f"/button?code={fakehu.DPAD_LEFT}&pressed=0",
                     "/turn?delta=-2"):
            self.ask(path)
        asked = [self.viewer.controls.get(timeout=fakehu.TIMEOUT) for _ in range(3)]
        self.assertEqual(asked, [("button", fakehu.DPAD_LEFT, True), ("button", fakehu.DPAD_LEFT, False),
                                 ("turn", fakehu.ROTARY, -2)])

    def test_page_up_and_down_turn_the_knob_as_the_wheel_does(self):
        """for a keyboard without a mouse: down is the wheel rolled toward
        the hand, which is a turn clockwise, and held, they go on turning"""
        self.assertEqual(viewer.TURNS, {"PageUp": -1, "PageDown": 1})
        self.assertNotIn("PageUp", viewer.KEYS, "a key that turns presses no button too")
        page = viewer.PAGE.decode().replace(" ", "")
        self.assertIn("turns[event.code]", page)
        self.assertIn('"PageDown":1', page)

    def test_a_place_typed_into_the_page_is_where_the_car_is(self):
        """a car knows where it is and the page's does not, so the page
        takes a place, in degrees, to send as the car's sensor would"""
        self.ask("/location?latitude=37.7749&longitude=-122.4194")
        self.assertEqual(self.viewer.controls.get(timeout=fakehu.TIMEOUT), ("location", 377749000, -1224194000))
        self.assertIn('id="place"', viewer.PAGE.decode())

    def test_what_the_car_plays_reaches_a_page_that_listens(self):
        self.viewer.play(44100, 1, b"before anyone listened")
        with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/audio", timeout=fakehu.TIMEOUT) as response:
            self.assertEqual((response.headers["X-Rate"], response.headers["X-Channels"]), ("44100", "1"))
            for piece in (b"\x01\x02\x03\x04", b"\x05\x06"):
                self.viewer.play(44100, 1, piece)
            self.assertEqual(response.read(6), b"\x01\x02\x03\x04\x05\x06", "the sound from when it began to listen")
        # a page that left is found out by the next piece it cannot be sent
        deadline = time.monotonic() + fakehu.TIMEOUT
        while self.viewer.listeners and time.monotonic() < deadline:
            self.viewer.play(44100, 1, bytes(viewer.READ_CHUNK))
            time.sleep(0.05)
        self.assertEqual(self.viewer.listeners, [])

    def test_a_page_that_reads_too_slowly_loses_sound_and_holds_nothing_up(self):
        with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/audio", timeout=fakehu.TIMEOUT):
            started = time.monotonic()
            for _ in range(viewer.AUDIO_BACKLOG * 20):
                self.viewer.play(48000, 2, bytes(viewer.READ_CHUNK))
            self.assertLess(time.monotonic() - started, 1)

    def test_the_page_offers_every_key_the_headunit_should_advertise(self):
        """a car only gets bound the keys it listed, so the page's keys and
        the list a headunit is created with have to agree"""
        page = viewer.PAGE.decode()
        self.assertEqual(viewer.ROTARY, fakehu.ROTARY)
        for code in viewer.KEYS.values():
            self.assertIn(code, viewer.KEYCODES)
        self.assertIn(fakehu.ROTARY, viewer.KEYCODES)
        self.assertIn('"ArrowLeft":21', page.replace(" ", ""))

    def test_a_picture_larger_than_a_pipe_is_shown_whole(self):
        """a keyframe of a busy screen is more than a pipe holds at once.
        every byte of it has to reach the decoder, or the picture is wrong
        from where it was cut, and so is every one after it until the next
        keyframe"""
        noise = subprocess.run(
            ["ffmpeg", "-loglevel", "error", "-f", "lavfi", "-i", f"nullsrc=size={NOISE_SIZE},geq=random(1)*255:128:128",
             "-frames:v", "1", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-profile:v", "baseline", "-qp", "20",
             "-f", "h264", "-"], check=True, capture_output=True).stdout
        self.assertGreater(len(noise), PIPE_HOLDS)
        # twice: the decoder shows a stream's first picture once it has a second
        for _ in range(2):
            self.viewer.feed(noise)
        deadline = time.monotonic() + fakehu.TIMEOUT
        while not self.viewer.latest and time.monotonic() < deadline:
            time.sleep(0.05)
        grey = subprocess.run(["ffmpeg", "-loglevel", "error", "-f", "mjpeg", "-i", "-", "-pix_fmt", "gray",
                               "-f", "rawvideo", "-"], input=self.viewer.latest, capture_output=True).stdout
        self.assertTrue(grey, "a picture came out")
        last_rows = grey[-NOISE_ROWS * NOISE_WIDTH:]
        self.assertGreater(statistics.pstdev(last_rows), NOISE_DEVIATION, "noise down to the last rows")

    def test_the_keyboard_types(self):
        """letters, digits and what goes with them reach the phone as
        android's keys, by where they are on the keyboard, with shift a key
        of its own, so that the phone and not the page decides what is typed"""
        typed = {"KeyA": 29, "KeyZ": 54, "Digit0": 7, "Digit9": 16, "Space": 62, "Backspace": 67,
                 "ShiftLeft": 59, "Slash": 76, "Home": 3, "Escape": 4}
        self.assertEqual({key: viewer.KEYS.get(key) for key in typed}, typed)
        self.assertNotIn("h", viewer.KEYS, "a letter that does not type")
        self.assertIn("keys[event.code]", viewer.PAGE.decode())


if __name__ == "__main__":
    unittest.main()
