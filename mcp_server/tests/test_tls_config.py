"""The MQTT config's TLS settings: an optional private CA bundle, never a silent fallback."""

import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import mqtt_service
from mqtt_service import CONFIG_ENV, Config, load_config

BASE = {
    "mode": "worker",
    "host": "broker.example.internal",
    "port": 8883,
    "client_id": "node-1",
    "topic_prefix": "todo/test",
    "worker_db_path": "/nonexistent/replica.db",
}


class LoadConfigCaCertsTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.dir = Path(scratch.name)
        self.ca = self.dir / "ca.pem"
        self.ca.write_text("-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n")

    def load(self, **fields):
        path = self.dir / "mqtt.json"
        path.write_text(json.dumps({**BASE, **fields}))
        with patch.dict(os.environ, {CONFIG_ENV: str(path)}):
            return load_config()

    def test_unset_means_system_trust(self):
        self.assertIsNone(self.load().ca_certs)
        self.assertIsNone(self.load(ca_certs=None).ca_certs)

    def test_set_path_is_kept(self):
        self.assertEqual(self.load(ca_certs=str(self.ca)).ca_certs, str(self.ca))

    def test_tilde_is_expanded(self):
        with patch.dict(os.environ, {"HOME": str(self.dir)}):
            self.assertEqual(self.load(ca_certs="~/ca.pem").ca_certs, str(self.ca))

    def test_missing_file_fails_naming_the_path(self):
        missing = self.dir / "nope" / "ca.pem"
        with self.assertRaises(RuntimeError) as cm:
            self.load(ca_certs=str(missing))
        self.assertIn(str(missing), str(cm.exception))
        self.assertIn("ca_certs", str(cm.exception))

    def test_unreadable_file_fails(self):
        if os.geteuid() == 0:
            self.skipTest("root can read a mode-000 file")
        self.ca.chmod(0)
        self.addCleanup(self.ca.chmod, 0o600)
        with self.assertRaises(RuntimeError) as cm:
            self.load(ca_certs=str(self.ca))
        self.assertIn(str(self.ca), str(cm.exception))

    def test_directory_fails(self):
        with self.assertRaises(RuntimeError) as cm:
            self.load(ca_certs=str(self.dir))
        self.assertIn(str(self.dir), str(cm.exception))

    def test_non_string_or_empty_fails(self):
        for bad in (True, 5, "", "  "):
            with self.subTest(bad=bad), self.assertRaises(RuntimeError):
                self.load(ca_certs=bad)

    def test_ignored_with_warning_when_tls_off(self):
        with patch.object(mqtt_service, "_warn") as warn:
            config = self.load(tls=False, ca_certs=str(self.ca))
        self.assertFalse(config.tls)
        warn.assert_called_once()
        self.assertIn("ca_certs", warn.call_args.args[0])


class MakeClientTlsTests(unittest.TestCase):
    def make(self, **fields):
        client = MagicMock()
        with patch.object(mqtt_service.mqtt, "Client", return_value=client):
            mqtt_service._make_client(Config(**{**BASE, **fields}), on_connect=None, on_message=None)
        return client

    def test_ca_certs_is_passed_to_tls_set(self):
        client = self.make(ca_certs="/etc/ipa/ca.crt")
        client.tls_set.assert_called_once_with(ca_certs="/etc/ipa/ca.crt")

    def test_system_trust_without_ca_certs(self):
        client = self.make()
        client.tls_set.assert_called_once_with()

    def test_no_tls_means_no_tls_set(self):
        client = self.make(tls=False, ca_certs="/etc/ipa/ca.crt")
        client.tls_set.assert_not_called()

    def test_hostname_verification_is_not_disabled(self):
        client = self.make(ca_certs="/etc/ipa/ca.crt")
        client.tls_insecure_set.assert_not_called()


if __name__ == "__main__":
    unittest.main()
