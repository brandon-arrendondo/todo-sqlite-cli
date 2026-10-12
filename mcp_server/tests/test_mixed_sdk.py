"""SDK-only interoperability against the pre-migration server on temporary DBs.

Set TODO_MCP_V1_PYTHON to a Python with mcp<2 and paho-mqtt. The driver
requires SDK v2, paho-mqtt, mosquitto and the built CLI. No fleet access.
"""

import asyncio
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from mqtt_fleet_scenario import Scenario, SERVER_DIR, find_prereqs


V1_CLIENT_CODE = '''
import asyncio, json, os, sys
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
async def main():
    python, server, binary, db, cwd = sys.argv[1:]
    env = {**os.environ, "TODO_SQLITE_CLI_DB": db, "TODO_SQLITE_CLI_BIN": binary}
    env.pop("TODO_SQLITE_CLI_MQTT_CONFIG", None)
    params = StdioServerParameters(command=python, args=[server], env=env, cwd=cwd)
    async with stdio_client(params) as (r, w):
        async with ClientSession(r, w) as client:
            await client.initialize()
            assert "show_task" in {t.name for t in (await client.list_tools()).tools}
            added = await client.call_tool("add_task", {"title": "Legacy client"})
            assert not added.isError
            task = json.loads(added.content[0].text)
            shown = await client.call_tool("show_task", {"id": task["uuid"]})
            assert json.loads(shown.content[0].text)["title"] == "Legacy client"
            missing = await client.call_tool("show_task", {"id": "missing-task"})
            assert missing.isError and "missing-task" in missing.content[0].text
asyncio.run(main())
'''


class MixedSDKTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.v1_python = os.environ.get("TODO_MCP_V1_PYTHON")
        if not self.v1_python:
            self.skipTest("set TODO_MCP_V1_PYTHON to an SDK v1 environment")
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.workdir = Path(self.scratch.name)
        try:
            self.prereqs = find_prereqs()
        except SystemExit:
            self.skipTest("mixed-SDK broker, dependency or CLI prerequisites missing")
        self.prereqs["compat_python"] = self.v1_python

    async def test_actual_v1_client_reads_writes_and_reports_errors(self):
        db = str(self.workdir / "client.db")
        subprocess.run([self.prereqs["cli"], "--db", db, "init"], check=True, capture_output=True)
        command = [self.v1_python, "-c", V1_CLIENT_CODE, sys.executable,
                   str(SERVER_DIR / "server.py"), self.prereqs["cli"], db, str(self.workdir)]
        result = await asyncio.to_thread(subprocess.run, command, capture_output=True,
                                         text=True, timeout=40)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def old_source(self):
        old = self.workdir / "old-server"
        old.mkdir()
        for name in ("server.py", "mqtt_service.py", "cli_ops.py"):
            content = subprocess.check_output(["git", "-C", str(SERVER_DIR.parent),
                                               "show", f"2dc5f45:mcp_server/{name}"])
            (old / name).write_bytes(content)
        return old

    async def pair(self, old_coordinator):
        old = self.old_source()
        scenario = Scenario(self.prereqs, self.workdir, None)
        scenario.coord_db = self.workdir / "coordinator.db"
        scenario.cli(scenario.coord_db, "init")
        scenario.broker.start()
        try:
            coord = await scenario.coordinator(source_dir=old if old_coordinator else SERVER_DIR)
            worker = await scenario.worker("w1", source_dir=SERVER_DIR if old_coordinator else old)
            await coord.call("add_task", title="Coordinator seed")
            replica = await scenario.until(lambda: scenario._when(
                worker.call("list_tasks"), lambda result: len(result["tasks"]) == 1))
            self.assertTrue(replica)
            request = await worker.call("add_task", title="Worker request")
            self.assertEqual(request["status"], "pending")
            pending = await scenario.until(lambda: scenario._when(
                coord.call("list_pending_requests"), lambda result: bool(result["pending"])))
            self.assertTrue(pending)
            await coord.call("approve_request", request_id=request["request_id"])
            result = await scenario.until(lambda: scenario._when(
                worker.call("check_request", request_id=request["request_id"]),
                lambda reply: reply["status"] == "approved"))
            self.assertTrue(result)
            self.assertEqual(result["task"]["title"], "Worker request")
        finally:
            for node in scenario.nodes:
                await node.stop()
            scenario.broker.stop()

    async def test_v1_coordinator_with_v2_worker(self):
        await self.pair(True)

    async def test_v2_coordinator_with_v1_worker(self):
        await self.pair(False)


if __name__ == "__main__":
    unittest.main()
