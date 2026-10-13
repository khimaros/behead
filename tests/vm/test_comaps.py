"""a map that also searches and routes without a network: comaps in the
phosh session, as the flatpak the pi image carries, opens on the map of
where the car is, from a region tools/fetch-comaps.py fetched beforehand.
the region is handed to the session as an image hands it, in BEHEAD_COMAPS,
and the session puts it where comaps keeps its own and answers the questions
comaps asks of a first start. needs the flatpak in the vm
(tests/vm/provision.sh) and, for the fetch, the internet."""

import os
import pathlib
import shlex
import subprocess
import unittest

import desktop
import test_phosh
from desktop import fakehu, tcp

COMAPS = "app.comaps.comaps"
FETCH = tcp.ROOT / "tools/fetch-comaps.py"
# where an image carries its flatpak applications, which the session runs from
IMAGE_FLATPAKS = "/usr/share/behead/flatpak"
os.environ["FLATPAK_USER_DIR"] = IMAGE_FLATPAKS
# knoxville, in the east of tennessee, as a car reports it: degrees times
# ten million, then accuracy, altitude, speed and bearing
REGION = "US_Tennessee_East"
CAR = (None, 359606000, -839207000, 5000, 3000, 0, 0)
# the lower part of the map, left of comaps' buttons. the shell's overview
# has nothing there before comaps is up
MAP_BOX = (100, 290, 700, 440)
# a light map's land is most of it, lighter than this in every channel. a
# dark map has next to none that light
LIGHT, LAND_SHARE = 150, 0.3
# the red button among comaps' own, at the right of its window, beside the
# white star in the middle of it. it is there once the window is, and not on
# what comaps asks at a first start, a page of text with as many colours as
# a map
STAR = (765, 297)
# and roads, woods, water and names make more colours than this. a map with
# no region for the place is its land's one colour
DRAWN_COLOURS = 1000
# comaps takes a while to start cold inside its sandbox, and to draw
SHOWN = 120
# for the map to come to rest once drawn, detents of the knob that change
# its scale for anyone to see, and how much of it is then other than it was
SETTLE = 3
ZOOM_DETENTS = 3
REDRAWN_SHARE = 0.2
# the button with a flag on it, where its list of three opens, left of it
# over the map, and how much of that is then the list
FLAG_BUTTON = (765, 345)
LIST_BOX = (600, 335, 730, 385)
LISTED_SHARE = 0.6
# a finger's short way across the map, which moves it
DRAG_FROM, DRAG_TO, DRAG_ROW = 300, 340, 200
# where an arrow would be drawn, down and right of the middle of the screen,
# which the knob's pointer is put at. the shell's overview has a button
# there, whose letters are as light with a pointer on it as without
POINTER_BOX = (desktop.WIDTH // 2, desktop.HEIGHT // 2, desktop.WIDTH // 2 + 16, desktop.HEIGHT // 2 + 24)


def installed():
    return subprocess.run(["flatpak", "info", COMAPS], capture_output=True).returncode == 0


@unittest.skipUnless(installed(), f"needs the {COMAPS} flatpak in {IMAGE_FLATPAKS}")
class ComapsTest(test_phosh.PhoshSession):
    # dark like the rest of the session, which is dark unless told otherwise
    LIGHT_MAP = False

    @classmethod
    def launcher(cls):
        """what the shell runs for comaps' icon: the session's own entry
        for it, which comes before the one flatpak made, with no network"""
        own = cls.dir / "home/.local/share/applications" / f"{COMAPS}.desktop"
        entry = own if own.exists() else pathlib.Path(IMAGE_FLATPAKS, "exports/share/applications", own.name)
        line = next(line for line in entry.read_text().splitlines() if line.startswith("Exec="))
        flatpak, run, *rest = shlex.split(line.removeprefix("Exec="))
        return [flatpak, run, "--unshare=network", *rest]

    @classmethod
    def settings(cls):
        return {**super().settings(), "BEHEAD_COMAPS": cls.dir / "comaps"}

    @classmethod
    def prepare(cls):
        """fetch as a build host does, and leave the region where an image would"""
        super().prepare()
        subprocess.run([FETCH, "--flatpak", IMAGE_FLATPAKS, "--out", cls.dir / "comaps", REGION], check=True)

    @classmethod
    def looks(cls, box):
        """the share of a box of the latest picture that is light, as the
        land of comaps' map is, and how many different colours it has"""
        frame, (left, top, right, bottom) = cls.last_frame(), box
        if not frame:
            return 0, 0
        pixels = [bytes(frame[(y * desktop.WIDTH + x) * 3:(y * desktop.WIDTH + x) * 3 + 3])
                  for y in range(top, bottom) for x in range(left, right)]
        return sum(min(pixel) > LIGHT for pixel in pixels) / len(pixels), len(set(pixels))

    @classmethod
    def server_options(cls):
        """the knob as a mouse's wheel, the one way comaps zooms"""
        return [*super().server_options(), "--knob", "scroll"]

    @classmethod
    def map_pixels(cls):
        return cls.box_pixels(MAP_BOX)

    @classmethod
    def box_pixels(cls, box):
        frame, (left, top, right, bottom) = cls.last_frame(), box
        return [bytes(frame[(y * desktop.WIDTH + x) * 3:(y * desktop.WIDTH + x) * 3 + 3])
                for y in range(top, bottom) for x in range(left, right)]

    def test_the_cars_knob_zooms_the_map(self):
        """comaps is made for a desk, and zooms for a wheel alone. a car's
        knob is one, when the server is told so"""
        self.open_map()
        self.pump_for(SETTLE)
        before = self.map_pixels()

        def redrawn():
            now = self.map_pixels()
            return sum(old != new for old, new in zip(before, now)) / len(before) >= REDRAWN_SHARE
        self.headunit.turn(fakehu.ROTARY, -ZOOM_DETENTS)
        self.wait_for(redrawn, "drew the map at another scale for a turn of the knob")

    def test_the_knob_still_zooms_once_a_finger_moved_the_map(self):
        """a finger on the screen takes the place of the pointer the wheel
        turns under, and the wheel has to have it back"""
        self.open_map()
        self.pump_for(SETTLE)
        for action, x in ((fakehu.TOUCH_DOWN, DRAG_FROM), (fakehu.TOUCH_MOVED, DRAG_TO), (fakehu.TOUCH_UP, DRAG_TO)):
            self.headunit.touch(action, [(0, x, DRAG_ROW)])
            self.pump_for(desktop.HOLD)
        self.pump_for(SETTLE)
        before = self.map_pixels()

        def redrawn():
            now = self.map_pixels()
            return sum(old != new for old, new in zip(before, now)) / len(before) >= REDRAWN_SHARE
        self.headunit.turn(fakehu.ROTARY, -ZOOM_DETENTS)
        self.wait_for(redrawn, "drew the map at another scale for a turn of the knob after a touch")

    def test_a_finger_held_on_a_button_opens_its_list(self):
        """comaps opens a button's list for the arrow beside it, which is a
        mouse's target and no finger's. the session has it open for a
        finger that stays on the button instead"""
        self.open_map()
        self.pump_for(SETTLE)
        before = self.box_pixels(LIST_BOX)
        self.headunit.touch(fakehu.TOUCH_DOWN, [(0, *FLAG_BUTTON)])
        self.addCleanup(self.headunit.touch, fakehu.TOUCH_UP, [(0, *FLAG_BUTTON)])

        def listed():
            now = self.box_pixels(LIST_BOX)
            return sum(old != new for old, new in zip(before, now)) / len(before) >= LISTED_SHARE
        self.wait_for(listed, "opened the list of a button a finger stayed on")

    def test_the_knob_shows_no_pointer(self):
        """a wheel needs a pointer to turn under, and a car has nothing to
        move one with: it is not drawn"""
        def light():
            self.pump_for(SETTLE)
            frame, (left, top, right, bottom) = self.last_frame(), POINTER_BOX
            return [(x, y) for y in range(top, bottom) for x in range(left, right)
                    if min(frame[(y * desktop.WIDTH + x) * 3:(y * desktop.WIDTH + x) * 3 + 3]) > LIGHT]
        before = light()
        self.headunit.turn(fakehu.ROTARY, 1)
        self.assertEqual(light(), before, "an arrow's light outline over the shell's overview")

    def test_opens_on_the_map_of_where_the_car_is_with_no_network(self):
        self.open_map()

    def open_map(self):
        runtime = self.dir / "runtime"
        display = next(path.name for path in runtime.iterdir() if path.name.startswith("wayland-")
                       and not path.name.endswith(".lock"))
        session = {**os.environ, "HOME": str(self.dir / "home"), "XDG_RUNTIME_DIR": str(runtime),
                   "WAYLAND_DISPLAY": display, "DBUS_SESSION_BUS_ADDRESS": f"unix:path={runtime / 'bus'}"}
        comaps = subprocess.Popen(self.launcher(), env=session,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        # in this order from the last: the application, then what started it,
        # which would otherwise be waited for without end if it never came up
        self.addCleanup(comaps.wait)
        self.addCleanup(comaps.kill)
        self.addCleanup(subprocess.run, ["flatpak", "kill", COMAPS], capture_output=True)

        def drawn():
            self.headunit.sense(fakehu.SENSOR_LOCATION, *CAR)
            red, green, blue = self.pixel(*STAR) or (0, 0, 0)
            land, colours = self.looks(MAP_BOX)
            return red > LIGHT > max(green, blue) and colours >= DRAWN_COLOURS and (land >= LAND_SHARE) == self.LIGHT_MAP
        style = "light" if self.LIGHT_MAP else "dark"
        self.wait_for(drawn, f"drew the map around the car, in its {style} style", SHOWN)


class LightComapsTest(ComapsTest):
    SCHEME = "light"
    LIGHT_MAP = True
    # the knob is the other class's to turn
    test_the_cars_knob_zooms_the_map = None
    test_the_knob_shows_no_pointer = None


if __name__ == "__main__":
    unittest.main()
