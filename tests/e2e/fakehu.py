"""scripted headunit: an independent implementation of the car side of the
protocol, used to exercise the server over any byte-stream transport."""

import ssl
import struct
import time

import viewer

FLAG_FIRST, FLAG_LAST, FLAG_CONTROL, FLAG_ENCRYPTED = 1, 2, 4, 8
FLAG_BULK = FLAG_FIRST | FLAG_LAST
MAX_FRAME_PAYLOAD = 0x4000

VERSION_REQUEST, VERSION_RESPONSE, SSL_HANDSHAKE, AUTH_COMPLETE = 1, 2, 3, 4
SERVICE_DISCOVERY_REQUEST, SERVICE_DISCOVERY_RESPONSE = 5, 6
CHANNEL_OPEN_REQUEST, CHANNEL_OPEN_RESPONSE = 7, 8
PING_REQUEST, PING_RESPONSE, SHUTDOWN_REQUEST, SHUTDOWN_RESPONSE = 0x0B, 0x0C, 0x0F, 0x10
MEDIA_WITH_TIMESTAMP, MEDIA = 0x0000, 0x0001
SETUP_REQUEST, START_INDICATION, STOP_INDICATION = 0x8000, 0x8001, 0x8002
SETUP_RESPONSE, MEDIA_ACK = 0x8003, 0x8004
VIDEO_FOCUS_REQUEST, VIDEO_FOCUS_INDICATION = 0x8007, 0x8008
INPUT_EVENT, BINDING_REQUEST, BINDING_RESPONSE = 0x8001, 0x8002, 0x8003
AUDIO_FOCUS_REQUEST, AUDIO_FOCUS_RESPONSE = 0x12, 0x13
MICROPHONE_REQUEST, MICROPHONE_RESPONSE = 0x8005, 0x8006
SENSOR_START_REQUEST, SENSOR_START_RESPONSE, SENSOR_EVENT = 0x8001, 0x8002, 0x8003

# channel numbers as openauto assigns them
VIDEO_CHANNEL, INPUT_CHANNEL, MEDIA_AUDIO_CHANNEL, MICROPHONE_CHANNEL, SENSOR_CHANNEL = 3, 1, 4, 7, 2
# sensor types. each is also the field its readings take in a sensor event
(SENSOR_LOCATION, SENSOR_COMPASS, SENSOR_SPEED, SENSOR_RPM, SENSOR_ODOMETER, SENSOR_FUEL, SENSOR_PARKING_BRAKE,
 SENSOR_GEAR) = range(1, 9)
SENSOR_NIGHT, SENSOR_ENVIRONMENT, SENSOR_DRIVING_STATUS = 10, 11, 13
SENSORS = (SENSOR_LOCATION, SENSOR_COMPASS, SENSOR_SPEED, SENSOR_RPM, SENSOR_ODOMETER, SENSOR_FUEL,
           SENSOR_PARKING_BRAKE, SENSOR_GEAR, SENSOR_NIGHT, SENSOR_ENVIRONMENT, SENSOR_DRIVING_STATUS)
STREAM_AUDIO, AUDIO_TYPE_MEDIA, CODEC_PCM = 1, 3, 1
MEDIA_RATE, MEDIA_CHANNELS, MICROPHONE_RATE, MICROPHONE_CHANNELS, SAMPLE_BITS = 48000, 2, 16000, 1, 16
FOCUS_GAIN, FOCUS_RELEASE = 1, 4
FOCUS_STATE_GAIN, FOCUS_STATE_LOSS = 1, 3
SETUP_READY = 2
RESOLUTION_800X480, RESOLUTION_1280X720 = 1, 2
FPS_60, FPS_30 = 1, 2
FOCUS_PROJECTED, FOCUS_NATIVE = 1, 2
TOUCH_DOWN, TOUCH_UP, TOUCH_MOVED, TOUCH_POINTER_DOWN, TOUCH_POINTER_UP = 0, 1, 2, 5, 6
# android key codes
HOME, BACK, DPAD_UP, DPAD_DOWN, DPAD_LEFT, DPAD_RIGHT, DPAD_CENTER = 3, 4, 19, 20, 21, 22, 23
VARINT, LENGTH_DELIMITED = 0, 2
TIMEOUT = 10


def varint(value):
    # negative numbers go out as 64 bit two's complement
    value &= (1 << 64) - 1
    out = bytearray()
    while value > 0x7F:
        out.append(value & 0x7F | 0x80)
        value >>= 7
    return bytes(out + bytes([value]))


def field(number, value):
    """encode one protobuf field: ints as varints, bytes as length-delimited"""
    if isinstance(value, int):
        return varint(number << 3 | VARINT) + varint(value)
    return varint(number << 3 | LENGTH_DELIMITED) + varint(len(value)) + value


def parse(data):
    """decode a protobuf message into a list of (field number, value)"""
    fields, pos = [], 0

    def read_varint():
        nonlocal pos
        value = shift = 0
        while True:
            byte = data[pos]
            pos += 1
            value |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80:
                return value

    while pos < len(data):
        key = read_varint()
        if key & 7 == VARINT:
            fields.append((key >> 3, read_varint()))
        else:
            length = read_varint()
            fields.append((key >> 3, data[pos:pos + length]))
            pos += length
    return fields


def service_discovery_response(resolution, frame_rate, keycodes):
    video_config = field(1, resolution) + field(2, frame_rate) + field(3, 0) + field(4, 0) + field(5, 140)
    video = field(1, VIDEO_CHANNEL) + field(3, field(1, 3) + field(4, video_config))
    keys = b"".join(field(1, keycode) for keycode in keycodes)
    touch = field(1, INPUT_CHANNEL) + field(4, keys + field(2, field(1, 800) + field(2, 480)))
    media_config = field(1, MEDIA_RATE) + field(2, SAMPLE_BITS) + field(3, MEDIA_CHANNELS)
    media = field(1, MEDIA_AUDIO_CHANNEL) + field(3, field(1, STREAM_AUDIO) + field(2, AUDIO_TYPE_MEDIA)
                                                   + field(3, media_config))
    microphone_config = field(1, MICROPHONE_RATE) + field(2, SAMPLE_BITS) + field(3, MICROPHONE_CHANNELS)
    microphone = field(1, MICROPHONE_CHANNEL) + field(5, field(1, STREAM_AUDIO) + field(2, microphone_config))
    sensors = field(1, SENSOR_CHANNEL) + field(2, b"".join(field(1, field(1, sensor)) for sensor in SENSORS))
    channels = b"".join(field(1, channel) for channel in (video, touch, media, microphone, sensors))
    return channels + field(2, b"fakehu")


class FakeHeadunit:
    def __init__(self, sock, cert, key, max_unacked=4, auto_ack=True, unsolicited_focus=False,
                 resolution=RESOLUTION_800X480, frame_rate=FPS_30, verify_phone=False, trusted_phone=None,
                 keycodes=(HOME, BACK), hold_focus=False):
        """verify_phone: check the phone certificate like a strict car, against
        no trusted authority at all, so any certificate is rejected.
        trusted_phone: check it against this one certificate instead.
        keycodes: the android key codes the car says it has buttons for.
        hold_focus: stay on the car's own screen until set_video_focus"""
        self.sock, self.max_unacked, self.auto_ack = sock, max_unacked, auto_ack
        self.keycodes, self.hold_focus = keycodes, hold_focus
        self.unsolicited_focus, self.resolution, self.frame_rate = unsolicited_focus, resolution, frame_rate
        self.sock.settimeout(TIMEOUT)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.check_hostname = False
        context.verify_mode = ssl.CERT_REQUIRED if verify_phone or trusted_phone else ssl.CERT_NONE
        if trusted_phone:
            context.load_verify_locations(trusted_phone)
        context.maximum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(cert, key)
        self.incoming, self.outgoing = ssl.MemoryBIO(), ssl.MemoryBIO()
        self.tls = context.wrap_bio(self.incoming, self.outgoing, server_side=False)
        self.buffer, self.partial = b"", {}
        self.log = []  # every (channel, flags, id, body) received
        self.config, self.frames, self.arrivals, self.multi_frame_messages = [], [], [], 0
        # media audio packets as (timestamp, pcm), and the session the phone started, while started
        self.audio, self.audio_session = [], None
        self.microphone_open = False
        # the sensor types the phone asked for
        self.sensors_started = []
        self.viewer = viewer.shared()

    def read_exact(self, count):
        while len(self.buffer) < count:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise EOFError("server closed the connection")
            self.buffer += chunk
        data, self.buffer = self.buffer[:count], self.buffer[count:]
        return data

    def decrypt(self, payload):
        self.incoming.write(payload)
        plain = b""
        while True:
            try:
                plain += self.tls.read(65536)
            except ssl.SSLWantReadError:
                return plain

    def recv(self):
        """read frames until one whole message is assembled"""
        while True:
            channel, flags, length = struct.unpack(">BBH", self.read_exact(4))
            if flags & FLAG_BULK == FLAG_FIRST:
                self.read_exact(4)
                self.multi_frame_messages += 1
            payload = self.read_exact(length)
            chunk = self.decrypt(payload) if flags & FLAG_ENCRYPTED else payload
            self.partial[channel] = chunk if flags & FLAG_FIRST else self.partial[channel] + chunk
            if flags & FLAG_LAST:
                plain = self.partial.pop(channel)
                message = (channel, flags, struct.unpack(">H", plain[:2])[0], plain[2:])
                self.log.append(message)
                return message

    def send(self, channel, message_id, body=b"", encrypted=True, flags=0):
        plain = struct.pack(">H", message_id) + body
        if encrypted:
            self.tls.write(plain)
            plain, flags = self.outgoing.read(), flags | FLAG_ENCRYPTED
        self.sock.sendall(struct.pack(">BBH", channel, flags | FLAG_BULK, len(plain)) + plain)

    def expect(self, channel, message_id):
        got_channel, _, got_id, body = self.recv()
        assert (got_channel, got_id) == (channel, message_id), f"expected {message_id:#x} on {channel}, got {got_id:#x} on {got_channel}"
        return body

    def handshake(self):
        self.send(0, VERSION_REQUEST, struct.pack(">HH", 1, 1), encrypted=False)
        major, _, status = struct.unpack(">HHH", self.expect(0, VERSION_RESPONSE))
        assert (major, status) == (1, 0)
        while True:
            try:
                self.tls.do_handshake()
                break
            except ssl.SSLWantReadError:
                self.send(0, SSL_HANDSHAKE, self.outgoing.read(), encrypted=False)
                self.incoming.write(self.expect(0, SSL_HANDSHAKE))
            except ssl.SSLError:
                # deliver the alert explaining the failure, as a car would
                self.send(0, SSL_HANDSHAKE, self.outgoing.read(), encrypted=False)
                raise
        self.send(0, AUTH_COMPLETE, field(1, 0), encrypted=False)

    def step(self):
        """receive one message and answer it the way a headunit would"""
        channel, flags, message_id, body = self.recv()
        control = channel == 0 or flags & FLAG_CONTROL
        if control and message_id == SERVICE_DISCOVERY_REQUEST:
            self.send(0, SERVICE_DISCOVERY_RESPONSE,
                      service_discovery_response(self.resolution, self.frame_rate, self.keycodes))
        elif control and message_id == CHANNEL_OPEN_REQUEST:
            self.send(channel, CHANNEL_OPEN_RESPONSE, field(1, 0), flags=FLAG_CONTROL)
        elif channel == VIDEO_CHANNEL and message_id == SETUP_REQUEST:
            self.send(channel, SETUP_RESPONSE, field(1, 2) + field(2, self.max_unacked) + field(3, 0))
            if self.unsolicited_focus:
                self.send(channel, VIDEO_FOCUS_INDICATION, field(1, FOCUS_PROJECTED) + field(2, 1))
        elif channel == VIDEO_CHANNEL and message_id == VIDEO_FOCUS_REQUEST and not self.hold_focus:
            self.send(channel, VIDEO_FOCUS_INDICATION, field(1, FOCUS_PROJECTED) + field(2, 0))
        elif channel == VIDEO_CHANNEL and message_id == MEDIA:
            self.config.append(body)
            self.show(body)
        elif channel == VIDEO_CHANNEL and message_id == MEDIA_WITH_TIMESTAMP:
            self.frames.append((struct.unpack(">Q", body[:8])[0], body[8:]))
            self.show(body[8:])
            self.arrivals.append(time.monotonic())
            if self.auto_ack:
                self.ack()
        elif channel == INPUT_CHANNEL and message_id == BINDING_REQUEST:
            self.send(channel, BINDING_RESPONSE, field(1, 0))
        elif channel == SENSOR_CHANNEL and message_id == SENSOR_START_REQUEST:
            self.sensors_started.append(dict(parse(body))[1])
            self.send(channel, SENSOR_START_RESPONSE, field(1, 0))
        elif control and message_id == AUDIO_FOCUS_REQUEST:
            # like openauto: grant anything but a release
            state = FOCUS_STATE_LOSS if dict(parse(body))[1] == FOCUS_RELEASE else FOCUS_STATE_GAIN
            self.send(0, AUDIO_FOCUS_RESPONSE, field(1, state))
        elif channel in (MEDIA_AUDIO_CHANNEL, MICROPHONE_CHANNEL) and message_id == SETUP_REQUEST:
            # like openauto: one packet in flight per channel
            self.send(channel, SETUP_RESPONSE, field(1, SETUP_READY) + field(2, 1) + field(3, 0))
        elif channel == MEDIA_AUDIO_CHANNEL and message_id == START_INDICATION:
            self.audio_session = dict(parse(body))[1]
        elif channel == MEDIA_AUDIO_CHANNEL and message_id == STOP_INDICATION:
            self.audio_session = None
        elif channel == MEDIA_AUDIO_CHANNEL and message_id == MEDIA_WITH_TIMESTAMP:
            self.audio.append((struct.unpack(">Q", body[:8])[0], body[8:]))
            self.ack(channel, self.audio_session)
        elif channel == MICROPHONE_CHANNEL and message_id == MICROPHONE_REQUEST:
            self.microphone_open = bool(dict(parse(body)).get(1))
            self.send(channel, MICROPHONE_RESPONSE, field(1, 0) + field(2, 0))
        return channel, message_id, body

    def show(self, annexb):
        """in headed mode, put the video on screen as a car would"""
        if self.viewer:
            self.viewer.feed(annexb)

    def ack(self, channel=VIDEO_CHANNEL, session=0):
        self.send(channel, MEDIA_ACK, field(1, session) + field(2, 1))

    def speak(self, pcm):
        """one packet of microphone audio, as the car records it"""
        self.send(MICROPHONE_CHANNEL, MEDIA_WITH_TIMESTAMP, struct.pack(">Q", time.monotonic_ns() // 1000) + pcm)

    def sense(self, sensor, *values):
        """one reading from a sensor: its fields, in order, None for one left out"""
        reading = b"".join(field(number, value) for number, value in enumerate(values, 1) if value is not None)
        self.send(SENSOR_CHANNEL, SENSOR_EVENT, field(sensor, reading))

    def set_video_focus(self, projected):
        """the driver switches to the car's own screen, or back to the phone"""
        mode = FOCUS_PROJECTED if projected else FOCUS_NATIVE
        self.send(VIDEO_CHANNEL, VIDEO_FOCUS_INDICATION, field(1, mode) + field(2, 1))

    def take_audio_focus(self):
        """the car's own sound starts, as for a call: an unprompted focus loss"""
        self.send(0, AUDIO_FOCUS_RESPONSE, field(1, FOCUS_STATE_LOSS))

    def run_until(self, done, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while not done():
            assert time.monotonic() < deadline, "timed out waiting on the server"
            self.step()

    def received(self, channel, message_id):
        return [body for c, _, i, body in self.log if (c, i) == (channel, message_id)]

    def touch(self, action, points, action_index=0):
        """points are (pointer id, x, y) for every finger currently down"""
        locations = b"".join(field(1, field(1, x) + field(2, y) + field(3, pointer)) for pointer, x, y in points)
        event = locations + field(2, action_index) + field(3, action)
        self.send(INPUT_CHANNEL, INPUT_EVENT, field(1, time.monotonic_ns() // 1000) + field(3, event))

    def button(self, keycode, pressed):
        """keycode is an android key code, as advertised in service discovery"""
        event = field(1, field(1, keycode) + field(2, int(pressed)))
        self.send(INPUT_CHANNEL, INPUT_EVENT, field(1, time.monotonic_ns() // 1000) + field(4, event))
