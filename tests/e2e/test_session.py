"""end-to-end: run the real server binary against the scripted headunit over tcp"""

import base64
import functools
import operator
import os
import pathlib
import re
import select
import shutil
import socket
import ssl
import struct
import subprocess
import tempfile
import time
import unittest

import fakehu

ROOT = pathlib.Path(__file__).resolve().parents[2]
SERVER = pathlib.Path(os.environ.get("BEHEAD_BIN", ROOT / "target/debug/behead"))
FRAME_COUNT = 30
KEYFRAME_INTERVAL = 10
SPS, PPS = 7, 8
START_CODE = b"\x00\x00\x00\x01"
VERSION_TAG, EXTENSIONS_TAG = 0xA0, 0xA3
DEMO = pathlib.Path(os.environ.get("BEHEAD_DEMO_BIN", ROOT / "target/debug/behead-demo"))
DEMO_WIDTH, DEMO_HEIGHT = 800, 480
DEMO_CMD = (f"{DEMO} --width {{width}} --height {{height}} --fps {{fps}} | ffmpeg -loglevel error -f rawvideo "
            "-pix_fmt rgb24 -s {width}x{height} -r {fps} -i - -pix_fmt yuv420p -c:v libx264 -preset ultrafast "
            "-tune zerolatency -profile:v baseline -g {fps} -f h264 -")
TONE = 440
TONE_TOLERANCE = 5
TONE_CMD = (f"ffmpeg -loglevel error -f lavfi -i sine=frequency={TONE}:sample_rate={{rate}} "
            "-ac {channels} -f s16le -")
# the same at the speed it is heard, as a sound card records
REALTIME_TONE_CMD = TONE_CMD.replace("ffmpeg ", "ffmpeg -re ", 1)
AUDIO_SECONDS = 1
AUDIO_DELAY_MS = 1000
# how much sooner than the delay the first sound may arrive, measured from
# when the test sees the stream start
AUDIO_DELAY_TOLERANCE = 0.2
MEDIA_BYTES_PER_SECOND = fakehu.MEDIA_RATE * fakehu.MEDIA_CHANNELS * fakehu.SAMPLE_BITS // 8
MICROPHONE_PACKET, MICROPHONE_PACKETS = 640, 10
EXPIRED_FROM, EXPIRED_UNTIL = "20200101000000Z", "20200102000000Z"
# a real car verifies the phone certificate against google's automotive link
# root. the root is public but not bundled; the phone certificate it signed,
# with its key, is what R8 is about. tests needing them skip without them
GOOGLE_CA = os.environ.get("BEHEAD_GOOGLE_CA", "")
GOOGLE_PAIR = (os.environ.get("BEHEAD_GOOGLE_CERT", ""), os.environ.get("BEHEAD_GOOGLE_KEY", ""))
# a live source with a keyframe every second, as the session scripts make
LIVE_CMD = ("ffmpeg -loglevel error -f lavfi -i testsrc2=size={width}x{height}:rate={fps},realtime "
            "-pix_fmt yuv420p -c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -g {fps} -f h264 -")
LIVE_FRAMES = 10
# a message id no channel uses
UNKNOWN_MESSAGE = 0x80F9
# passes an annex-b stream on with only its first sps and pps, which is how a
# hardware encoder such as the pi's writes one: no parameter sets at later
# keyframes. x264 writes a picture at once, parameter sets first, so each
# read holds them whole, and everything else passes without waiting
HEADERS_ONCE = r"""
import re, sys
seen = set()
def once(found):
    kind = found.group(1)[0] & 0x1f
    kept = b"" if kind in seen else found.group()
    seen.add(kind)
    return kept
# an sps or pps, whatever its reference bits, up to the next start code of
# either length
PARAMETER_SET = re.compile(rb"\x00\x00\x01([\x07\x08\x27\x28\x47\x48\x67\x68]).*?(?=\x00{2,3}\x01)", re.S)
while chunk := sys.stdin.buffer.read1(1 << 20):
    sys.stdout.buffer.write(PARAMETER_SET.sub(once, chunk))
    sys.stdout.buffer.flush()
"""
DEMO_WARMUP_FRAMES = 10
DEMO_SETTLE_FRAMES = 10
DEMO_TOUCH = (600, 300)
# the time of day in an nmea sentence: hhmmss and hundredths
NMEA_CLOCK = r"^[0-2]\d[0-5]\d[0-5]\d\.\d\d$"


def run(*command):
    subprocess.run(command, check=True, capture_output=True)


def make_cert(directory, name):
    cert, key = directory / f"{name}.crt", directory / f"{name}.key"
    run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", f"/O={name}", "-keyout", key, "-out", cert)
    return cert, key


def make_rpi_certs(certs, cache):
    """the pi image's certificate directory, collected by `make rpi-certs`"""
    run("make", "-C", ROOT, "rpi-certs", f"RPI_CERTS={certs}", f"CERT_CACHE={cache}")


def make_expired_cert(directory, name):
    cert, key = directory / f"{name}.crt", directory / f"{name}.key"
    run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-subj", f"/O={name}", "-keyout", key,
        "-out", cert, "-not_before", EXPIRED_FROM, "-not_after", EXPIRED_UNTIL)
    return cert, key


def der(tag, content):
    length = len(content)
    if length < 0x80:
        return bytes([tag, length]) + content
    size = length.to_bytes((length.bit_length() + 7) // 8, "big")
    return bytes([tag, 0x80 | len(size)]) + size + content


def der_children(data):
    """the encoded elements inside one der element"""
    def length_at(pos):
        if data[pos] < 0x80:
            return data[pos], pos + 1
        count = data[pos] & 0x7F
        return int.from_bytes(data[pos + 1:pos + 1 + count], "big"), pos + 1 + count

    _, pos = length_at(1)
    while pos < len(data):
        length, start = length_at(pos + 1)
        yield data[pos:start + length]
        pos = start + length


def make_v1_cert(directory, name):
    """an x.509 v1 certificate, like the ones openauto and older headunits
    present. openssl 3 only writes v3, so drop the version and extension
    fields from its output and sign the result again."""
    cert, key = make_cert(directory, name)
    original = subprocess.run(["openssl", "x509", "-in", cert, "-outform", "der"], check=True, capture_output=True).stdout
    tbs, algorithm, _ = der_children(original)
    tbs = der(0x30, b"".join(item for item in der_children(tbs) if item[0] not in (VERSION_TAG, EXTENSIONS_TAG)))
    signature = subprocess.run(["openssl", "dgst", "-sha256", "-sign", key], input=tbs, check=True,
                               capture_output=True).stdout
    body = base64.encodebytes(der(0x30, tbs + algorithm + der(0x03, b"\0" + signature))).decode()
    cert.write_text(f"-----BEGIN CERTIFICATE-----\n{body}-----END CERTIFICATE-----\n")
    return cert, key


def make_sample(path):
    """a detailed source at low qp keeps frames above one protocol frame, to exercise fragmentation"""
    run("ffmpeg", "-f", "lavfi", "-i", "testsrc2=size=800x480:rate=30", "-frames:v", str(FRAME_COUNT),
        "-pix_fmt", "yuv420p", "-c:v", "libx264", "-profile:v", "baseline", "-qp", "10", "-g", str(KEYFRAME_INTERVAL),
        "-f", "h264", "-y", path)


def tone_frequency(pcm, rate, channels):
    """the pitch of a pure tone in signed 16 bit pcm, from its first channel's rising zero crossings"""
    samples = struct.unpack(f"<{len(pcm) // 2}h", pcm[:len(pcm) // 2 * 2])[::channels]
    rising = sum(1 for before, after in zip(samples, samples[1:]) if before < 0 <= after)
    return rising * rate / len(samples)


def count_frames(path):
    out = subprocess.run(["ffprobe", "-v", "error", "-count_frames", "-select_streams", "v:0",
                          "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0", path],
                         check=True, capture_output=True, text=True)
    return int(out.stdout.strip().strip(","))


class SessionTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.dir = pathlib.Path(cls.tmp.name)
        cls.phone = make_cert(cls.dir, "phone")
        cls.car = make_v1_cert(cls.dir, "car")
        cls.sample = cls.dir / "sample.h264"
        make_sample(cls.sample)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    TRANSPORT_ARGS = ["--listen", "127.0.0.1:0"]
    READY = r"listening on .*:(\d+)"

    def setUp(self):
        self.start_server()
        self.addCleanup(self.stop_server)

    def start_server(self, video_cmd=None, credentials=None, options=()):
        self.server = subprocess.Popen(
            [SERVER, *self.TRANSPORT_ARGS, *(credentials or ["--cert", self.phone[0], "--key", self.phone[1]]),
             "--video-cmd", video_cmd or f"cat {self.sample}", *options],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        # lines before the banner, such as the tls certificate, are kept for tests
        self.startup_log, self.ready = b"", None
        while not self.ready:
            line = self.server.stderr.readline()
            self.assertTrue(line, f"server did not start: {self.startup_log.decode()}")
            self.startup_log += line
            self.ready = re.search(self.READY, line.decode())

    def stop_server(self):
        self.server.kill()
        self.server.wait()
        self.server.stdout.close()
        self.server.stderr.close()

    def open_link(self):
        """a socket-like connection to the server, as the headunit would make it"""
        return socket.create_connection(("127.0.0.1", int(self.ready.group(1))))

    def connect(self, **options):
        link = self.open_link()
        self.addCleanup(link.close)
        headunit = fakehu.FakeHeadunit(link, *self.car, **options)
        try:
            headunit.handshake()
        except ssl.SSLError:
            # a car that refuses the phone drops the link, freeing it for the next attempt
            link.close()
            raise
        return headunit

    def expect_closed(self, headunit):
        self.assertEqual(headunit.sock.recv(1), b"", "server closes the connection after shutdown")

    def read_output(self, stream, pattern):
        deadline, seen = time.monotonic() + fakehu.TIMEOUT, b""
        while pattern not in seen:
            ready, _, _ = select.select([stream], [], [], max(0, deadline - time.monotonic()))
            self.assertTrue(ready, f"server never printed {pattern!r}, got {seen!r}")
            seen += os.read(stream.fileno(), 4096)
        return seen

    def test_negotiates_and_streams_video(self):
        headunit = self.connect()
        headunit.run_until(lambda: len(headunit.frames) == FRAME_COUNT)

        request = dict(fakehu.parse(headunit.received(0, fakehu.SERVICE_DISCOVERY_REQUEST)[0]))
        self.assertEqual(request[4], b"behead")
        self.assertEqual(headunit.received(fakehu.VIDEO_CHANNEL, fakehu.SETUP_REQUEST), [fakehu.field(1, 3)])
        self.assertEqual(headunit.received(fakehu.VIDEO_CHANNEL, fakehu.VIDEO_FOCUS_REQUEST),
                         [fakehu.field(2, fakehu.FOCUS_PROJECTED)])
        self.assertEqual(headunit.received(fakehu.VIDEO_CHANNEL, fakehu.START_INDICATION), [fakehu.field(1, 0) + fakehu.field(2, 0)])

        self.assertEqual(len(headunit.config), 1, "parameter sets are delivered once")
        nal_types = [nal[0] & 0x1F for nal in headunit.config[0].split(START_CODE)[1:]]
        self.assertEqual(nal_types, [SPS, PPS])
        self.assertGreater(headunit.multi_frame_messages, 0, "keyframes should span several protocol frames")
        timestamps = [timestamp for timestamp, _ in headunit.frames]
        self.assertEqual(timestamps, sorted(timestamps))

        received = self.dir / "received.h264"
        received.write_bytes(headunit.config[0] + b"".join(data for _, data in headunit.frames))
        self.assertEqual(count_frames(received), FRAME_COUNT)

    def test_sends_the_last_picture_before_the_screen_goes_still(self):
        """an encoder writes nothing while the screen does not change, so the
        last picture cannot wait for the next one to show where it ends"""
        self.restart_server(f"cat {self.sample}; exec sleep 60")
        headunit = self.connect()
        headunit.run_until(lambda: len(headunit.frames) == FRAME_COUNT)

    def start_live_server(self):
        """a live source that logs each start, so a test can see it survive"""
        starts = self.dir / "starts.log"
        starts.unlink(missing_ok=True)
        self.restart_server(f"echo started >> {starts}; exec {LIVE_CMD}")
        return starts

    def assert_decodes(self, config, frames):
        received = self.dir / "resumed.h264"
        received.write_bytes(config + b"".join(data for _, data in frames))
        self.assertEqual(count_frames(received), len(frames), "the stream resumes at a keyframe")

    def test_video_command_outlives_lost_focus(self):
        """the driver switching to the car's own screen and back must not
        restart what the phone shows"""
        starts = self.start_live_server()
        headunit = self.connect()
        headunit.run_until(lambda: len(headunit.frames) >= LIVE_FRAMES)
        headunit.set_video_focus(False)
        headunit.run_until(lambda: headunit.received(fakehu.VIDEO_CHANNEL, fakehu.STOP_INDICATION))
        headunit.set_video_focus(True)
        resumed = len(headunit.frames)
        headunit.run_until(lambda: len(headunit.frames) >= resumed + LIVE_FRAMES)
        self.assertEqual(starts.read_text().count("started"), 1)
        self.assert_decodes(headunit.config[-1], headunit.frames[resumed:])

    def test_video_command_outlives_a_reconnect(self):
        starts = self.start_live_server()
        first = self.connect()
        first.run_until(lambda: len(first.frames) >= LIVE_FRAMES)
        first.sock.close()
        second = self.connect()
        second.run_until(lambda: len(second.frames) >= LIVE_FRAMES)
        self.assertEqual(starts.read_text().count("started"), 1)
        self.assert_decodes(second.config[0], second.frames)

    def test_resumes_a_stream_whose_parameter_sets_came_only_once(self):
        """a hardware encoder writes its parameter sets at the start and
        never again, so a car that connects later gets them from the server"""
        script = self.dir / "headers_once.py"
        script.write_text(HEADERS_ONCE)
        self.restart_server(f"{LIVE_CMD} | python3 {script}")
        first = self.connect()
        first.run_until(lambda: len(first.frames) >= LIVE_FRAMES)
        first.sock.close()
        second = self.connect()
        second.run_until(lambda: len(second.frames) >= LIVE_FRAMES)
        self.assert_decodes(second.config[0], second.frames)

    def test_restarts_the_video_command_for_a_new_mode(self):
        starts = self.start_live_server()
        first = self.connect()
        first.run_until(lambda: len(first.frames) >= LIVE_FRAMES)
        first.sock.close()
        second = self.connect(resolution=fakehu.RESOLUTION_1280X720)
        second.run_until(lambda: len(second.frames) >= LIVE_FRAMES)
        self.assertEqual(starts.read_text().count("started"), 2)

    def test_unsolicited_focus_starts_the_stream_once(self):
        headunit = self.connect(unsolicited_focus=True)
        headunit.run_until(lambda: len(headunit.frames) == FRAME_COUNT)
        self.assertEqual(len(headunit.received(fakehu.VIDEO_CHANNEL, fakehu.START_INDICATION)), 1)

    def test_encoder_gets_the_negotiated_mode(self):
        headunit = self.connect(resolution=fakehu.RESOLUTION_1280X720, frame_rate=fakehu.FPS_60)
        headunit.run_until(lambda: headunit.received(fakehu.VIDEO_CHANNEL, fakehu.START_INDICATION))
        self.assertIn(b"video started 1280x720@60\n", self.read_output(self.server.stderr, b"video started"))

    def test_respects_ack_window(self):
        headunit = self.connect(max_unacked=1, auto_ack=False)
        headunit.run_until(lambda: len(headunit.frames) == 1)
        headunit.sock.settimeout(0.5)
        with self.assertRaises(TimeoutError, msg="a second frame arrived before the first was acked"):
            headunit.run_until(lambda: len(headunit.frames) == 2)
        headunit.sock.settimeout(fakehu.TIMEOUT)
        headunit.ack()
        headunit.run_until(lambda: len(headunit.frames) == 2)

    def test_reports_touch_input(self):
        headunit = self.connect()
        headunit.run_until(lambda: headunit.received(fakehu.INPUT_CHANNEL, fakehu.BINDING_REQUEST))
        self.assertEqual(headunit.received(fakehu.INPUT_CHANNEL, fakehu.BINDING_REQUEST), [fakehu.field(1, 3) + fakehu.field(1, 4)])
        first, second = (0, 123, 456), (1, 300, 200)
        headunit.touch(fakehu.TOUCH_DOWN, [first])
        headunit.touch(fakehu.TOUCH_POINTER_DOWN, [first, second], action_index=1)
        headunit.touch(fakehu.TOUCH_MOVED, [first, (1, 310, 210)], action_index=1)
        headunit.touch(fakehu.TOUCH_POINTER_UP, [first, (1, 310, 210)], action_index=1)
        headunit.touch(fakehu.TOUCH_UP, [first])
        expected = (b"touch down 0 0:123,456\n"
                    b"touch pointer-down 1 0:123,456 1:300,200\n"
                    b"touch move 1 0:123,456 1:310,210\n"
                    b"touch pointer-up 1 0:123,456 1:310,210\n"
                    b"touch up 0 0:123,456\n")
        self.assertEqual(self.read_output(self.server.stdout, b"touch up"), expected)

    def test_reports_a_rotary_knob(self):
        """a knob turns as relative events, either way, tilts as the dpad and
        clicks as its centre. a slider reports the position it was moved to"""
        headunit = self.connect(keycodes=(fakehu.ROTARY, fakehu.DPAD_LEFT, fakehu.DPAD_CENTER))
        headunit.run_until(lambda: headunit.received(fakehu.INPUT_CHANNEL, fakehu.BINDING_REQUEST))
        headunit.turn(fakehu.ROTARY, 2)
        headunit.turn(fakehu.ROTARY, -1)
        headunit.button(fakehu.DPAD_LEFT, True)
        headunit.button(fakehu.DPAD_LEFT, False)
        headunit.set_control(fakehu.ROTARY, 40)
        headunit.button(fakehu.DPAD_CENTER, True)
        expected = (b"relative 65536 2\nrelative 65536 -1\nbutton 21 down\nbutton 21 up\n"
                    b"absolute 65536 40\nbutton 23 down\n")
        self.assertEqual(self.read_output(self.server.stdout, b"button 23 down"), expected)

    def test_serves_a_car_without_a_touchscreen(self):
        """a car driven by a knob and buttons alone gets its picture, and
        what its controls send is heard"""
        headunit = self.connect(touchscreen=None, keycodes=(fakehu.ROTARY, fakehu.DPAD_CENTER))
        headunit.run_until(lambda: len(headunit.frames) == FRAME_COUNT)
        self.assertIn(b"input channel 1: keys 65536 23\n", self.read_output(self.server.stderr, b"input channel"))
        headunit.turn(fakehu.ROTARY, 1)
        headunit.button(fakehu.DPAD_CENTER, True)
        self.assertEqual(self.read_output(self.server.stdout, b"button 23 down"),
                         b"relative 65536 1\nbutton 23 down\n")

    def test_reports_a_touchpad_on_a_channel_of_its_own(self):
        """a car's touchpad, such as the pads on a steering wheel, can be an
        input service beside the touchscreen's, with keys of its own. both
        are bound, and both are heard"""
        headunit = self.connect(touchpad=(1000, 600, (fakehu.DPAD_UP,)))
        for channel, keys in ((fakehu.INPUT_CHANNEL, (3, 4)), (fakehu.TOUCHPAD_CHANNEL, (fakehu.DPAD_UP,))):
            headunit.run_until(lambda: headunit.received(channel, fakehu.BINDING_REQUEST))
            asked = b"".join(fakehu.field(1, key) for key in keys)
            self.assertEqual(headunit.received(channel, fakehu.BINDING_REQUEST), [asked])
        headunit.touch_pad(fakehu.TOUCH_DOWN, [(0, 10, 20)])
        headunit.touch_pad(fakehu.TOUCH_MOVED, [(0, 400, 20)])
        headunit.touch_pad(fakehu.TOUCH_UP, [(0, 400, 20)])
        headunit.button(fakehu.DPAD_UP, True, channel=fakehu.TOUCHPAD_CHANNEL)
        headunit.touch(fakehu.TOUCH_DOWN, [(0, 1, 2)])
        expected = (b"touchpad down 0 0:10,20\ntouchpad move 0 0:400,20\ntouchpad up 0 0:400,20\n"
                    b"button 19 down\ntouch down 0 0:1,2\n")
        self.assertEqual(self.read_output(self.server.stdout, b"touch down"), expected)
        log = self.read_output(self.server.stderr, b"touchpad 1000x600")
        self.assertIn(f"input channel {fakehu.TOUCHPAD_CHANNEL}: keys 19, touchpad 1000x600\n".encode(), log)
        self.assertIn(f"input channel {fakehu.INPUT_CHANNEL}: keys 3 4, touchscreen 800x480\n".encode(), log)

    def test_reports_what_it_has_no_name_for(self):
        """a car sends more than is known here. whatever arrives unread comes
        out with its bytes, so a control that does nothing can be looked into:
        a field of an input or sensor event, or a whole message"""
        headunit = self.connect()
        headunit.run_until(lambda: headunit.received(fakehu.INPUT_CHANNEL, fakehu.BINDING_REQUEST))
        stamp = fakehu.field(1, 5)
        headunit.send(fakehu.INPUT_CHANNEL, fakehu.INPUT_EVENT,
                      stamp + fakehu.field(9, b"\x01\x02") + fakehu.field(12, 7))
        headunit.send(fakehu.SENSOR_CHANNEL, fakehu.SENSOR_EVENT, fakehu.field(21, b"\x08\x01"))
        headunit.send(fakehu.INPUT_CHANNEL, UNKNOWN_MESSAGE, b"\xab\xcd")
        headunit.button(fakehu.HOME, True)
        expected = (f"unknown input {fakehu.INPUT_CHANNEL} field 9 0102\n"
                    f"unknown input {fakehu.INPUT_CHANNEL} field 12 07\n"
                    f"unknown sensor {fakehu.SENSOR_CHANNEL} field 21 0801\n"
                    f"unknown message {fakehu.INPUT_CHANNEL} {UNKNOWN_MESSAGE:#06x} abcd\n"
                    "button 3 down\n").encode()
        printed = self.read_output(self.server.stdout, b"button 3 down")
        self.assertEqual(b"".join(line for line in printed.splitlines(True) if b"unknown" in line or b"button" in line),
                         expected)

    def test_reports_sensor_readings(self):
        """the car's sensors, each asked for, come out as lines in plain
        units where the scale is known and as sent where it is not"""
        headunit = self.connect()
        headunit.run_until(lambda: set(headunit.sensors_started) == set(fakehu.SENSORS))
        timestamp = 1790000000000
        # berlin, then a fix with nothing but a position, west of greenwich
        headunit.sense(fakehu.SENSOR_LOCATION, timestamp, 525200066, 134049540, 5000, 3450, 12500, 90500000)
        headunit.sense(fakehu.SENSOR_LOCATION, timestamp, 407127530, -740059730)
        headunit.sense(fakehu.SENSOR_COMPASS, 90500000, 0, -1500000)
        headunit.sense(fakehu.SENSOR_SPEED, 27780)
        headunit.sense(fakehu.SENSOR_RPM, 2100)
        headunit.sense(fakehu.SENSOR_ODOMETER, 123456, 789)
        headunit.sense(fakehu.SENSOR_FUEL, 60, 420, 0)
        headunit.sense(fakehu.SENSOR_PARKING_BRAKE, 1)
        headunit.sense(fakehu.SENSOR_GEAR, 100)
        headunit.sense(fakehu.SENSOR_ENVIRONMENT, 21500, 101300, 0)
        headunit.sense(fakehu.SENSOR_NIGHT, 1)
        headunit.sense(fakehu.SENSOR_DRIVING_STATUS, 0)
        expected = (b"location 52.5200066 13.404954 5 34.5 12.5 90.5\n"
                    b"location 40.712753 -74.005973 - - - -\n"
                    b"compass 90.5 0 -1.5\n"
                    b"speed 27.78\n"
                    b"rpm 2100\n"
                    b"odometer 123456 789\n"
                    b"fuel 60 420 0\n"
                    b"parking-brake 1\n"
                    b"gear 100\n"
                    b"environment 21500 101300 0\n"
                    b"night 1\n"
                    b"driving 0\n")
        self.assertEqual(self.read_output(self.server.stdout, b"driving 0\n"), expected)

    def test_video_command_learns_readings_from_before_it_started(self):
        """a car reports night mode once, at connect, before it shows the
        phone. a session started later still has to hear of it"""
        received = self.dir / "sensor-lines.log"
        received.unlink(missing_ok=True)
        self.restart_server(f"cat {self.sample} & cat > {received}")
        headunit = self.connect(hold_focus=True)
        headunit.run_until(lambda: fakehu.SENSOR_NIGHT in headunit.sensors_started)
        headunit.sense(fakehu.SENSOR_NIGHT, 1)
        headunit.sense(fakehu.SENSOR_DRIVING_STATUS, 0)
        headunit.sense(fakehu.SENSOR_NIGHT, 0)
        self.read_output(self.server.stdout, b"night 0\n")
        self.assertFalse(received.exists(), "the video command started without video focus")
        headunit.set_video_focus(True)
        headunit.run_until(lambda: headunit.frames)
        deadline = time.monotonic() + fakehu.TIMEOUT
        while "night" not in (received.read_text() if received.exists() else ""):
            self.assertLess(time.monotonic(), deadline, "the video command never heard of the readings")
            time.sleep(0.05)
        self.assertEqual(sorted(received.read_text().splitlines()), ["driving 0", "night 0"])

    def read_nmea(self, path):
        """the pair of sentences a reader of the nmea socket gets first, with
        their time of day and checksums taken off once checked"""
        with socket.socket(socket.AF_UNIX) as reader:
            reader.settimeout(fakehu.TIMEOUT)
            reader.connect(str(path))
            received = b""
            while received.count(b"\r\n") < 2:
                received += reader.recv(4096)
        sentences = []
        for sentence in received.decode().split("\r\n")[:2]:
            body, checksum = sentence[1:].split("*")
            self.assertEqual(f"{functools.reduce(operator.xor, body.encode()):02X}", checksum, sentence)
            kind, clock, rest = body.split(",", 2)
            self.assertRegex(clock, NMEA_CLOCK)
            sentences.append(f"{kind},{rest}")
        return sentences

    def test_offers_the_cars_location_as_nmea(self):
        """location services such as geoclue read a gps receiver's nmea
        sentences from a socket, so the car's fixes are offered as one"""
        path = self.dir / "nmea.sock"
        self.restart_server(options=["--nmea-socket", path])
        headunit = self.connect()
        headunit.run_until(lambda: fakehu.SENSOR_LOCATION in headunit.sensors_started)
        # berlin, moving east, to a reader that connected after the fix
        headunit.sense(fakehu.SENSOR_LOCATION, 1790000000000, 525200066, 134049540, 5000, 3450, 12500, 90500000)
        self.read_output(self.server.stdout, b"location 52.5")
        self.assertEqual(self.read_nmea(path), ["GPGGA,5231.20039,N,01324.29724,E,1,,1.0,34.5,M,,M,,",
                                                "GPRMC,A,5231.20039,N,01324.29724,E,24.30,90.5,,,,A"])
        # a fix with nothing but a position, south and west
        headunit.sense(fakehu.SENSOR_LOCATION, None, -339000000, -740059730)
        self.read_output(self.server.stdout, b"location -33.9")
        self.assertEqual(self.read_nmea(path), ["GPGGA,3354.00000,S,07400.35838,W,1,,,,M,,M,,",
                                                "GPRMC,A,3354.00000,S,07400.35838,W,,,,,,A"])

    def test_nmea_readers_follow_the_car(self):
        """a reader stays connected and hears of every fix, also from the
        next headunit connection"""
        path = self.dir / "nmea.sock"
        self.restart_server(options=["--nmea-socket", path])
        with socket.socket(socket.AF_UNIX) as reader:
            reader.settimeout(fakehu.TIMEOUT)
            reader.connect(str(path))
            for latitude, sentence in ((525200066, b",5231.20039,N,"), (525300066, b",5231.80039,N,")):
                headunit = self.connect()
                headunit.run_until(lambda: fakehu.SENSOR_LOCATION in headunit.sensors_started)
                headunit.sense(fakehu.SENSOR_LOCATION, None, latitude, 134049540)
                received = b""
                while received.count(b"\r\n") < 2:
                    received += reader.recv(4096)
                self.assertIn(sentence, received)
                headunit.sock.close()

    def test_answers_ping_and_shutdown(self):
        headunit = self.connect()
        headunit.run_until(lambda: len(headunit.frames) == FRAME_COUNT)
        headunit.send(0, fakehu.PING_REQUEST, fakehu.field(1, 42))
        headunit.run_until(lambda: headunit.received(0, fakehu.PING_RESPONSE))
        self.assertEqual(headunit.received(0, fakehu.PING_RESPONSE), [fakehu.field(1, 42)])
        headunit.send(0, fakehu.SHUTDOWN_REQUEST, fakehu.field(1, 1))
        headunit.run_until(lambda: headunit.received(0, fakehu.SHUTDOWN_RESPONSE))
        self.expect_closed(headunit)

    def restart_server(self, video_cmd=None, credentials=None, options=()):
        SessionTest.stop_server(self)
        self.start_server(video_cmd, credentials, options)

    def test_video_command_receives_input(self):
        """input lines also go to the video command, so it can react to them"""
        received = self.dir / "video-input.log"
        received.unlink(missing_ok=True)
        # the background cat sends the video; the foreground one records stdin
        self.restart_server(f"cat {self.sample} & cat > {received}")
        headunit = self.connect()
        # input goes to the command once video, and so the command, has started
        headunit.run_until(lambda: headunit.received(fakehu.INPUT_CHANNEL, fakehu.BINDING_REQUEST) and headunit.frames)
        headunit.touch(fakehu.TOUCH_DOWN, [(0, 123, 456)])
        headunit.touch(fakehu.TOUCH_UP, [(0, 123, 456)])
        self.read_output(self.server.stdout, b"touch up")
        deadline = time.monotonic() + fakehu.TIMEOUT
        while b"touch up" not in (received.read_bytes() if received.exists() else b""):
            self.assertLess(time.monotonic(), deadline, "the video command never saw the input")
            time.sleep(0.05)
        self.assertEqual(received.read_text(), "touch down 0 0:123,456\ntouch up 0 0:123,456\n")

    DEMO_CMD = DEMO_CMD

    def test_demo_program_draws_touches(self):
        self.restart_server(self.DEMO_CMD)
        headunit = self.connect()
        headunit.run_until(lambda: len(headunit.frames) >= DEMO_WARMUP_FRAMES)
        x, y = DEMO_TOUCH
        headunit.touch(fakehu.TOUCH_DOWN, [(0, x, y)])
        touched_at = len(headunit.frames)
        headunit.run_until(lambda: len(headunit.frames) >= touched_at + DEMO_SETTLE_FRAMES)

        stream = self.dir / "demo.h264"
        stream.write_bytes(headunit.config[0] + b"".join(data for _, data in headunit.frames))
        # passthrough keeps every frame whatever timing the encoder declared
        raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", stream, "-fps_mode", "passthrough",
                              "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], check=True, capture_output=True).stdout
        frame_bytes = DEMO_WIDTH * DEMO_HEIGHT * 3
        before, after = raw[:frame_bytes], raw[-frame_bytes:]
        offset = (y * DEMO_WIDTH + x) * 3
        red, green, blue = after[offset:offset + 3]
        self.assertGreater(red, 180, f"no touch marker under the finger: {red},{green},{blue}")
        self.assertLess(green, 120)
        self.assertNotEqual(before[offset:offset + 3], after[offset:offset + 3])

    def test_logs_the_tls_handshake(self):
        self.connect()
        log = self.startup_log + self.read_output(self.server.stderr, b"tls: handshake complete")
        self.assertIn(b"tls: presenting certificate subject O=phone, issuer O=phone, valid ", log)
        self.assertIn(b"tls: headunit offers TLSv1_2 with cipher suites ", log)
        self.assertRegex(log, rb"TLS_ECDHE_RSA_WITH_AES_\d+_GCM_SHA\d+")
        self.assertRegex(log, rb"tls: handshake complete: TLSv1_2 TLS_ECDHE_\w+, headunit certificate "
                              rb"subject O=car, issuer O=car, valid \d{4}-\d\d-\d\d to \d{4}-\d\d-\d\d")

    def test_logs_a_rejected_phone_certificate(self):
        """what a car that verifies the phone certificate looks like in the log"""
        with self.assertRaises(ssl.SSLCertVerificationError):
            self.connect(verify_phone=True)
        log = self.read_output(self.server.stderr, b"session ended")
        self.assertIn(b"tls: the headunit rejected the phone certificate (alert UnknownCA)", log)

    def test_falls_back_to_the_next_certificate(self):
        """a car that refuses the phone certificate is offered the next one in
        --cert-dir, by name, on its following connection. an accepted one is
        kept, and the list starts over after the last."""
        certs = self.dir / "certs"
        certs.mkdir(exist_ok=True)
        first, second = make_cert(certs, "10-first"), make_cert(certs, "20-second")
        self.restart_server(credentials=["--cert-dir", certs])
        steps = [(second, False), (second, True), (second, True), (first, False), (first, True)]
        for trusted, accepted in steps:
            if accepted:
                self.connect(trusted_phone=trusted[0]).sock.close()
            else:
                with self.assertRaises(ssl.SSLCertVerificationError):
                    self.connect(trusted_phone=trusted[0])
        log = b""
        while log.count(b"tls: presenting certificate") < len(steps):
            log += self.read_output(self.server.stderr, b"tls: presenting certificate")
        presented = re.findall(rb"tls: presenting certificate subject O=([\w-]+), issuer ", log)
        self.assertEqual(presented, [b"10-first", b"20-second", b"20-second", b"20-second", b"10-first"])

    def test_logs_an_expired_phone_certificate(self):
        """what a car that checks validity dates does with an expired
        certificate, even one it trusts. openauto checks nothing"""
        expired = make_expired_cert(self.dir, "expired")
        self.restart_server(credentials=["--cert", expired[0], "--key", expired[1]])
        with self.assertRaises(ssl.SSLCertVerificationError):
            self.connect(trusted_phone=expired[0])
        log = self.read_output(self.server.stderr, b"session ended")
        self.assertIn(b"tls: the headunit rejected the phone certificate (alert CertificateExpired)", log)

    @unittest.skipUnless(GOOGLE_CA, "set BEHEAD_GOOGLE_CA to the google automotive link root certificate")
    def test_a_car_trusting_google_rejects_a_self_signed_certificate(self):
        with self.assertRaises(ssl.SSLCertVerificationError):
            self.connect(trusted_phone=GOOGLE_CA)

    @unittest.skipUnless(GOOGLE_CA and all(GOOGLE_PAIR),
                         "set BEHEAD_GOOGLE_CA, and BEHEAD_GOOGLE_CERT and BEHEAD_GOOGLE_KEY to a google signed pair")
    def test_a_car_trusting_google_accepts_a_google_signed_certificate(self):
        """the phone pair ../dexter recovers from the app is current (R8)"""
        self.restart_server(credentials=["--cert", GOOGLE_PAIR[0], "--key", GOOGLE_PAIR[1]])
        self.connect(trusted_phone=GOOGLE_CA)

    @unittest.skipUnless(GOOGLE_CA and all(GOOGLE_PAIR),
                         "set BEHEAD_GOOGLE_CA, and BEHEAD_GOOGLE_CERT and BEHEAD_GOOGLE_KEY to a google signed pair")
    def test_a_car_trusting_google_falls_back_to_the_google_signed_certificate(self):
        """the pi image's certificate directory, as `make rpi-certs` builds it
        from a cache holding the root and the google signed pair: the
        self-signed pair first, which a strict car refuses, then the google
        signed one, which it accepts and keeps"""
        certs, cache = self.dir / "rpi-certs", self.dir / "cache"
        cache.mkdir(exist_ok=True)
        for source, name in zip((GOOGLE_CA, *GOOGLE_PAIR), ("ca.crt", "carservice.crt", "carservice.key")):
            shutil.copy(source, cache / name)
        make_rpi_certs(certs, cache)
        self.restart_server(credentials=["--cert-dir", certs])
        with self.assertRaises(ssl.SSLCertVerificationError):
            self.connect(trusted_phone=GOOGLE_CA)
        self.connect(trusted_phone=GOOGLE_CA).sock.close()
        self.connect(trusted_phone=GOOGLE_CA)
        log = self.startup_log
        while log.count(b"tls: presenting certificate") < 3:
            log += self.read_output(self.server.stderr, b"tls: presenting certificate")
        presented = re.findall(rb"tls: presenting certificate subject .*?O=([\w-]+), issuer ", log)
        self.assertEqual(presented, [b"behead-dev", b"CarService", b"CarService"])
        self.assertIn(b"tls: the headunit rejected the phone certificate (alert UnknownCA)", log)

    def test_refuses_a_certificate_without_its_key(self):
        certs = self.dir / "keyless"
        certs.mkdir(exist_ok=True)
        make_cert(certs, "phone")[1].unlink()
        result = subprocess.run([SERVER, *self.TRANSPORT_ARGS, "--cert-dir", certs, "--video-cmd", "true"],
                                capture_output=True, timeout=fakehu.TIMEOUT)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"no key for", result.stderr)

    def test_refuses_a_knob_it_has_no_keys_for(self):
        result = subprocess.run([SERVER, *self.TRANSPORT_ARGS, "--cert", self.phone[0], "--key", self.phone[1],
                                 "--video-cmd", "true", "--knob", "sideways"],
                                capture_output=True, timeout=fakehu.TIMEOUT)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"--knob takes focus or arrows, not sideways", result.stderr)

    def test_streams_media_audio(self):
        self.restart_server(options=["--audio-cmd", TONE_CMD])
        headunit = self.connect()
        headunit.run_until(lambda: sum(len(pcm) for _, pcm in headunit.audio) >= AUDIO_SECONDS * MEDIA_BYTES_PER_SECOND)
        self.assertEqual(headunit.received(0, fakehu.AUDIO_FOCUS_REQUEST), [fakehu.field(1, fakehu.FOCUS_GAIN)])
        timestamps = [timestamp for timestamp, _ in headunit.audio]
        self.assertEqual(timestamps, sorted(timestamps))
        pcm = b"".join(data for _, data in headunit.audio)
        self.assertAlmostEqual(tone_frequency(pcm, fakehu.MEDIA_RATE, fakehu.MEDIA_CHANNELS), TONE, delta=TONE_TOLERANCE)

    def test_an_empty_audio_command_is_none(self):
        """a service unit passes the commands from its environment, where one
        left unset comes out empty. the car then keeps its own sound"""
        self.restart_server(options=["--audio-cmd", "", "--mic-cmd", "", "--audio-delay", ""])
        headunit = self.connect()
        headunit.run_until(lambda: len(headunit.frames) == FRAME_COUNT)
        self.assertEqual(headunit.received(0, fakehu.AUDIO_FOCUS_REQUEST), [])
        self.assertEqual(headunit.received(fakehu.MICROPHONE_CHANNEL, fakehu.CHANNEL_OPEN_REQUEST), [])

    def test_holds_sound_back_for_the_picture(self):
        """a picture takes longer to reach the car's screen than sound its
        speakers, so --audio-delay keeps the sound that long"""
        self.restart_server(options=["--audio-cmd", REALTIME_TONE_CMD, "--audio-delay", str(AUDIO_DELAY_MS)])
        headunit = self.connect()
        headunit.run_until(lambda: headunit.audio_session is not None)
        started = time.monotonic()
        headunit.run_until(lambda: headunit.audio)
        self.assertGreaterEqual(time.monotonic() - started, AUDIO_DELAY_MS / 1000 - AUDIO_DELAY_TOLERANCE)
        headunit.run_until(lambda: sum(len(pcm) for _, pcm in headunit.audio) >= AUDIO_SECONDS * MEDIA_BYTES_PER_SECOND)
        pcm = b"".join(data for _, data in headunit.audio)
        self.assertAlmostEqual(tone_frequency(pcm, fakehu.MEDIA_RATE, fakehu.MEDIA_CHANNELS), TONE, delta=TONE_TOLERANCE)

    def test_stops_media_audio_when_the_car_takes_focus(self):
        self.restart_server(options=["--audio-cmd", TONE_CMD])
        headunit = self.connect()
        headunit.run_until(lambda: headunit.audio)
        headunit.take_audio_focus()
        headunit.run_until(lambda: headunit.audio_session is None)

    def test_delivers_microphone_audio(self):
        recorded = self.dir / "microphone.pcm"
        recorded.unlink(missing_ok=True)
        self.restart_server(options=["--mic-cmd", f"cat > {recorded}"])
        headunit = self.connect()
        headunit.run_until(lambda: headunit.microphone_open)
        packets = [bytes([index]) * MICROPHONE_PACKET for index in range(MICROPHONE_PACKETS)]
        for packet in packets:
            headunit.speak(packet)
        headunit.run_until(lambda: len(headunit.received(fakehu.MICROPHONE_CHANNEL, fakehu.MEDIA_ACK)) == len(packets))
        deadline = time.monotonic() + fakehu.TIMEOUT
        while (recorded.read_bytes() if recorded.exists() else b"") != b"".join(packets):
            self.assertLess(time.monotonic(), deadline, "the microphone command never got the audio")
            time.sleep(0.05)

    def test_accepts_a_second_headunit(self):
        self.connect().sock.close()
        headunit = self.connect()
        headunit.run_until(lambda: len(headunit.frames) == FRAME_COUNT)


if __name__ == "__main__":
    unittest.main()
