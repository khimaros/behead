#!/usr/bin/env python3
"""ask the behead on the usb cable to let go of the port, so that what had
it before, usb networking on the rpi image, comes back and the device can be
reached over the same cable.

    tools/release-usb.py [--wait SECONDS]

the server answers a control request no car sends by removing its gadget and
ending. it is found as the phone it first appears as, or as the android
accessory it becomes for a headunit. with no behead on the bus this says so
and succeeds: the port may be the network's already. needs pyusb, and access
to the device (tests/device/70-behead.rules)."""

import argparse
import sys
import time

import usb.core

# as the server appears: before a headunit asks for an accessory, and after
IDS = ((0x12D1, 0x107E), (0x18D1, 0x2D00))
VENDOR_OUT = 0x40
# the request and the value that goes with it, from crates/aap/src/accessory.rs
RELEASE, RELEASE_KEY = 0xBE, 0x6568
TIMEOUT_MS = 2000
POLL = 0.2
DEFAULT_WAIT = 10


def find():
    for vendor, product in IDS:
        device = usb.core.find(idVendor=vendor, idProduct=product)
        if device is not None:
            return device
    return None


def release(wait):
    """true once no behead is on the bus, false if one stayed past `wait` seconds"""
    device = find()
    if device is None:
        print("no behead on the usb bus: nothing to release")
        return True
    try:
        device.ctrl_transfer(VENDOR_OUT, RELEASE, RELEASE_KEY, 0, None, timeout=TIMEOUT_MS)
    except usb.core.USBError:
        pass  # it may leave the bus before the request completes
    deadline = time.monotonic() + wait
    while time.monotonic() < deadline:
        if find() is None:
            print("behead released the usb port")
            return True
        time.sleep(POLL)
    print("behead is still on the usb bus: a server from before this request existed?", file=sys.stderr)
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--wait", type=float, default=DEFAULT_WAIT, help="seconds for the server to leave the bus")
    sys.exit(0 if release(parser.parse_args().wait) else 1)


if __name__ == "__main__":
    main()
