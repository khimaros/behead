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


class PhoshTest(desktop.DesktopTest):
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
        frame = cls.last_frame()
        if not frame:
            return False
        left, top, right, bottom = APP_ICON_BOX
        pixels = (frame[(y * desktop.WIDTH + x) * 3:(y * desktop.WIDTH + x) * 3 + 3]
                  for y in range(top, bottom) for x in range(left, right))
        return sum(1 for pixel in pixels if min(pixel) > LIGHT) >= ICON_PIXELS

    def test_a_tapped_app_opens_and_takes_touch(self):
        self.addCleanup(subprocess.run, ["pkill", "-f", f"{tcp.DEMO} --wayland"])
        self.tap(*APP_ICON)
        self.wait_for(demo_running, "launched the app")
        self.wait_for(lambda: near(self.pixel(desktop.WIDTH // 2, desktop.HEIGHT // 2), DEMO_BACKGROUND),
                      "showed the app")
        self.headunit.touch(fakehu.TOUCH_DOWN, [(0, *TOUCH)])
        self.addCleanup(self.headunit.touch, fakehu.TOUCH_UP, [(0, *TOUCH)])
        self.wait_for(lambda: near(self.pixel(*TOUCH), DEMO_MARKER), "drew a marker under the finger")


if __name__ == "__main__":
    import unittest
    unittest.main()
