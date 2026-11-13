"""end-to-end: the certificates `make certs-rpi` collects for the pi image"""

import pathlib
import tempfile
import unittest

from test_session import make_cert, make_rpi_certs

SELF_SIGNED = "90-self-signed"
CACHED_PREFIX = "50-"


class RpiCerts(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.certs, self.cache = pathlib.Path(tmp.name, "certs"), pathlib.Path(tmp.name, "cache")

    def collect(self, *variables):
        """the names `make certs-rpi` leaves in the image's certificate directory, in order"""
        make_rpi_certs(self.certs, self.cache, *variables)
        return sorted(path.name for path in self.certs.iterdir())

    def test_without_a_cache_only_the_self_signed_pair(self):
        self.assertEqual(self.collect(), [f"{SELF_SIGNED}.crt", f"{SELF_SIGNED}.key"])

    def test_phone_cert_is_for_the_esp32_alone(self):
        """the RPi4 takes extra pairs from the cache only. PHONE_CERT names
        the one certificate the ESP32-P4 firmware is built with"""
        cert, key = make_cert(pathlib.Path(self.certs.parent), "phone")
        names = self.collect(f"PHONE_CERT={cert}", f"PHONE_KEY={key}")
        self.assertEqual(names, [f"{SELF_SIGNED}.crt", f"{SELF_SIGNED}.key"])

    def test_cached_pairs_precede_the_self_signed_pair(self):
        """a pair in the cache is offered before the self-signed one: the RPi4
        forgets the accepted certificate at every boot, so a strict car would
        otherwise refuse the first connection after each. a certificate
        without a key, as the root is, stays out: the server refuses a
        directory holding one"""
        self.cache.mkdir()
        cert, key = make_cert(self.cache, "carservice")
        make_cert(self.cache, "ca")[1].unlink()
        names = self.collect()
        self.assertEqual(names, [f"{CACHED_PREFIX}carservice.crt", f"{CACHED_PREFIX}carservice.key",
                                 f"{SELF_SIGNED}.crt", f"{SELF_SIGNED}.key"])
        self.assertEqual((self.certs / names[0]).read_bytes(), cert.read_bytes())
        self.assertEqual((self.certs / names[1]).stat().st_mode & 0o777, 0o600)
        key.unlink()
        self.assertEqual(self.collect(), names[2:], "a pair gone from the cache leaves the image too")
