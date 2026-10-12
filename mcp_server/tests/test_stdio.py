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
            self.assertEqual(len((await client.list_tools()).tools), 13)
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

    async def test_simultaneous_adds_return_their_own_tasks(self):
        async with Client(self.parameters()) as client:
            tasks = await asyncio.gather(*(self.task(client, "add_task", title=f"Task {n}")
                                           for n in range(8)))
            self.assertEqual([task["title"] for task in tasks], [f"Task {n}" for n in range(8)])
            self.assertEqual(len({task["uuid"] for task in tasks}), 8)


if __name__ == "__main__":
    unittest.main()
