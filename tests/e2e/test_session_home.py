"""end-to-end: the home sessions give their applications, and the media
sources `sessions/kodi.sh` gives a kodi that has none"""

import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest
import xml.etree.ElementTree

from test_session import ROOT

MODE = ("800", "480", "30")
KINDS = ("video", "music")
DIRECTORIES = {"video": "videos", "music": "music"}
SESSIONS = ROOT / "sessions"
# the session's own directory in the home of whoever runs it
STATE = ".behead"
KODI_SOURCES = ".kodi/userdata/sources.xml"
KODI_SETTINGS = ".kodi/userdata/guisettings.xml"
# kodi's name for the playback end of the alsa loopback
LOOPBACK_DEVICE = "ALSA:@:CARD=Loopback,DEV=0|"
# what the session scripts read from the environment
SETTINGS = ("BEHEAD_HOME", "BEHEAD_MEDIA")
# where the pi keeps media, which the image's unit hands to the session
DEVICE_MEDIA = "/home/behead/media"
# the sessions' home on the pi, and the list that has raspi-provision keep it
DEVICE_HOME = "/var/lib/behead"
UNIT = ROOT / "rpi/overlay/etc/systemd/system/behead.service"
PERSISTED = ROOT / "rpi/overlay/etc/persist.d/behead"
# stands in for kiosk.sh, which needs sway and kodi, and notes how it was run
KIOSK_STUB = """#!/bin/sh
echo "$@" > "$(dirname "$0")/kiosk.args"
echo "$HOME" > "$(dirname "$0")/kiosk.home"
"""


class SessionHome(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = pathlib.Path(tmp.name)
        self.home, self.media = self.dir / "home", self.dir / "media"
        self.state = self.home / STATE
        self.sources = self.state / KODI_SOURCES
        for script in ("kodi.sh", "home.sh", "kodi-library.py", "kodi-addons.py"):
            shutil.copy(SESSIONS / script, self.dir)
        kiosk = self.dir / "kiosk.sh"
        kiosk.write_text(KIOSK_STUB)
        kiosk.chmod(0o755)

    def run_as_server(self, *command, **env):
        inherited = {name: value for name, value in os.environ.items() if name not in SETTINGS}
        return subprocess.run(command, check=True, capture_output=True, text=True,
                              env={**inherited, "HOME": str(self.home), **env}).stdout

    def start(self, **env):
        """run kodi.sh as the server does, up to the kiosk"""
        self.run_as_server(self.dir / "kodi.sh", *MODE, **env)

    def paths(self, sources, kind):
        root = xml.etree.ElementTree.parse(sources).getroot()
        return [path.text for path in root.findall(f"{kind}/source/path")]

    def test_applications_get_a_home_of_the_sessions_own(self):
        """every session reads lib.sh, so what an application keeps stays out
        of the files of whoever runs behead"""
        home = self.run_as_server("sh", "-c", f'. {SESSIONS / "lib.sh"}; echo "$HOME"', SESSIONS / "kiosk.sh").strip()
        self.assertEqual(home, str(self.state))
        self.assertTrue(self.state.is_dir())

    def test_the_sessions_home_can_be_named(self):
        named = self.dir / "named"
        self.start(BEHEAD_HOME=str(named), BEHEAD_MEDIA=str(self.media))
        self.assertTrue((named / KODI_SOURCES).exists())
        self.assertEqual((self.dir / "kiosk.home").read_text().strip(), str(named))
        self.assertFalse(self.home.exists(), "the home of whoever runs behead was written to")

    def test_a_fresh_kodi_gets_the_media_directories_as_sources(self):
        self.start(BEHEAD_MEDIA=str(self.media))
        for kind in KINDS:
            directory = self.media / DIRECTORIES[kind]
            self.assertEqual(self.paths(self.sources, kind), [f"{directory}/"])
            self.assertTrue(directory.is_dir(), f"{directory} is not there to copy files into")
        self.assertEqual((self.dir / "kiosk.args").read_text().split(), [*MODE, "kodi", "--windowing=wayland"])
        self.assertEqual((self.dir / "kiosk.home").read_text().strip(), str(self.state))

    def test_videos_are_described_by_their_file_names(self):
        """kodi lists a film without the internet only if a .nfo beside it
        says what it is. one the user wrote is kept"""
        videos = self.media / "videos"
        (videos / "shows").mkdir(parents=True)
        for name in ("Road Trip (2019).mkv", "shows/Tom & Jerry.mp4", "Kept (2001).mp4", "cover.jpg"):
            (videos / name).touch()
        (videos / "Kept (2001).nfo").write_text("<movie/>")
        self.start(BEHEAD_MEDIA=str(self.media))
        described = {str(nfo.relative_to(videos)): nfo.read_text() for nfo in videos.rglob("*.nfo")}
        self.assertEqual(sorted(described), ["Kept (2001).nfo", "Road Trip (2019).nfo", "shows/Tom & Jerry.nfo"])
        self.assertEqual(described["Kept (2001).nfo"], "<movie/>")
        movie = xml.etree.ElementTree.fromstring(described["Road Trip (2019).nfo"])
        self.assertEqual((movie.findtext("title"), movie.findtext("year")), ("Road Trip", "2019"))
        movie = xml.etree.ElementTree.fromstring(described["shows/Tom & Jerry.nfo"])
        self.assertEqual((movie.findtext("title"), movie.findtext("year")), ("Tom & Jerry", None))

    def test_sources_the_user_set_up_are_kept(self):
        self.sources.parent.mkdir(parents=True)
        self.sources.write_text("<sources/>")
        self.start(BEHEAD_MEDIA=str(self.media))
        self.assertEqual(self.sources.read_text(), "<sources/>")

    def test_without_a_media_directory_kodi_gets_no_sources(self):
        self.start()
        self.assertFalse(self.sources.exists(), "kodi was given sources")
        self.assertTrue((self.dir / "kiosk.args").exists(), "the kiosk never ran")

    def test_a_fresh_kodi_plays_to_the_loopback(self):
        """sessions/audio.sh records the loopback's other end for the car"""
        self.start()
        settings = xml.etree.ElementTree.parse(self.state / KODI_SETTINGS).getroot()
        device = settings.find("setting[@id='audiooutput.audiodevice']").text
        self.assertTrue(device.startswith(LOOPBACK_DEVICE), device)

    def test_settings_the_user_made_are_kept(self):
        settings = self.state / KODI_SETTINGS
        settings.parent.mkdir(parents=True)
        settings.write_text("<settings/>")
        self.start()
        self.assertEqual(settings.read_text(), "<settings/>")

    def test_the_pi_keeps_state_and_media_where_they_survive_a_reboot(self):
        """raspi-provision keeps /home on the persist partition, and the
        directories the image lists in /etc/persist.d"""
        unit = UNIT.read_text()
        self.assertIn(f"Environment=BEHEAD_MEDIA={DEVICE_MEDIA}\n", unit)
        self.assertIn(f"Environment=BEHEAD_HOME={DEVICE_HOME}\n", unit)
        self.assertNotIn("Environment=HOME=", unit)
        self.assertIn(DEVICE_HOME, PERSISTED.read_text().splitlines())


if __name__ == "__main__":
    unittest.main()
