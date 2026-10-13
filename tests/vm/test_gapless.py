"""music in the phosh session: gapless, as the flatpak the pi image carries,
plays what is under BEHEAD_MEDIA/music, where kodi and the launcher look too,
and the car hears it. a sandbox sees none of that directory unasked and
gapless looks in ~/Music, so the session has to arrange both. needs the
flatpak in the vm (tests/vm/provision.sh)."""

import os
import subprocess
import unittest

import desktop
import test_phosh
from desktop import fakehu, tcp

GAPLESS = "com.github.neithern.g4music"
PLAYER = f"org.mpris.MediaPlayer2.{GAPLESS}"
# where an image carries its flatpak applications, which the session runs from
IMAGE_FLATPAKS = "/usr/share/behead/flatpak"
os.environ["FLATPAK_USER_DIR"] = IMAGE_FLATPAKS
SONG_SECONDS = 120
LOOKED = 5


def installed():
    return subprocess.run(["flatpak", "info", GAPLESS], capture_output=True).returncode == 0


@unittest.skipUnless(installed(), f"needs the {GAPLESS} flatpak in {IMAGE_FLATPAKS}")
class GaplessTest(test_phosh.PhoshSession):
    @classmethod
    def settings(cls):
        return {**super().settings(), "BEHEAD_MEDIA": cls.dir / "media"}

    @classmethod
    def prepare(cls):
        super().prepare()
        (cls.dir / "media/music").mkdir(parents=True)
        tcp.run("ffmpeg", "-f", "lavfi", "-i", f"sine=frequency={tcp.TONE}:sample_rate={fakehu.MEDIA_RATE}",
                "-t", str(SONG_SECONDS), "-metadata", "title=tone", cls.dir / "media/music/tone.flac")

    def test_plays_the_music_of_the_media_directory_to_the_car(self):
        runtime = self.dir / "runtime"
        display = next(path.name for path in runtime.iterdir() if path.name.startswith("wayland-")
                       and not path.name.endswith(".lock"))
        session = {**os.environ, "HOME": str(self.dir / "home"), "XDG_RUNTIME_DIR": str(runtime),
                   "WAYLAND_DISPLAY": display, "DBUS_SESSION_BUS_ADDRESS": f"unix:path={runtime / 'bus'}"}
        gapless = subprocess.Popen(["flatpak", "run", "--unshare=network", GAPLESS], env=session,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(gapless.wait)
        self.addCleanup(gapless.kill)
        self.addCleanup(subprocess.run, ["flatpak", "kill", GAPLESS], capture_output=True, env=session)
        heard_from = len(self.headunit.audio)

        def ask(destination, path, method, *arguments):
            return subprocess.run(["gdbus", "call", "--session", "--dest", destination, "--object-path", path,
                                   "--method", method, *arguments], capture_output=True, text=True, env=session).stdout
        self.wait_for(lambda: "true" in ask("org.freedesktop.DBus", "/org/freedesktop/DBus",
                                            "org.freedesktop.DBus.NameHasOwner", PLAYER), "started gapless")
        # for it to have looked through the music
        self.pump_for(LOOKED)
        ask(PLAYER, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player.Play")
        self.wait_for_tone(heard_from)


if __name__ == "__main__":
    unittest.main()
