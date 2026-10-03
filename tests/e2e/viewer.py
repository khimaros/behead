"""headed mode for the tests: with BEHEAD_HEADED set, everything the scripted
headunit receives plays live in a browser. ffmpeg decodes the h.264 to jpeg
pictures, and the page reloads /frame.jpg each time the last one arrives.
BEHEAD_HEADED is the port to serve on, or 1 for the default."""

import atexit
import http.server
import os
import queue
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
RELOAD_MS = 40
PAGE = f"""<!doctype html><title>behead headunit</title>
<body style="margin:0;background:#000;display:flex;justify-content:center;align-items:center;height:100vh">
<p id="waiting" style="color:#888;font:16px sans-serif">waiting for video</p>
<img id="picture" draggable="false" style="max-width:100%;max-height:100%;display:none">
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
            ["ffmpeg", "-loglevel", "error", "-fflags", "nobuffer", "-flags", "low_delay", "-probesize", "4096",
             "-f", "h264", "-i", "-", "-c:v", "mjpeg", "-q:v", "5", "-f", "image2pipe", "-"],
            # it complains of a broken pipe when the tests exit before it
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        os.set_blocking(self.decoder.stdin.fileno(), False)
        self.latest = b""
        # (action, x, y) for each mouse event on the picture, for whoever
        # wants to pass them on as touches. the tests ignore them
        self.touches = queue.SimpleQueue()
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
        """pass on h.264. what the decoder has no room for is dropped, so a
        slow browser never holds up a test"""
        try:
            self.decoder.stdin.write(annexb + PICTURE_END)
        except (BlockingIOError, BrokenPipeError):
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

            def do_GET(self):
                url = urllib.parse.urlparse(self.path)
                if url.path == "/touch":
                    query = urllib.parse.parse_qs(url.query)
                    viewer.touches.put((query["action"][0], int(query["x"][0]), int(query["y"][0])))
                frame = url.path == "/frame.jpg"
                body = viewer.latest if frame else b"" if url.path == "/touch" else PAGE
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
