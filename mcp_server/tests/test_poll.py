"""Combined polls must never hide consumed items or unhealthy empty channels."""

import json
from pathlib import Path
import sys
import tempfile
import threading
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mqtt_service import CoordinatorService, Mailbox, WorkerService


class WorkerPollTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.worker = WorkerService.__new__(WorkerService)
        self.worker._lock = threading.Lock()
        self.worker._local = {"pending": {"status": "pending"},
                              "approved": {"status": "approved", "task": {"uuid": "task"}},
                              "rejected": {"status": "rejected", "error": "held"}}
        self.worker._directives = {"task": {"seq": 2}}
        self.worker._client = None
        self.receipts = []
        self.worker._publish_receipt = lambda client, status, channel, ids: self.receipts.append((channel, ids))
        for key in ("assignments", "messages", "broadcasts"):
            setattr(self.worker, "_" + key, Mailbox(Path(self.scratch.name) / (key + ".json")))
        self.context = {"fleet_state": {"state": "paused", "instruction": "Stop now"},
                        "coordinator": {"status": "offline"},
                        "link": {"stale": False, "stale_since": None, "error": None}}
        self.worker._context = lambda: self.context

    def test_all_channels_requests_receipts_and_supersession_survive(self):
        self.worker._assignments.add({"message_id": "assignment", "task_uuid": "task", "seq": 1, "kind": "go"})
        self.worker._messages.add({"message_id": "message", "task_uuid": "task", "seq": 2, "kind": "hold"})
        self.worker._broadcasts.add({"message_id": "broadcast", "body": "Pause"})
        result = json.loads(self.worker.poll_worker(["pending", "approved", "rejected", "unknown"]))
        self.assertTrue(result["assignments"][0]["superseded"])
        self.assertFalse(result["messages"][0]["superseded"])
        self.assertEqual(result["broadcasts"][0]["body"], "Pause")
        self.assertEqual(result["requests"]["approved"]["task"]["uuid"], "task")
        self.assertEqual(result["requests"]["rejected"]["error"], "held")
        self.assertEqual(result["requests"]["pending"]["status"], "pending")
        self.assertEqual(result["requests"]["unknown"]["status"], "unknown")
        self.assertNotIn("link", result["requests"]["approved"])
        self.assertEqual(result["fleet_state"]["instruction"], "Stop now")
        self.assertEqual(set(channel for channel, ids in self.receipts), {"assignment", "message"})
        again = json.loads(self.worker.poll_worker())
        for key in ("assignments", "messages", "broadcasts"):
            self.assertEqual(again[key], [])

    def test_stale_empty_channels_are_errors_without_discarding_broadcasts(self):
        self.context["link"].update(stale=True, stale_since=1.0, connected=False,
                                    last_rx_at=None, age_s=None)
        self.worker._broadcasts.add({"message_id": "broadcast", "body": "Keep this"})
        result = json.loads(self.worker.poll_worker())
        self.assertEqual(set(result["channel_errors"]), {"assignments", "messages"})
        self.assertEqual(result["broadcasts"][0]["body"], "Keep this")
        self.assertEqual(result["link_stale_since"], 1.0)

    def test_acl_error_does_not_hide_a_nonempty_assignment(self):
        self.context["link"]["error"] = "subscription denied by broker ACL"
        self.worker._assignments.add({"message_id": "assignment", "body": "Read this"})
        result = json.loads(self.worker.poll_worker())
        self.assertEqual(result["assignments"][0]["body"], "Read this")
        self.assertIn("messages", result["channel_errors"])
        self.assertIn("denied", result["channel_errors"]["messages"])


class CoordinatorPollTests(unittest.TestCase):
    def test_stale_mailbox_does_not_hide_pending_requests_or_presence(self):
        with tempfile.TemporaryDirectory() as scratch:
            coordinator = CoordinatorService.__new__(CoordinatorService)
            coordinator._inbox = Mailbox(Path(scratch) / "messages.json")
            coordinator._fleet_state = {"state": "paused", "note": "Hold"}
            link = {"stale": True, "stale_since": 1.0, "error": None,
                    "last_rx_at": None, "age_s": None, "connected": False}
            class Link:
                def status(self):
                    return link
            coordinator._link = Link()
            coordinator.list_workers = lambda: json.dumps({"workers": [{"worker_id": "worker", "work_state": "idle"}], "deadman": {"enabled": False}})
            coordinator.list_pending = lambda: json.dumps({"pending": [{"request_id": "request", "op": "start", "args": {"id": "task"}}]})
            result = json.loads(coordinator.poll_coordinator())
            self.assertIn("messages", result["channel_errors"])
            self.assertEqual(result["pending"][0]["request_id"], "request")
            self.assertEqual(result["workers"][0]["work_state"], "idle")
            self.assertEqual(result["fleet_state"]["state"], "paused")


if __name__ == "__main__":
    unittest.main()
