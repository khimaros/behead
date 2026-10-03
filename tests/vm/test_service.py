"""the rpi deployment, rehearsed in the test vm: the systemd unit and config
from rpi/overlay, a live encoder, and the handover of the usb controller
with g_ether. the device tests then run against the service exactly as the
laptop would run them against a pi."""

import pathlib
import re
import shutil
import subprocess
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "rpi"))
import test_device
import test_uinput as uinput_tier
import test_usb as usb_tier
from test_device import fakehu

ROOT = pathlib.Path(__file__).resolve().parents[2]
OVERLAY = ROOT / "rpi/overlay"
UNIT = "behead.service"
DEFAULT_ENV = OVERLAY / "etc/behead/behead.env"
SESSION_ENV = pathlib.Path("/etc/behead/session.env")
GEOCLUE_CONF = ROOT / "sessions/geoclue.conf"
INSTALLED = {
    OVERLAY / "etc/systemd/system" / UNIT: pathlib.Path("/etc/systemd/system") / UNIT,
    DEFAULT_ENV: pathlib.Path("/etc/behead/behead.env"),
    ROOT / "target/debug/behead": pathlib.Path("/usr/local/bin/behead"),
    ROOT / "target/debug/behead-demo": pathlib.Path("/usr/local/bin/behead-demo"),
}
CERTS = pathlib.Path("/etc/behead/certs")
USB_ETHERNET = "g_ether"
SETTLE = 5


def run(*command):
    return subprocess.run(command, check=True, capture_output=True, text=True).stdout


def module_loaded(name):
    return any(line.split()[0] == name for line in pathlib.Path("/proc/modules").read_text().splitlines())


def systemctl(*args):
    return run("systemctl", *args)


class ServiceTest(test_device.DeviceTest):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        run("modprobe", "-a", *usb_tier.MODULES)
        for source, target in INSTALLED.items():
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(source, target)
        CERTS.mkdir(parents=True, exist_ok=True)
        run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/O=service",
            "-keyout", CERTS / "service.key", "-out", CERTS / "service.crt")
        # the base image boots with the controller given to usb ethernet
        run("modprobe", USB_ETHERNET)
        systemctl("daemon-reload")
        systemctl("start", UNIT)

    @classmethod
    def tearDownClass(cls):
        subprocess.run(["systemctl", "stop", UNIT], check=False)
        subprocess.run(["modprobe", "-r", USB_ETHERNET], check=False)
        for target in INSTALLED.values():
            target.unlink(missing_ok=True)
        shutil.rmtree(CERTS, ignore_errors=True)
        subprocess.run(["systemctl", "daemon-reload"], check=False)
        super().tearDownClass()

    def test_service_took_the_controller_from_usb_ethernet(self):
        self.assertEqual(systemctl("is-active", UNIT).strip(), "active")
        self.assertFalse(module_loaded(USB_ETHERNET))

    def test_headunit_input_becomes_kernel_devices(self):
        """desktop sessions take touch and buttons from the uinput devices"""
        headunit = self.connect()
        headunit.run_until(lambda: headunit.received(fakehu.INPUT_CHANNEL, fakehu.BINDING_REQUEST))
        uinput_tier.wait_for(lambda: {uinput_tier.TOUCHSCREEN, uinput_tier.KEYS} <= uinput_tier.input_devices().keys(),
                             "the uinput devices")

    def test_offers_the_location_socket_geoclue_reads(self):
        socket = pathlib.Path(re.search(r"^nmea-socket=(\S+)$", GEOCLUE_CONF.read_text(), re.MULTILINE).group(1))
        # another test's server may have left one behind
        systemctl("stop", UNIT)
        shutil.rmtree(socket.parent, ignore_errors=True)
        systemctl("start", UNIT)
        time.sleep(SETTLE)
        self.assertTrue(socket.is_socket(), f"no socket at {socket}")

    def test_session_env_replaces_the_video_command(self):
        """`make rpi-image SESSION=...` chooses a session by writing session.env"""
        marker = self.dir / "session-ran"
        command = re.search(r'^VIDEO_CMD="(.*)"$', DEFAULT_ENV.read_text(), re.MULTILINE).group(1)
        SESSION_ENV.write_text(f'VIDEO_CMD="touch {marker}; {command}"\n')
        self.addCleanup(systemctl, "restart", UNIT)
        self.addCleanup(SESSION_ENV.unlink)
        systemctl("restart", UNIT)
        time.sleep(SETTLE)
        self.stream()
        self.assertTrue(marker.exists(), "the service ran the default video command")

    def test_stop_hands_the_controller_back(self):
        systemctl("stop", UNIT)
        try:
            self.assertFalse(usb_tier.GADGET_DIR.exists(), "gadget left in configfs")
            self.assertTrue(module_loaded(USB_ETHERNET), "usb ethernet was not restored")
        finally:
            systemctl("start", UNIT)
            time.sleep(SETTLE)
            self.assertEqual(systemctl("is-active", UNIT).strip(), "active")


if __name__ == "__main__":
    import unittest
    unittest.main()
