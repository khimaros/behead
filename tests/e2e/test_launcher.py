"""end-to-end: the launcher as the server's video and audio source, worked
the way a car's driver would: by touch, by the knob, and by the media keys.
what plays is told by its pitch, each track being one steady tone"""

import importlib.util
import math
import os
import pathlib
import struct
import subprocess
import time
import unittest
import wave

import fakehu
import test_session as tcp

LAUNCHER = pathlib.Path(os.environ.get("BEHEAD_LAUNCHER_BIN", tcp.ROOT / "target/debug/behead-launcher"))
SESSION = tcp.ROOT / "sessions/launcher.sh"
WIDTH, HEIGHT = 800, 480
ENCODE = ("ffmpeg -loglevel error -f rawvideo -pix_fmt rgb24 -s {width}x{height} -r {fps} -i - -pix_fmt yuv420p "
          "-c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -g {fps} -f h264 -")
# where things are on a screen of that size
MUSIC_TILE, INPUT_TILE = (200, 240), (600, 240)
FIRST_LINE, SECOND_LINE = (200, 69), (200, 111)
PLAY_PAUSE = (600, 456)
# in the music tile, in the first of two rows of three, clear of its name,
# and the same spot on the music screen
TILE_SPOT = (60, 230)
# the tile the knob is on is blue, and the music screen under its two lines is not
BLUE_OVER_RED = 60
# the tracks on the card: a wav file beside a folder of an mp3, a flac and
# an opus file, the first three at a rate other than the car's
WAV_PITCH, MP3_PITCH, FLAC_PITCH, OPUS_PITCH = 440, 330, 660, 550
TRACK_RATE = 44100
TRACK_SECONDS = 6
PITCH_TOLERANCE = 8
AMPLITUDE = 8000
# how much is listened to, once what was asked for has had time to start
SETTLE, LISTEN = 1.0, 0.5
BYTES_PER_SECOND = tcp.MEDIA_BYTES_PER_SECOND
WARMUP_FRAMES = 10
# the map's tile, between the other two, and a car east of nashville, in
# degrees times ten million. the road through it reaches this many degrees
# each way, which the map as it opens draws 230 pixels long each way
MAP_TILE = (400, 240)
CAR = (360000000, -860000000)
ROAD_REACH = 0.01
# on that road beside the car, further along it, and off it
ROAD_NEAR, ROAD_FAR, OFF_ROAD = (340, 240), (220, 240), (340, 180)
# the world's map has a shore this many degrees west of the car, which is
# 276 pixels left of it, and this spot is in the sea beyond
SHORE = 0.012
AT_SEA = (40, 100)
# tools/fetch-map.py, by its path: its name is none python imports
_tool = importlib.util.spec_from_file_location("fetch_map", tcp.ROOT / "tools/fetch-map.py")
fetch_map = importlib.util.module_from_spec(_tool)
_tool.loader.exec_module(fetch_map)


def is_road(colour):
    """a main road is drawn in a yellow that is light"""
    red, _, blue = colour
    return red > 150 and red - blue > 50


def is_land(colour):
    return max(colour) < 90 and not is_sea(colour)


def is_sea(colour):
    """the sea is a dark blue, and land a grey with next to no colour"""
    red, _, blue = colour
    return blue - red > 25


def make_wav(path, pitch):
    samples = (int(AMPLITUDE * math.sin(2 * math.pi * pitch * n / TRACK_RATE)) for n in range(TRACK_RATE * TRACK_SECONDS))
    with wave.open(str(path), "wb") as file:
        file.setparams((1, 2, TRACK_RATE, 0, "NONE", "NONE"))
        file.writeframes(struct.pack(f"<{TRACK_RATE * TRACK_SECONDS}h", *samples))


def make_encoded(path, pitch, *codec):
    tcp.run("ffmpeg", "-f", "lavfi", "-i", f"sine=frequency={pitch}:sample_rate={TRACK_RATE}:duration={TRACK_SECONDS}",
            "-ac", "2", *codec, "-y", path)


class LauncherCase(tcp.ServerTest):
    """the server showing the launcher, a card of tracks, and a car connected"""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.music = cls.dir / "music"
        (cls.music / "Album").mkdir(parents=True)
        make_wav(cls.music / "tone.wav", WAV_PITCH)
        # with a tag ahead of the sound, as a file with a cover has
        make_encoded(cls.music / "Album" / "1 low.mp3", MP3_PITCH, "-c:a", "libmp3lame", "-metadata", "title=low")
        make_encoded(cls.music / "Album" / "2 high.flac", FLAC_PITCH, "-c:a", "flac")
        # opus has one rate
        make_encoded(cls.music / "Album" / "3 between.opus", OPUS_PITCH, "-c:a", "libopus", "-ar", str(fakehu.MEDIA_RATE))

    def commands(self, pipe):
        """the video and the audio command, which share a pipe that is not there yet"""
        os.mkfifo(pipe)
        launcher = (f"{LAUNCHER} --width {{width}} --height {{height}} --fps {{fps}} --music {self.music} "
                    f"--audio {pipe} | {ENCODE}")
        return launcher, f"cat {pipe}"

    def setUp(self):
        pipe = self.dir / "audio.pipe"
        pipe.unlink(missing_ok=True)
        video, audio = self.commands(pipe)
        self.start_server(video, options=["--audio-cmd", audio])
        self.addCleanup(self.stop_server)
        self.headunit = self.connect(keycodes=(fakehu.HOME, fakehu.BACK, fakehu.DPAD_CENTER, fakehu.ROTARY))
        self.headunit.run_until(lambda: len(self.headunit.frames) >= WARMUP_FRAMES and self.headunit.audio)

    def tap(self, spot):
        x, y = spot
        self.headunit.touch(fakehu.TOUCH_DOWN, [(0, x, y)])
        self.headunit.touch(fakehu.TOUCH_UP, [(0, x, y)])

    def press(self, keycode):
        self.headunit.button(keycode, True)
        self.headunit.button(keycode, False)

    def listen(self):
        """the sound arriving once what was just asked for has started, as samples of the first channel"""
        def heard():
            return sum(len(pcm) for _, pcm in self.headunit.audio)
        for seconds in (SETTLE, LISTEN):
            target = heard() + seconds * BYTES_PER_SECOND
            self.headunit.run_until(lambda: heard() >= target)
        pcm = b"".join(data for _, data in self.headunit.audio)[-int(LISTEN * BYTES_PER_SECOND):]
        return pcm

    def assert_pitch(self, pitch):
        heard = tcp.tone_frequency(self.listen(), fakehu.MEDIA_RATE, fakehu.MEDIA_CHANNELS)
        self.assertAlmostEqual(heard, pitch, delta=PITCH_TOLERANCE)

    def assert_silent(self):
        self.assertEqual(set(self.listen()), {0})

    def picture(self):
        """the latest picture the car got, as rgb"""
        seen = len(self.headunit.frames)
        self.headunit.run_until(lambda: len(self.headunit.frames) >= seen + WARMUP_FRAMES)
        stream = self.dir / "launcher.h264"
        stream.write_bytes(self.headunit.config[0] + b"".join(data for _, data in self.headunit.frames))
        raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", stream, "-fps_mode", "passthrough",
                              "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], check=True, capture_output=True).stdout
        return raw[-WIDTH * HEIGHT * 3:]

    def is_blue(self, spot):
        x, y = spot
        red, _, blue = self.picture()[(y * WIDTH + x) * 3:][:3]
        return blue - red > BLUE_OVER_RED

    def assert_blue(self, spot, blue, message):
        """the spot becomes blue, or stops being it, in the time a busy machine
        takes to draw, encode and send what was just asked for"""
        deadline = time.monotonic() + fakehu.TIMEOUT
        while self.is_blue(spot) != blue:
            self.assertLess(time.monotonic(), deadline, message)


class LauncherSessionTest(LauncherCase):
    """the launcher as the pi image runs it: `sessions/launcher.sh` for the
    picture and for the sound, with the music where the image keeps media"""

    def commands(self, pipe):
        settings = f"env BEHEAD_MEDIA={self.dir} BEHEAD_LAUNCHER_PIPE={pipe} PATH={LAUNCHER.parent}:$PATH {SESSION}"
        return f"{settings} {{width}} {{height}} {{fps}}", f"{settings} --audio {{rate}} {{channels}}"

    def test_shows_the_launcher_and_plays_its_music(self):
        self.assert_blue(TILE_SPOT, True, "the launcher's tiles are not on the screen")
        self.tap(MUSIC_TILE)
        self.tap(SECOND_LINE)
        self.assert_pitch(WAV_PITCH)


class LauncherTest(LauncherCase):
    def test_plays_and_pauses_a_track_by_touch(self):
        self.assert_silent()
        self.tap(MUSIC_TILE)
        self.tap(SECOND_LINE)
        self.assert_pitch(WAV_PITCH)
        self.tap(PLAY_PAUSE)
        self.assert_silent()
        self.tap(PLAY_PAUSE)
        self.assert_pitch(WAV_PITCH)

    def test_plays_mp3_flac_and_opus_from_a_folder_by_the_knob_and_keys(self):
        """a car without a touchscreen: a click opens the tile the knob is
        on, then the folder, then its first track, and a turn reaches the next"""
        for _ in range(3):
            self.press(fakehu.DPAD_CENTER)
        self.assert_pitch(MP3_PITCH)
        for pitch in (FLAC_PITCH, OPUS_PITCH):
            self.headunit.turn(fakehu.ROTARY, 1)
            self.press(fakehu.DPAD_CENTER)
            self.assert_pitch(pitch)

    def test_goes_on_to_the_next_track_when_one_ends(self):
        for spot in (MUSIC_TILE, FIRST_LINE, FIRST_LINE):
            self.tap(spot)
        self.assert_pitch(MP3_PITCH)
        end = time.monotonic() + TRACK_SECONDS
        self.headunit.run_until(lambda: time.monotonic() >= end, timeout=TRACK_SECONDS + fakehu.TIMEOUT)
        self.assert_pitch(FLAC_PITCH)

    def test_shows_the_screen_asked_for(self):
        self.assert_blue(TILE_SPOT, True, "the knob starts on the music tile")
        self.tap(MUSIC_TILE)
        self.assert_blue(TILE_SPOT, False, "the music screen did not open")
        self.press(fakehu.HOME)
        self.assert_blue(TILE_SPOT, True, "home did not bring the tiles back")
        self.headunit.turn(fakehu.ROTARY, 1)
        self.assert_blue(TILE_SPOT, False, "the knob did not move to the other tile")


class LauncherMapTest(LauncherCase):
    """the launcher's map: the roads around the car, from a file
    tools/fetch-map.py makes of openstreetmap's. here of one road, west to
    east through where the car says it is, as openstreetmap would give it"""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        latitude, longitude = (degrees / 1e7 for degrees in CAR)
        road = {"type": "way", "tags": {"highway": "primary", "name": "Main Street"},
                "geometry": [{"lat": latitude, "lon": longitude - ROAD_REACH},
                             {"lat": latitude, "lon": longitude + ROAD_REACH}]}
        area = (longitude - 2 * ROAD_REACH, latitude - 2 * ROAD_REACH,
                longitude + 2 * ROAD_REACH, latitude + 2 * ROAD_REACH)
        (cls.dir / "maps").mkdir()
        (cls.dir / "maps/here.bmap").write_bytes(fetch_map.build(area, [road]))
        # and a map of the world, as natural earth would give its land: here
        # one piece of it, whose west shore is a little west of the car
        shore = longitude - SHORE
        land = [[shore, latitude - 1], [longitude + 1, latitude - 1], [longitude + 1, latitude + 1],
                [shore, latitude + 1], [shore, latitude - 1]]
        layer = {"features": [{"geometry": {"type": "Polygon", "coordinates": [land]}}]}
        (cls.dir / "maps/world.bmap").write_bytes(fetch_map.build_world([(layer, fetch_map.LAND)]))

    def commands(self, pipe):
        video, audio = super().commands(pipe)
        return video.replace(" --music ", f" --maps {self.dir / 'maps'} --music "), audio

    def colour(self, spot):
        x, y = spot
        return tuple(self.picture()[(y * WIDTH + x) * 3:][:3])

    def shows(self, spot, wanted, message):
        deadline = time.monotonic() + fakehu.TIMEOUT
        while not wanted(self.colour(spot)):
            self.assertLess(time.monotonic(), deadline, f"{message}: {self.colour(spot)} at {spot}")

    def test_shows_the_road_the_car_is_on_and_the_knob_zooms(self):
        self.tap(MAP_TILE)
        self.headunit.run_until(lambda: fakehu.SENSOR_LOCATION in self.headunit.sensors_started)
        self.headunit.sense(fakehu.SENSOR_LOCATION, None, *CAR)
        self.shows(ROAD_NEAR, is_road, "no road beside the car")
        self.shows(ROAD_FAR, is_road, "the road ends short of where it goes")
        self.shows(OFF_ROAD, is_land, "something where there is only land")
        self.shows(AT_SEA, is_sea, "land west of the shore the map of the world has there")
        # one detent back: everything is half as far from the car, the shore too
        self.headunit.turn(fakehu.ROTARY, -1)
        self.shows(ROAD_FAR, is_sea, "the map is no smaller for a turn of the knob")
        self.shows(ROAD_NEAR, is_road, "the road is gone with the map made smaller")


if __name__ == "__main__":
    unittest.main()
