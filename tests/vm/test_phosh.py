"""phosh on the headunit: the phosh session runs the mobile shell in a
headless phoc, and the car's touch reaches it through --uinput. the shell
lists `behead-demo --wayland`, so a tap on its icon has to launch it, and a
touch inside it has to come back as a marker in the video."""

import os
import re
import subprocess

import desktop
from desktop import fakehu, tcp

DESKTOP_ENTRY = f"""[Desktop Entry]
Type=Application
Name=Behead Demo
Exec={tcp.DEMO} --wayland
Icon=applications-system
X-Purism-FormFactor=Workstation;Mobile;
"""
# where phosh puts the only app in its overview, at 800x480: the centre to
# tap, and the box around the icon, which shows light on phosh's dark
# background. the gear's own centre is a dark hole
APP_ICON = (57, 145)
APP_ICON_BOX = (25, 112, 90, 180)
LIGHT, ICON_PIXELS = 200, 300
# the same icon once an application runs, when its thumbnail takes the top
RUNNING_ICON_BOX = (25, 295, 90, 363)
# the top edge of the ring phosh draws around the icon that has focus, and
# how much bluer than red it is, where the rest of the overview is grey. the
# ring is two pixels thick
FOCUS_RING = (20, 108, 95, 118)
RING_BLUE, RING_PIXELS = 50, 100
# more turns than the overview has places for focus to rest
MAX_TURNS = 8
# how long a move of focus takes to reach the car's picture
SETTLE = 1
# the demo's background and its marker for the first finger
DEMO_BACKGROUND, DEMO_MARKER = (24, 28, 36), (255, 64, 64)
TOUCH = (600, 300)
COLOUR_TOLERANCE = 24
# how long the application's tone lasts, more than the car needs to hear it
TONE_SECONDS = 60
# where sessions/geoclue.conf, installed by the vm's provisioning, has
# geoclue look for the car's location
NMEA_SOCKET = "/run/behead/nmea.sock"
# geoclue's own example application, asking for the most exact location
WHERE_AM_I, EXACT = "/usr/libexec/geoclue-2.0/demos/where-am-i", "8"
# what it prints for the fix the test sends, in degrees, metres and m/s
LOCATION = {"Latitude": 52.52, "Longitude": 13.405, "Altitude": 34.5, "Speed": 12.501, "Heading": 90.5}


def near(colour, expected):
    return colour is not None and all(abs(a - b) <= COLOUR_TOLERANCE for a, b in zip(colour, expected))


def demo_running():
    return subprocess.run(["pgrep", "-f", f"{tcp.DEMO} --wayland"], capture_output=True).returncode == 0


def is_light(pixel):
    return min(pixel) > LIGHT


def is_ring(pixel):
    red, _, blue = pixel
    return blue - red > RING_BLUE


class PhoshTest(desktop.DesktopTest):
    KEYCODES = (fakehu.HOME, fakehu.BACK, fakehu.DPAD_CENTER, fakehu.ROTARY)

    @classmethod
    def count(cls, box, wanted):
        """how many pixels of a box in the latest picture pass a check"""
        frame, (left, top, right, bottom) = cls.last_frame(), box
        pixels = (frame[(y * desktop.WIDTH + x) * 3:(y * desktop.WIDTH + x) * 3 + 3]
                  for y in range(top, bottom) for x in range(left, right))
        return sum(1 for pixel in pixels if wanted(pixel)) if frame else 0

    @classmethod
    def prepare(cls):
        applications = cls.dir / "home/.local/share/applications"
        applications.mkdir(parents=True)
        (applications / "behead-demo.desktop").write_text(DESKTOP_ENTRY)

    @classmethod
    def video_cmd(cls):
        return (f"env BEHEAD_HOME={cls.dir / 'home'} BEHEAD_RUNTIME_DIR={cls.dir / 'runtime'} "
                f"{desktop.SESSIONS / 'phosh.sh'} {{width}} {{height}} {{fps}}")

    @classmethod
    def style(cls):
        """the dark or light style the session's settings ask applications for"""
        settings = {**os.environ, "GSETTINGS_BACKEND": "keyfile", "XDG_CONFIG_HOME": str(cls.dir / "runtime/config")}
        return subprocess.run(["gsettings", "get", "org.gnome.desktop.interface", "color-scheme"],
                              capture_output=True, text=True, env=settings).stdout.strip()

    @classmethod
    def server_options(cls):
        return ["--nmea-socket", NMEA_SOCKET, "--audio-cmd", desktop.AUDIO_CMD]

    def test_an_applications_sound_reaches_the_car(self):
        """an application plays a tone to the session's sound server, as a
        pulseaudio client, and the car gets it as media audio"""
        session = {**os.environ, "XDG_RUNTIME_DIR": str(self.dir / "runtime")}
        tone = f"sine=frequency={tcp.TONE}:sample_rate={fakehu.MEDIA_RATE}"
        playing = subprocess.Popen(["ffmpeg", "-loglevel", "error", "-re", "-f", "lavfi", "-i", tone,
                                    "-t", str(TONE_SECONDS), "-f", "pulse", "behead test"], env=session)
        self.addCleanup(playing.wait)
        self.addCleanup(playing.kill)
        self.wait_for_tone(len(self.headunit.audio))

    def test_applications_learn_the_cars_location(self):
        """geoclue reads the car's fixes from the server's socket, and
        phosh, as its agent, lets an application have them"""
        # berlin, moving east. a car reports its position about once a second
        fix = (None, 525200066, 134049540, 5000, 3450, 12500, 90500000)
        output = self.dir / "where-am-i.log"
        with output.open("wb") as log:
            asking = subprocess.Popen([WHERE_AM_I, "-a", EXACT, "-t", str(desktop.REACT)], stdout=log, stderr=log)
        self.addCleanup(asking.wait)
        self.addCleanup(asking.kill)

        def answered():
            self.headunit.sense(fakehu.SENSOR_LOCATION, *fix)
            return "Heading" in output.read_text()
        self.wait_for(answered, "told an application where the car is")
        answer = dict(re.findall(r"(\w+): +(-?[\d.]+)", output.read_text()))
        self.assertEqual({name: round(float(answer[name]), 3) for name in LOCATION}, LOCATION)

    def test_follows_the_cars_night_mode(self):
        for night, style in ((1, "'prefer-dark'"), (0, "'default'")):
            self.headunit.sense(fakehu.SENSOR_NIGHT, night)
            self.wait_for(lambda: self.style() == style, f"asked for {style}, still {self.style()}")

    @classmethod
    def started(cls):
        return cls.count(APP_ICON_BOX, is_light) >= ICON_PIXELS

    def showing_the_app(self):
        return near(self.pixel(desktop.WIDTH // 2, desktop.HEIGHT // 2), DEMO_BACKGROUND)

    def test_the_knob_alone_opens_an_app_and_home_leaves_it(self):
        """a car without a touchscreen: turns walk the overview's icons, a
        click opens the one in focus, and home goes to the overview and back"""
        self.addCleanup(subprocess.run, ["pkill", "-f", f"{tcp.DEMO} --wayland"])
        for _ in range(MAX_TURNS):
            if self.count(FOCUS_RING, is_ring) >= RING_PIXELS:
                break
            self.headunit.turn(fakehu.ROTARY, 1)
            self.pump_for(SETTLE)
        else:
            self.fail("turning the knob never put the app's icon in focus")
        self.press(fakehu.DPAD_CENTER)
        self.wait_for(demo_running, "launched the app")
        self.wait_for(self.showing_the_app, "showed the app")
        self.press(fakehu.HOME)
        self.wait_for(lambda: self.count(RUNNING_ICON_BOX, is_light) >= ICON_PIXELS, "showed its overview")
        self.press(fakehu.HOME)
        self.wait_for(self.showing_the_app, "went back to the app")

    def test_a_tapped_app_opens_and_takes_touch(self):
        self.addCleanup(subprocess.run, ["pkill", "-f", f"{tcp.DEMO} --wayland"])
        self.tap(*APP_ICON)
        self.wait_for(demo_running, "launched the app")
        self.wait_for(self.showing_the_app, "showed the app")
        self.headunit.touch(fakehu.TOUCH_DOWN, [(0, *TOUCH)])
        self.addCleanup(self.headunit.touch, fakehu.TOUCH_UP, [(0, *TOUCH)])
        self.wait_for(lambda: near(self.pixel(*TOUCH), DEMO_MARKER), "drew a marker under the finger")


if __name__ == "__main__":
    import unittest
    unittest.main()
