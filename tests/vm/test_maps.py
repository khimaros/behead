"""a map without a network: gnome maps in the phosh session, as the flatpak
the pi image carries, shows a place that tools/fetch-map-areas.js fetched
beforehand, with its own network taken away. the fetched files are handed
to the session as an image hands them, in BEHEAD_MAPS, and the session puts
them where gnome maps keeps its own. needs the flatpak in the vm
(tests/vm/provision.sh) and, for the fetch, the internet."""

import os
import subprocess
import unittest

import desktop
import test_phosh
from desktop import tcp

MAPS = "org.gnome.Maps"
FETCH = tcp.ROOT / "tools/fetch-map-areas.js"
# a few blocks of san francisco, small enough to fetch in seconds
AREA = "Test=-122.42,37.77,-122.39,37.80"
INSIDE = "geo:37.785,-122.405?z=15"
FETCH_TIMEOUT = 300
# a part of the map, right of the panel that names the place and clear of the
# map's buttons. it is also clear of where phosh asks whether an application
# may know the location, which it must not ask of the map: nobody in a car
# should have to answer, and the map does not show until somebody has
MAP_BOX = (560, 80, 780, 250)
# streets, blocks and names make far more colours than this. a map with no
# tiles for the place is one flat background, and so is a shell still asking
DRAWN_COLOURS = 100
# gnome maps takes a while to start cold inside its sandbox
SHOWN = 90


# where a flatpak application keeps its data, under the home it is run with
DATA = f".var/app/{MAPS}/data/gnome-maps"
# where an image carries its flatpak applications (tools/fetch-flatpak.sh),
# which the session then runs from. a vm provisioned so is tested so; one
# with the application installed for the whole system, from there
IMAGE_FLATPAKS = "/usr/share/behead/flatpak"
if os.path.isdir(IMAGE_FLATPAKS):
    os.environ["FLATPAK_USER_DIR"] = IMAGE_FLATPAKS


def installed():
    return subprocess.run(["flatpak", "info", MAPS], capture_output=True).returncode == 0


@unittest.skipUnless(installed(), f"needs the {MAPS} flatpak")
class MapsTest(test_phosh.PhoshSession):
    @classmethod
    def settings(cls):
        return {**super().settings(), "BEHEAD_MAPS": cls.dir / "maps"}

    @classmethod
    def colours(cls, box):
        """how many different colours a box of the latest picture has"""
        frame, (left, top, right, bottom) = cls.last_frame(), box
        return len({bytes(frame[(y * desktop.WIDTH + x) * 3:(y * desktop.WIDTH + x) * 3 + 3])
                    for y in range(top, bottom) for x in range(left, right)}) if frame else 0

    @classmethod
    def session_env(cls):
        """what an application started beside the session needs to join it"""
        runtime = cls.dir / "runtime"
        display = next(path.name for path in runtime.iterdir() if path.name.startswith("wayland-")
                       and not path.name.endswith(".lock"))
        return {**os.environ, "HOME": str(cls.dir / "home"), "XDG_RUNTIME_DIR": str(runtime),
                "WAYLAND_DISPLAY": display, "DBUS_SESSION_BUS_ADDRESS": f"unix:path={runtime / 'bus'}"}

    @classmethod
    def prepare(cls):
        """fetch as a build host does, in a home that is not the session's,
        and leave what was fetched where an image would"""
        super().prepare()
        builder = cls.dir / "builder"
        builder.mkdir()
        subprocess.run(["flatpak", "run", "--command=gjs", f"--filesystem={FETCH.parent}:ro", MAPS, "-m", FETCH, AREA],
                       env={**os.environ, "HOME": str(builder)}, check=True, timeout=FETCH_TIMEOUT)
        (builder / DATA).rename(cls.dir / "maps")

    def show(self, place):
        """gnome maps at a place, with no network of its own"""
        maps = subprocess.Popen(["flatpak", "run", "--unshare=network", MAPS, place], env=self.session_env(),
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        # in this order from the last: the application, then what started it,
        # which would otherwise be waited for without end if it never came up
        self.addCleanup(maps.wait)
        self.addCleanup(maps.kill)
        self.addCleanup(subprocess.run, ["flatpak", "kill", MAPS], capture_output=True)

    def test_shows_a_fetched_place_with_no_network(self):
        self.show(INSIDE)
        self.wait_for(lambda: self.colours(MAP_BOX) >= DRAWN_COLOURS,
                      f"drew the map: a street map has {DRAWN_COLOURS} colours or more there", SHOWN)


if __name__ == "__main__":
    unittest.main()
