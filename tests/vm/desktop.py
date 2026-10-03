"""shared by the desktop session tests: the server with --uinput running a
session script, and the scripted headunit over tcp. runs as root inside the
test vm. subclasses say which session to run and when it has started."""

import os
import pathlib
import re
import select
import signal
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "e2e"))
import fakehu
import test_session as tcp

SESSIONS = tcp.ROOT / "sessions"
WIDTH, HEIGHT = 800, 480
FRAME_BYTES = WIDTH * HEIGHT * 3
POLL = 0.05
# how often a slow condition, such as decoding the whole stream, is checked
CHECK_EVERY = 0.5
# sessions start slowly when they render on the cpu
START = 120
REACT = 20
# how long a session may take to stop before it is killed
STOP = 10
# how long a finger or button stays down. kodi ignores a tap whose down and
# up arrive together, and a real car never sends one
HOLD = 0.15
# records what a session plays into the alsa loopback
AUDIO_CMD = f"{SESSIONS / 'audio.sh'} {{rate}} {{channels}}"
# how much of a session's sound to hear before checking its pitch
LISTEN_SECONDS = 1
# how much of the server's log a failed start shows
LOG_TAIL = 3000


def running(group):
    """whether a process group still has members"""
    try:
        os.killpg(group, 0)
    except ProcessLookupError:
        return False
    return True


class DesktopTest(unittest.TestCase):
    KEYCODES = (fakehu.HOME, fakehu.BACK)

    @classmethod
    def prepare(cls):
        """set up files under cls.dir before the server starts"""

    @classmethod
    def video_cmd(cls):
        raise NotImplementedError

    @classmethod
    def server_options(cls):
        return []

    @classmethod
    def started(cls):
        """whether the session is up and ready for input"""
        return True

    @classmethod
    def setUpClass(cls):
        subprocess.run(["modprobe", "uinput"], check=True)
        cls.tmp = tempfile.TemporaryDirectory()
        cls.dir = pathlib.Path(cls.tmp.name)
        cls.phone, cls.car = tcp.make_cert(cls.dir, "phone"), tcp.make_v1_cert(cls.dir, "car")
        cls.prepare()
        # the session logs a lot through the server's stderr
        cls.log = cls.dir / "server.log"
        with cls.log.open("wb") as log:
            cls.server = subprocess.Popen(
                [tcp.SERVER, "--listen", "127.0.0.1:0", "--uinput", "--cert", cls.phone[0], "--key", cls.phone[1],
                 "--video-cmd", cls.video_cmd(), *cls.server_options()], stdout=subprocess.DEVNULL, stderr=log)
        cls.addClassCleanup(cls.stop)
        deadline, port = time.monotonic() + fakehu.TIMEOUT, None
        while not port:
            assert time.monotonic() < deadline and cls.server.poll() is None, cls.log.read_text()
            port = re.search(r"listening on .*:(\d+)", cls.log.read_text())
            time.sleep(POLL)
        link = tcp.socket.create_connection(("127.0.0.1", int(port.group(1))))
        # a touch must not wait behind the car's last ack, which nagle's
        # algorithm would hold for a frame
        link.setsockopt(tcp.socket.IPPROTO_TCP, tcp.socket.TCP_NODELAY, 1)
        cls.addClassCleanup(link.close)
        cls.headunit = fakehu.FakeHeadunit(link, *cls.car, keycodes=cls.KEYCODES)
        cls.headunit.handshake()
        try:
            cls.wait_for(cls.started, "started", START)
        except AssertionError as failure:
            raise AssertionError(f"{failure}. the server's log ends:\n{cls.log.read_text()[-LOG_TAIL:]}") from None

    @classmethod
    def stop(cls):
        """end the server and wait for its commands to follow. each runs in
        a process group of its own, and is told to stop only once the server
        is gone, so an application may still be writing its files"""
        children = subprocess.run(["pgrep", "-P", str(cls.server.pid)], capture_output=True, text=True).stdout
        cls.server.terminate()
        cls.server.wait()
        deadline = time.monotonic() + STOP
        for group in map(int, children.split()):
            while running(group):
                if time.monotonic() > deadline:
                    os.killpg(group, signal.SIGKILL)
                time.sleep(POLL)
        cls.tmp.cleanup()

    @classmethod
    def pump_for(cls, seconds):
        """answer the server, as a car would, for a while. a still screen
        leaves long gaps with nothing to read"""
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            # one read can bring several messages; the rest wait in the
            # headunit's buffer, where select cannot see them
            if cls.headunit.buffer or select.select([cls.headunit.sock], [], [], POLL)[0]:
                cls.headunit.step()

    @classmethod
    def wait_for(cls, condition, what, timeout=REACT):
        deadline = time.monotonic() + timeout
        while not condition():
            assert time.monotonic() < deadline, f"the session never {what}"
            cls.pump_for(CHECK_EVERY)

    @classmethod
    def last_frame(cls):
        """the latest picture the car received, as rgb24, or None before the first"""
        if not cls.headunit.frames:
            return None
        stream = cls.dir / "received.h264"
        stream.write_bytes(cls.headunit.config[0] + b"".join(data for _, data in cls.headunit.frames))
        raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", stream, "-fps_mode", "passthrough",
                              "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], check=True, capture_output=True).stdout
        assert len(raw) % FRAME_BYTES == 0, "frames are not at the negotiated size"
        return raw[-FRAME_BYTES:] or None

    @classmethod
    def pixel(cls, x, y):
        frame = cls.last_frame()
        return frame and tuple(frame[(y * WIDTH + x) * 3:(y * WIDTH + x) * 3 + 3])

    def wait_for_tone(self, heard_from):
        """wait until the media audio the car got since packet `heard_from` ends in the test tone"""
        listen_bytes = LISTEN_SECONDS * tcp.MEDIA_BYTES_PER_SECOND

        def hears_the_tone():
            pcm = b"".join(data for _, data in self.headunit.audio[heard_from:])[-listen_bytes:]
            pitch = tcp.tone_frequency(pcm, fakehu.MEDIA_RATE, fakehu.MEDIA_CHANNELS) if pcm else 0
            return len(pcm) == listen_bytes and abs(pitch - tcp.TONE) <= tcp.TONE_TOLERANCE

        self.wait_for(hears_the_tone, f"played a {tcp.TONE} hz tone to the car")

    def tap(self, x, y):
        self.headunit.touch(fakehu.TOUCH_DOWN, [(0, x, y)])
        self.pump_for(HOLD)
        self.headunit.touch(fakehu.TOUCH_UP, [(0, x, y)])

    def press(self, keycode):
        self.headunit.button(keycode, True)
        self.pump_for(HOLD)
        self.headunit.button(keycode, False)
