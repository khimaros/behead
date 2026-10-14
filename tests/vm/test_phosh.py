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
# longer than the server takes a held click for a long one
LONG_CLICK = 0.8
# where the shell shows a note, under its top bar, and how much of that the
# note changes. a text cursor that blinks there is far less
NOTE_BOX = (250, 30, 550, 90)
NOTE_SHARE = 0.15
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
# what gsettings prints for the style applications are asked for
DARK_STYLE, LIGHT_STYLE = "'prefer-dark'", "'default'"
# the name an on-screen keyboard takes on the session's bus for phosh to find
OSK_NAME = "sm.puri.OSK0"
# the overview's search field, which brings the keyboard up when tapped, and
# the middle of the keyboard's space bar. how bright that is, of 255: the
# bare screen is nearly black, a dark key a dim grey, a light key near white
SEARCH_FIELD = (260, 72)
SPACE_BAR = (300, 435, 500, 465)
BARE_SCREEN, LIGHT_KEY = 30, 130


def near(colour, expected):
    return colour is not None and all(abs(a - b) <= COLOUR_TOLERANCE for a, b in zip(colour, expected))


def demo_running():
    return subprocess.run(["pgrep", "-f", f"{tcp.DEMO} --wayland"], capture_output=True).returncode == 0


def is_light(pixel):
    return min(pixel) > LIGHT


def is_ring(pixel):
    red, _, blue = pixel
    return blue - red > RING_BLUE


class PhoshSession(desktop.DesktopTest):
    """the phosh session behind a car, and how to look at it"""
    KEYCODES = (fakehu.HOME, fakehu.BACK, fakehu.DPAD_CENTER, fakehu.ROTARY)
    # BEHEAD_COLOR_SCHEME for the session, or None for what it does unasked
    SCHEME = None

    @classmethod
    def count(cls, box, wanted):
        """how many pixels of a box in the latest picture pass a check"""
        frame, (left, top, right, bottom) = cls.last_frame(), box
        pixels = (frame[(y * desktop.WIDTH + x) * 3:(y * desktop.WIDTH + x) * 3 + 3]
                  for y in range(top, bottom) for x in range(left, right))
        return sum(1 for pixel in pixels if wanted(pixel)) if frame else 0

    @classmethod
    def box(cls, box):
        """the pixels of a box in the latest picture"""
        frame, (left, top, right, bottom) = cls.last_frame(), box
        return [bytes(frame[(y * desktop.WIDTH + x) * 3:(y * desktop.WIDTH + x) * 3 + 3])
                for y in range(top, bottom) for x in range(left, right)]

    @classmethod
    def prepare(cls):
        applications = cls.dir / "home/.local/share/applications"
        applications.mkdir(parents=True)
        (applications / "behead-demo.desktop").write_text(DESKTOP_ENTRY)

    @classmethod
    def settings(cls):
        """what the session is told through its environment"""
        scheme = {"BEHEAD_COLOR_SCHEME": cls.SCHEME} if cls.SCHEME else {}
        return {**scheme, "BEHEAD_HOME": cls.dir / "home", "BEHEAD_RUNTIME_DIR": cls.dir / "runtime"}

    @classmethod
    def video_cmd(cls):
        settings = " ".join(f"{name}={value}" for name, value in cls.settings().items())
        return f"env {settings} {desktop.SESSIONS / 'phosh.sh'} {{width}} {{height}} {{fps}}"

    @classmethod
    def style(cls):
        """the dark or light style the session's settings ask applications for"""
        settings = {**os.environ, "GSETTINGS_BACKEND": "keyfile", "XDG_CONFIG_HOME": str(cls.dir / "runtime/config")}
        return subprocess.run(["gsettings", "get", "org.gnome.desktop.interface", "color-scheme"],
                              capture_output=True, text=True, env=settings).stdout.strip()

    @classmethod
    def server_options(cls):
        return ["--nmea-socket", NMEA_SOCKET, "--audio-cmd", desktop.AUDIO_CMD]

    @classmethod
    def started(cls):
        return cls.count(APP_ICON_BOX, is_light) >= ICON_PIXELS

    def tell(self, night):
        self.headunit.sense(fakehu.SENSOR_NIGHT, night)


class PhoshTest(PhoshSession):
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

    def test_has_a_keyboard_on_screen_to_type_with(self):
        """a car has no keyboard. phosh shows the one that answers to this
        name on the session's bus, when it is told to show one at all"""
        session = {**os.environ, "DBUS_SESSION_BUS_ADDRESS": f"unix:path={self.dir / 'runtime/bus'}"}

        def keyboard_running():
            asked = subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus", "--object-path",
                                    "/org/freedesktop/DBus", "--method", "org.freedesktop.DBus.NameHasOwner",
                                    OSK_NAME], capture_output=True, text=True, env=session)
            return "true" in asked.stdout
        self.wait_for(keyboard_running, f"started an on-screen keyboard ({OSK_NAME})")
        settings = {**os.environ, "GSETTINGS_BACKEND": "keyfile", "XDG_CONFIG_HOME": str(self.dir / "runtime/config")}
        enabled = subprocess.run(["gsettings", "get", "org.gnome.desktop.a11y.applications", "screen-keyboard-enabled"],
                                 capture_output=True, text=True, env=settings).stdout.strip()
        self.assertEqual(enabled, "true", "the shell is not told to show it")

    def test_the_keyboard_on_screen_is_dark_like_the_rest(self):
        """it takes its look from the gtk theme's name, not from the style
        applications are asked for, so the session has to say both"""
        session = {**os.environ, "DBUS_SESSION_BUS_ADDRESS": f"unix:path={self.dir / 'runtime/bus'}"}
        hide = ["gdbus", "call", "--session", "--dest", OSK_NAME, "--object-path", "/sm/puri/OSK0", "--method",
                f"{OSK_NAME}.SetVisible", "false"]
        self.addCleanup(subprocess.run, hide, capture_output=True, env=session)
        self.tap(*SEARCH_FIELD)

        def space_bar():
            frame, (left, top, right, bottom) = self.last_frame(), SPACE_BAR
            levels = [frame[(y * desktop.WIDTH + x) * 3 + 1] for y in range(top, bottom) for x in range(left, right)]
            return sum(levels) / len(levels)
        self.wait_for(lambda: space_bar() > BARE_SCREEN, "showed its keyboard for a text field")
        self.assertLess(space_bar(), LIGHT_KEY, "the keyboard is in its light style on a dark screen")

    def test_is_dark_whatever_the_car_says(self):
        self.assertEqual(self.style(), DARK_STYLE)
        self.tell(night=0)
        self.pump_for(SETTLE)
        self.assertEqual(self.style(), DARK_STYLE, "a car in daylight turned the lights on, unasked")

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


class PhoshFollowingTheCarTest(PhoshSession):
    SCHEME = "car"

    def test_is_dark_until_the_car_says_day_and_follows_it_from_then(self):
        self.assertEqual(self.style(), DARK_STYLE, "a car that has said nothing yet gets the light style")
        for night, style in ((0, LIGHT_STYLE), (1, DARK_STYLE)):
            self.tell(night)
            self.wait_for(lambda: self.style() == style, f"asked for {style}, still {self.style()}")


class PhoshKnobTest(PhoshSession):
    """a car with a knob, its click and a back button, and no touchscreen"""

    @classmethod
    def server_options(cls):
        return [*super().server_options(), "--knob", "focus,type", "--back", "escape"]

    def long_click(self):
        self.headunit.button(fakehu.DPAD_CENTER, True)
        self.pump_for(LONG_CLICK)
        self.headunit.button(fakehu.DPAD_CENTER, False)

    def test_the_knob_writes_into_the_search_and_says_so(self):
        """the first turn is on the overview's search. a long click makes
        the knob write, which the shell says in a note, and a detent back
        writes a digit no application has in its name, so none are left.
        back takes the digit away again"""
        self.headunit.turn(fakehu.ROTARY, 1)
        self.pump_for(SETTLE)
        before = self.box(NOTE_BOX)
        self.long_click()

        def noted():
            now = self.box(NOTE_BOX)
            return sum(old != new for old, new in zip(before, now)) / len(before) >= NOTE_SHARE
        self.wait_for(noted, "said what the knob does now")
        self.headunit.turn(fakehu.ROTARY, -1)
        self.wait_for(lambda: self.count(APP_ICON_BOX, is_light) < ICON_PIXELS, "left no application for a 0")
        self.press(fakehu.BACK)
        self.wait_for(lambda: self.count(APP_ICON_BOX, is_light) >= ICON_PIXELS, "showed them again with the 0 gone")


class LightPhoshTest(PhoshSession):
    SCHEME = "light"

    def test_is_light_whatever_the_car_says(self):
        self.assertEqual(self.style(), LIGHT_STYLE)
        self.tell(night=1)
        self.pump_for(SETTLE)
        self.assertEqual(self.style(), LIGHT_STYLE, "a car at night turned the lights off, unasked")


if __name__ == "__main__":
    import unittest
    unittest.main()
