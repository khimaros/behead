"""end-to-end: which h.264 encoder a session picks. the hardware encoder is
tried with a short encode and used where that works, as on a pi 4"""

import os
import pathlib
import subprocess
import tempfile
import unittest

from test_session import ROOT

ENCODER = ROOT / "sessions/encoder.sh"
MODE, SMALL_MODE = "1280 720 30", "800 480 30"
# stands in for ffmpeg: notes what it was asked and ends as the test says
FFMPEG_STUB = '#!/bin/sh\necho "$@" > "$(dirname "$0")/asked"\nexit {status}\n'


class EncoderChoice(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = pathlib.Path(tmp.name)

    def chosen(self, trial_works, mode=MODE, **env):
        """what a session would encode with, given how the trial encode goes"""
        ffmpeg = self.dir / "ffmpeg"
        ffmpeg.write_text(FFMPEG_STUB.format(status=0 if trial_works else 1))
        ffmpeg.chmod(0o755)
        inherited = {name: value for name, value in os.environ.items() if name != "BEHEAD_ENCODER"}
        return subprocess.run(["sh", "-c", f". {ENCODER}; encoder {mode}"], check=True, capture_output=True, text=True,
                              env={**inherited, "PATH": f"{self.dir}:{os.environ['PATH']}", **env}).stdout.strip()

    def test_the_hardware_encoder_is_used_where_it_works(self):
        self.assertEqual(self.chosen(True), "v4l2m2m")
        asked = (self.dir / "asked").read_text()
        self.assertIn("h264_v4l2m2m", asked)
        self.assertIn("1280x720", asked)

    def test_software_is_the_fallback(self):
        self.assertEqual(self.chosen(False), "x264")

    def test_a_small_picture_stays_in_software(self):
        """where x264 costs little, the hardware would only add its delay"""
        self.assertEqual(self.chosen(True, mode=SMALL_MODE), "x264")
        self.assertFalse((self.dir / "asked").exists(), "the hardware was tried for nothing")

    def test_the_choice_can_be_named(self):
        self.assertEqual(self.chosen(True, BEHEAD_ENCODER="x264"), "x264")
        self.assertEqual(self.chosen(False, BEHEAD_ENCODER="v4l2m2m"), "v4l2m2m")
        self.assertFalse((self.dir / "asked").exists(), "a named encoder needs no trial")


if __name__ == "__main__":
    unittest.main()
