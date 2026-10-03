"""the session tests' own footing: a session is told to stop only once the
server is gone, and takes a moment to. a test class must not clean up, or
the next one start, while its applications still run and write files."""

import os
import pathlib
import tempfile
import unittest

import desktop

# how long the stand-in session takes to stop, as kodi does saving its profile
LINGER = 1


class SlowSession(desktop.DesktopTest):
    pid_file = pathlib.Path(tempfile.gettempdir()) / "behead-slow-session.pid"

    @classmethod
    def video_cmd(cls):
        return f"echo $$ > {cls.pid_file}; trap 'sleep {LINGER}; kill 0' TERM; sleep infinity & wait"

    @classmethod
    def started(cls):
        return cls.pid_file.exists()


class StopTest(unittest.TestCase):
    def test_stopping_waits_for_the_session(self):
        SlowSession.pid_file.unlink(missing_ok=True)
        SlowSession.setUpClass()
        group = int(SlowSession.pid_file.read_text())
        SlowSession.doClassCleanups()
        with self.assertRaises(ProcessLookupError, msg="the session outlived its test class"):
            os.killpg(group, 0)


if __name__ == "__main__":
    unittest.main()
