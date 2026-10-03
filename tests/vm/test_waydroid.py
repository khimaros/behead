"""android on the headunit: the waydroid session runs android's full ui in
the kiosk, and the car's touch and buttons reach it through --uinput.
android reports which app is in front, so the tests see what it did with
the input. osmand~ and the f-droid client come from f-droid, fetched by the
vm provisioning and installed by the session.

`waydroid app` finds the session over the session's own bus, so the tests
name the session's runtime directory and point at the bus in it."""

import os
import pathlib
import re
import subprocess

import desktop
from desktop import fakehu

OSMAND, FDROID = "net.osmand.plus", "org.fdroid.fdroid"
# where tools/fetch-fdroid.py leaves both, in the vm's provisioning
APKS = pathlib.Path.home() / ".cache/behead/apk/fdroid"
# osmand~'s welcome screen at 800x480: the three dot menu, and a spot on the
# sheet it opens, on the words "restore from osmand cloud". android's own
# touch overlays would be app independent, but waydroid does not draw them
MENU = (773, 60)
SHEET = (150, 275)
# the picture around SHEET changes by far more than encoder noise when the
# sheet covers it
PATCH = 12
PATCH_CHANGE = 2000
LOCATION_PERMISSIONS = ("android.permission.ACCESS_FINE_LOCATION", "android.permission.ACCESS_COARSE_LOCATION")
NOTIFICATIONS_PERMISSION = "android.permission.POST_NOTIFICATIONS"
SETTLE = 5
ANDROID_TIMEOUT = 60
# installing compiles the app, on a cpu that also renders everything
INSTALL_TIMEOUT = 600
RESUMED = re.compile(r"(?:topResumedActivity|mResumedActivity)[=:].*? u\d+ ([\w.]+)/")
# an app's first screen has far more colours than a blank one
MIN_COLOURS = 100
# a fix as the car sends it, degrees times 1e7, and as android's location
# service then lists it for its gps provider, with the accuracy in metres
HAMBURG = (535511000, 99937000)
GPS_IN_HAMBURG = "last location=Location[gps 53.551100,9.993700 hAcc=5.0"


class WaydroidTest(desktop.DesktopTest):
    @classmethod
    def video_cmd(cls):
        return (f"env BEHEAD_RUNTIME_DIR={cls.dir / 'runtime'} BEHEAD_APKS={APKS} "
                f"{desktop.SESSIONS / 'waydroid.sh'} {{width}} {{height}} {{fps}}")

    @classmethod
    def waydroid(cls, *args):
        environment = {**os.environ, "DBUS_SESSION_BUS_ADDRESS": f"unix:path={cls.dir / 'runtime/bus'}"}
        return subprocess.run(["waydroid", *args], capture_output=True, text=True, timeout=ANDROID_TIMEOUT,
                              env=environment).stdout

    @classmethod
    def android(cls, *command):
        """run a command inside android, as root"""
        return cls.waydroid("shell", "--", *command)

    @classmethod
    def in_front(cls):
        """the package of the app android shows, or None while it cannot say"""
        found = RESUMED.search(cls.android("dumpsys", "activity", "activities"))
        return found and found.group(1)

    @classmethod
    def started(cls):
        return cls.android("getprop", "sys.boot_completed").strip() == "1"

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        # a car screen should not lock
        cls.android("locksettings", "set-disabled", "true")
        cls.android("input", "keyevent", "KEYCODE_WAKEUP")
        cls.android("input", "keyevent", "KEYCODE_MENU")
        # the session installs what is in BEHEAD_APKS by itself
        for package in (OSMAND, FDROID):
            cls.wait_for(lambda: f"packageName: {package}\n" in cls.waydroid("app", "list"),
                         f"installed {package}", INSTALL_TIMEOUT)
        # android keeps app data between runs. start osmand~ from its welcome
        # screen each time, without the dialog asking for these
        cls.android("pm", "clear", OSMAND)
        for permission in LOCATION_PERMISSIONS:
            cls.android("pm", "grant", OSMAND, permission)
        # f-droid would open on a dialog asking for this one
        cls.android("pm", "grant", FDROID, NOTIFICATIONS_PERMISSION)
        cls.android("input", "keyevent", "KEYCODE_HOME")
        cls.wait_for(lambda: cls.in_front() not in (None, OSMAND), "showed its home screen")
        cls.launcher = cls.in_front()

    def setUp(self):
        self.waydroid("app", "launch", OSMAND)
        self.wait_for(lambda: self.in_front() == OSMAND, f"opened {OSMAND}, still showing {self.in_front()}")
        # android says the app is in front before its window, one per app,
        # is on screen and focused. input sent before then goes nowhere
        self.pump_for(SETTLE)

    def went_home(self):
        self.wait_for(lambda: self.in_front() == self.launcher, f"went home, still showing {self.in_front()}")

    def test_runs_the_fdroid_client(self):
        """f-droid, to install further apps from the car's screen"""
        self.waydroid("app", "launch", FDROID)
        self.wait_for(lambda: self.in_front() == FDROID, f"opened {FDROID}, still showing {self.in_front()}")

    def test_apps_come_with_android(self):
        """tools/preinstall-apks.py put both in android's system image, so
        a first start has nothing to install, which takes minutes on a pi"""
        for package in (OSMAND, FDROID):
            self.assertIn(f"package:/system/app/{package}/", self.android("pm", "path", package))

    def test_runs_osmand(self):
        frame = self.last_frame()
        colours = {frame[offset:offset + 3] for offset in range(0, len(frame), 3)}
        self.assertGreater(len(colours), MIN_COLOURS, "the screen looks blank")

    def patch(self):
        """the pixels around SHEET in the latest picture"""
        frame, (x, y) = self.last_frame(), SHEET
        return [frame[(row * desktop.WIDTH + column) * 3 + channel]
                for row in range(y - PATCH, y + PATCH) for column in range(x - PATCH, x + PATCH)
                for channel in range(3)]

    def test_a_tap_opens_the_apps_menu(self):
        """the menu button is small and in a corner, so its sheet only opens
        if the tap arrives, and at the right place"""
        before = self.patch()
        self.tap(*MENU)
        self.addCleanup(self.android, "input", "keyevent", "KEYCODE_BACK")
        self.wait_for(lambda: sum(abs(a - b) for a, b in zip(before, self.patch())) > PATCH_CHANGE,
                      "opened the menu's sheet")

    def test_home_button_returns_to_the_launcher(self):
        self.press(fakehu.HOME)
        self.went_home()

    def test_learns_where_the_car_is(self):
        """the car's fix becomes android's gps location, which is what
        osmand~ navigates by"""
        self.headunit.sense(fakehu.SENSOR_LOCATION, None, *HAMBURG, 5000, 3450, 12500, 90500000)
        self.wait_for(lambda: GPS_IN_HAMBURG in self.android("dumpsys", "location"), "learned where the car is")

    def test_follows_the_cars_night_mode(self):
        for night, mode in ((1, "Night mode: yes"), (0, "Night mode: no")):
            self.headunit.sense(fakehu.SENSOR_NIGHT, night)
            self.wait_for(lambda: mode in self.android("cmd", "uimode", "night"), f"switched to {mode}")


if __name__ == "__main__":
    import unittest
    unittest.main()
