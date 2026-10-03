"""end-to-end: the certificates `make rpi-certs` collects for the pi image"""

import pathlib
import tempfile
import unittest

from test_session import make_cert, make_rpi_certs

SELF_SIGNED = "90-self-signed"
CACHED_PREFIX = "95-"


class RpiCerts(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.certs, self.cache = pathlib.Path(tmp.name, "certs"), pathlib.Path(tmp.name, "cache")

    def collect(self):
        """the names `make rpi-certs` leaves in the image's certificate directory, in order"""
        make_rpi_certs(self.certs, self.cache)
        return sorted(path.name for path in self.certs.iterdir())

    def test_without_a_cache_only_the_self_signed_pair(self):
        self.assertEqual(self.collect(), [f"{SELF_SIGNED}.crt", f"{SELF_SIGNED}.key"])

    def test_cached_pairs_follow_the_self_signed_pair(self):
        """a pair in the cache is offered after the self-signed one. a
        certificate without a key, as the root is, stays out: the server
        refuses a directory holding one"""
        self.cache.mkdir()
        cert, key = make_cert(self.cache, "carservice")
        make_cert(self.cache, "ca")[1].unlink()
        names = self.collect()
        self.assertEqual(names, [f"{SELF_SIGNED}.crt", f"{SELF_SIGNED}.key",
                                 f"{CACHED_PREFIX}carservice.crt", f"{CACHED_PREFIX}carservice.key"])
        self.assertEqual((self.certs / names[2]).read_bytes(), cert.read_bytes())
        self.assertEqual((self.certs / names[3]).stat().st_mode & 0o777, 0o600)
        key.unlink()
        self.assertEqual(self.collect(), names[:2], "a pair gone from the cache leaves the image too")
