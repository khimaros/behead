"""a session asked for the hardware encoder on a machine without one, as the
vm is, has to show its picture all the same, encoded in software."""

import desktop
from desktop import tcp


class HardwareEncoderMissing(desktop.DesktopTest):
    @classmethod
    def video_cmd(cls):
        return f"env BEHEAD_ENCODER=v4l2m2m PATH={tcp.DEMO.parent}:$PATH {desktop.SESSIONS / 'wayland.sh'} {{width}} {{height}} {{fps}}"

    @classmethod
    def started(cls):
        return len(cls.headunit.frames) > 0

    def test_falls_back_to_software(self):
        self.assertIsNotNone(self.last_frame(), "no picture decodes")
        self.assertIn("falling back to x264", self.log.read_text())


if __name__ == "__main__":
    import unittest
    unittest.main()
