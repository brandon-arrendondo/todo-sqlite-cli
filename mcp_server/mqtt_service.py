"""Optional MQTT coordinator/worker sync for the todo-sqlite-cli MCP server.

See the "MQTT sync" section of the top-level README.md for the full picture.
In short: one node runs as `coordinator` and owns the master database
directly; other nodes run as `worker` and never write their own database —
every mutation becomes a pending request the coordinator's agent explicitly
approves or rejects, and the worker's local database is a disposable read
replica kept current by full-db-file snapshots the coordinator publishes
after every applied change. Alongside that, both sides can exchange direct
messages (optionally with a small file attached) and assignments, the
coordinator can broadcast to every worker, and workers announce presence
(including an optional work-state) so the coordinator knows who's currently
around and what they're doing.

Only imported when TODO_SQLITE_CLI_MQTT_CONFIG is set, so a standalone
deployment never needs the `paho-mqtt` optional dependency installed.

Topic layout, all namespaced under `topic_prefix`:

  requests/<worker_id>              worker -> coordinator (write requests)
  responses/<worker_id>              coordinator -> worker (request outcome)
  assign/<worker_id>                 coordinator -> worker (work assignment)
  messages/to-coordinator/<worker_id> worker -> coordinator (direct message)
  messages/to-worker/<worker_id>      coordinator -> worker (direct message)
  presence/<worker_id>                worker -> coordinator (online/state)
  broadcast/<message_id>              coordinator -> all workers
  state                               coordinator -> all workers (db snapshot)

Every topic a worker publishes to, or subscribes to for messages meant only
for it, is scoped to that worker's own subtopic (keyed by its `client_id`)
rather than one topic shared by every worker. That's deliberate: a broker
ACL can then grant each worker write access to only its own subtopic (the
same isolation principle as the coordinator-owned `fleet/tasks/<node>` /
worker-owned `fleet/workers/<node>` split this sync sits alongside),
instead of requiring a shared write topic every worker must be trusted
with. The coordinator subscribes to the wildcard form of each worker-owned
topic (`.../+`); a worker subscribes only to its own leaf.

`broadcast` similarly gets its own subtopic per message
(`broadcast/<message_id>`) rather than one flat topic, so a retained
publish occupies its own permanent slot instead of silently overwriting
whatever standing announcement was retained there before.
"""

import base64
import json
import os
import sqlite3
import threading
import time
import uuid
from collections import OrderedDict
from dataclasses import dataclass
from pathlib import Path

import paho.mqtt.client as mqtt
from paho.mqtt.packettypes import PacketTypes
from paho.mqtt.properties import Properties

import cli_ops

CONFIG_ENV = "TODO_SQLITE_CLI_MQTT_CONFIG"


@dataclass
class Config:
    mode: str  # "coordinator" | "worker"
    host: str
    port: int
    client_id: str
    topic_prefix: str
    tls: bool = True
    username: str | None = None
    password_env: str | None = None
    db_path: str | None = None  # coordinator only
    worker_db_path: str | None = None  # worker only
    request_timeout_s: float = 30.0
    session_expiry_s: int = 86400
    heartbeat_interval_s: float = 60.0  # worker only
    message_max_chars: int = 4096
    max_file_bytes: int = 1_048_576
    files_dir: str | None = None

    @property
    def password(self) -> str | None:
        return os.environ.get(self.password_env) if self.password_env else None

    # Per-worker topics: pass a worker_id to get that worker's own subtopic
    # (what it publishes to, or subscribes to for itself); pass none (or
    # omit it) to get the coordinator's subscribe-side wildcard.

    def requests_topic(self, worker_id: str | None = None) -> str:
        return f"{self.topic_prefix}/requests/{worker_id or '+'}"

    def responses_topic(self, worker_id: str) -> str:
        return f"{self.topic_prefix}/responses/{worker_id}"

    def assign_topic(self, worker_id: str | None = None) -> str:
        return f"{self.topic_prefix}/assign/{worker_id or '+'}"

    def messages_to_coordinator_topic(self, worker_id: str | None = None) -> str:
        return f"{self.topic_prefix}/messages/to-coordinator/{worker_id or '+'}"

    def messages_to_worker_topic(self, worker_id: str) -> str:
        return f"{self.topic_prefix}/messages/to-worker/{worker_id}"

    def broadcast_topic(self, message_id: str | None = None) -> str:
        return f"{self.topic_prefix}/broadcast/{message_id or '#'}"

    @property
    def state_topic(self) -> str:
        return f"{self.topic_prefix}/state"

    @property
    def presence_prefix(self) -> str:
        return f"{self.topic_prefix}/presence/"

    @property
    def presence_wildcard(self) -> str:
        return f"{self.presence_prefix}+"

    def presence_topic(self, worker_id: str) -> str:
        return f"{self.presence_prefix}{worker_id}"

    @property
    def files_dir_path(self) -> Path:
        if self.files_dir:
            return Path(self.files_dir)
        base = self.db_path or self.worker_db_path
        return Path(base).parent / "mqtt-files"


def load_config() -> Config | None:
    path = os.environ.get(CONFIG_ENV)
    if not path:
        return None
    with open(path) as f:
        raw = json.load(f)
    return Config(**raw)


def _make_client(
    config: Config,
    on_connect,
    will_topic: str | None = None,
    will_payload: str | None = None,
) -> mqtt.Client:
    """Build a connected, auto-reconnecting MQTT client.

    Uses a persistent session (`clean_start=False` + `SessionExpiryInterval`)
    keyed by the stable `client_id`, so the broker holds this client's
    subscriptions and any QoS-1 messages published while it's briefly
    disconnected — covering both a network blip mid-session and the gap
    between one MCP process dying and the next one starting. Subscriptions
    themselves are established in `on_connect`, not here, so they're
    re-applied identically on first connect and on every reconnect.
    """
    client = mqtt.Client(
        callback_api_version=mqtt.CallbackAPIVersion.VERSION2,
        client_id=config.client_id,
        protocol=mqtt.MQTTv5,
    )
    if config.username:
        client.username_pw_set(config.username, config.password)
    if config.tls:
        client.tls_set()
    if will_topic is not None:
        client.will_set(will_topic, will_payload, qos=1, retain=True)
    client.on_connect = on_connect
    client.reconnect_delay_set(min_delay=1, max_delay=30)
    props = Properties(PacketTypes.CONNECT)
    props.SessionExpiryInterval = config.session_expiry_s
    client.connect(config.host, config.port, clean_start=False, properties=props)
    client.loop_start()
    return client


def _checkpoint_and_read(db_path: str) -> bytes:
    """Force any WAL contents into the main db file, then read it whole —
    a plain byte-copy of just the .db file would otherwise miss recent
    writes still sitting in the -wal file (journal_mode=WAL, per db::open)."""
    conn = sqlite3.connect(db_path)
    try:
        conn.execute("PRAGMA wal_checkpoint(FULL);")
    finally:
        conn.close()
    return Path(db_path).read_bytes()


def _atomic_replace(path: str, data: bytes) -> None:
    tmp = f"{path}.tmp-{os.getpid()}"
    Path(tmp).write_bytes(data)
    os.replace(tmp, path)


def _encode_file(file_path: str, max_bytes: int) -> dict:
    data = Path(file_path).read_bytes()
    if len(data) > max_bytes:
        raise RuntimeError(
            f"{file_path} is {len(data)} bytes, over the {max_bytes}-byte MQTT attachment cap"
        )
    return {"filename": Path(file_path).name, "content_b64": base64.b64encode(data).decode()}


def _save_incoming_file(files_dir: Path, message_id: str, file_field: dict) -> str:
    files_dir.mkdir(parents=True, exist_ok=True)
    safe_name = Path(file_field["filename"]).name  # strip any path, no traversal
    out_path = files_dir / f"{message_id}-{safe_name}"
    out_path.write_bytes(base64.b64decode(file_field["content_b64"]))
    return str(out_path)


def _check_message_length(body: str, max_chars: int) -> None:
    if len(body) > max_chars:
        raise RuntimeError(f"message body is {len(body)} chars, over the {max_chars}-char cap")


_DEDUP_WINDOW = 500


class Mailbox:
    """Thread-safe, disk-persisted FIFO. `add` appends and persists;
    `drain` returns everything and clears it — reading a mailbox consumes
    its queue rather than leaving items to re-read.

    QoS 1 is "at least once", not "exactly once" — and re-subscribing on
    every reconnect (needed so a fresh process or a post-blip reconnect
    reliably re-establishes subscriptions) can itself trigger a duplicate
    retained-message redelivery on top of a session's queued redelivery.
    So `add` drops anything whose `message_id` it's already seen, in an
    in-memory (process-lifetime) LRU window — that covers exactly the
    back-to-back-duplicate case a reconnect produces.
    """

    def __init__(self, path: Path):
        self._path = path
        self._lock = threading.Lock()
        self._items: list[dict] = self._load()
        self._seen: OrderedDict[str, None] = OrderedDict()

    def _load(self) -> list[dict]:
        if self._path.exists():
            return json.loads(self._path.read_text())
        return []

    def _save(self) -> None:
        self._path.write_text(json.dumps(self._items))

    def add(self, item: dict) -> None:
        with self._lock:
            message_id = item.get("message_id")
            if message_id is not None:
                if message_id in self._seen:
                    return
                self._seen[message_id] = None
                if len(self._seen) > _DEDUP_WINDOW:
                    self._seen.popitem(last=False)
            self._items.append(item)
            self._save()

    def drain(self) -> list[dict]:
        with self._lock:
            items, self._items = self._items, []
            self._save()
            return items


class CoordinatorService:
    """Owns the master db. Holds a pending-request queue fed by workers;
    nothing is applied until the coordinator's agent calls approve/reject.
    Also fields direct messages/broadcasts/assignments and tracks worker
    presence (including each worker's self-reported work-state).
    """

    def __init__(self, config: Config, run):
        if not config.db_path:
            raise RuntimeError("coordinator mode requires 'db_path' in the MQTT config")
        self.config = config
        self.run = run
        self._lock = threading.Lock()
        self._pending_path = Path(config.db_path).with_suffix(".mqtt-pending.json")
        self._pending: dict[str, dict] = self._load_pending()
        self._inbox = Mailbox(Path(config.db_path).with_suffix(".mqtt-inbox.json"))
        self._presence: dict[str, dict] = {}
        self._client = _make_client(config, on_connect=self._on_connect)
        self._client.on_message = self._on_message

    def _on_connect(self, client, _userdata, _connect_flags, _reason_code, _properties):
        client.subscribe(self.config.requests_topic(), qos=1)
        client.subscribe(self.config.messages_to_coordinator_topic(), qos=1)
        client.subscribe(self.config.presence_wildcard, qos=1)

    def _load_pending(self) -> dict:
        if self._pending_path.exists():
            return json.loads(self._pending_path.read_text())
        return {}

    def _save_pending(self) -> None:
        self._pending_path.write_text(json.dumps(self._pending))

    def _on_message(self, _client, _userdata, msg):
        prefix = self.config.topic_prefix
        if msg.topic.startswith(f"{prefix}/requests/"):
            request = json.loads(msg.payload.decode())
            with self._lock:
                self._pending[request["request_id"]] = request
                self._save_pending()
        elif msg.topic.startswith(f"{prefix}/messages/to-coordinator/"):
            self._inbox.add(self._land_file(json.loads(msg.payload.decode())))
        elif msg.topic.startswith(self.config.presence_prefix):
            payload = json.loads(msg.payload.decode())
            with self._lock:
                # Stamp with receipt time, not the payload's own `ts`: a
                # Last-Will-Testament payload is fixed at connect time and
                # can't be updated when the broker actually fires it on
                # disconnect, so its embedded ts would misreport an
                # "offline" notice as having happened back at connect time.
                self._presence[payload["worker_id"]] = {
                    "status": payload["status"],
                    "work_state": payload.get("work_state"),
                    "ts": time.time(),
                }

    def _land_file(self, message: dict) -> dict:
        """Replace an inline base64 `file` field with a local `file_path` —
        callers read the file, they never see the raw bytes as tool output."""
        file_field = message.pop("file", None)
        if file_field:
            message["file_path"] = _save_incoming_file(
                self.config.files_dir_path, message["message_id"], file_field
            )
        return message

    def list_pending(self) -> str:
        with self._lock:
            return json.dumps({"pending": list(self._pending.values())})

    def approve(self, request_id: str) -> str:
        with self._lock:
            request = self._pending.get(request_id)
            if request is None:
                raise RuntimeError(f"no pending request {request_id}")
            try:
                task_json = self._apply(request)
            except Exception as e:
                self._respond(request, "rejected", error=str(e))
                del self._pending[request_id]
                self._save_pending()
                raise
            self._respond(request, "approved", task=json.loads(task_json))
            del self._pending[request_id]
            self._save_pending()
        self.publish_snapshot()
        return task_json

    def _apply(self, request: dict) -> str:
        """Apply the request's op, then auto-stamp implementation_client
        from the requesting worker's id on a `start` (unless the request
        already set one explicitly — `start` has no --implementation-client
        flag of its own, so this needs a follow-up `edit`). `add --start`
        can just pass it straight through to `add`'s own flag."""
        op, args = request["op"], request["args"]
        if op == "add" and args.get("start") and not args.get("implementation_client"):
            args["implementation_client"] = request["worker_id"]
        task_json = cli_ops.apply_op(self.run, op, args)
        if op == "start" and not args.get("implementation_client"):
            task_id = json.loads(task_json)["id"]
            self.run("edit", str(task_id), "--implementation-client", request["worker_id"])
            task_json = self.run("show", str(task_id), "--format", "json")
        return task_json

    def reject(self, request_id: str, reason: str | None = None) -> str:
        with self._lock:
            request = self._pending.get(request_id)
            if request is None:
                raise RuntimeError(f"no pending request {request_id}")
            self._respond(request, "rejected", error=reason or "rejected by coordinator")
            del self._pending[request_id]
            self._save_pending()
        return json.dumps({"request_id": request_id, "status": "rejected"})

    def _respond(self, request: dict, status: str, *, task=None, error=None) -> None:
        payload = {
            "request_id": request["request_id"],
            "worker_id": request["worker_id"],
            "status": status,
        }
        if task is not None:
            payload["task"] = task
        if error is not None:
            payload["error"] = error
        self._client.publish(
            self.config.responses_topic(request["worker_id"]), json.dumps(payload), qos=1
        )

    def publish_snapshot(self) -> None:
        data = _checkpoint_and_read(self.config.db_path)
        payload = json.dumps(
            {"ts": time.time(), "snapshot_b64": base64.b64encode(data).decode()}
        )
        self._client.publish(self.config.state_topic, payload, qos=1, retain=True)

    def send_message(self, worker_id: str, body: str, file_path: str | None = None) -> str:
        _check_message_length(body, self.config.message_max_chars)
        message_id = str(uuid.uuid4())
        payload = {
            "message_id": message_id,
            "from": self.config.client_id,
            "to": worker_id,
            "body": body,
            "ts": time.time(),
        }
        if file_path:
            payload["file"] = _encode_file(file_path, self.config.max_file_bytes)
        self._client.publish(self.config.messages_to_worker_topic(worker_id), json.dumps(payload), qos=1)
        return json.dumps({"message_id": message_id, "status": "sent"})

    def check_messages(self) -> str:
        return json.dumps({"messages": self._inbox.drain()})

    def assign(
        self,
        worker_id: str,
        body: str,
        task_id: int | None = None,
        file_path: str | None = None,
    ) -> str:
        """Publish a work assignment to one worker's own assign topic.
        Not retained — a worker still gets it on reconnect within
        session_expiry_s via its persistent MQTT session, same as any
        other QoS-1 message; retaining would mean a worker that reconnects
        long after finishing the assignment sees it again as if new."""
        _check_message_length(body, self.config.message_max_chars)
        message_id = str(uuid.uuid4())
        payload = {
            "message_id": message_id,
            "from": self.config.client_id,
            "task_id": task_id,
            "body": body,
            "ts": time.time(),
        }
        if file_path:
            payload["file"] = _encode_file(file_path, self.config.max_file_bytes)
        self._client.publish(self.config.assign_topic(worker_id), json.dumps(payload), qos=1)
        return json.dumps({"message_id": message_id, "status": "sent"})

    def broadcast(self, body: str, retain: bool = False, file_path: str | None = None) -> str:
        _check_message_length(body, self.config.message_max_chars)
        message_id = str(uuid.uuid4())
        payload = {
            "message_id": message_id,
            "from": self.config.client_id,
            "body": body,
            "retain": retain,
            "ts": time.time(),
        }
        if file_path:
            payload["file"] = _encode_file(file_path, self.config.max_file_bytes)
        self._client.publish(
            self.config.broadcast_topic(message_id), json.dumps(payload), qos=1, retain=retain
        )
        return json.dumps({"message_id": message_id, "status": "sent", "retained": retain})

    def list_workers(self) -> str:
        with self._lock:
            workers = [{"worker_id": w, **info} for w, info in self._presence.items()]
        return json.dumps({"workers": workers})


class WorkerService:
    """Never writes its own database directly. Every mutation becomes a
    request published to the coordinator; the local db is a disposable
    replica kept current by subscribing to the coordinator's snapshots.
    Also sends/receives direct messages, broadcasts, and assignments, and
    announces presence (an immediate "online" plus a periodic heartbeat and
    any self-reported work-state, backed by an MQTT Last-Will-Testament
    that fires "offline" on an unclean disconnect).
    """

    def __init__(self, config: Config):
        if not config.worker_db_path:
            raise RuntimeError("worker mode requires 'worker_db_path' in the MQTT config")
        self.config = config
        self._lock = threading.Lock()
        self._local: dict[str, dict] = {}
        self._last_synced: float | None = None
        self._work_state: str | None = None
        self._messages = Mailbox(Path(config.worker_db_path).with_suffix(".mqtt-messages.json"))
        self._broadcasts = Mailbox(Path(config.worker_db_path).with_suffix(".mqtt-broadcasts.json"))
        self._assignments = Mailbox(Path(config.worker_db_path).with_suffix(".mqtt-assignments.json"))
        self._ensure_replica_initialized()
        will_payload = json.dumps(
            {"worker_id": config.client_id, "status": "offline", "work_state": None, "ts": time.time()}
        )
        self._client = _make_client(
            config,
            on_connect=self._on_connect,
            will_topic=config.presence_topic(config.client_id),
            will_payload=will_payload,
        )
        self._client.on_message = self._on_message
        self._heartbeat_stop = threading.Event()
        self._heartbeat_thread = threading.Thread(target=self._heartbeat_loop, daemon=True)
        self._heartbeat_thread.start()

    def _on_connect(self, client, _userdata, _connect_flags, _reason_code, _properties):
        client.subscribe(self.config.responses_topic(self.config.client_id), qos=1)
        client.subscribe(self.config.state_topic, qos=1)
        client.subscribe(self.config.messages_to_worker_topic(self.config.client_id), qos=1)
        client.subscribe(self.config.assign_topic(self.config.client_id), qos=1)
        client.subscribe(self.config.broadcast_topic(), qos=1)
        self._publish_presence(client, "online")

    def _publish_presence(self, client, status: str) -> None:
        payload = json.dumps(
            {
                "worker_id": self.config.client_id,
                "status": status,
                "work_state": self._work_state,
                "ts": time.time(),
            }
        )
        client.publish(
            self.config.presence_topic(self.config.client_id), payload, qos=1, retain=True
        )

    def _heartbeat_loop(self) -> None:
        while not self._heartbeat_stop.wait(self.config.heartbeat_interval_s):
            self._publish_presence(self._client, "online")

    def _ensure_replica_initialized(self) -> None:
        # So reads don't hard-fail with "not initialized" before the first
        # snapshot arrives; the coordinator's next publish overwrites this.
        if not Path(self.config.worker_db_path).exists():
            import subprocess

            bin_path = os.environ.get("TODO_SQLITE_CLI_BIN", "todo-sqlite-cli")
            subprocess.run(
                [bin_path, "--db", self.config.worker_db_path, "init"],
                capture_output=True,
                text=True,
                check=True,
            )

    def _on_message(self, _client, _userdata, msg):
        cid = self.config.client_id
        if msg.topic == self.config.responses_topic(cid):
            response = json.loads(msg.payload.decode())
            with self._lock:
                self._local[response["request_id"]] = response
        elif msg.topic == self.config.state_topic:
            payload = json.loads(msg.payload.decode())
            ts = payload["ts"]
            with self._lock:
                # A reconnect can deliver an older snapshot after a newer
                # one: resubscribing (needed so a fresh process or a
                # post-blip reconnect re-establishes cleanly) triggers an
                # immediate retained-message redelivery of the CURRENT
                # snapshot, which can arrive before the persistent
                # session's own queued backlog from the offline window
                # finishes draining — without this guard, "last message
                # wins" would then let that queued-but-stale backlog
                # overwrite the fresher retained one it just applied.
                if self._last_synced is not None and ts <= self._last_synced:
                    return
                data = base64.b64decode(payload["snapshot_b64"])
                _atomic_replace(self.config.worker_db_path, data)
                self._last_synced = ts
        elif msg.topic == self.config.messages_to_worker_topic(cid):
            self._messages.add(self._land_file(json.loads(msg.payload.decode())))
        elif msg.topic == self.config.assign_topic(cid):
            self._assignments.add(self._land_file(json.loads(msg.payload.decode())))
        elif msg.topic.startswith(f"{self.config.topic_prefix}/broadcast/"):
            self._broadcasts.add(self._land_file(json.loads(msg.payload.decode())))

    def _land_file(self, message: dict) -> dict:
        file_field = message.pop("file", None)
        if file_field:
            message["file_path"] = _save_incoming_file(
                self.config.files_dir_path, message["message_id"], file_field
            )
        return message

    def submit(self, op: str, args: dict) -> dict:
        request_id = str(uuid.uuid4())
        request = {
            "request_id": request_id,
            "op": op,
            "args": args,
            "worker_id": self.config.client_id,
            "ts": time.time(),
        }
        with self._lock:
            self._local[request_id] = {"status": "pending"}
        self._client.publish(self.config.requests_topic(self.config.client_id), json.dumps(request), qos=1)
        return {"request_id": request_id, "status": "pending"}

    def check(self, request_id: str) -> dict:
        with self._lock:
            return self._local.get(request_id, {"status": "unknown"})

    def sync_state(self) -> dict:
        with self._lock:
            if self._last_synced is None:
                return {"synced": False, "last_synced": None}
            return {"synced": True, "last_synced": self._last_synced}

    def report_state(self, state: str) -> dict:
        """Update this worker's self-reported work-state (whatever
        convention the fleet agrees on, e.g. idle/busy/blocked) and
        republish presence immediately rather than waiting for the next
        heartbeat tick. Rides the same presence payload as connection
        status, so there's no separate retained "status" topic to manage."""
        self._work_state = state
        self._publish_presence(self._client, "online")
        return {"worker_id": self.config.client_id, "work_state": state}

    def send_message(self, body: str, file_path: str | None = None) -> str:
        _check_message_length(body, self.config.message_max_chars)
        message_id = str(uuid.uuid4())
        payload = {
            "message_id": message_id,
            "from": self.config.client_id,
            "to": "coordinator",
            "body": body,
            "ts": time.time(),
        }
        if file_path:
            payload["file"] = _encode_file(file_path, self.config.max_file_bytes)
        self._client.publish(
            self.config.messages_to_coordinator_topic(self.config.client_id),
            json.dumps(payload),
            qos=1,
        )
        return json.dumps({"message_id": message_id, "status": "sent"})

    def check_messages(self) -> str:
        return json.dumps({"messages": self._messages.drain()})

    def check_broadcasts(self) -> str:
        return json.dumps({"broadcasts": self._broadcasts.drain()})

    def check_assignments(self) -> str:
        return json.dumps({"assignments": self._assignments.drain()})
