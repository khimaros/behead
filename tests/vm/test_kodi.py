"""kodi on the headunit: the kiosk session runs kodi in a headless sway, and
the car's touch and buttons reach it through --uinput. kodi reports which
window it shows over json-rpc, so the tests see what it did with the input."""

import json
import urllib.request

import desktop
from desktop import fakehu, tcp

KODI_PORT = 8080
# the web server on, for json-rpc without a login. these override kodi's
# settings without being them, so the session still finds a kodi with none
ADVANCED_SETTINGS = f"""<advancedsettings version="1.0">
    <services>
        <webserver>true</webserver>
        <webserverauthentication>false</webserverauthentication>
        <webserverport>{KODI_PORT}</webserverport>
    </services>
</advancedsettings>
"""
# what the session's media directory holds: a film named as kodi's wiki
# asks, "Title (Year)", and a tagged song
MOVIE, MOVIE_YEAR, SONG = "Road Trip", 2019, "Engine Hum"
# an add-on that comes as a package of its own, kodi-visualization-spectrum
PACKAGED_ADDON = "visualization.spectrum"
CLIP_SECONDS = 60
# estuary's window animations, during which kodi ignores input. they run
# slower when rendering on the cpu
SETTLE = 1.5
HOME_WINDOW, SETTINGS_WINDOW = 10000, 10004
# the settings gear above the main menu, in estuary at 800x480
SETTINGS_GEAR = (101, 102)
# a drawn home screen has far more colours than a blank or single colour one
MIN_COLOURS = 1000


def rpc(method, **params):
    request = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    post = urllib.request.Request(f"http://127.0.0.1:{KODI_PORT}/jsonrpc", request,
                                  {"Content-Type": "application/json"})
    with urllib.request.urlopen(post, timeout=fakehu.TIMEOUT) as response:
        return json.load(response)["result"]


def gui(name):
    """one of kodi's gui properties, or None while it is not answering"""
    try:
        return rpc("GUI.GetProperties", properties=[name])[name]
    except OSError:
        return None


def current_window():
    return (gui("currentwindow") or {}).get("id")


def focused_label():
    return (gui("currentcontrol") or {}).get("label")


class KodiTest(desktop.DesktopTest):
    KEYCODES = (fakehu.BACK, fakehu.DPAD_UP, fakehu.DPAD_DOWN, fakehu.DPAD_CENTER)

    @classmethod
    def prepare(cls):
        (cls.dir / "media/videos").mkdir(parents=True)
        (cls.dir / "media/music").mkdir()
        cls.clip = cls.dir / f"media/videos/{MOVIE} ({MOVIE_YEAR}).mp4"
        tcp.run("ffmpeg", "-f", "lavfi", "-i", f"sine=frequency={tcp.TONE}", "-t", "1", "-metadata", f"title={SONG}",
                "-metadata", "artist=behead", cls.dir / "media/music/song.flac")
        tcp.run("ffmpeg", "-f", "lavfi", "-i", f"testsrc2=size={desktop.WIDTH}x{desktop.HEIGHT}:rate=30",
                "-f", "lavfi", "-i", f"sine=frequency={tcp.TONE}:sample_rate={fakehu.MEDIA_RATE}",
                "-t", str(CLIP_SECONDS), "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac",
                "-ac", str(fakehu.MEDIA_CHANNELS), cls.clip)
        (cls.dir / "home/.kodi/userdata").mkdir(parents=True)
        (cls.dir / "home/.kodi/userdata/advancedsettings.xml").write_text(ADVANCED_SETTINGS)

    @classmethod
    def video_cmd(cls):
        return (f"env BEHEAD_HOME={cls.dir / 'home'} BEHEAD_MEDIA={cls.dir / 'media'} "
                f"{desktop.SESSIONS / 'kodi.sh'} {{width}} {{height}} {{fps}}")

    @classmethod
    def server_options(cls):
        return ["--audio-cmd", desktop.AUDIO_CMD]

    @classmethod
    def started(cls):
        return current_window() == HOME_WINDOW

    @classmethod
    def wait_for_window(cls, window):
        """wait until kodi shows the window, then for its opening animation"""
        cls.wait_for(lambda: current_window() == window, f"showed window {window}, still on {current_window()}")
        cls.pump_for(SETTLE)

    def setUp(self):
        rpc("GUI.ActivateWindow", window="home")
        self.wait_for_window(HOME_WINDOW)

    def test_a_video_in_the_media_directory_is_a_movie(self):
        """a file copied to the media directory is in kodi's library, named
        by its file name, with nothing set up on screen and no internet"""
        def movies():
            return [(movie["label"], movie["year"]) for movie in
                    rpc("VideoLibrary.GetMovies", properties=["year"]).get("movies", [])]
        self.wait_for(lambda: movies() == [(MOVIE, MOVIE_YEAR)], f"listed {MOVIE} as a movie, only {movies()}")

    def test_a_file_in_the_music_directory_is_a_song(self):
        def songs():
            return [song["label"] for song in rpc("AudioLibrary.GetSongs").get("songs", [])]
        self.wait_for(lambda: songs() == [SONG], f"listed {SONG} as a song, only {songs()}")

    def test_packaged_addons_are_on_without_asking(self):
        """kodi asks on its first start whether to enable each add-on the
        system's packages brought, which nobody can answer well on a car's
        screen. the class would not have started with that question up"""
        details = rpc("Addons.GetAddonDetails", addonid=PACKAGED_ADDON, properties=["enabled"])
        self.assertTrue(details["addon"]["enabled"], f"{PACKAGED_ADDON} is off")

    def test_streams_the_home_screen(self):
        frame = self.last_frame()
        colours = {frame[offset:offset + 3] for offset in range(0, len(frame), 3)}
        self.assertGreater(len(colours), MIN_COLOURS, "the screen looks blank")

    def test_touch_opens_settings(self):
        self.tap(*SETTINGS_GEAR)
        self.wait_for_window(SETTINGS_WINDOW)

    def test_back_button_returns_home(self):
        rpc("GUI.ActivateWindow", window="settings")
        self.wait_for_window(SETTINGS_WINDOW)
        self.press(fakehu.BACK)
        self.wait_for_window(HOME_WINDOW)

    def test_plays_a_clip_with_sound(self):
        """the clip's tone, played by kodi, reaches the car as media audio"""
        rpc("Player.Open", item={"file": str(self.clip)})
        self.addCleanup(lambda: rpc("Player.Stop", playerid=1))
        self.wait_for_tone(len(self.headunit.audio))

    def test_dpad_moves_focus_and_enter_opens_it(self):
        start = focused_label()
        self.press(fakehu.DPAD_DOWN)
        self.wait_for(lambda: focused_label() not in (start, None), f"moved focus down from {start}")
        self.press(fakehu.DPAD_UP)
        self.wait_for(lambda: focused_label() == start, f"moved focus back up to {start}")
        self.press(fakehu.DPAD_CENTER)
        self.wait_for(lambda: current_window() not in (HOME_WINDOW, None), f"opened {start}")


if __name__ == "__main__":
    import unittest
    unittest.main()
