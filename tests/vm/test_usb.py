"""end-to-end over usb. runs as root inside the test vm, where the kernel's
dummy controller loops the server's gadget back to a host port. the test
plays the headunit: it performs the android open accessory switch, then
runs every tcp session test over the bulk endpoints."""

import pathlib
import signal
import subprocess
import sys
import time

import usb.core
import usb.util

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "e2e"))
import fakehu
import test_session as tcp

INITIAL_ID = (0x12D1, 0x107E)
ACCESSORY_ID = (0x18D1, 0x2D00)
GADGET_DIR = pathlib.Path("/sys/kernel/config/usb_gadget/behead")
FFS_MOUNT = pathlib.Path("/run/behead-ffs")
MODULES = ["dummy_hcd", "libcomposite"]
VENDOR_IN, VENDOR_OUT = 0xC0, 0x40
AOA_GET_PROTOCOL, AOA_SEND_STRING, AOA_START = 51, 52, 53
AOA_STRINGS = ["Android", "Android Auto", "Android Auto", "2.0.1", "https://example.org", "HU-0001"]
USB_TIMEOUT = 10
POLL = 0.1
# how long the server waits after a disable before deciding it was an unplug
UNPLUG_GRACE = 2
BOUNDARY_FRAMES = 512
BOUNDARY_PERIOD = 0.02
# a frame held back for the next write arrives about two periods late
BOUNDARY_HELD = 1.8
# a paced stream of single-slice pictures, one of each size from 1000 bytes up.
# 0xff filler cannot form a start code, and 0x80 marks each slice as a new picture.
PACKET_BOUNDARY_SOURCE = f"""
import sys, time
start = time.monotonic()
for index, size in enumerate(range(1000, 1000 + {BOUNDARY_FRAMES})):
    sys.stdout.buffer.write(b"\\0\\0\\0\\1\\x41\\x80" + b"\\xff" * (size - 6))
    sys.stdout.buffer.flush()
    time.sleep(max(0, start + (index + 1) * {BOUNDARY_PERIOD} - time.monotonic()))
"""


def find(ids):
    return usb.core.find(idVendor=ids[0], idProduct=ids[1])


def wait_for(condition, what, timeout=USB_TIMEOUT):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = condition()
        if result:
            return result
        time.sleep(POLL)
    raise AssertionError(f"timed out waiting for {what}")


def switch_to_accessory():
    """do what a headunit does on plug-in, and return the accessory device.
    like a headunit, use a device that is already an accessory as it is."""
    device = wait_for(lambda: find(ACCESSORY_ID) or find(INITIAL_ID), "the gadget to enumerate")
    if (device.idVendor, device.idProduct) == ACCESSORY_ID:
        return device
    version = device.ctrl_transfer(VENDOR_IN, AOA_GET_PROTOCOL, 0, 0, 2, timeout=USB_TIMEOUT * 1000)
    assert bytes(version) == b"\x02\x00", f"unexpected accessory protocol version {bytes(version)!r}"
    for index, text in enumerate(AOA_STRINGS):
        device.ctrl_transfer(VENDOR_OUT, AOA_SEND_STRING, 0, index, text.encode() + b"\0", timeout=USB_TIMEOUT * 1000)
    try:
        device.ctrl_transfer(VENDOR_OUT, AOA_START, 0, 0, None, timeout=USB_TIMEOUT * 1000)
    except usb.core.USBError:
        pass  # the device may drop off the bus before the request completes
    usb.util.dispose_resources(device)
    return wait_for(lambda: find(ACCESSORY_ID), "the accessory to enumerate")


class UsbLink:
    """the accessory's bulk endpoints behind the socket methods FakeHeadunit uses"""

    def __init__(self, device):
        self.device, self.timeout_ms = device, USB_TIMEOUT * 1000
        device.set_configuration()
        interface = device.get_active_configuration()[(0, 0)]
        inbound = lambda e: usb.util.endpoint_direction(e.bEndpointAddress) == usb.util.ENDPOINT_IN
        self.read_endpoint = usb.util.find_descriptor(interface, custom_match=inbound)
        self.write_endpoint = usb.util.find_descriptor(interface, custom_match=lambda e: not inbound(e))

    def settimeout(self, seconds):
        self.timeout_ms = int(seconds * 1000)

    def recv(self, count):
        try:
            return bytes(self.read_endpoint.read(count, self.timeout_ms))
        except usb.core.USBTimeoutError as error:
            raise TimeoutError from error

    def sendall(self, data):
        self.write_endpoint.write(data, self.timeout_ms)

    def close(self):
        """a bus reset ends the session and sends the gadget back to its initial identity"""
        try:
            self.device.reset()
        except usb.core.USBError:
            pass  # already off the bus
        usb.util.dispose_resources(self.device)


class UsbTest(tcp.SessionTest):
    TRANSPORT_ARGS = ["--usb", "auto"]
    READY = "usb gadget ready"

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        subprocess.run(["modprobe", "-a", *MODULES], check=True)

    def stop_server(self):
        self.server.send_signal(signal.SIGTERM)
        self.server.wait(timeout=USB_TIMEOUT)
        self.server.stdout.close()
        self.server.stderr.close()

    def open_link(self):
        return UsbLink(switch_to_accessory())

    def expect_closed(self, headunit):
        """usb has no close to observe; a finished session leaves the link up"""

    def assert_no_gadget(self):
        self.assertFalse(GADGET_DIR.exists(), "gadget left in configfs")
        self.assertFalse(FFS_MOUNT.exists(), "functionfs mountpoint left behind")
        self.assertNotIn(str(FFS_MOUNT), pathlib.Path("/proc/mounts").read_text())
        wait_for(lambda: not find(INITIAL_ID) and not find(ACCESSORY_ID), "the gadget to leave the bus")

    def test_writes_on_a_packet_boundary_are_not_held_back(self):
        """a usb transfer that fills its last packet exactly needs a zero length
        packet after it, or a host reading with a larger buffer waits for the
        next one. frames of 512 consecutive sizes make some writes land there."""
        generator = self.dir / "sizes.py"
        generator.write_text(PACKET_BOUNDARY_SOURCE)
        self.restart_server(f"python3 {generator}")
        headunit = self.connect()
        headunit.run_until(lambda: len(headunit.frames) >= BOUNDARY_FRAMES - 1, timeout=BOUNDARY_FRAMES * BOUNDARY_PERIOD * 3)
        gaps = [later - earlier for earlier, later in zip(headunit.arrivals, headunit.arrivals[1:])]
        held = [(index + 1, round(gap * 1000)) for index, gap in enumerate(gaps) if gap > BOUNDARY_HELD * BOUNDARY_PERIOD]
        self.assertEqual(held, [], "frames that waited for the next write (frame, gap ms)")

    def accessory_in_session(self):
        """switch to accessory mode and wait until the server counts the session as live"""
        device = switch_to_accessory()
        self.read_output(self.server.stderr, b"headunit connected")
        return device

    def test_stays_an_accessory_when_the_host_reauthorizes_it(self):
        """usbguard and the kernel may deauthorize and reauthorize a new device,
        which the gadget sees as disable then enable. that is not an unplug."""
        device = self.accessory_in_session()
        authorized = pathlib.Path(f"/sys/bus/usb/devices/{device.bus}-{'.'.join(map(str, device.port_numbers))}/authorized")
        usb.util.dispose_resources(device)
        authorized.write_text("0")
        authorized.write_text("1")
        time.sleep(UNPLUG_GRACE * 2)
        self.assertTrue(find(ACCESSORY_ID), "the gadget left accessory mode")
        headunit = self.connect()
        headunit.run_until(lambda: len(headunit.frames) == tcp.FRAME_COUNT)

    def test_returns_to_its_first_identity_when_unplugged(self):
        usb.util.dispose_resources(self.accessory_in_session())
        udc = next(pathlib.Path("/sys/class/udc").iterdir())
        (udc / "soft_connect").write_text("disconnect")
        wait_for(lambda: find(INITIAL_ID), "the gadget to return to its first identity", timeout=UNPLUG_GRACE + USB_TIMEOUT)

    def test_sigterm_removes_the_gadget(self):
        self.connect()
        self.server.send_signal(signal.SIGTERM)
        self.assertEqual(self.server.wait(timeout=USB_TIMEOUT), 0)
        self.assert_no_gadget()

    def test_teardown_command_cleans_up_after_a_crash(self):
        self.connect()
        self.server.kill()
        self.server.wait()
        self.assertTrue(GADGET_DIR.exists(), "a killed server cannot clean up after itself")
        subprocess.run([tcp.SERVER, "teardown"], check=True)
        self.assert_no_gadget()

    def test_restarts_over_a_stale_gadget(self):
        tcp.SessionTest.stop_server(self)
        self.start_server()
        headunit = self.connect()
        headunit.run_until(lambda: len(headunit.frames) == tcp.FRAME_COUNT)


if __name__ == "__main__":
    import unittest
    unittest.main()
