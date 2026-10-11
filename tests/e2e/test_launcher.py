"""end-to-end: the launcher as the server's video and audio source, worked
the way a car's driver would: by touch, by the knob, and by the media keys.
what plays is told by its pitch, each track being one steady tone"""

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
WIDTH, HEIGHT = 800, 480
ENCODE = ("ffmpeg -loglevel error -f rawvideo -pix_fmt rgb24 -s {width}x{height} -r {fps} -i - -pix_fmt yuv420p "
          "-c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -g {fps} -f h264 -")
# where things are on a screen of that size
MUSIC_TILE, INPUT_TILE = (200, 240), (600, 240)
FIRST_LINE, SECOND_LINE = (200, 69), (200, 111)
PLAY_PAUSE = (600, 456)
# in the music tile, clear of its name, and the same spot on the music screen
TILE_SPOT = (60, 400)
# the tile the knob is on is blue, and the footer of the music screen is not
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


def make_wav(path, pitch):
    samples = (int(AMPLITUDE * math.sin(2 * math.pi * pitch * n / TRACK_RATE)) for n in range(TRACK_RATE * TRACK_SECONDS))
    with wave.open(str(path), "wb") as file:
        file.setparams((1, 2, TRACK_RATE, 0, "NONE", "NONE"))
        file.writeframes(struct.pack(f"<{TRACK_RATE * TRACK_SECONDS}h", *samples))


def make_encoded(path, pitch, *codec):
    tcp.run("ffmpeg", "-f", "lavfi", "-i", f"sine=frequency={pitch}:sample_rate={TRACK_RATE}:duration={TRACK_SECONDS}",
            "-ac", "2", *codec, "-y", path)


class LauncherTest(tcp.ServerTest):
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

    def setUp(self):
        pipe = self.dir / "audio.pipe"
        pipe.unlink(missing_ok=True)
        os.mkfifo(pipe)
        launcher = (f"{LAUNCHER} --width {{width}} --height {{height}} --fps {{fps}} --music {self.music} "
                    f"--audio {pipe} | {ENCODE}")
        self.start_server(launcher, options=["--audio-cmd", f"cat {pipe}"])
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
        self.assertTrue(self.is_blue(TILE_SPOT), "the knob starts on the music tile")
        self.tap(MUSIC_TILE)
        self.assertFalse(self.is_blue(TILE_SPOT), "the music screen did not open")
        self.press(fakehu.HOME)
        self.assertTrue(self.is_blue(TILE_SPOT), "home did not bring the tiles back")
        self.headunit.turn(fakehu.ROTARY, 1)
        self.assertFalse(self.is_blue(TILE_SPOT), "the knob did not move to the other tile")


if __name__ == "__main__":
    unittest.main()
