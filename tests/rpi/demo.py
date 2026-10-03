"""an interactive headunit in the browser, for a device on a usb cable: the
device's video shows on a page, and the mouse on the picture is a finger on
the car's screen. ctrl-c stops it. needs access to the usb device nodes: see
70-behead.rules."""

import atexit
import os
import pathlib
import queue
import signal
import sys
import tempfile

import usb.core
import usb.util

# the page is the viewer the headed tests use
os.environ.setdefault("BEHEAD_HEADED", "1")
for tier in ("e2e", "vm"):
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / tier))
import fakehu
import test_session as tcp
import test_usb as usb_tier

ACTIONS = {"down": fakehu.TOUCH_DOWN, "move": fakehu.TOUCH_MOVED, "up": fakehu.TOUCH_UP}


def stop(*_):
    raise KeyboardInterrupt


def fresh_accessory():
    """the device as an accessory with no session under way. one left in
    accessory mode by a run that was killed still has its old session, which
    a bus reset ends. it stays an accessory through the reset"""
    stale = usb_tier.find(usb_tier.ACCESSORY_ID)
    if stale:
        try:
            stale.reset()
        except usb.core.USBError:
            pass  # already off the bus
        usb.util.dispose_resources(stale)
    return usb_tier.switch_to_accessory()


def main():
    signal.signal(signal.SIGTERM, stop)
    with tempfile.TemporaryDirectory() as directory:
        car = tcp.make_v1_cert(pathlib.Path(directory), "car")
        link = usb_tier.UsbLink(fresh_accessory())
        try:
            headunit = fakehu.FakeHeadunit(link, *car)
            # the tests keep their last picture up for a while; this should just stop
            atexit.unregister(headunit.viewer.linger)
            headunit.handshake()
            while True:
                # the device streams steadily, so this returns every frame
                headunit.step()
                try:
                    while True:
                        action, x, y = headunit.viewer.touches.get_nowait()
                        headunit.touch(ACTIONS[action], [(0, x, y)])
                except queue.Empty:
                    pass
        except KeyboardInterrupt:
            pass
        finally:
            link.close()


if __name__ == "__main__":
    main()
