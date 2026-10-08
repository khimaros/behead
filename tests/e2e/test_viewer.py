"""end-to-end: the browser page behind HEADED=1 as a headunit's controls.
what the page asks for over http has to come out as input for a headunit"""

import atexit
import socket
import unittest
import urllib.request

import fakehu
import viewer


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class ViewerControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.port = free_port()
        cls.viewer = viewer.Viewer(cls.port)
        # nobody is watching this one
        atexit.unregister(cls.viewer.linger)
        cls.addClassCleanup(cls.viewer.decoder.kill)

    def ask(self, path):
        with urllib.request.urlopen(f"http://127.0.0.1:{self.port}{path}", timeout=fakehu.TIMEOUT) as response:
            return response.status

    def test_the_mouse_is_a_finger(self):
        self.ask("/touch?action=down&x=12&y=34")
        self.assertEqual(self.viewer.touches.get(timeout=fakehu.TIMEOUT), ("down", 12, 34))

    def test_keys_are_the_cars_buttons_and_the_wheel_its_knob(self):
        for path in (f"/button?code={fakehu.DPAD_LEFT}&pressed=1", f"/button?code={fakehu.DPAD_LEFT}&pressed=0",
                     "/turn?delta=-2"):
            self.ask(path)
        asked = [self.viewer.controls.get(timeout=fakehu.TIMEOUT) for _ in range(3)]
        self.assertEqual(asked, [("button", fakehu.DPAD_LEFT, True), ("button", fakehu.DPAD_LEFT, False),
                                 ("turn", fakehu.ROTARY, -2)])

    def test_the_page_offers_every_key_the_headunit_should_advertise(self):
        """a car only gets bound the keys it listed, so the page's keys and
        the list a headunit is created with have to agree"""
        page = viewer.PAGE.decode()
        self.assertEqual(viewer.ROTARY, fakehu.ROTARY)
        for code in viewer.KEYS.values():
            self.assertIn(code, viewer.KEYCODES)
        self.assertIn(fakehu.ROTARY, viewer.KEYCODES)
        self.assertIn('"ArrowLeft":21', page.replace(" ", ""))


if __name__ == "__main__":
    unittest.main()
