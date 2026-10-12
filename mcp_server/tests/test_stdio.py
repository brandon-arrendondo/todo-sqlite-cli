"""MCP wire regression checks using only throwaway task databases."""

import asyncio
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from mcp import Client, StdioServerParameters

SERVER = Path(__file__).resolve().parents[1] / "server.py"


class StdioTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.binary = os.environ.get("TODO_SQLITE_CLI_BIN") or shutil.which("todo-sqlite-cli")
        if not self.binary:
            self.skipTest("build the CLI or set TODO_SQLITE_CLI_BIN")
        self.db = str(Path(self.scratch.name) / "tasks.db")
        subprocess.run([self.binary, "--db", self.db, "init"], check=True, capture_output=True)

    def parameters(self):
        env = {**os.environ, "TODO_SQLITE_CLI_DB": self.db, "TODO_SQLITE_CLI_BIN": self.binary}
        env.pop("TODO_SQLITE_CLI_MQTT_CONFIG", None)
        return StdioServerParameters(command=sys.executable, args=[str(SERVER)], env=env,
                                     cwd=self.scratch.name)

    async def task(self, client, tool, **arguments):
        result = await client.call_tool(tool, arguments)
        self.assertFalse(result.is_error, result.content)
        return json.loads(result.content[0].text)

    async def exercise(self, mode):
        async with Client(self.parameters(), mode=mode) as client:
            self.assertEqual(len((await client.list_tools()).tools), 14)
            added = await self.task(client, "add_task", title="Wire task", details="Original")
            uuid = added["uuid"]
            edited = await self.task(client, "edit_task", id=uuid, append_details="Progress")
            self.assertEqual(edited["details"], "Original\nProgress")
            await self.task(client, "start_task", id=uuid)
            shown = await self.task(client, "show_task", id=uuid)
            self.assertEqual(shown["status"], "in-progress")
            listed = await self.task(client, "list_tasks", limit=1)
            self.assertEqual(listed["tasks"][0]["uuid"], uuid)
            await self.task(client, "done_task", id=uuid)
            deleted = await self.task(client, "rm_task", id=uuid)
            self.assertEqual(deleted, {"deleted": uuid})
            failed = await client.call_tool("show_task", {"id": uuid})
            self.assertTrue(failed.is_error)
            self.assertIn(uuid, failed.content[0].text)
            self.assertNotEqual(failed.content[0].text, "Error executing tool show_task")

    async def test_modern_stdio_read_write_and_errors(self):
        await self.exercise("auto")

    async def test_legacy_stdio_read_write_and_errors(self):
        await self.exercise("legacy")

    async def test_compact_list_preserves_selection_and_full_details(self):
        async with Client(self.parameters()) as client:
            details = "Long progress history. " * 100
            added = await self.task(client, "add_task", title="Compact task", details=details,
                                    tags=["review"], project_name="example", location="desk")
            await self.task(client, "add_task", title="Other task", project_name="other")
            arguments = {"project_name": "example", "tags": ["review"], "limit": 1}
            full = await self.task(client, "list_tasks", **arguments)
            explicit_full = await self.task(client, "list_tasks", compact=False, **arguments)
            self.assertEqual(full, explicit_full)
            compact = await self.task(client, "list_tasks", compact=True, **arguments)
            self.assertEqual(len(compact["tasks"]), 1)
            summary = compact["tasks"][0]
            self.assertNotIn("details", summary)
            self.assertNotIn("created_at", summary)
            for key, value in summary.items():
                self.assertEqual(value, full["tasks"][0][key])
            for key in ("uuid", "id", "project_name", "blocked", "depends_on", "is_gate",
                        "location", "implementation_client", "tags"):
                self.assertIn(key, summary)
            shown = await self.task(client, "show_task", id=added["uuid"])
            self.assertEqual(shown["details"], details)
            empty = await self.task(client, "list_tasks", compact=True, project_name="missing")
            self.assertEqual(empty, {"tasks": []})

    async def test_ambiguous_id_error_preserves_uuid_choices(self):
        async with Client(self.parameters()) as client:
            first = await self.task(client, "add_task", title="First", project_name="one")
            second = await self.task(client, "add_task", title="Second", project_name="two")
            await self.task(client, "renumber_task", id=second["uuid"], new_id=first["id"], force=True)
            failed = await client.call_tool("show_task", {"id": first["id"]})
            self.assertTrue(failed.is_error)
            for task in (first, second):
                self.assertIn(task["uuid"], failed.content[0].text)
            shown = await self.task(client, "show_task", id=second["uuid"])
            self.assertEqual(shown["title"], "Second")

    async def test_batch_details_preserve_successes_and_errors(self):
        async with Client(self.parameters()) as client:
            task = await self.task(client, "add_task", title="Plan candidate", details="Full body")
            batch = await self.task(client, "show_tasks", ids=[task["uuid"], "missing-task"])
            self.assertEqual(batch["tasks"][0]["details"], "Full body")
            self.assertEqual(batch["errors"][0]["id"], "missing-task")
            self.assertTrue(batch["errors"][0]["error"])
            over_limit = await client.call_tool("show_tasks", {"ids": [task["uuid"]] * 51})
            self.assertTrue(over_limit.is_error)
            empty = await self.task(client, "show_tasks", ids=[])
            self.assertEqual(empty, {"tasks": [], "errors": []})

    async def test_simultaneous_adds_return_their_own_tasks(self):
        async with Client(self.parameters()) as client:
            tasks = await asyncio.gather(*(self.task(client, "add_task", title=f"Task {n}")
                                           for n in range(8)))
            self.assertEqual([task["title"] for task in tasks], [f"Task {n}" for n in range(8)])
            self.assertEqual(len({task["uuid"] for task in tasks}), 8)


if __name__ == "__main__":
    unittest.main()
