"""touch and buttons from the headunit become kernel input devices through
uinput, which compositors pick up through libinput. runs as root inside the
test vm, over tcp, and repeats every session test with --uinput set. the
demo test runs the demo as a wayland client in the kiosk session, so a touch
travels uinput, sway, the client and the screen capture before it shows."""

import fcntl
import os
import pathlib
import re
import select
import struct
import subprocess
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "e2e"))
import fakehu
import test_session as tcp

# struct input_event on 64 bit: timeval, type, code, value
EVENT = struct.Struct("qqHHi")
ABSINFO = struct.Struct("6i")
EV_SYN, EV_KEY, EV_ABS = 0, 1, 3
SYN_REPORT = 0
ABS_MT_SLOT, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_TRACKING_ID = 0x2F, 0x35, 0x36, 0x39
KEY_HOMEPAGE, KEY_BACK = 172, 158
KEY_TAB, KEY_ENTER, KEY_LEFTSHIFT, KEY_UP, KEY_DOWN = 15, 28, 42, 103, 108
KEY_ESC, KEY_BACKSPACE, KEY_A, KEY_B = 1, 14, 30, 48
# longer than the server takes a held click for a long one
LONG_CLICK = 0.8
TOUCHSCREEN, KEYS = "behead touchscreen", "behead keys"
DEVICES = pathlib.Path("/proc/bus/input/devices")
KIOSK = tcp.ROOT / "sessions/kiosk.sh"
POLL = 0.05


def eviocgabs(code):
    return 0x80000000 | (ABSINFO.size << 16) | (ord("E") << 8) | (0x40 + code)


def input_devices():
    """event node by device name, for every input device the kernel knows"""
    blocks = DEVICES.read_text().split("\n\n")
    found = ((re.search(r'N: Name="(.*)"', block), re.search(r"\bevent\d+", block)) for block in blocks)
    return {name.group(1): pathlib.Path("/dev/input") / node.group() for name, node in found if name and node}


def wait_for(condition, what):
    deadline = time.monotonic() + fakehu.TIMEOUT
    while not condition():
        assert time.monotonic() < deadline, f"timed out waiting for {what}"
        time.sleep(POLL)


class Device:
    """one evdev node, folded into the state the kernel reports after each SYN"""

    def __init__(self, path):
        self.fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
        self.slot, self.slots, self.keys, self.buffer = 0, {}, set(), b""

    def close(self):
        os.close(self.fd)

    def range(self, code):
        return ABSINFO.unpack(fcntl.ioctl(self.fd, eviocgabs(code), bytes(ABSINFO.size)))[1:3]

    def apply(self, kind, code, value):
        if kind == EV_KEY:
            (self.keys.add if value else self.keys.discard)(code)
        elif kind == EV_ABS and code == ABS_MT_SLOT:
            self.slot = value
        elif kind == EV_ABS and code == ABS_MT_TRACKING_ID:
            if value < 0:
                self.slots.pop(self.slot, None)
            else:
                self.slots[self.slot] = [0, 0]
        elif kind == EV_ABS and code in (ABS_MT_POSITION_X, ABS_MT_POSITION_Y) and self.slot in self.slots:
            self.slots[self.slot][code - ABS_MT_POSITION_X] = value

    def reports(self, count):
        """contacts as (x, y) in slot order, and pressed keys, after each of the next count reports"""
        reports, deadline = [], time.monotonic() + fakehu.TIMEOUT
        while len(reports) < count:
            ready, _, _ = select.select([self.fd], [], [], max(0, deadline - time.monotonic()))
            assert ready, f"only {len(reports)} of {count} reports arrived: {reports}"
            self.buffer += os.read(self.fd, EVENT.size * 64)
            while len(self.buffer) >= EVENT.size:
                _, _, kind, code, value = EVENT.unpack_from(self.buffer)
                self.buffer = self.buffer[EVENT.size:]
                if (kind, code) == (EV_SYN, SYN_REPORT):
                    reports.append(([tuple(self.slots[slot]) for slot in sorted(self.slots)], set(self.keys)))
                else:
                    self.apply(kind, code, value)
        return reports


class UinputTest(tcp.SessionTest):
    TRANSPORT_ARGS = ["--listen", "127.0.0.1:0", "--uinput"]
    DEMO_CMD = f"{KIOSK} {{width}} {{height}} {{fps}} {tcp.DEMO} --wayland"

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        subprocess.run(["modprobe", "uinput"], check=True)

    def bound(self, **options):
        """a headunit whose input channel is open, so the devices exist"""
        headunit = self.connect(**options)
        headunit.run_until(lambda: headunit.received(fakehu.INPUT_CHANNEL, fakehu.BINDING_REQUEST))
        wait_for(lambda: {TOUCHSCREEN, KEYS} <= input_devices().keys(), "the uinput devices")
        return headunit

    def open_device(self, name):
        device = Device(input_devices()[name])
        self.addCleanup(device.close)
        return device

    def test_touchscreen_spans_the_headunit_touch_area(self):
        self.bound()
        screen = self.open_device(TOUCHSCREEN)
        self.assertEqual(screen.range(ABS_MT_POSITION_X), (0, 799))
        self.assertEqual(screen.range(ABS_MT_POSITION_Y), (0, 479))

    def test_touches_become_multitouch_contacts(self):
        headunit = self.bound()
        screen = self.open_device(TOUCHSCREEN)
        first, second = (0, 123, 456), (1, 300, 200)
        headunit.touch(fakehu.TOUCH_DOWN, [first])
        headunit.touch(fakehu.TOUCH_POINTER_DOWN, [first, second], action_index=1)
        headunit.touch(fakehu.TOUCH_MOVED, [first, (1, 310, 210)], action_index=1)
        headunit.touch(fakehu.TOUCH_POINTER_UP, [first, (1, 310, 210)], action_index=1)
        headunit.touch(fakehu.TOUCH_UP, [first])
        contacts = [contacts for contacts, _ in screen.reports(5)]
        self.assertEqual(contacts, [[(123, 456)], [(123, 456), (300, 200)], [(123, 456), (310, 210)],
                                    [(123, 456)], []])

    def test_buttons_become_keys(self):
        headunit = self.bound()
        keys = self.open_device(KEYS)
        headunit.button(fakehu.HOME, True)
        headunit.button(fakehu.HOME, False)
        headunit.button(fakehu.BACK, True)
        self.assertEqual([pressed for _, pressed in keys.reports(3)], [{KEY_HOMEPAGE}, set(), {KEY_BACK}])

    def test_a_knob_turn_moves_focus(self):
        """each detent is a key press: tab one way, shift and tab the other"""
        headunit = self.bound(keycodes=(fakehu.ROTARY,))
        keys = self.open_device(KEYS)
        headunit.turn(fakehu.ROTARY, 2)
        headunit.turn(fakehu.ROTARY, -1)
        self.assertEqual([pressed for _, pressed in keys.reports(8)],
                         [{KEY_TAB}, set(), {KEY_TAB}, set(),
                          {KEY_LEFTSHIFT}, {KEY_LEFTSHIFT, KEY_TAB}, {KEY_LEFTSHIFT}, set()])

    def test_a_knob_turn_can_be_arrow_keys(self):
        self.restart_server(options=["--knob", "arrows"])
        headunit = self.bound(keycodes=(fakehu.ROTARY,))
        keys = self.open_device(KEYS)
        headunit.turn(fakehu.ROTARY, 1)
        headunit.turn(fakehu.ROTARY, -2)
        self.assertEqual([pressed for _, pressed in keys.reports(6)],
                         [{KEY_DOWN}, set(), {KEY_UP}, set(), {KEY_UP}, set()])

    def test_a_long_click_changes_what_the_knob_is_and_it_can_type(self):
        """a car with a knob and no more: held, its click steps the knob
        from moving the focus to writing, a letter for each detent over the
        one before, kept by a click. and back is the key that closes things"""
        self.restart_server(options=["--knob", "focus,type", "--back", "escape"])
        headunit = self.bound(keycodes=(fakehu.ROTARY, fakehu.DPAD_CENTER, fakehu.BACK))
        keys = self.open_device(KEYS)
        headunit.turn(fakehu.ROTARY, 1)
        headunit.button(fakehu.BACK, True)
        headunit.button(fakehu.BACK, False)
        headunit.button(fakehu.DPAD_CENTER, True)
        time.sleep(LONG_CLICK)
        headunit.button(fakehu.DPAD_CENTER, False)
        headunit.turn(fakehu.ROTARY, 2)
        for pressed in (True, False, True, False):
            headunit.button(fakehu.DPAD_CENTER, pressed)
        typed = [key for _, pressed in keys.reports(12) for key in pressed]
        self.assertEqual(typed, [KEY_TAB, KEY_ESC, KEY_A, KEY_BACKSPACE, KEY_B, KEY_ENTER],
                         "tab and escape, then a, b in its place, kept by one click, and enter for the next")

    def test_a_car_without_a_touchscreen_gets_keys_alone(self):
        headunit = self.connect(touchscreen=None, keycodes=(fakehu.ROTARY, fakehu.DPAD_CENTER))
        headunit.run_until(lambda: headunit.received(fakehu.INPUT_CHANNEL, fakehu.BINDING_REQUEST))
        wait_for(lambda: KEYS in input_devices(), "the keys device")
        self.assertNotIn(TOUCHSCREEN, input_devices())
        keys = self.open_device(KEYS)
        headunit.turn(fakehu.ROTARY, 1)
        headunit.button(fakehu.DPAD_CENTER, True)
        self.assertEqual([pressed for _, pressed in keys.reports(3)], [{KEY_TAB}, set(), {KEY_ENTER}])

    def test_devices_leave_with_the_headunit(self):
        self.bound().sock.close()
        wait_for(lambda: not {TOUCHSCREEN, KEYS} & input_devices().keys(), "the uinput devices to go")


if __name__ == "__main__":
    import unittest
    unittest.main()
