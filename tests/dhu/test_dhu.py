"""an independent headunit from Google itself: the DHU from the Android SDK,
driven through its console, against the server over TCP. like openauto in
the VM, it checks the server against someone else's reading of the
protocol, and unlike openauto it checks the phone certificate as a car
does. skips without the DHU (`make dhu`) or a Google signed pair
(docs/certificates.md)."""

import os
import pathlib
import re
import signal
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "e2e"))
import test_session as tcp
from lines import Lines

HERE = pathlib.Path(__file__).resolve().parent
DHU_DIR = pathlib.Path(os.environ.get("BEHEAD_DHU_DIR", ""))
DHU = DHU_DIR / "desktop-head-unit"
DHU_LIBS = os.environ.get("BEHEAD_DHU_LIBS", "")
CONFIG = HERE / "headunit.ini"
CERT_CACHE = pathlib.Path(os.environ.get("CERT_CACHE", pathlib.Path.home() / ".cache/behead/certs"))
GOOGLE_PAIR = (CERT_CACHE / "carservice.crt", CERT_CACHE / "carservice.key")
WIDTH, HEIGHT = 800, 480
FRAME_BYTES = WIDTH * HEIGHT * 3
SESSION_TIMEOUT = 30
EVENT_TIMEOUT = 10
SCREENSHOT_TIMEOUT = 10
SCREENSHOT_RETRY = 0.5
# time for the console to take a command before the next
COMMAND_GAP = 0.3
# the default ALSA device, as the headunit plays it: a null one, so a test
# makes no sound. media audio is left out of these tests, since the
# headunit fills a null device as fast as it can write
SILENT_ALSA = "pcm.!default {\n    type plug\n    slave.pcm { type null }\n}\n"
MICROPHONE_RATE, SAMPLE_BYTES = 16000, 2
SPEECH_SECONDS = 1
# search (84) is left out: it is the voice key, which opens the microphone
KEYCODES = {"home": 3, "back": 4, "media_next": 87, "media_previous": 88}
DPAD = {"up": 19, "down": 20, "left": 21, "right": 22, "click": 23}
ROTARY = 65536


def spoken(path):
    """the sound in what the microphone recorded, without the silence the
    DHU records around it from its null capture device"""
    pcm = path.read_bytes() if path.exists() else b""
    samples = [pcm[at:at + SAMPLE_BYTES] for at in range(0, len(pcm) - 1, SAMPLE_BYTES)]
    loud = [index for index, sample in enumerate(samples) if sample != bytes(SAMPLE_BYTES)]
    return b"".join(samples[loud[0]:loud[-1] + 1]) if loud else b""


def missing():
    """why these tests cannot run here, or None"""
    if not os.access(DHU, os.X_OK):
        return f"no DHU at {DHU}: run `make dhu`"
    if not all(path.exists() for path in GOOGLE_PAIR):
        return f"no Google signed pair in {CERT_CACHE}: see docs/certificates.md"
    return None


class Headunit:
    """the DHU, headless, its console on a pipe. it quits when its standard
    input ends, so the pipe stays open until stop"""

    def __init__(self, port, home):
        (home / ".asoundrc").write_text(SILENT_ALSA)
        env = {**os.environ, "HOME": str(home), "LD_LIBRARY_PATH": DHU_LIBS}
        self.process = subprocess.Popen(
            [DHU, "--headless", f"--config={CONFIG}", f"--adb=localhost:{port}"], cwd=DHU_DIR, env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.log = Lines(self.process.stdout)

    def command(self, line):
        self.process.stdin.write(f"{line}\n".encode())
        self.process.stdin.flush()
        time.sleep(COMMAND_GAP)

    def screenshot(self, path):
        """the headunit's screen as RGB24, once it has written the PNG whole.
        until it shows its first picture, it takes no screenshot, so the
        command is repeated"""
        deadline = time.monotonic() + SCREENSHOT_TIMEOUT
        while time.monotonic() < deadline:
            if not path.exists():
                self.command(f"screenshot {path}")
            decoded = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", path, "-f", "rawvideo", "-pix_fmt", "rgb24",
                                      "-"], capture_output=True).stdout if path.exists() else b""
            if len(decoded) == FRAME_BYTES:
                path.unlink()
                return decoded
            time.sleep(SCREENSHOT_RETRY)
        raise AssertionError(f"no screenshot at {path}")

    def stop(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=EVENT_TIMEOUT)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        self.process.stdout.close()


class Server:
    """the server on a free TCP port, its log and its input lines collected"""

    def __init__(self, certs, video_cmd=tcp.LIVE_CMD):
        # a group of its own, so stopping it stops the video command's pipeline too
        self.process = subprocess.Popen(
            [tcp.SERVER, "--listen", "127.0.0.1:0", *certs, "--video-cmd", video_cmd],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        self.log, self.input = Lines(self.process.stderr), Lines(self.process.stdout)
        self.port = int(re.search(r":(\d+)$", self.log.wait(r"^listening on ")[0]).group(1))

    def stop(self):
        os.killpg(self.process.pid, signal.SIGKILL)
        self.process.wait()
        self.process.stdout.close()
        self.process.stderr.close()


@unittest.skipIf(missing(), missing())
class DhuTest(unittest.TestCase):
    """one session for the class, as a driver would use it"""

    @classmethod
    def setUpClass(cls):
        tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(tmp.cleanup)
        cls.dir = pathlib.Path(tmp.name)
        cls.recorded = cls.dir / "microphone.pcm"
        cls.server = Server(["--cert", GOOGLE_PAIR[0], "--key", GOOGLE_PAIR[1], "--mic-cmd", f"cat > {cls.recorded}"],
                            tcp.DEMO_CMD)
        cls.addClassCleanup(cls.server.stop)
        cls.headunit = Headunit(cls.server.port, cls.dir)
        cls.addClassCleanup(cls.headunit.stop)
        cls.server.log.wait(r"^video started ", timeout=SESSION_TIMEOUT)

    def test_negotiates_a_video_session(self):
        self.server.log.wait(r"^headunit authenticated$")
        self.server.log.wait(rf"^video started {WIDTH}x{HEIGHT}@30$")
        self.headunit.log.wait(r"Verify returned: ok")

    def test_the_microphone_hears_what_the_dhu_plays_into_it(self):
        """`mic play` presses the voice key, and once the phone opens the
        microphone plays a file into it. the DHU took no setup request
        there, and refused to play into a microphone held open"""
        speech = self.dir / "speech.wav"
        subprocess.run(["ffmpeg", "-loglevel", "error", "-f", "lavfi", "-i",
                        f"sine=frequency={tcp.TONE}:sample_rate={MICROPHONE_RATE}:duration={SPEECH_SECONDS}",
                        "-ac", "1", speech], check=True)
        self.headunit.command(f"mic play {speech}")
        self.server.log.wait(rf"^microphone open {MICROPHONE_RATE}hz 16 bit 1 channels$")
        deadline = time.monotonic() + EVENT_TIMEOUT
        while len(heard := spoken(self.recorded)) < SPEECH_SECONDS * MICROPHONE_RATE * SAMPLE_BYTES:
            self.assertLess(time.monotonic(), deadline, "the microphone command never heard the speech")
            time.sleep(SCREENSHOT_RETRY)
        self.assertAlmostEqual(tcp.tone_frequency(heard, MICROPHONE_RATE, 1), tcp.TONE, delta=tcp.TONE_TOLERANCE)
        self.headunit.command("mic begin")
        self.server.log.wait(r"^microphone closed$")
        self.assertFalse([line for line in self.server.log.lines if "did not expect" in line])

    def test_offers_the_touchscreen_and_the_rotary_controller(self):
        line = self.server.log.wait(r"^input channel \d+: ")[0]
        self.assertIn(f"touchscreen {WIDTH}x{HEIGHT}", line)
        self.assertIn(f" {ROTARY} ", line)

    def test_draws_the_moving_video(self):
        first = self.headunit.screenshot(self.dir / "first.png")
        time.sleep(1)
        second = self.headunit.screenshot(self.dir / "second.png")
        self.assertNotEqual(first, second, "the picture is not moving")

    def test_taps_become_touch_events(self):
        self.headunit.command("tap 200 300")
        self.server.input.wait(r"^touch down 0 0:200,300$")
        self.server.input.wait(r"^touch up 0 0:200,300$")

    def test_keycodes_become_button_events(self):
        for name, code in KEYCODES.items():
            self.headunit.command(f"keycode {name}")
            self.server.input.wait(rf"^button {code} down$")
            self.server.input.wait(rf"^button {code} up$")

    def test_the_controller_tilts_and_clicks_as_the_dpad(self):
        for name, code in DPAD.items():
            self.headunit.command(f"dpad {name}")
            self.server.input.wait(rf"^button {code} down$")
            self.server.input.wait(rf"^button {code} up$")

    def test_the_controller_turns_as_relative_events(self):
        self.headunit.command("dpad rotate left")
        self.server.input.wait(rf"^relative {ROTARY} -1$")
        self.headunit.command("dpad rotate right")
        self.server.input.wait(rf"^relative {ROTARY} 1$")

    def test_reports_night_and_driving_restrictions(self):
        self.headunit.command("night")
        self.server.input.wait(r"^night 1$")
        self.headunit.command("day")
        self.server.input.wait(r"^night 0$", count=2)
        self.headunit.command("restrict all")
        self.server.input.wait(r"^driving [1-9]\d*$")
        self.headunit.command("restrict none")
        self.server.input.wait(r"^driving 0$", count=2)

    def test_reports_speed_and_fuel(self):
        self.headunit.command("speed")
        self.server.input.wait(r"^speed ")
        self.headunit.command("fuel")
        self.server.input.wait(r"^fuel ")

    def test_video_comes_back_after_the_cars_own_screen(self):
        """the driver switches to the car's screen and back: the stream starts again"""
        started = len(self.server.log.wait(r"^video started "))
        self.headunit.command("focus video toggle")
        self.headunit.command("focus video toggle")
        self.server.log.wait(r"^video started ", count=started + 1)


@unittest.skipIf(missing(), missing())
class DhuCertificateTest(unittest.TestCase):
    """the headunit checks the phone certificate as a strict car does"""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = pathlib.Path(tmp.name)

    def connect(self, server, attempt=1):
        """the attempt'th connection, from a fresh headunit, once the server
        has accepted or refused it"""
        headunit = Headunit(server.port, self.dir)
        self.addCleanup(headunit.stop)
        server.log.wait(r"^(session ended|headunit authenticated)", count=attempt, timeout=SESSION_TIMEOUT)
        return headunit

    def test_refuses_a_self_signed_certificate_after_the_handshake(self):
        cert, key = tcp.make_cert(self.dir, "self-signed")
        server = Server(["--cert", cert, "--key", key])
        self.addCleanup(server.stop)
        headunit = self.connect(server)
        server.log.wait(r"^tls: handshake complete: ")
        server.log.wait(r"^session ended: auth: the headunit rejected the phone certificate after the TLS "
                        r"handshake \(status -3, authentication failure\)$")
        headunit.log.wait(r"Invalid peer certificate")

    def test_falls_back_to_the_google_signed_certificate(self):
        """the self-signed pair first, refused, then Google's on the next
        connection"""
        certs = self.dir / "certs"
        certs.mkdir()
        tcp.make_cert(certs, "10-self-signed")
        for path, suffix in zip(GOOGLE_PAIR, (".crt", ".key")):
            (certs / f"20-carservice{suffix}").write_bytes(path.read_bytes())
        (certs / "20-carservice.key").chmod(0o600)
        server = Server(["--cert-dir", certs])
        self.addCleanup(server.stop)
        self.connect(server).stop()
        server.log.wait(r"the next connection gets certificate 2 of 2$")
        self.connect(server, attempt=2)
        server.log.wait(r"^headunit authenticated$")
        presented = server.log.wait(r"^tls: presenting certificate ", count=2)
        self.assertIn("O=CarService", presented[1])


if __name__ == "__main__":
    unittest.main()
