"""End-to-end regression scenario for the MQTT coordinator/worker sync.

Runs a throwaway local mosquitto (scratch port, password file, and an ACL
file in the same per-worker `pattern` form the fleet broker uses), then
drives real `server.py` processes over MCP stdio: one coordinator and two
workers, all on throwaway databases. It never touches any configured or
installed server, and never connects anywhere but 127.0.0.1.

    python mcp_server/tests/mqtt_fleet_scenario.py [--compat-ref REF] [--keep]

--compat-ref REF also runs mixed-version checks against the server as of
git REF (an old worker against this coordinator, and this worker against
the old coordinator). --keep leaves the scratch dir (logs, dbs) in place.

Exits 0 with a SKIP message when mosquitto, mosquitto_passwd, paho-mqtt,
mcp or the todo-sqlite-cli binary isn't available; exits 1 if any check
fails.
"""

import argparse
import asyncio
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
SERVER_DIR = HERE.parent
REPO = SERVER_DIR.parent
PREFIX = "t"
PASSWORD = "scenario-pw"
HEARTBEAT_S = 1.0
STALE_AFTER = 3


def skip(reason: str) -> None:
    print(f"SKIP: {reason}")
    sys.exit(0)


def find_prereqs() -> dict:
    try:
        import paho.mqtt.client  # noqa: F401
        from mcp import ClientSession  # noqa: F401
    except ImportError as e:
        skip(f"python dependency missing ({e.name}); install mcp<2 and paho-mqtt")
    candidates = (os.environ.get("MOSQUITTO"), shutil.which("mosquitto"), "/usr/sbin/mosquitto")
    mosquitto = next((c for c in candidates if c and Path(c).exists()), None)
    if not mosquitto:
        skip("mosquitto not installed")
    passwd = shutil.which("mosquitto_passwd")
    if not passwd:
        skip("mosquitto_passwd not installed")
    cli = os.environ.get("TODO_SQLITE_CLI_BIN")
    if not cli:
        for candidate in (REPO / "target/debug/todo-sqlite-cli", REPO / "target/release/todo-sqlite-cli"):
            if candidate.exists():
                cli = str(candidate)
                break
    cli = cli or shutil.which("todo-sqlite-cli")
    if not cli:
        skip("todo-sqlite-cli binary not found (cargo build, or set TODO_SQLITE_CLI_BIN)")
    return {"mosquitto": mosquitto, "passwd": passwd, "cli": cli}


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


ACL = """\
# Coordinator: full access under the prefix.
user coord
topic readwrite {p}/#

# Workers: %c is the connecting client id. Existing lines:
pattern write {p}/requests/%c
pattern read {p}/responses/%c
pattern read {p}/assign/%c
pattern write {p}/messages/to-coordinator/%c
pattern read {p}/messages/to-worker/%c
pattern read {p}/broadcast/#
pattern read {p}/state
# presence was write-only; workers now also read their own leaf (link echo):
pattern readwrite {p}/presence/%c
# New:
pattern read {p}/fleet/state
pattern read {p}/coordinator/presence
pattern write {p}/receipts/%c
"""


class Broker:
    def __init__(self, prereqs, workdir: Path):
        self.bin = prereqs["mosquitto"]
        self.dir = workdir / "broker"
        self.dir.mkdir()
        self.port = free_port()
        self.log = self.dir / "mosquitto.log"
        passwd = self.dir / "passwd"
        for user in ("coord", "w1", "w2", "w3", "w4", "w9"):
            flag = ["-c"] if not passwd.exists() else []
            subprocess.run([prereqs["passwd"], "-b", *flag, str(passwd), user, PASSWORD], check=True)
        os.chmod(passwd, 0o600)
        (self.dir / "acl").write_text(ACL.format(p=PREFIX))
        os.chmod(self.dir / "acl", 0o600)
        (self.dir / "mosquitto.conf").write_text(
            f"listener {self.port} 127.0.0.1\n"
            "allow_anonymous false\n"
            f"password_file {passwd}\n"
            f"acl_file {self.dir / 'acl'}\n"
            "persistence true\n"
            f"persistence_location {self.dir}/\n"
            f"log_dest file {self.log}\n"
            "log_type all\n"
        )
        self.proc = None

    def start(self) -> None:
        self.proc = subprocess.Popen([self.bin, "-c", str(self.dir / "mosquitto.conf")])
        deadline = time.time() + 10
        while time.time() < deadline:
            try:
                socket.create_connection(("127.0.0.1", self.port), timeout=0.2).close()
                return
            except OSError:
                time.sleep(0.1)
        raise RuntimeError("mosquitto didn't start")

    def stop(self) -> None:
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            self.proc.wait(10)

    def denied_lines(self) -> list[str]:
        return [l for l in self.log.read_text().splitlines() if "Denied" in l]


class ToolError(Exception):
    pass


class Node:
    """One server.py process, driven over MCP stdio from its own asyncio
    task (anyio cancel scopes must be exited by the task that entered them,
    so each node's stdio_client lives and dies inside its own task)."""

    def __init__(self, name, source_dir: Path, config: dict, workdir: Path, prereqs):
        self.name = name
        self.source_dir = source_dir
        self.config_path = workdir / f"{name}.mqtt.json"
        self.config_path.write_text(json.dumps(config))
        self.config = config
        self.env = {
            **os.environ,
            "TODO_SQLITE_CLI_MQTT_CONFIG": str(self.config_path),
            "TODO_SQLITE_CLI_BIN": prereqs["cli"],
            "XDG_STATE_HOME": str(workdir / "xdg"),
            "SCENARIO_MQTT_PW": PASSWORD,
        }
        self.errlog = open(workdir / f"{name}.stderr.log", "a")
        self.session = None
        self._task = None

    async def start(self) -> None:
        from mcp import ClientSession, StdioServerParameters
        from mcp.client.stdio import stdio_client

        ready = asyncio.Event()
        self._stop = asyncio.Event()
        params = StdioServerParameters(
            command=sys.executable,
            args=[str(self.source_dir / "server.py")],
            env=self.env,
            cwd=str(self.source_dir),
        )

        async def run():
            async with stdio_client(params, errlog=self.errlog) as (r, w):
                async with ClientSession(r, w) as session:
                    await session.initialize()
                    self.session = session
                    ready.set()
                    await self._stop.wait()

        self._task = asyncio.create_task(run())
        waiter = asyncio.create_task(ready.wait())
        done, _ = await asyncio.wait({self._task, waiter}, timeout=30, return_when=asyncio.FIRST_COMPLETED)
        if waiter not in done:
            waiter.cancel()
            if self._task.done():
                self._task.result()  # surface the startup failure
            raise RuntimeError(f"{self.name} didn't start")

    async def call(self, tool: str, **args):
        result = await asyncio.wait_for(self.session.call_tool(tool, args), timeout=30)
        text = result.content[0].text if result.content else ""
        if result.isError:
            raise ToolError(text)
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return text

    async def tools(self) -> list[str]:
        return sorted(t.name for t in (await self.session.list_tools()).tools)

    def pid(self) -> int:
        needle = f"TODO_SQLITE_CLI_MQTT_CONFIG={self.config_path}".encode()
        for proc in Path("/proc").iterdir():
            if not proc.name.isdigit():
                continue
            try:
                if needle in (proc / "environ").read_bytes().split(b"\0"):
                    return int(proc.name)
            except OSError:
                continue
        raise RuntimeError(f"no process for {self.name}")

    async def stop(self) -> None:
        """Close stdin like a real MCP client exiting (the server's atexit
        hook then runs)."""
        self._stop.set()
        try:
            await asyncio.wait_for(self._task, timeout=15)
        except Exception:
            pass

    async def kill(self) -> None:
        """SIGKILL: no atexit, so only the broker-fired Last-Will says offline."""
        os.kill(self.pid(), signal.SIGKILL)
        await self.stop()


class Scenario:
    def __init__(self, prereqs, workdir: Path, compat_ref: str | None):
        self.prereqs = prereqs
        self.workdir = workdir
        self.compat_ref = compat_ref
        self.failures = 0
        self.broker = Broker(prereqs, workdir)
        self.nodes: list[Node] = []

    # -- helpers ------------------------------------------------------------

    def check(self, name: str, ok: bool, detail="") -> bool:
        print(f"{'PASS' if ok else 'FAIL'} {name}" + (f": {detail}" if detail else ""))
        if not ok:
            self.failures += 1
        return ok

    async def until(self, fn, timeout=10.0, interval=0.25):
        """Poll an async fn until it returns something truthy."""
        deadline = time.time() + timeout
        last = None
        while time.time() < deadline:
            try:
                last = await fn()
                if last:
                    return last
            except ToolError as e:
                last = e
            await asyncio.sleep(interval)
        return None

    def cli(self, db: Path, *args) -> str:
        out = subprocess.run([self.prereqs["cli"], "--db", str(db), *args], capture_output=True, text=True, check=True)
        return out.stdout.strip()

    def config(self, mode, client_id, *, new_fields=True, **extra) -> dict:
        cfg = {
            "mode": mode,
            "host": "127.0.0.1",
            "port": self.broker.port,
            "client_id": client_id,
            "topic_prefix": PREFIX,
            "tls": False,
            "username": client_id,
            "password_env": "SCENARIO_MQTT_PW",
            "heartbeat_interval_s": HEARTBEAT_S,
            **extra,
        }
        if new_fields:
            cfg["stale_after_heartbeats"] = STALE_AFTER
        return cfg

    async def node(self, name, cfg, source_dir=SERVER_DIR) -> Node:
        n = Node(name, source_dir, cfg, self.workdir, self.prereqs)
        await n.start()
        self.nodes.append(n)
        return n

    async def coordinator(self, source_dir=SERVER_DIR, new_fields=True, **extra) -> Node:
        return await self.node(
            "coord",
            self.config("coordinator", "coord", new_fields=new_fields, db_path=str(self.coord_db), **extra),
            source_dir,
        )

    async def restart_coordinator(self, coord, **extra) -> Node:
        await coord.stop()
        self.nodes.remove(coord)
        return await self.coordinator(**extra)

    async def worker(self, wid, source_dir=SERVER_DIR, new_fields=True, **extra) -> Node:
        return await self.node(
            wid,
            self.config("worker", wid, new_fields=new_fields, worker_db_path=str(self.workdir / f"{wid}.db"), **extra),
            source_dir,
        )

    def raw_client(self, client_id, username):
        import paho.mqtt.client as mqtt

        c = mqtt.Client(callback_api_version=mqtt.CallbackAPIVersion.VERSION2, client_id=client_id, protocol=mqtt.MQTTv5)
        c.username_pw_set(username, PASSWORD)
        c.connect("127.0.0.1", self.broker.port)
        c.loop_start()
        return c

    # -- scenario -----------------------------------------------------------

    async def run(self) -> int:
        self.broker.start()
        self.coord_db = self.workdir / "coord.db"
        self.cli(self.coord_db, "init")
        t1 = json.loads(self.cli(self.coord_db, "add", "task one", "--json"))["task"]
        t2 = json.loads(self.cli(self.coord_db, "add", "task two", "--json"))["task"]
        w2_files = self.workdir / "w2-explicit-files"
        try:
            coord = await self.coordinator()
            w1 = await self.worker("w1")
            w2 = await self.worker("w2", files_dir=str(w2_files))
            await self.startup(coord, w1, w2)
            await self.fleet_state(coord, w1)
            await self.directives(coord, w1, t1, t2)
            await self.receipts(coord, w1, w2)
            await self.attachments(coord, w1, w2, w2_files)
            await self.writes_and_broadcasts(coord, w1)
            await self.acl(coord)
            coord = await self.coordinator_offline(coord, w1, w2)
            await self.broker_outage(coord, w1)
            coord = await self.deadman(coord, w1)
            coord, w4 = await self.offline_fallback(coord, w1, t1)
            if self.compat_ref:
                await self.compat(coord, w1, t1, w4)
            denied = [l for l in self.broker.denied_lines() if " w9 " not in l and "(w9)" not in l and "w9," not in l]
            self.check("broker: no ACL denials for real nodes", not denied, "; ".join(denied[:3]))
        finally:
            for n in self.nodes:
                await n.stop()
            self.broker.stop()
        print(f"\n{'OK' if not self.failures else 'FAILED'}: {self.failures} failure(s)")
        return 1 if self.failures else 0

    async def startup(self, coord, w1, w2):
        print("\n== startup")
        print("coordinator tools:", ", ".join(await coord.tools()))
        print("worker tools:", ", ".join(await w1.tools()))
        fs = await self.until(
            lambda: self._when(w1.call("fleet_state"), lambda r: r["coordinator"]["status"] == "online" and not r["link"]["stale"])
        )
        self.check("worker sees coordinator online, link fresh", bool(fs), json.dumps(fs and fs["coordinator"]))
        self.check("missing retained fleet state means active", fs and fs["fleet_state"]["state"] == "active"
                   and fs["fleet_state"]["source"] == "default", json.dumps(fs and fs["fleet_state"]))
        workers = await self.until(lambda: self._when(coord.call("list_workers"), lambda r: len(r["workers"]) >= 2))
        self.check("list_workers has age_s and link", workers and all("age_s" in w for w in workers["workers"])
                   and "link" in workers, json.dumps(workers))

    @staticmethod
    async def _when(coro, pred):
        r = await coro
        return r if pred(r) else None

    async def fleet_state(self, coord, w1):
        print("\n== fleet state")
        r = await coord.call("set_fleet_state", state="draining", note="scenario: wind down")
        self.check("set_fleet_state draining", r["state"] == "draining" and r["set_by"] == "coord", json.dumps(r))
        got = await self.until(lambda: self._when(w1.call("check_assignments"), lambda r: r["fleet_state"]["state"] == "draining"))
        self.check("worker check_assignments carries fleet_state", bool(got), json.dumps(got and got["fleet_state"]))
        for tool in ("check_messages", "check_broadcasts", "sync_state"):
            r = await w1.call(tool)
            self.check(f"{tool} carries fleet_state/coordinator/link",
                       all(k in r for k in ("fleet_state", "coordinator", "link")) and r["fleet_state"]["state"] == "draining")
        try:
            await coord.call("set_fleet_state", state="sleeping")
            self.check("invalid fleet state rejected", False)
        except ToolError as e:
            self.check("invalid fleet state rejected", "unknown fleet state" in str(e), str(e))
        await coord.call("set_fleet_state", state="active")
        g = await coord.call("get_fleet_state")
        self.check("get_fleet_state", g["state"] == "active" and g["source"] == "retained", json.dumps(g))

    async def directives(self, coord, w1, t1, t2):
        print("\n== directive sequencing")
        go = await coord.call("assign_task", worker_id="w1", body="go on task one", task_id=t1["id"])
        self.check("assign with task_id stamps seq, default kind go",
                   go.get("kind") == "go" and go.get("seq") and go.get("task_uuid") == t1["uuid"] and "warning" not in go, json.dumps(go))
        hold = await coord.call("send_message", worker_id="w1", body="HOLD task one", task_id=t1["uuid"], kind="hold")
        info = await coord.call("send_message", worker_id="w1", body="fyi task one", task_id=t1["id"])
        self.check("uuid and display id share one sequence; message default kind info",
                   hold["seq"] > go["seq"] and info["seq"] > hold["seq"] and info["kind"] == "info",
                   f"go={go['seq']} hold={hold['seq']} info={info['seq']}")
        a = await self.until(lambda: self._when(w1.call("check_assignments"), lambda r: r["assignments"]))
        m = {"messages": []}

        async def drain_both():
            m["messages"] += (await w1.call("check_messages"))["messages"]
            return len(m["messages"]) >= 2

        await self.until(drain_both, timeout=5)
        self.check("older go marked superseded by the hold", a and a["assignments"][0]["superseded"] is True, json.dumps(a and a["assignments"]))
        by_kind = {x["kind"]: x for x in m["messages"]}
        self.check("hold not superseded; info never supersedes nor is superseded",
                   by_kind.get("hold", {}).get("superseded") is False and by_kind.get("info", {}).get("superseded") is False,
                   json.dumps(m["messages"]))
        ld = await w1.call("latest_directive", task_id=t1["id"])
        self.check("latest_directive: hold effective, not ok_to_act",
                   ld["effective"]["kind"] == "hold" and ld["ok_to_act"] is False, json.dumps({k: ld[k] for k in ("effective", "ok_to_act", "draining")}))
        ld_uuid = await w1.call("latest_directive", task_id=t1["uuid"])
        self.check("latest_directive by uuid matches", ld_uuid["effective"]["seq"] == ld["effective"]["seq"])

        # An older directive delivered late (e.g. a go that crossed a hold
        # on the wire) must not become effective.
        raw = self.raw_client("coord-raw", "coord")
        late = {"message_id": "late-go", "from": "coord", "task_id": t1["id"], "task_uuid": t1["uuid"],
                "seq": go["seq"], "kind": "go", "body": "late go", "ts": time.time()}
        raw.publish(f"{PREFIX}/assign/w1", json.dumps(late), qos=1).wait_for_publish(5)
        raw.disconnect()
        a = await self.until(lambda: self._when(w1.call("check_assignments"), lambda r: r["assignments"]))
        ld = await w1.call("latest_directive", task_id=t1["id"])
        self.check("late older go is superseded and doesn't displace the hold",
                   a and a["assignments"][0]["superseded"] is True and ld["effective"]["kind"] == "hold")

        go2 = await coord.call("assign_task", worker_id="w1", body="go again", task_id=t1["id"], kind="go")
        await self.until(lambda: self._when(w1.call("check_assignments"), lambda r: r["assignments"]))
        ld = await w1.call("latest_directive", task_id=t1["id"])
        self.check("new go after hold is effective, ok_to_act", ld["effective"]["seq"] == go2["seq"] and ld["ok_to_act"] is True
                   and ld["draining"] is False)

        await coord.call("set_fleet_state", state="draining")
        ld = await self.until(lambda: self._when(w1.call("latest_directive", task_id=t1["id"]), lambda r: r["draining"]))
        self.check("draining: ok_to_act stays true, draining true", ld and ld["ok_to_act"] is True and ld["draining"] is True)
        await coord.call("set_fleet_state", state="paused", note="scenario pause")
        ld = await self.until(lambda: self._when(w1.call("latest_directive", task_id=t1["id"]), lambda r: r["fleet_state"]["state"] == "paused"))
        self.check("paused: ok_to_act false", ld and ld["ok_to_act"] is False)

        warn_go = await coord.call("assign_task", worker_id="w1", body="go while paused", task_id=t2["id"])
        no_warn_hold = await coord.call("send_message", worker_id="w1", body="hold while paused", task_id=t2["id"], kind="hold")
        no_warn_info = await coord.call("send_message", worker_id="w1", body="info while paused", task_id=t2["id"], kind="info")
        self.check("go while paused is sent with a warning; hold/info aren't warned",
                   "warning" in warn_go and "warning" not in no_warn_hold and "warning" not in no_warn_info, warn_go.get("warning", ""))
        await coord.call("set_fleet_state", state="active")
        await self.until(lambda: self._when(w1.call("check_assignments"), lambda r: r["assignments"]))
        await self.until(lambda: self._when(w1.call("check_messages"), lambda r: r["messages"]))

        for args, needle in (({"kind": "hold"}, "kind requires task_id"), ({"task_id": t1["id"], "kind": "maybe"}, "unknown directive kind"),
                             ({"task_id": 9999}, "")):
            try:
                await coord.call("assign_task", worker_id="w1", body="x", **args)
                self.check(f"assign_task rejects {args}", False)
            except ToolError as e:
                self.check(f"assign_task rejects {args}", needle in str(e), str(e)[:100])

        ld = await w1.call("latest_directive", task_id=12345)
        self.check("latest_directive for an unknown task: null, not ok", ld["effective"] is None and ld["ok_to_act"] is False)

    async def receipts(self, coord, w1, w2):
        print("\n== receipts")
        m1 = await coord.call("send_message", worker_id="w1", body="read me")
        m2 = await coord.call("send_message", worker_id="w2", body="leave me unread")
        await self.until(lambda: self._when(w1.call("check_messages"), lambda r: r["messages"]))

        async def status(worker, mid):
            sent = (await coord.call("list_sent", worker_id=worker))["sent"]
            return next((e for e in sent if e["message_id"] == mid), None)

        e1 = await self.until(lambda: self._when(status("w1", m1["message_id"]), lambda e: e and e["status"] == "read"))
        self.check("drained message is read", bool(e1) and e1["delivered_at"] and e1["read_at"], json.dumps(e1))
        e2 = await self.until(lambda: self._when(status("w2", m2["message_id"]), lambda e: e and e["status"] == "delivered"))
        self.check("undrained message is delivered, not read", bool(e2) and e2["read_at"] is None, json.dumps(e2))
        unread = await coord.call("list_sent", unread_only=True)
        self.check("list_sent unread_only", any(e["message_id"] == m2["message_id"] for e in unread["sent"])
                   and all(e["status"] != "read" for e in unread["sent"]))
        all_w1 = (await coord.call("list_sent", worker_id="w1"))["sent"]
        self.check("every directive sent to w1 got read", all(e["status"] == "read" for e in all_w1),
                   ", ".join(f"{e['kind']}:{e['status']}" for e in all_w1))
        await w2.call("check_messages")

    async def attachments(self, coord, w1, w2, w2_files):
        print("\n== attachments dir")
        attachment = self.workdir / "note.txt"
        attachment.write_text("attached\n")
        await coord.call("send_message", worker_id="w1", body="file", file_path=str(attachment))
        await coord.call("send_message", worker_id="w2", body="file", file_path=str(attachment))
        m1 = await self.until(lambda: self._when(w1.call("check_messages"), lambda r: r["messages"]))
        m2 = await self.until(lambda: self._when(w2.call("check_messages"), lambda r: r["messages"]))
        xdg = self.workdir / "xdg" / "todo-sqlite-cli" / "mqtt-files"
        p1 = Path(m1["messages"][0]["file_path"]) if m1 else None
        p2 = Path(m2["messages"][0]["file_path"]) if m2 else None
        self.check("default: attachment lands under $XDG_STATE_HOME/todo-sqlite-cli/mqtt-files", p1 and p1.parent == xdg and p1.read_text() == "attached\n", str(p1))
        self.check("explicit files_dir still wins", p2 and p2.parent == w2_files, str(p2))
        self.check("nothing lands next to the db", not (self.workdir / "mqtt-files").exists())

    async def writes_and_broadcasts(self, coord, w1):
        print("\n== worker writes, broadcasts")
        sub = await w1.call("add_task", title="from w1")
        self.check("worker write returns pending + fleet_state", sub["status"] == "pending" and "fleet_state" in sub)
        pend = await self.until(lambda: self._when(coord.call("list_pending_requests"), lambda r: r["pending"]))
        await coord.call("approve_request", request_id=pend["pending"][0]["request_id"])
        res = await self.until(lambda: self._when(w1.call("check_request", request_id=sub["request_id"]), lambda r: r["status"] == "approved"))
        self.check("request approved; check_request carries fleet_state", bool(res) and "fleet_state" in res)
        b = await coord.call("broadcast", body="standing order", retain=True)
        got = await self.until(lambda: self._when(w1.call("check_broadcasts"), lambda r: r["broadcasts"]))
        self.check("retained broadcast delivered", bool(got))
        await coord.call("delete_broadcast", message_id=b["message_id"])
        await asyncio.sleep(1)
        r = await w1.call("check_broadcasts")
        fs = await w1.call("fleet_state")
        self.check("empty payload from delete_broadcast is ignored, worker still healthy",
                   r["broadcasts"] == [] and not fs["link"]["stale"])

    async def acl(self, coord):
        print("\n== ACL")
        target = await coord.call("send_message", worker_id="w1", body="acl probe")
        intruder = self.raw_client("w9", "w9")
        forged = {"worker_id": "w1", "event": "read", "channel": "message", "message_ids": [target["message_id"]], "ts": time.time()}
        intruder.publish(f"{PREFIX}/receipts/w1", json.dumps(forged), qos=1)
        intruder.publish(f"{PREFIX}/fleet/state", json.dumps({"state": "paused", "set_by": "w9"}), qos=1, retain=True)
        intruder.publish(f"{PREFIX}/coordinator/presence", json.dumps({"status": "offline"}), qos=1, retain=True)
        intruder.publish(f"{PREFIX}/receipts/w9", json.dumps(forged), qos=1)
        await asyncio.sleep(1.5)
        intruder.disconnect()
        sent = (await coord.call("list_sent", worker_id="w1"))["sent"]
        entry = next(e for e in sent if e["message_id"] == target["message_id"])
        self.check("forged receipt from another worker isn't accepted", entry["read_at"] is None, entry["status"])
        fs = await coord.call("get_fleet_state")
        self.check("a worker can't write fleet/state", fs["state"] == "active", fs["state"])
        denied = [l for l in self.broker.denied_lines() if "w9" in l]
        self.check("broker denied the intruder's writes outside its own leaf", len(denied) >= 3, f"{len(denied)} denials")

    async def coordinator_offline(self, coord, w1, w2):
        print("\n== coordinator presence / offline")
        await coord.call("set_fleet_state", state="draining", note="before crash")
        await self.until(lambda: self._when(w1.call("fleet_state"), lambda r: r["fleet_state"]["state"] == "draining"))
        await coord.kill()
        self.nodes.remove(coord)
        fs = await self.until(lambda: self._when(w1.call("fleet_state"), lambda r: r["coordinator"]["status"] == "offline"))
        self.check("kill -9 coordinator: worker sees offline via last-will", bool(fs), json.dumps(fs and fs["coordinator"]))
        self.check("worker link stays fresh while only the coordinator is gone", fs and not fs["link"]["stale"])
        r = await w1.call("check_messages")
        self.check("check_messages with coordinator offline: fresh link, empty list is fine", isinstance(r.get("messages"), list))
        coord = await self.coordinator()
        fs = await self.until(lambda: self._when(w1.call("fleet_state"), lambda r: r["coordinator"]["status"] == "online"))
        self.check("restarted coordinator: worker sees online again", bool(fs), json.dumps(fs and fs["coordinator"]))
        g = await self.until(lambda: self._when(coord.call("get_fleet_state"), lambda r: r["state"] == "draining"), timeout=5)
        self.check("restarted coordinator recovers retained fleet state", bool(g), json.dumps(g))
        sent = await coord.call("list_sent", worker_id="w1")
        self.check("sent log survives coordinator restart", len(sent["sent"]) > 0)
        await coord.call("set_fleet_state", state="active")

        await w2.stop()
        self.nodes.remove(w2)
        ws = await self.until(lambda: self._when(coord.call("list_workers"),
                                                 lambda r: any(w["worker_id"] == "w2" and w["status"] == "offline" for w in r["workers"])))
        self.check("clean worker exit publishes offline", bool(ws))
        return coord

    async def broker_outage(self, coord, w1):
        print("\n== stale link (broker outage)")
        await coord.call("send_message", worker_id="w1", body="queued before outage")
        await self.until(lambda: self._when(w1.call("sync_state"), lambda r: True), timeout=2)
        await asyncio.sleep(1)
        self.broker.stop()
        await asyncio.sleep(0.5)
        r = await w1.call("check_messages")
        self.check("stale link + items: items returned with link_stale_since",
                   len(r["messages"]) == 1 and r.get("link_stale_since"), json.dumps({k: r.get(k) for k in ("link_stale_since", "link")}))
        for node, tool, args in ((w1, "check_messages", {}), (w1, "check_assignments", {}), (w1, "latest_directive", {"task_id": 1}),
                                 (coord, "check_messages", {})):
            try:
                await node.call(tool, **args)
                self.check(f"{node.name} {tool} on a stale link errors", False)
            except ToolError as e:
                advice = "report_state and retry next poll" if node is w1 else "retry next poll"
                self.check(f"{node.name} {tool} on a stale link errors", "stale since" in str(e) and "Do not act on silence" in str(e)
                           and advice in str(e), str(e))
        fs = await w1.call("fleet_state")
        self.check("fleet_state reports link stale, coordinator not online", fs["link"]["stale"] and fs["coordinator"]["status"] != "online",
                   json.dumps(fs["coordinator"]))
        self.broker.start()
        fresh = await self.until(lambda: self._when(w1.call("fleet_state"), lambda r: not r["link"]["stale"]
                                                    and r["coordinator"]["status"] == "online"), timeout=45)
        self.check("link recovers after broker restart", bool(fresh))
        r = await w1.call("check_messages")
        self.check("check_messages normal again", r["messages"] == [] and "link_stale_since" not in r)
        cl = await self.until(lambda: self._when(coord.call("list_workers"), lambda r: not r["link"]["stale"]), timeout=10)
        self.check("coordinator link recovers", bool(cl))

    async def deadman(self, coord, w1):
        print("\n== dead-man switch")
        checkin = self.workdir / "checkin"
        coord = await self.restart_coordinator(coord, checkin_file=str(checkin), deadman_drain_after_s=4, deadman_pause_after_s=8)
        g = await coord.call("get_fleet_state")
        dm = g["deadman"]
        self.check("fresh start with no check-in file: no instant drain, timer runs from process start",
                   g["state"] == "active" and dm["last_checkin"] is None and dm["drains_at"] - dm["timer_from"] == 4
                   and dm["pauses_at"] - dm["timer_from"] == 8, json.dumps(dm))
        g = await self.until(lambda: self._when(coord.call("get_fleet_state"), lambda r: r["state"] == "draining"), timeout=10)
        self.check("no check-in past drain_after: dead-man drains", bool(g) and g["set_by"] == "deadman"
                   and g["note"].startswith("dead-man: no check-in since coordinator start at "),
                   json.dumps(g and {k: g[k] for k in ("state", "note", "set_by")}))
        fs = await self.until(lambda: self._when(w1.call("check_assignments"), lambda r: r["fleet_state"]["state"] == "draining"))
        self.check("worker poll carries draining + instruction", bool(fs) and "finish your current task" in (fs["fleet_state"]["instruction"] or ""),
                   fs and fs["fleet_state"]["instruction"])
        c = await coord.call("checkin", note="scenario")
        dm = c["deadman"]
        self.check("checkin tool resets the timer but doesn't resume", c["fleet_state"]["state"] == "draining"
                   and dm["last_checkin"] and abs(dm["pauses_at"] - dm["last_checkin"] - 8) < 0.5, json.dumps(dm))
        await asyncio.sleep(5)
        g = await coord.call("get_fleet_state")
        self.check("check-in pushed the pause out (still draining 5s later)", g["state"] == "draining", g["state"])
        g = await self.until(lambda: self._when(coord.call("get_fleet_state"), lambda r: r["state"] == "paused"), timeout=8)
        self.check("no check-in past pause_after: dead-man pauses", bool(g) and g["set_by"] == "deadman", json.dumps(g and g["note"]))
        fs = await self.until(lambda: self._when(w1.call("fleet_state"), lambda r: r["fleet_state"]["state"] == "paused"))
        self.check("worker sees paused + instruction", bool(fs) and "stop now" in (fs["fleet_state"]["instruction"] or ""))
        checkin.touch()  # what the UserPromptSubmit hook does
        await asyncio.sleep(2.5)
        g = await coord.call("get_fleet_state")
        self.check("a hook-style check-in (touch) never auto-resumes", g["state"] == "paused" and g["deadman"]["last_checkin"] > g["set_at"])
        g = await coord.call("set_fleet_state", state="active", note="back")
        self.check("explicit set_fleet_state resumes", g["state"] == "active")
        await coord.call("set_fleet_state", state="paused", note="by hand")
        checkin.touch()
        await asyncio.sleep(5.5)
        g = await coord.call("get_fleet_state")
        self.check("dead-man never relaxes a hand-set pause", g["state"] == "paused" and g["set_by"] == "coord", json.dumps({k: g[k] for k in ("state", "set_by")}))
        coord = await self.restart_coordinator(coord, deadman_drain_after_s=0)
        await coord.call("set_fleet_state", state="active")
        g = await coord.call("get_fleet_state")
        self.check("deadman_drain_after_s=0 disables both stages", g["deadman"]["enabled"] is False and g["deadman"]["pauses_at"] is None)
        return coord

    async def offline_fallback(self, coord, w1, t1):
        print("\n== worker fallback: coordinator gone")
        w4 = await self.worker("w4", coordinator_offline_drain_after_s=3)
        await self.until(lambda: self._when(w4.call("fleet_state"), lambda r: r["coordinator"]["status"] == "online"))
        await coord.call("assign_task", worker_id="w4", body="go", task_id=t1["id"])
        await self.until(lambda: self._when(w4.call("check_assignments"), lambda r: r["assignments"]))
        ld = await w4.call("latest_directive", task_id=t1["id"])
        self.check("baseline: go, ok_to_act", ld["ok_to_act"] is True)
        await coord.kill()
        self.nodes.remove(coord)
        fs = await self.until(lambda: self._when(w4.call("fleet_state"), lambda r: r["fleet_state"]["state"] == "draining"), timeout=10)
        self.check("coordinator killed: worker derives draining past the threshold",
                   bool(fs) and fs["fleet_state"]["source"] == "coordinator-offline" and fs["fleet_state"]["since"]
                   and fs["fleet_state"]["retained"]["state"] == "active" and fs["fleet_state"]["instruction"],
                   json.dumps(fs and {k: fs["fleet_state"][k] for k in ("state", "source", "since", "note")}))
        ld = await w4.call("latest_directive", task_id=t1["id"])
        self.check("derived draining: ok_to_act true, draining true", ld["ok_to_act"] is True and ld["draining"] is True)
        fs = await self.until(lambda: self._when(w4.call("fleet_state"), lambda r: r["fleet_state"]["state"] == "paused"), timeout=10)
        self.check("past 2x the threshold: derived paused", bool(fs) and fs["fleet_state"]["source"] == "coordinator-offline")
        ld = await w4.call("latest_directive", task_id=t1["id"])
        self.check("derived paused: ok_to_act false", ld["ok_to_act"] is False)
        fs1 = await w1.call("fleet_state")
        self.check("worker with the default 2h threshold is unaffected", fs1["fleet_state"]["state"] == "active")
        coord = await self.coordinator(deadman_drain_after_s=0)
        g = await coord.call("get_fleet_state")
        self.check("retained fleet/state untouched by the worker fallback", g["state"] == "active" and g["source"] == "retained")
        fs = await self.until(lambda: self._when(w4.call("fleet_state"), lambda r: r["fleet_state"]["state"] == "active"
                                                 and r["fleet_state"]["source"] == "retained"))
        self.check("derived state lapses once the coordinator is back", bool(fs))
        return coord, w4

    async def compat(self, coord, w1, t1, w4):
        print(f"\n== mixed versions vs {self.compat_ref}")
        old = self.workdir / "old-server"
        old.mkdir()
        for f in ("server.py", "mqtt_service.py", "cli_ops.py"):
            src = subprocess.run(["git", "-C", str(REPO), "show", f"{self.compat_ref}:mcp_server/{f}"], capture_output=True, text=True, check=True)
            (old / f).write_text(src.stdout)

        w3 = await self.worker("w3", source_dir=old, new_fields=False)
        await asyncio.sleep(1.5)
        a = await coord.call("assign_task", worker_id="w3", body="old worker go", task_id=t1["id"])
        m = await coord.call("send_message", worker_id="w3", body="old worker hold", task_id=t1["id"], kind="hold")
        got = await self.until(lambda: self._when(w3.call("check_assignments"), lambda r: r["assignments"]))
        self.check("old worker receives a sequenced assignment (extra fields ignored)",
                   bool(got) and got["assignments"][0]["body"] == "old worker go", json.dumps(got))
        got = await self.until(lambda: self._when(w3.call("check_messages"), lambda r: r["messages"]))
        self.check("old worker receives a sequenced message", bool(got))
        sub = await w3.call("add_task", title="from old worker")
        pend = await self.until(lambda: self._when(coord.call("list_pending_requests"), lambda r: r["pending"]))
        await coord.call("approve_request", request_id=pend["pending"][0]["request_id"])
        res = await self.until(lambda: self._when(w3.call("check_request", request_id=sub["request_id"]), lambda r: r["status"] == "approved"))
        self.check("old worker write path works against new coordinator", bool(res))
        sent = (await coord.call("list_sent", worker_id="w3"))["sent"]
        self.check("old worker never acks: its messages stay 'sent'", {e["message_id"] for e in sent} >= {a["message_id"], m["message_id"]}
                   and all(e["status"] == "sent" for e in sent))
        ws = await coord.call("list_workers")
        self.check("old worker shows in list_workers", any(w["worker_id"] == "w3" and w["status"] == "online" for w in ws["workers"]))
        await w3.stop()
        self.nodes.remove(w3)

        await coord.stop()
        self.nodes.remove(coord)
        # A fleet that never ran the new coordinator has nothing retained on
        # coordinator/presence; clear what the new one left to emulate that.
        raw = self.raw_client("coord-raw", "coord")
        raw.publish(f"{PREFIX}/coordinator/presence", payload=None, qos=1, retain=True).wait_for_publish(5)
        raw.disconnect()
        old_coord = await self.coordinator(source_dir=old, new_fields=False)
        fs = await self.until(lambda: self._when(w1.call("fleet_state"), lambda r: r["coordinator"]["status"] == "unknown"), timeout=10)
        self.check("new worker vs old coordinator: coordinator 'unknown', link still fresh", bool(fs) and not fs["link"]["stale"],
                   json.dumps(fs and {"coordinator": fs["coordinator"], "link": fs["link"]}))
        await old_coord.call("assign_task", worker_id="w1", body="old coordinator go", task_id=t1["id"])
        got = await self.until(lambda: self._when(w1.call("check_assignments"), lambda r: r["assignments"]))
        self.check("new worker takes an unsequenced assignment (no superseded flag)",
                   bool(got) and "superseded" not in got["assignments"][0] and "seq" not in got["assignments"][0], json.dumps(got and got["assignments"]))
        await old_coord.call("send_message", worker_id="w1", body="old coordinator hello")
        got = await self.until(lambda: self._when(w1.call("check_messages"), lambda r: r["messages"]))
        self.check("new worker receives messages from old coordinator", bool(got))
        sub = await w1.call("add_task", title="new worker, old coordinator")
        pend = await self.until(lambda: self._when(old_coord.call("list_pending_requests"), lambda r: r["pending"]))
        await old_coord.call("approve_request", request_id=pend["pending"][0]["request_id"])
        res = await self.until(lambda: self._when(w1.call("check_request", request_id=sub["request_id"]), lambda r: r["status"] == "approved"))
        self.check("new worker write path works against old coordinator", bool(res))
        await asyncio.sleep(7)
        fs = await w4.call("fleet_state")
        self.check("old coordinator (never published presence): no offline fallback past 2x threshold",
                   fs["fleet_state"]["state"] == "active" and fs["fleet_state"]["source"] == "retained"
                   and fs["coordinator"]["status"] == "unknown", json.dumps({"fleet_state": fs["fleet_state"]["state"], "coordinator": fs["coordinator"]}))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--compat-ref", help="also run mixed-version checks against this git ref's server")
    parser.add_argument("--keep", action="store_true", help="keep the scratch dir")
    args = parser.parse_args()
    prereqs = find_prereqs()
    workdir = Path(tempfile.mkdtemp(prefix="mqtt-fleet-scenario-"))
    print(f"scratch dir: {workdir}\ncli: {prereqs['cli']}\nmosquitto: {prereqs['mosquitto']}")
    try:
        return asyncio.run(Scenario(prereqs, workdir, args.compat_ref).run())
    finally:
        if args.keep:
            print(f"kept {workdir}")
        else:
            shutil.rmtree(workdir, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
