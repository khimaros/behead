"""end-to-end: the desktop sessions `make sessions-rpi` puts in the pi image"""

import os
import pathlib
import re
import subprocess
import tempfile
import unittest

from test_session import ROOT, run

UNIT = ROOT / "rpi/overlay/etc/systemd/system/behead.service"
DEVICE_SESSIONS = "/usr/local/lib/behead/sessions"
DEPENDS_PREFIX = "session-"
ANDROID_IMAGES = ("system.img", "vendor.img")
# stands in for tools/fetch-waydroid.py, which downloads about 1 GB
FETCH_STUB = """#!/bin/sh
[ "$1 $2 $3" = "--arch arm64 --out" ] || exit 2
mkdir -p "$4"
for image in %s; do echo "$image" > "$4/$image"; done
""" % " ".join(ANDROID_IMAGES)
# stands in for tools/fetch-fdroid.py, and names the apk it verified as that does
FETCH_APP_STUB = """#!/bin/sh
[ "$2 $3 $4" = "--abi arm64-v8a --out" ] || exit 2
mkdir -p "$5"
echo stale > "$5/$1_1.apk"
echo "$1" > "$5/$1_2.apk"
echo "$1 2.0 (2) for arm64-v8a: verified $5/$1_2.apk"
"""
ANDROID_APPS = ("org.fdroid.fdroid", "net.osmand.plus")
# stands in for the hostname in rpi/customize.env, which names the images
HOSTNAME = "car"


class RpiSessions(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = pathlib.Path(tmp.name)
        self.scripts, self.env, self.depends = self.dir / "sessions", self.dir / "session.env", self.dir / "depends"
        self.android, self.sources = self.dir / "android", self.dir / "sources"
        self.cmdline, self.apk_cache = self.dir / "cmdline", self.dir / "apk-cache"
        self.geoclue, self.preferences = self.dir / "geoclue.conf", self.dir / "preferences"
        self.fetch, self.fetch_app = self.stub("fetch", FETCH_STUB), self.stub("fetch-app", FETCH_APP_STUB)
        # stands in for tools/preinstall-apks.py, and keeps what it was asked
        self.preinstalled = self.dir / "preinstalled"
        self.preinstall = self.stub("preinstall", f'#!/bin/sh\ntest -f "$1" && echo "$@" > {self.preinstalled}\n')
        # stand in for tools/fetch-flatpak.sh and tools/fetch-maps.sh, which
        # fetch gigabytes: each leaves what it was asked, a line an argument
        self.flatpaks, self.maps = self.dir / "flatpak", self.dir / "maps"
        asked = '#!/bin/sh\nmkdir -p "$%d"\nprintf "%%s\\n" "$@" > "$%d/asked"\n'
        self.fetch_flatpak, self.fetch_maps = self.stub("fetch-flatpak", asked % (2, 2)), self.stub("fetch-maps", asked % (1, 1))
        # and for tools/fetch-comaps.py, which is told its directory after --flatpak DIR --out
        self.comaps, self.fetch_comaps = self.dir / "comaps", self.stub("fetch-comaps", asked % (4, 4))

    def stub(self, name, script):
        path = self.dir / name
        path.write_text(script)
        path.chmod(0o755)
        return path

    def collect(self, **chosen):
        """run `make sessions-rpi`, as `make image-rpi` does, with SESSION and SESSIONS as given"""
        run("make", "-C", ROOT, "sessions-rpi", f"RPI_SESSIONS={self.scripts}", f"RPI_SESSION_ENV={self.env}",
            f"RPI_DEPENDS={self.depends}", f"RPI_APT_SOURCES={self.sources}", f"RPI_WAYDROID={self.android}",
            f"RPI_SESSION_CMDLINE={self.cmdline}", f"WAYDROID_CACHE={self.dir / 'cache'}",
            f"APK_CACHE={self.apk_cache}", f"FETCH_WAYDROID={self.fetch}", f"FETCH_FDROID={self.fetch_app}",
            f"PREINSTALL_APKS={self.preinstall}", f"RPI_GEOCLUE={self.geoclue}",
            f"RPI_APT_PREFERENCES={self.preferences}", f"RPI_FLATPAK={self.flatpaks}", f"RPI_MAPS={self.maps}",
            f"FETCH_FLATPAK={self.fetch_flatpak}", f"FETCH_MAPS={self.fetch_maps}",
            f"RPI_COMAPS={self.comaps}", f"FETCH_COMAPS={self.fetch_comaps}",
            *(f"{name}={value}" for name, value in chosen.items()))

    def package_lists(self):
        return sorted(path.name for path in self.depends.glob(f"{DEPENDS_PREFIX}*"))

    def packages(self, session):
        lines = (self.depends / f"{DEPENDS_PREFIX}{session}").read_text().splitlines()
        return [line for line in lines if line and not line.startswith("#")]

    def test_without_a_choice_the_image_keeps_the_demo(self):
        """the scripts are there to switch to by hand, but nothing they need is installed"""
        self.collect()
        self.assertFalse(self.env.exists(), "the default video command was overridden")
        self.assertEqual(self.package_lists(), [])
        self.assertFalse(self.android.exists())
        for name in ("lib.sh", "kiosk.sh", "phosh.sh"):
            self.assertTrue((self.scripts / name).exists(), f"{name} is missing")

    def test_session_is_shown_and_its_packages_installed(self):
        self.collect(SESSION="phosh")
        self.assertIn(f'VIDEO_CMD="{DEVICE_SESSIONS}/phosh.sh {{width}} {{height}} {{fps}}"\n', self.env.read_text())
        self.assertEqual(self.package_lists(), [f"{DEPENDS_PREFIX}phosh"])
        self.assertIn("phosh", self.packages("phosh"))
        self.assertTrue((self.scripts / "phosh.sh").stat().st_mode & 0o111, "the session script cannot be run")

    def test_waydroid_renders_on_the_gpu(self):
        """android is slow when the pi's cpu draws it. a session untried on the gpu stays on the cpu"""
        self.collect(SESSION="waydroid")
        self.assertIn("BEHEAD_RENDERER=gles2\n", self.env.read_text())
        self.assertIn(f'VIDEO_CMD="{DEVICE_SESSIONS}/waydroid.sh ', self.env.read_text())
        self.collect(SESSION="wayland", SESSIONS="waydroid")
        self.assertNotIn("BEHEAD_RENDERER", self.env.read_text())

    def test_phosh_renders_on_the_gpu(self):
        """with the drivers the gpu needs, which the kiosk's packages bring the other sessions"""
        self.collect(SESSION="phosh")
        self.assertIn("BEHEAD_RENDERER=gles2\n", self.env.read_text())
        self.assertIn("libgl1-mesa-dri", self.packages("phosh"))

    def test_phosh_gets_the_wlroots_its_compositor_was_built_for(self):
        """raspberry pi os carries a wlroots of its own, which debian's phoc
        crashes on at startup, so apt is told to take debian's"""
        self.collect(SESSION="kodi", SESSIONS="phosh")
        self.assertEqual([path.name for path in self.preferences.iterdir()], [f"{DEPENDS_PREFIX}phosh"])
        pin = next(self.preferences.iterdir()).read_text()
        self.assertRegex(pin, r"(?m)^Package: .*\blibwlroots-")
        self.assertRegex(pin, r"(?m)^Pin: origin deb\.debian\.org$")
        self.collect(SESSION="kodi")
        self.assertEqual(list(self.preferences.glob(f"{DEPENDS_PREFIX}*")), [])

    def test_phosh_plays_its_sound_to_the_car(self):
        """the session runs a sound server that plays into the loopback, and
        the image has it and what records the loopback's other end"""
        self.collect(SESSION="phosh")
        self.assertIn(f'AUDIO_CMD="{DEVICE_SESSIONS}/audio.sh {{rate}} {{channels}}"\n', self.env.read_text())
        self.assertRegex(self.env.read_text(), r"(?m)^AUDIO_DELAY=[1-9]\d*$")
        for package in ("pulseaudio", "alsa-utils"):
            self.assertIn(package, self.packages("phosh"))

    def test_kodi_plays_its_sound_to_the_car(self):
        """the session names the command that records kodi, the unit passes
        it on, and the image has what the command needs"""
        self.collect(SESSION="kodi")
        self.assertIn(f'AUDIO_CMD="{DEVICE_SESSIONS}/audio.sh {{rate}} {{channels}}"\n', self.env.read_text())
        self.assertTrue((self.scripts / "audio.sh").stat().st_mode & 0o111, "the audio command cannot be run")
        self.assertIn("alsa-utils", self.packages("kodi"))
        self.assertIn(" --audio-cmd ${AUDIO_CMD} ", UNIT.read_text())

    def test_kodi_brings_what_fills_its_library(self):
        self.collect(SESSION="kodi")
        self.assertTrue((self.scripts / "kodi-library.py").stat().st_mode & 0o111, "the library helper cannot be run")
        self.assertIn("python3", self.packages("kodi"))

    def test_kodi_holds_its_sound_back_for_the_picture(self):
        """the picture reaches the car's screen later than the sound its
        speakers, by an amount found by ear on the pi"""
        self.collect(SESSION="kodi")
        self.assertRegex(self.env.read_text(), r"(?m)^AUDIO_DELAY=[1-9]\d*$")
        self.assertIn(" --audio-delay ${AUDIO_DELAY} ", UNIT.read_text())

    def test_kodi_takes_the_knob_as_arrow_keys(self):
        """tab, what a knob's turn is elsewhere, leaves kodi's menus for
        whatever is playing"""
        self.collect(SESSION="kodi")
        self.assertIn("KNOB=arrows\n", self.env.read_text())
        self.assertIn(" --knob ${KNOB} ", UNIT.read_text())

    def test_phosh_is_worked_by_a_knob_alone(self):
        """applications made for a desk want more of a knob than one kind
        of key: phosh's steps through its uses, and back closes things"""
        self.collect(SESSION="phosh")
        self.assertIn("KNOB=focus,arrows,scroll,type\n", self.env.read_text())
        self.assertIn("BACK=escape\n", self.env.read_text())
        self.assertIn(" --back ${BACK} ", UNIT.read_text())

    def test_kodi_renders_on_the_gpu(self):
        self.collect(SESSION="kodi")
        self.assertIn("BEHEAD_RENDERER=gles2\n", self.env.read_text())

    def test_phosh_applications_can_ask_where_the_car_is(self):
        """geoclue comes with phosh, told to read the socket the service offers the car's location on"""
        self.collect(SESSION="phosh")
        self.assertIn("geoclue-2.0", self.packages("phosh"))
        socket = re.search(r"^nmea-socket=(\S+)$", self.geoclue.read_text(), re.MULTILINE).group(1)
        self.assertIn(f" --nmea-socket {socket} ", UNIT.read_text())

    def test_phosh_comes_with_applications_to_open(self):
        """a shell with nothing in its launcher shows the car nothing: the
        settings its own menus lead to, a terminal, and a map, which is a
        flatpak, the image's debian having none that keeps maps for later"""
        self.collect(SESSION="phosh")
        for package in ("flatpak", "xdg-desktop-portal-gtk", "gnome-control-center", "gnome-console", "squeekboard"):
            self.assertIn(package, self.packages("phosh"))
        self.assertNotIn("gnome-maps", self.packages("phosh"), "the packaged map beside the flatpak one")
        self.assertEqual((self.flatpaks / "asked").read_text().split(),
                         ["aarch64", str(self.flatpaks), "org.gnome.Maps", "app.comaps.comaps",
                          "com.github.neithern.g4music"])

    def test_the_map_that_searches_and_routes_comes_with_its_regions(self):
        """comaps does all of it with no network, on regions of its own
        kind: those of the two cities unless COMAPS_REGIONS names others,
        taken from the list of the comaps the image carries"""
        self.collect(SESSION="phosh")
        asked = (self.comaps / "asked").read_text().splitlines()
        self.assertEqual(asked[:4], ["--flatpak", str(self.flatpaks), "--out", str(self.comaps)])
        self.assertEqual(asked[4:], ["US_California_Santa_Clara_Palo Alto", "US_Tennessee_West", "US_Tennessee_East"])
        self.collect(SESSION="phosh", COMAPS_REGIONS="US_Maine;France_Ile-de-France_Paris")
        self.assertEqual((self.comaps / "asked").read_text().splitlines()[4:], ["US_Maine", "France_Ile-de-France_Paris"])
        self.collect(SESSION="phosh", COMAPS_REGIONS="")
        self.assertFalse(self.comaps.exists(), "regions fetched though none were asked for")

    def test_the_map_comes_with_the_places_asked_for(self):
        """for where there is no network: the ones MAP_AREAS names, and
        none unless it does"""
        self.collect(SESSION="phosh")
        self.assertFalse(self.maps.exists(), "maps fetched though none were asked for")
        self.collect(SESSION="phosh", MAP_AREAS="Mount Desert Island=-68.5,44.2,-68.1,44.5;Bar=1,2,3,4")
        asked = (self.maps / "asked").read_text().splitlines()
        self.assertEqual(asked[0], str(self.maps))
        self.assertEqual(asked[1:], ["Mount Desert Island=-68.5,44.2,-68.1,44.5", "Bar=1,2,3,4"])

    def test_other_sessions_bring_no_flatpaks_and_no_maps(self):
        self.collect(SESSION="phosh")
        self.collect(SESSION="kodi")
        left = [path for path in (self.flatpaks, self.maps, self.comaps) if path.exists()]
        self.assertEqual(left, [], "left from the image built before")

    def test_the_launcher_needs_no_packages_and_plays_its_own_sound(self):
        """it draws its own pictures and decodes its own music, so the image
        gets its program, and a sound command that reads what it plays"""
        self.collect(SESSION="launcher")
        self.assertIn(f'VIDEO_CMD="{DEVICE_SESSIONS}/launcher.sh {{width}} {{height}} {{fps}}"\n', self.env.read_text())
        self.assertIn(f'AUDIO_CMD="{DEVICE_SESSIONS}/launcher.sh --audio {{rate}} {{channels}}"\n', self.env.read_text())
        self.assertEqual(self.package_lists(), [])
        self.assertTrue((self.scripts / "launcher.sh").stat().st_mode & 0o111, "the session script cannot be run")
        planned = subprocess.run(["make", "-n", "-C", ROOT, "image-rpi", f"RPI_HOSTNAME={HOSTNAME}", "SESSION=launcher"],
                                 check=True, capture_output=True, text=True).stdout
        self.assertRegex(planned, r"(?m)^install .*/usr/local/bin .*/behead-launcher$")
        self.assertRegex(planned, rf"(?m)images/{HOSTNAME}-launcher\.img\.gz$")

    def test_the_launcher_refuses_sound_it_would_play_at_the_wrong_speed(self):
        refused = subprocess.run([ROOT / "sessions/launcher.sh", "--audio", "44100", "2"], capture_output=True,
                                 env={"PATH": os.environ["PATH"], "BEHEAD_LAUNCHER_PIPE": str(self.dir / "pipe")})
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn(b"48000 2, not 44100 2", refused.stderr)

    def test_sessions_installs_more_than_the_one_shown(self):
        """kodi runs in the kiosk, so the kiosk's packages come with it"""
        self.collect(SESSION="kodi", SESSIONS="phosh")
        self.assertIn(f"{DEVICE_SESSIONS}/kodi.sh ", self.env.read_text())
        self.assertEqual(self.package_lists(), [f"{DEPENDS_PREFIX}{name}" for name in ("kiosk", "kodi", "phosh")])
        self.assertIn("sway", self.packages("kiosk"))

    def test_sessions_alone_leaves_the_demo_showing(self):
        self.collect(SESSIONS="kodi phosh")
        self.assertFalse(self.env.exists())
        self.assertEqual(len(self.package_lists()), 3)

    def test_a_session_left_out_of_the_next_build_leaves_the_image(self):
        self.collect(SESSION="waydroid", SESSIONS="phosh")
        self.collect()
        self.assertFalse(self.env.exists())
        self.assertEqual(self.package_lists(), [])
        self.assertFalse(self.android.exists())

    def test_waydroid_brings_android_images(self):
        """waydroid starts from images it finds on the device, so it needs no download there"""
        self.collect(SESSION="waydroid")
        self.assertEqual(sorted(path.name for path in self.android.iterdir()), sorted(ANDROID_IMAGES))
        self.assertEqual(self.package_lists(), [f"{DEPENDS_PREFIX}kiosk", f"{DEPENDS_PREFIX}waydroid"])

    def test_waydroid_comes_with_its_apps(self):
        """the newest verified apk of f-droid and osmand~ goes into the
        image's copy of android's system image, so android starts with them"""
        self.collect(SESSION="waydroid")
        apks = [str(self.apk_cache / f"{app}_2.apk") for app in ANDROID_APPS]
        self.assertEqual(self.preinstalled.read_text().split(),
                         [str(self.android / "system.img"), "--abi", "arm64-v8a", *apks])
        self.preinstalled.unlink()
        self.collect(SESSION="phosh")
        self.assertFalse(self.preinstalled.exists())

    def test_waydroid_asks_for_pressure_stall_information(self):
        """the pi kernel has it switched off, and android's low memory killer,
        which android cannot start without, needs it"""
        self.collect(SESSIONS="waydroid phosh")
        self.assertEqual(self.cmdline.read_text().split(), ["psi=1"])
        self.collect(SESSION="phosh")
        self.assertEqual(self.cmdline.read_text().split(), [])

    def test_waydroid_brings_the_repository_that_carries_it(self):
        """debian stable has no waydroid package; its backports do"""
        self.collect(SESSIONS="waydroid phosh")
        self.assertEqual([path.name for path in self.sources.iterdir()], [f"{DEPENDS_PREFIX}waydroid.sources"])
        self.assertIn("backports", next(self.sources.iterdir()).read_text())
        self.collect(SESSION="phosh")
        self.assertEqual(list(self.sources.glob(f"{DEPENDS_PREFIX}*")), [])

    def test_each_session_has_an_image_of_its_own(self):
        """`make image-rpi` has raspi-provision write a gzip image named by
        the session, which leaves the other sessions' images in place, and
        `make flash-rpi` hands the same one to `raspi-provision flash`"""
        asked = self.dir / "asked"
        provision = self.stub("provision", f'#!/bin/sh\necho "$(basename "$PWD") $*" >> {asked}\n')
        for session, name in (("phosh", "phosh"), ("", "demo")):
            image = f"images/{HOSTNAME}-{name}.img.gz"
            chosen = (f"RPI_HOSTNAME={HOSTNAME}", f"SESSION={session}", f"RASPI_PROVISION={provision}")
            planned = subprocess.run(["make", "-n", "-C", ROOT, "image-rpi", *chosen], check=True,
                                     capture_output=True, text=True).stdout
            self.assertRegex(planned, rf"(?m)^cd rpi && .*{provision} image {image}$")
            run("make", "-C", ROOT, "flash-rpi", *chosen)
            self.assertEqual(asked.read_text().splitlines()[-1], f"rpi flash {image}")

    def test_unknown_session_fails_the_build(self):
        for chosen in ({"SESSION": "lib"}, {"SESSIONS": "phosh gnome"}):
            with self.assertRaises(subprocess.CalledProcessError):
                self.collect(**chosen)
