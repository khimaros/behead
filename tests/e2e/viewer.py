"""headed mode for the tests: with BEHEAD_HEADED set, everything the scripted
headunit receives plays live in a browser. ffmpeg decodes the h.264 to jpeg
pictures, and the page reloads /frame.jpg each time the last one arrives.
media audio goes out at /audio as the pcm it came in, for as long as the
page listens, which it does from the first click or key.
BEHEAD_HEADED is the port to serve on, or 1 for the default."""

import atexit
import http.server
import json
import os
import queue
import re
import socket
import subprocess
import threading
import time
import urllib.parse

DEFAULT_PORT = 8090
LINGER_SECONDS = 10
JPEG_END = b"\xff\xd9"
# an access unit delimiter. ffmpeg's parser only knows a picture is complete
# when the next one starts, and a still screen sends no next one
PICTURE_END = b"\x00\x00\x00\x01\x09\xf0"
# a picture that needs none before it: one with a slice of an idr picture
# (nal type 5) or the parameter sets x264 puts in front of one (7)
KEYFRAME = re.compile(rb"\x00\x00\x01[\x05\x07\x25\x27\x45\x47\x65\x67]")
# pictures kept for a decoder that is behind, a second of them
VIDEO_BACKLOG = 30
# the page waits this long after one picture before it asks for the next:
# with the time a picture takes to come, under the 33 ms between a car's
RELOAD_MS = 15
# the keyboard as a car's buttons, by android key code: the arrows tilt a
# rotary knob, enter clicks it, and the mouse wheel turns it
BUTTONS = {"ArrowUp": 19, "ArrowDown": 20, "ArrowLeft": 21, "ArrowRight": 22, "Enter": 23, "Escape": 4, "Home": 3}
# and as a keyboard, which a headunit may have too. each key by its place,
# as the browser's `code` names it, and shift as a key of its own: what is
# typed is then the phone's to decide, as with a keyboard plugged into it.
# control and alt stay the browser's, for its own shortcuts
A, ZERO = 29, 7
TYPING = {
    **{f"Key{letter}": A + place for place, letter in enumerate("ABCDEFGHIJKLMNOPQRSTUVWXYZ")},
    **{f"Digit{digit}": ZERO + digit for digit in range(10)},
    "Comma": 55, "Period": 56, "ShiftLeft": 59, "ShiftRight": 60, "Tab": 61, "Space": 62, "Backspace": 67,
    "Backquote": 68, "Minus": 69, "Equal": 70, "BracketLeft": 71, "BracketRight": 72, "Backslash": 73,
    "Semicolon": 74, "Quote": 75, "Slash": 76, "Delete": 112,
}
KEYS = {**BUTTONS, **TYPING}
# keys that turn the knob a detent, as the wheel does, for a keyboard alone.
# down is the wheel rolled toward the hand, which is clockwise
TURNS = {"PageUp": -1, "PageDown": 1}
ROTARY = 65536
# what a headunit has to advertise for the page's controls to reach the phone
KEYCODES = sorted({*KEYS.values(), ROTARY})
HELP = ("mouse: finger. wheel, or page up and down: knob. arrows: tilt. enter: click, held for a second: the knob's next use. "
        "escape: back. home: home. the rest of the keyboard types. sound: after a click or a key")
# degrees as the car's location sensor sends them
DEGREES_E7 = 10 ** 7
# what media audio is until a headunit says: its rate and channels
AUDIO_FORMAT = (48000, 2)
SAMPLE_BYTES = 2
# pieces of sound kept for a page that reads slowly. more than that are
# dropped, so a slow browser never holds up a test
AUDIO_BACKLOG = 50
# seconds the page starts a piece of sound ahead of now, which evens out
# how pieces arrive, and how far ahead it may get before it drops one
AUDIO_LEAD, AUDIO_LIMIT = 0.1, 1
# keeps a browser from marking the picture as selected text when the mouse
# drags across it, and from scrolling or zooming under a real finger
UNSELECTABLE = "user-select:none;-webkit-user-select:none;touch-action:none"
PAGE = f"""<!doctype html><title>behead headunit</title>
<body style="margin:0;background:#000;display:flex;justify-content:center;align-items:center;height:100vh;{UNSELECTABLE}">
<p id="waiting" style="color:#888;font:16px sans-serif">waiting for video</p>
<p style="position:fixed;bottom:0;margin:4px;color:#666;font:12px sans-serif">{HELP}
<input id="place" placeholder="where the car is: 37.7749,-122.4194, then enter" size="40"
  style="background:#111;color:#888;border:1px solid #333;font:12px sans-serif"></p>
<img id="picture" draggable="false" style="max-width:100%;max-height:100%;display:none;{UNSELECTABLE}">
<script>
// looked up by id: a bare `screen` would be the browser's own window.screen
const picture = document.getElementById("picture"), waiting = document.getElementById("waiting");
const load = () => picture.src = "/frame.jpg?" + Date.now();
const next = () => setTimeout(load, {RELOAD_MS});
picture.onload = () => {{ waiting.style.display = "none"; picture.style.display = ""; next(); }};
picture.onerror = next;
load();
// the mouse is a finger: pressed is down, dragging moves, released is up
let down = false;
const touch = (action, event) => {{
  const x = Math.round(event.offsetX * picture.naturalWidth / picture.clientWidth);
  const y = Math.round(event.offsetY * picture.naturalHeight / picture.clientHeight);
  fetch(`/touch?action=${{action}}&x=${{x}}&y=${{y}}`);
}};
picture.onpointerdown = (event) => {{ down = true; picture.setPointerCapture(event.pointerId); touch("down", event); }};
picture.onpointermove = (event) => down && touch("move", event);
picture.onpointerup = (event) => {{ down = false; touch("up", event); }};
// a drag is a finger moving, and neither marks the picture nor picks it up
picture.onmousedown = picture.ondragstart = (event) => event.preventDefault();
// the keyboard is the car's buttons, and a keyboard. a held key repeats
// here, a held button does not: the phone repeats what is held down itself
const keys = {json.dumps(KEYS)};
// the one place on the page that is typed into itself: where the car is,
// which a car's sensor says and this page has to be told
const place = document.getElementById("place");
place.onchange = () => {{
  const [latitude, longitude] = place.value.split(",").map(Number);
  if (isFinite(latitude) && isFinite(longitude)) fetch(`/location?latitude=${{latitude}}&longitude=${{longitude}}`);
  place.blur();
}};
const button = (event, pressed) => {{
  const code = keys[event.code];
  if (code === undefined || event.repeat || event.target === place) return;
  event.preventDefault();
  fetch(`/button?code=${{code}}&pressed=${{pressed}}`);
}};
// one notch of the wheel is one detent of the knob, and so is a press of
// page up or down, or each time one held repeats
const turns = {json.dumps(TURNS)};
const turn = (delta) => fetch(`/turn?delta=${{delta}}`);
onkeydown = (event) => {{
  const delta = turns[event.code];
  if (delta === undefined || event.target === place) return button(event, 1);
  event.preventDefault();
  turn(delta);
}};
onkeyup = (event) => button(event, 0);
onwheel = (event) => turn(Math.sign(event.deltaY));
// sound: /audio is pcm without end, played a piece at a time, each where the
// last one stops. a browser plays nothing before a click or a key
let sound;
const play = (bytes, frames, rate, channels, due) => {{
  const samples = new Int16Array(bytes.buffer, 0, frames * channels);
  const piece = sound.createBuffer(channels, frames, rate);
  for (let channel = 0; channel < channels; channel++) {{
    const data = piece.getChannelData(channel);
    for (let frame = 0; frame < frames; frame++) data[frame] = samples[frame * channels + channel] / 32768;
  }}
  const source = sound.createBufferSource();
  source.buffer = piece;
  source.connect(sound.destination);
  source.start(due);
}};
const listen = async () => {{
  if (sound) return;
  sound = new AudioContext();
  const response = await fetch("/audio");
  const rate = +response.headers.get("X-Rate"), channels = +response.headers.get("X-Channels");
  const reader = response.body.getReader(), frame = {SAMPLE_BYTES} * channels;
  let due = 0, held = new Uint8Array(0);
  for (;;) {{
    const {{ value, done }} = await reader.read();
    if (done) break;
    const bytes = new Uint8Array(held.length + value.length);
    bytes.set(held);
    bytes.set(value, held.length);
    const frames = Math.floor(bytes.length / frame);
    held = bytes.slice(frames * frame);
    // a page that fell behind skips sound to catch up
    if (!frames || due > sound.currentTime + {AUDIO_LIMIT}) continue;
    due = Math.max(due, sound.currentTime + {AUDIO_LEAD});
    play(bytes, frames, rate, channels, due);
    due += frames / rate;
  }}
}};
addEventListener("pointerdown", listen);
addEventListener("keydown", listen);
</script></body>""".encode()
READ_CHUNK = 65536


def outward_address():
    """the address other machines reach this one on. connecting a udp socket
    picks the route without sending anything"""
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        try:
            probe.connect(("192.0.2.1", 9))
            return probe.getsockname()[0]
        except OSError:
            return "127.0.0.1"


class Viewer:
    """one decoder and web server for the whole test run. every scripted
    headunit feeds it, and a new stream's parameter sets reset the decoder"""

    def __init__(self, port):
        self.decoder = subprocess.Popen(
            # one thread each way and every picture written at once: a decoder
            # with a thread per core holds back a picture per thread, and
            # ffmpeg writes a pipe in blocks of several pictures
            ["ffmpeg", "-loglevel", "error", "-fflags", "nobuffer", "-flags", "low_delay", "-probesize", "4096",
             "-threads", "1", "-f", "h264", "-i", "-", "-c:v", "mjpeg", "-threads", "1", "-q:v", "5",
             "-flush_packets", "1", "-f", "image2pipe", "-"],
            # it complains of a broken pipe when the tests exit before it
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        # the pictures the decoder has yet to be given, and whether one was
        # lost since the last that stands alone
        self.pictures, self.waiting = queue.Queue(VIDEO_BACKLOG), False
        threading.Thread(target=self.decode, daemon=True).start()
        self.latest = b""
        # (action, x, y) for each mouse event on the picture, for whoever
        # wants to pass them on as touches. the tests ignore them
        self.touches = queue.SimpleQueue()
        # ("button", key code, pressed) and ("turn", key code, detents), likewise
        self.controls = queue.SimpleQueue()
        # the pages listening to the sound, each with the pieces it has yet to get
        self.audio_format, self.listeners = AUDIO_FORMAT, []
        threading.Thread(target=self.collect, daemon=True).start()
        server = http.server.ThreadingHTTPServer(("", port), self.handler())
        threading.Thread(target=server.serve_forever, daemon=True).start()
        print(f"\nheaded: watch the headunit at http://{outward_address()}:{port}/\n", flush=True)
        atexit.register(self.linger)

    def linger(self):
        """the tests may end faster than anyone can look"""
        print(f"\nheaded: showing the last picture for {LINGER_SECONDS} more seconds", flush=True)
        time.sleep(LINGER_SECONDS)

    def feed(self, annexb):
        """pass on a picture of h.264. a decoder that falls behind loses
        whole pictures, so it never holds up a test, and then every one up to
        the next that stands alone: a picture built on one it never saw is
        drawn wrong"""
        if self.waiting and not KEYFRAME.search(annexb):
            return
        try:
            self.pictures.put_nowait(annexb)
            self.waiting = False
        except queue.Full:
            self.waiting = True

    def decode(self):
        """hand the decoder each picture whole, however long it takes one"""
        try:
            while True:
                self.decoder.stdin.write(self.pictures.get() + PICTURE_END)
                # now, not once python has gathered a block of them: the
                # pictures of a still screen are a few hundred bytes each
                self.decoder.stdin.flush()
        except (BrokenPipeError, ValueError):
            pass

    def play(self, rate, channels, pcm):
        """pass on media audio, signed 16 bit little endian, to every page listening"""
        self.audio_format = (rate, channels)
        for listener in list(self.listeners):
            try:
                listener.put_nowait(pcm)
            except queue.Full:
                pass

    def collect(self):
        buffer = b""
        while chunk := self.decoder.stdout.read1(READ_CHUNK):
            buffer += chunk
            while (end := buffer.find(JPEG_END)) >= 0:
                self.latest, buffer = buffer[:end + len(JPEG_END)], buffer[end + len(JPEG_END):]

    def handler(self):
        viewer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def stream_audio(self):
                """the sound from now on, until the page goes away"""
                listener = queue.Queue(AUDIO_BACKLOG)
                viewer.listeners.append(listener)
                rate, channels = viewer.audio_format
                self.send_response(200)
                for name, value in (("Content-Type", "application/octet-stream"), ("Cache-Control", "no-store"),
                                    ("X-Rate", rate), ("X-Channels", channels)):
                    self.send_header(name, str(value))
                self.end_headers()
                try:
                    while True:
                        self.wfile.write(listener.get())
                        self.wfile.flush()
                except OSError:
                    viewer.listeners.remove(listener)

            def do_GET(self):
                url = urllib.parse.urlparse(self.path)
                query = {name: values[0] for name, values in urllib.parse.parse_qs(url.query).items()}
                if url.path == "/audio":
                    return self.stream_audio()
                if url.path == "/touch":
                    viewer.touches.put((query["action"], int(query["x"]), int(query["y"])))
                elif url.path == "/button":
                    viewer.controls.put(("button", int(query["code"]), query["pressed"] == "1"))
                elif url.path == "/turn":
                    viewer.controls.put(("turn", ROTARY, int(query["delta"])))
                elif url.path == "/location":
                    place = (round(float(query[name]) * DEGREES_E7) for name in ("latitude", "longitude"))
                    viewer.controls.put(("location", *place))
                frame = url.path == "/frame.jpg"
                body = viewer.latest if frame else PAGE if url.path == "/" else b""
                self.send_response(200 if body else 204)
                self.send_header("Content-Type", "image/jpeg" if frame else "text/html")
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(body)

        return Handler


_viewer = None


def shared():
    """the viewer, started on first use, or None when running headless"""
    global _viewer
    setting = os.environ.get("BEHEAD_HEADED", "")
    if setting and _viewer is None:
        _viewer = Viewer(DEFAULT_PORT if setting == "1" else int(setting))
    return _viewer
