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

On top of that plumbing sit a few fleet-control features:

- fleet state: a retained `active|draining|paused` switch the coordinator
  sets; every worker MQTT tool reply carries it, so an agent sees it on its
  next poll without a separate call. No retained state means `active`.
- coordinator presence: the coordinator heartbeats a retained "online"
  (backed by a Last-Will "offline"), and worker replies carry its status.
- a stale-link watchdog: each side counts any inbound broker traffic —
  including its own presence heartbeat echoed back — as proof of life, and
  a mailbox drain on a stale link errors instead of returning a misleading
  empty list.
- directive sequencing: an assign/send_message that names a task carries a
  per-(worker, task) `seq` and a `kind` (approve|hold|go|info); the worker
  marks older approve/hold/go directives for the same task as superseded.
- receipts: a worker acks each message/assignment when its service
  receives it ("delivered") and when its agent drains it ("read").
- a dead-man switch: the coordinator process itself (no model turn needed)
  drains, then pauses, the fleet once the operator's check-in file goes
  stale; a worker whose coordinator has been gone long enough derives a
  local draining/paused on its own.

Only imported when TODO_SQLITE_CLI_MQTT_CONFIG is set, so a standalone
deployment never needs the `paho-mqtt` optional dependency installed.

Topic layout, all namespaced under `topic_prefix` (C = coordinator,
W = worker; "own leaf" = the topic ends in that worker's own client_id):

  requests/<worker_id>                W -> C    write requests (own leaf)
  responses/<worker_id>               C -> W    request outcome
  assign/<worker_id>                  C -> W    work assignment
  messages/to-coordinator/<worker_id> W -> C    direct message (own leaf)
  messages/to-worker/<worker_id>      C -> W    direct message
  presence/<worker_id>                W -> C    online/work-state, retained,
                                                LWT "offline" (own leaf; the
                                                worker also reads it back as
                                                its link-liveness echo)
  receipts/<worker_id>                W -> C    delivered/read acks (own leaf)
  broadcast/<message_id>              C -> all  broadcast (optionally retained)
  state                               C -> all  db snapshot, retained
  fleet/state                         C -> all  active|draining|paused, retained
  coordinator/presence                C -> all  online heartbeat, retained,
                                                LWT "offline" (the coordinator
                                                also reads it back as its own
                                                link-liveness echo)

Every topic a worker publishes to, or subscribes to for messages meant only
for it, is scoped to that worker's own subtopic (keyed by its `client_id`)
rather than one topic shared by every worker. That's deliberate: a broker
ACL can then grant each worker write access to only its own subtopic (the
same isolation principle as the coordinator-owned `fleet/tasks/<node>` /
worker-owned `fleet/workers/<node>` split this sync sits alongside),
instead of requiring a shared write topic every worker must be trusted
with. The coordinator subscribes to the wildcard form of each worker-owned
topic (`.../+`); a worker subscribes only to its own leaf. The topics every
worker reads (`broadcast/#`, `state`, `fleet/state`, `coordinator/presence`)
are coordinator-written only.

`broadcast` similarly gets its own subtopic per message
(`broadcast/<message_id>`) rather than one flat topic, so a retained
publish occupies its own permanent slot instead of silently overwriting
whatever standing announcement was retained there before.

Every new field is optional on the wire, so mixed versions interoperate: an
older worker ignores fleet state/presence/seq and never sends receipts (its
messages just stay "sent" in list_sent); a newer worker against an older
coordinator reports the coordinator as "unknown" and treats its assignments
as unsequenced.
"""

import atexit
import base64
import dataclasses
import hashlib
import json
import os
import sqlite3
import sys
import threading
import time
import uuid
from collections import OrderedDict
from dataclasses import dataclass
from pathlib import Path

import paho.mqtt.client as mqtt

import cli_ops

CONFIG_ENV = "TODO_SQLITE_CLI_MQTT_CONFIG"

FLEET_STATES = ("active", "draining", "paused")
DIRECTIVE_KINDS = ("approve", "hold", "go", "info")
# Kinds that set a task's effective directive, and so supersede any older
# one. "info" is sequenced too, but an FYI must never cancel a go or a hold.
_ACTIONABLE_KINDS = ("approve", "hold", "go")
_SENT_LOG_CAP = 1000
_STATE_RANK = {"active": 0, "draining": 1, "paused": 2}
_OMITTED = "omitted"  # config default sentinel: omitted != explicit null/0

# Read literally by agents (the worker playbook points at this field).
FLEET_INSTRUCTIONS = {
    "draining": (
        "Fleet is draining: finish your current task (or checkpoint it if it can't finish soon) "
        "and do not pick up new work. Then push your work branch, append a state note to the task, "
        "send the coordinator a final message, report_state('offline'), and stop polling."
    ),
    "paused": (
        "Fleet is paused: stop now. Checkpoint your current step, push your work branch, "
        "append a state note to the task, send the coordinator a final message, "
        "report_state('offline'), and stop polling (schedule no more wakeups)."
    ),
}


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
    heartbeat_interval_s: float = 60.0
    # The link counts as stale after this many heartbeat intervals with no
    # inbound broker traffic at all (or immediately on a disconnect).
    stale_after_heartbeats: float = 3
    message_max_chars: int = 4096
    max_file_bytes: int = 1_048_576
    files_dir: str | None = None
    # Dead-man switch (coordinator). Off unless deadman_drain_after_s is set
    # (> 0; 10800 is the recommended value). Pause then defaults to
    # drain + 1h; an explicit 0 or null disables just the pause stage.
    checkin_file: str | None = None
    deadman_drain_after_s: float | None = None
    deadman_pause_after_s: float | None | str = _OMITTED
    # Worker fallback when the coordinator itself is gone. Same convention;
    # paused follows at twice this.
    coordinator_offline_drain_after_s: float | None | str = _OMITTED

    @property
    def password(self) -> str | None:
        return os.environ.get(self.password_env) if self.password_env else None

    @property
    def stale_after_s(self) -> float:
        return self.heartbeat_interval_s * self.stale_after_heartbeats

    @property
    def deadman_drain_s(self) -> float | None:
        return self.deadman_drain_after_s or None

    @property
    def deadman_pause_s(self) -> float | None:
        drain = self.deadman_drain_s
        if not drain:
            return None  # the whole switch is off
        if self.deadman_pause_after_s == _OMITTED:
            return drain + 3600.0
        return self.deadman_pause_after_s or None

    @property
    def coordinator_offline_drain_s(self) -> float | None:
        v = 2 * 3600.0 if self.coordinator_offline_drain_after_s == _OMITTED else self.coordinator_offline_drain_after_s
        return v or None

    @property
    def checkin_path(self) -> Path:
        if self.checkin_file:
            return Path(self.checkin_file).expanduser()
        return _state_home() / "todo-sqlite-cli" / "checkin"

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

    def receipts_topic(self, worker_id: str | None = None) -> str:
        return f"{self.topic_prefix}/receipts/{worker_id or '+'}"

    def broadcast_topic(self, message_id: str | None = None) -> str:
        return f"{self.topic_prefix}/broadcast/{message_id or '#'}"

    @property
    def state_topic(self) -> str:
        return f"{self.topic_prefix}/state"

    @property
    def fleet_state_topic(self) -> str:
        return f"{self.topic_prefix}/fleet/state"

    @property
    def coordinator_presence_topic(self) -> str:
        return f"{self.topic_prefix}/coordinator/presence"

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
        """Where incoming attachments land. Defaults to the XDG state dir,
        not next to the db: a db inside a repo checkout would otherwise put
        attachments where a `git add -A` sweeps them into a commit."""
        if self.files_dir:
            return Path(self.files_dir)
        return _state_home() / "todo-sqlite-cli" / "mqtt-files"


def _state_home() -> Path:
    return Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local" / "state")


_NUMERIC_FIELDS = {
    "port": False,
    "request_timeout_s": False,
    "heartbeat_interval_s": False,
    "stale_after_heartbeats": False,
    "message_max_chars": False,
    "max_file_bytes": False,
    # nullable (null disables):
    "deadman_drain_after_s": True,
    "deadman_pause_after_s": True,
    "coordinator_offline_drain_after_s": True,
}


def _warn(message: str) -> None:
    print(f"todo-sqlite-cli mqtt: {message}", file=sys.stderr, flush=True)


def load_config() -> Config | None:
    """Load the config, tolerating keys this version doesn't know (warned
    about on stderr, then ignored) so a config written for a newer server
    doesn't take the whole MCP server down, but rejecting a malformed
    numeric value (e.g. "3h") loudly rather than misbehaving later."""
    path = os.environ.get(CONFIG_ENV)
    if not path:
        return None
    with open(path) as f:
        raw = json.load(f)
    known = {f.name for f in dataclasses.fields(Config)}
    unknown = sorted(set(raw) - known)
    if unknown:
        _warn(f"ignoring unknown config key(s) in {path}: {', '.join(unknown)}")
    raw = {k: v for k, v in raw.items() if k in known}
    for key, nullable in _NUMERIC_FIELDS.items():
        if key not in raw:
            continue
        v = raw[key]
        if v is None and nullable:
            continue
        if isinstance(v, bool) or not isinstance(v, (int, float)):
            raise RuntimeError(
                f"MQTT config {path}: '{key}' must be a number of "
                f"{'seconds' if key.endswith('_s') else 'units'}{' or null' if nullable else ''}, got {v!r}"
            )
        if key.endswith("_after_s") and v < 0:
            # A typo'd sign would otherwise switch e.g. the dead-man on and
            # have it act on its very first tick.
            raise RuntimeError(f"MQTT config {path}: '{key}' must be >= 0 (0 or null disables it), got {v!r}")
    return Config(**raw)


def _make_client(
    config: Config,
    on_connect,
    on_message,
    on_disconnect=None,
    on_subscribe=None,
    on_publish=None,
    will_topic: str | None = None,
    will_payload: str | None = None,
) -> mqtt.Client:
    """Build a connected, auto-reconnecting MQTT client.

    Uses a clean session (`clean_start=True`) on every connect, including
    automatic reconnects — nodes are LAN-connected (network blips are rare)
    and this is a polling architecture, so a sender just resends if the
    other side doesn't respond, rather than relying on the broker to queue
    messages for an offline client. Subscriptions are established in
    `on_connect`, not here, so they're re-applied identically on first
    connect and on every reconnect. Every callback is attached before
    connecting, so a retained message delivered straight after the first
    subscribe can't slip past an on_message that isn't set yet.
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
    client.on_message = on_message
    if on_disconnect is not None:
        client.on_disconnect = on_disconnect
    if on_subscribe is not None:
        client.on_subscribe = on_subscribe
    if on_publish is not None:
        client.on_publish = on_publish
    client.reconnect_delay_set(min_delay=1, max_delay=30)
    client.connect(config.host, config.port, clean_start=True)
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
    with open(tmp, "wb") as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def _load_json(path: Path, default):
    """Read a JSON state file. A corrupt one (e.g. from a crash on a
    filesystem that lost the write) is set aside as `.corrupt-<ts>` and
    replaced by `default`, rather than crashing the server at startup."""
    if not path.exists():
        return default
    try:
        return json.loads(path.read_text())
    except (json.JSONDecodeError, UnicodeDecodeError) as e:
        aside = path.with_name(f"{path.name}.corrupt-{int(time.time())}")
        os.replace(path, aside)
        _warn(f"{path} was unreadable ({e}); moved it to {aside} and started empty")
        return default


def _save_json(path: Path, data) -> None:
    _atomic_replace(str(path), json.dumps(data).encode())


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


def _iso(ts: float) -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(ts))


def _fleet_state_view(state: dict | None) -> dict:
    """The fleet state as tool replies report it: no retained state on the
    broker means `active` (so a fleet that never uses the switch, or a
    coordinator too old to publish it, behaves exactly as before)."""
    if state is None:
        view = {"state": "active", "note": None, "set_at": None, "set_by": None, "source": "default"}
    else:
        view = {**state, "source": "retained"}
    view["instruction"] = FLEET_INSTRUCTIONS.get(view["state"])
    return view


_WORKER_ADVICE = "Do not act on silence; report_state and retry next poll."
_COORDINATOR_ADVICE = "Do not act on silence; retry next poll."
_ACL_ADVICE = (
    "The broker ACL predates this server version (README 'MQTT sync' rollout: update the ACL first, "
    "then the code), so this node can't tell a quiet link from a dead one. Do not act on silence; "
    "report this to the operator and retry next poll."
)


def _acl_error(link: dict) -> RuntimeError:
    return RuntimeError(f"MQTT {link['error']}. {_ACL_ADVICE}")


def _stale_link_error(link: dict, consequence: str, advice: str) -> RuntimeError:
    last = f"{link['age_s']}s ago" if link["age_s"] is not None else "never"
    return RuntimeError(
        f"MQTT link stale since {_iso(link['stale_since'])} (last broker traffic {last}; "
        f"connected={link['connected']}): {consequence}. {advice}"
    )


def _drained_reply(key: str, items: list, context: dict, advice: str, *, raise_if_empty: bool = True) -> dict:
    """Shape a mailbox drain's reply. On a stale link an empty drain is an
    error, not an empty list — silence there means "nothing arrived", not
    "nothing was sent". A non-empty drain is still returned (the items are
    already consumed), flagged with `link_stale_since`."""
    link = context["link"]
    if not items and raise_if_empty:
        if link["error"]:
            raise _acl_error(link)
        if link["stale"]:
            raise _stale_link_error(link, f"an empty {key} result is not trustworthy", advice)
    reply = {key: items, **context}
    if link["stale"]:
        reply["link_stale_since"] = link["stale_since"]
    return reply


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
        return _load_json(self._path, [])

    def _save(self) -> None:
        _save_json(self._path, self._items)

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


class LinkMonitor:
    """Tracks whether this node's broker link is actually carrying traffic.

    paho reporting "connected" isn't enough on its own: on 2026-09-26 a
    coordinator link went silent for ~70 minutes while local MCP calls kept
    succeeding and mailboxes kept draining empty. So each side subscribes
    to its own retained presence topic — every heartbeat it publishes comes
    straight back through the broker — and counts ANY inbound message as
    proof of life. No inbound traffic for `stale_after_s`, or a disconnect,
    means the link is stale.
    """

    def __init__(self, stale_after_s: float):
        self._stale_after_s = stale_after_s
        self._lock = threading.Lock()
        self._started_at = time.time()
        self._connected = False
        self._disconnected_at: float | None = None
        self._last_rx_at: float | None = None
        self._denied: dict[str, str] = {}
        self._echo_topic: str | None = None
        self._acks_since_echo = 0

    def subscription_result(self, topic: str, denied_reason: str | None) -> None:
        with self._lock:
            if denied_reason is None:
                self._denied.pop(topic, None)
            else:
                self._denied[topic] = denied_reason

    def rx(self) -> None:
        with self._lock:
            self._last_rx_at = time.time()

    def presence_acked(self, echo_topic: str) -> None:
        """The broker PUBACKed one of our presence heartbeats: that's a
        round trip, so it counts as traffic. It also means the echo of it
        should arrive; see echo()."""
        with self._lock:
            self._last_rx_at = time.time()
            self._echo_topic = echo_topic
            self._acks_since_echo += 1

    def echo(self) -> None:
        with self._lock:
            self._last_rx_at = time.time()
            self._acks_since_echo = 0

    def connected(self) -> None:
        with self._lock:
            self._connected = True
            self._disconnected_at = None

    def disconnected(self) -> None:
        with self._lock:
            if self._connected:
                self._connected = False
                self._disconnected_at = time.time()

    def status(self) -> dict:
        now = time.time()
        with self._lock:
            last = self._last_rx_at
            quiet_since = last if last is not None else self._started_at
            candidates = []
            if not self._connected:
                candidates.append(self._disconnected_at or self._started_at)
            if now - quiet_since > self._stale_after_s:
                candidates.append(quiet_since + self._stale_after_s)
            stale_since = min(candidates) if candidates else None
            denied = dict(self._denied)
            # mosquitto accepts a subscription its ACL won't let it deliver
            # (SUBACK success, then silence), so a missing read grant shows
            # up only as heartbeats the broker acks but never echoes back.
            if self._echo_topic and self._acks_since_echo >= 3 and self._echo_topic not in denied:
                denied[self._echo_topic] = (
                    f"{self._acks_since_echo} heartbeats acked by the broker but never echoed back"
                )
            error = None
            if denied:
                error = "; ".join(f"subscription to {t} denied by broker ACL ({r})" for t, r in sorted(denied.items()))
            return {
                "connected": self._connected,
                "last_rx_at": last,
                "age_s": round(now - last, 1) if last is not None else None,
                "stale": stale_since is not None,
                "stale_since": stale_since,
                "denied_subscriptions": denied,
                "error": error,
            }


class _Node:
    """Heartbeat and shutdown plumbing shared by both roles. A subclass sets
    `config`, `_link` and `_client`, and defines `_publish_presence`."""

    def _on_disconnect(self, _client, _userdata, _flags, _reason_code, _properties):
        self._link.disconnected()

    def _subscribe(self, client, topic: str) -> None:
        # on_subscribe runs on the same network thread as on_connect (where
        # every subscribe happens), so it can't fire before mid is recorded.
        _result, mid = client.subscribe(topic, qos=1)
        self._pending_subs[mid] = topic

    def _on_subscribe(self, _client, _userdata, mid, reason_codes, _properties):
        topic = self._pending_subs.pop(mid, None)
        if topic is None:
            return
        rc = reason_codes[0]
        self._link.subscription_result(topic, str(rc) if rc.is_failure else None)
        if rc.is_failure:
            _warn(f"subscription to {topic} denied by the broker ({rc})")
        self._subscribed(topic, not rc.is_failure)

    def _subscribed(self, topic: str, ok: bool) -> None:
        pass

    def _own_presence_topic(self) -> str:
        raise NotImplementedError

    def _on_publish(self, _client, _userdata, mid, reason_code, _properties):
        topic = self._presence_mids.pop(mid, None)
        if topic is not None and not reason_code.is_failure:
            self._link.presence_acked(topic)

    def _start_heartbeat(self) -> None:
        self._heartbeat_stop = threading.Event()
        self._heartbeat_thread = threading.Thread(target=self._heartbeat_loop, daemon=True)
        self._heartbeat_thread.start()
        atexit.register(self._shutdown)

    def _heartbeat_loop(self) -> None:
        while not self._heartbeat_stop.wait(self.config.heartbeat_interval_s):
            info = self._publish_presence(self._client, "online")
            # Only heartbeats feed the missing-echo (ACL) detector: they come
            # at a steady rate, whereas a burst of report_state publishes
            # could outrun their echoes and raise a transient ACL error.
            self._presence_mids[info.mid] = self._own_presence_topic()
            try:
                self._on_heartbeat()
            except Exception as e:
                # Never let a hook kill the heartbeat (the node would look
                # offline), but don't hide the failure either.
                _warn(f"heartbeat hook failed: {e!r}")

    def _on_heartbeat(self) -> None:
        pass

    def _shutdown(self) -> None:
        """On a normal exit (e.g. the MCP client closing stdin), publish an
        explicit "offline" rather than leaving a stale "online" retained.
        A clean DISCONNECT suppresses the Last-Will, so only disconnect once
        that publish has actually gone out; otherwise just let the socket
        drop and the broker fire the will instead."""
        self._heartbeat_stop.set()
        self._report_unsent()
        try:
            info = self._publish_presence(self._client, "offline")
            info.wait_for_publish(timeout=2)
            if info.is_published():
                self._client.disconnect()
        except Exception:
            pass

    def _report_unsent(self) -> None:
        pass


class CoordinatorService(_Node):
    """Owns the master db. Holds a pending-request queue fed by workers;
    nothing is applied until the coordinator's agent calls approve/reject.
    Also fields direct messages/broadcasts/assignments, tracks worker
    presence (including each worker's self-reported work-state), sets the
    fleet state, stamps per-task directive sequence numbers, and records
    delivery/read receipts for what it sends.
    """

    def __init__(self, config: Config, run):
        if not config.db_path:
            raise RuntimeError("coordinator mode requires 'db_path' in the MQTT config")
        self.config = config
        self.run = run
        self._lock = threading.Lock()
        base = Path(config.db_path)
        self._pending_path = base.with_suffix(".mqtt-pending.json")
        self._pending: dict[str, dict] = self._load_pending()
        self._inbox = Mailbox(base.with_suffix(".mqtt-inbox.json"))
        self._seq_path = base.with_suffix(".mqtt-directive-seq.json")
        self._seqs: dict[str, int] = _load_json(self._seq_path, {})
        self._sent_path = base.with_suffix(".mqtt-sent.json")
        self._sent: dict[str, dict] = _load_json(self._sent_path, {})
        self._presence: dict[str, dict] = {}
        self._snapshot_lock = threading.Lock()
        self._last_snapshot = None  # (sha256 hex, MQTTMessageInfo) of the last snapshot published
        self._fleet_state: dict | None = None
        # The dead-man switch must not act on a fleet state it hasn't
        # actually learned from the broker yet (see _deadman_tick).
        self._fleet_state_known = False
        self._fleet_sub_acked = False
        self._fleet_publish = None  # MQTTMessageInfo of the last fleet/state publish
        self._started_at = time.time()
        self._pending_subs: dict[int, str] = {}
        self._presence_mids: dict[int, str] = {}
        self._link = LinkMonitor(config.stale_after_s)
        will_payload = json.dumps({"client_id": config.client_id, "status": "offline", "ts": None})
        self._client = _make_client(
            config,
            on_connect=self._on_connect,
            on_message=self._on_message,
            on_disconnect=self._on_disconnect,
            on_subscribe=self._on_subscribe,
            on_publish=self._on_publish,
            will_topic=config.coordinator_presence_topic,
            will_payload=will_payload,
        )
        self._start_heartbeat()

    def _on_connect(self, client, _userdata, _connect_flags, reason_code, _properties):
        if reason_code.is_failure:
            return
        self._link.connected()
        self._subscribe(client, self.config.requests_topic())
        self._subscribe(client, self.config.messages_to_coordinator_topic())
        self._subscribe(client, self.config.presence_wildcard)
        self._subscribe(client, self.config.receipts_topic())
        # Read back our own retained topics: fleet/state so a restarted
        # coordinator knows the state it last set, and coordinator/presence
        # as the link-liveness echo of our own heartbeat. Order matters:
        # the broker delivers fleet/state's retained message (if any)
        # before the echo of the presence published below, so once that
        # echo arrives, the fleet state is known even if nothing was
        # retained.
        self._subscribe(client, self.config.fleet_state_topic)
        self._subscribe(client, self.config.coordinator_presence_topic)
        self._publish_presence(client, "online")

    def _subscribed(self, topic: str, ok: bool) -> None:
        if topic == self.config.fleet_state_topic and ok:
            self._fleet_sub_acked = True

    def _on_disconnect(self, client, userdata, flags, reason_code, properties):
        # Re-learn the retained fleet state after every reconnect before the
        # dead-man may act: it can have changed while we were away.
        self._fleet_state_known = False
        self._fleet_sub_acked = False
        super()._on_disconnect(client, userdata, flags, reason_code, properties)

    def _publish_presence(self, client, status: str):
        payload = json.dumps(
            {
                "client_id": self.config.client_id,
                "status": status,
                "ts": time.time(),
                "heartbeat_interval_s": self.config.heartbeat_interval_s,
            }
        )
        return client.publish(self.config.coordinator_presence_topic, payload, qos=1, retain=True)

    def _own_presence_topic(self) -> str:
        return self.config.coordinator_presence_topic

    def _load_pending(self) -> dict:
        return _load_json(self._pending_path, {})

    def _save_pending(self) -> None:
        _save_json(self._pending_path, self._pending)

    def _on_message(self, _client, _userdata, msg):
        self._link.rx()
        prefix = self.config.topic_prefix
        if msg.topic == self.config.fleet_state_topic:
            with self._lock:
                self._fleet_state = json.loads(msg.payload.decode()) if msg.payload else None
                self._fleet_state_known = True
            return
        if msg.topic == self.config.coordinator_presence_topic:
            # Our own heartbeat echo: the rx() above is its main purpose.
            self._link.echo()
            if self._fleet_sub_acked:
                self._fleet_state_known = True
            return
        if not msg.payload:
            # A cleared retained topic (e.g. delete_broadcast) — nothing to parse.
            return
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
        elif msg.topic.startswith(f"{prefix}/receipts/"):
            self._record_receipt(msg.topic.rsplit("/", 1)[1], json.loads(msg.payload.decode()))

    def _record_receipt(self, worker_id: str, receipt: dict) -> None:
        field = {"delivered": "delivered_at", "read": "read_at"}.get(receipt.get("event"))
        if field is None:
            return
        now = time.time()
        with self._lock:
            changed = False
            for message_id in receipt.get("message_ids", []):
                entry = self._sent.get(message_id)
                # Only the addressee's own receipts leaf may ack a message.
                if entry is None or entry["worker_id"] != worker_id:
                    continue
                if entry.get(field) is None:
                    entry[field] = now
                    changed = True
                if field == "read_at" and entry.get("delivered_at") is None:
                    entry["delivered_at"] = now
                    changed = True
            if changed:
                _save_json(self._sent_path, self._sent)

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

    def publish_snapshot(self, dedupe: bool = False) -> bool:
        """Publish the whole db as the retained state snapshot. Returns
        whether it actually published.

        dedupe=True (the pre-directive path) skips the publish when the
        checkpointed bytes hash the same as the last snapshot published, and
        that publish was accepted by paho (sent, or in flight on a live
        connection), so a burst of directives with no db change in between
        costs one snapshot rather than one each (~8 MB apiece on a large
        db). The approve/write paths stay unconditional: a real change
        always goes out, and they also refresh the hash dedupe compares to."""
        data = _checkpoint_and_read(self.config.db_path)
        digest = hashlib.sha256(data).hexdigest()
        with self._snapshot_lock:
            last = self._last_snapshot
            if dedupe and last is not None and last[0] == digest and (
                last[1].is_published() or last[1].rc == mqtt.MQTT_ERR_SUCCESS
            ):
                return False
            payload = json.dumps(
                {"ts": time.time(), "snapshot_b64": base64.b64encode(data).decode()}
            )
            info = self._client.publish(self.config.state_topic, payload, qos=1, retain=True)
            self._last_snapshot = (digest, info)
            return True

    # -- directives ---------------------------------------------------------

    def _directive(self, worker_id: str, task_id, kind: str | None, default_kind: str) -> dict:
        """The sequencing fields for an assign/send_message: {} (the old,
        unsequenced shape) when it names no task. The task is resolved
        through the master db, so a display id and a uuid for the same task
        share one sequence.

        seq is strictly increasing per (worker, task) but not dense: it's
        max(last + 1, now in ms). The last value is persisted, but a lost or
        reset counter file must still never hand out a seq below one a
        worker has already seen — that would make a fresh `hold` look
        superseded by the older `go` it was meant to cancel."""
        if kind is not None and kind not in DIRECTIVE_KINDS:
            raise RuntimeError(f"unknown directive kind '{kind}' (expected {'|'.join(DIRECTIVE_KINDS)})")
        if task_id is None:
            if kind is not None:
                raise RuntimeError("kind requires task_id: directives are sequenced per task")
            return {}
        task = json.loads(self.run("show", str(task_id), "--format", "json"))
        key = f"{worker_id}|{task['uuid']}"
        with self._lock:
            seq = max(self._seqs.get(key, 0) + 1, time.time_ns() // 1_000_000)
            self._seqs[key] = seq
            _save_json(self._seq_path, self._seqs)
        kind = kind or default_kind
        # Publish a snapshot first (same connection, so it arrives first):
        # a task added straight through the CLI on this host would otherwise
        # be missing from the worker's replica, and latest_directive there
        # couldn't resolve it. Not for an info, which never gates ok_to_act
        # and so doesn't need the replica row; and deduped, so an unchanged
        # db isn't re-sent per directive.
        if kind != "info":
            self.publish_snapshot(dedupe=True)
        return {"task_id": task["id"], "task_uuid": task["uuid"], "seq": seq, "kind": kind}

    def _record_sent(self, message_id: str, worker_id: str, channel: str, directive: dict, sent_at: float) -> None:
        with self._lock:
            self._sent[message_id] = {
                "message_id": message_id,
                "worker_id": worker_id,
                "channel": channel,
                "task_id": directive.get("task_id"),
                "task_uuid": directive.get("task_uuid"),
                "seq": directive.get("seq"),
                "kind": directive.get("kind"),
                "sent_at": sent_at,
                "delivered_at": None,
                "read_at": None,
            }
            while len(self._sent) > _SENT_LOG_CAP:
                del self._sent[next(iter(self._sent))]
            _save_json(self._sent_path, self._sent)

    def _sent_reply(self, message_id: str, directive: dict) -> str:
        reply = {"message_id": message_id, "status": "sent", **directive}
        # Warn, don't refuse: a `hold` or `info` is exactly what the
        # coordinator sends while pausing, so only go/approve get flagged.
        state = _fleet_state_view(self._fleet_state)["state"]
        if directive.get("kind") in ("go", "approve") and state != "active":
            reply["warning"] = (
                f"fleet state is '{state}'; sent a '{directive['kind']}' anyway. "
                "Workers see the fleet state on every poll."
            )
        return json.dumps(reply)

    def send_message(
        self,
        worker_id: str,
        body: str,
        file_path: str | None = None,
        task_id=None,
        kind: str | None = None,
    ) -> str:
        _check_message_length(body, self.config.message_max_chars)
        directive = self._directive(worker_id, task_id, kind, "info")
        message_id = str(uuid.uuid4())
        now = time.time()
        payload = {
            "message_id": message_id,
            "from": self.config.client_id,
            "to": worker_id,
            "body": body,
            "ts": now,
            **directive,
        }
        if file_path:
            payload["file"] = _encode_file(file_path, self.config.max_file_bytes)
        # Record before publishing: the worker's "delivered" receipt can
        # otherwise arrive before there's an entry to attach it to.
        self._record_sent(message_id, worker_id, "message", directive, now)
        self._client.publish(self.config.messages_to_worker_topic(worker_id), json.dumps(payload), qos=1)
        return self._sent_reply(message_id, directive)

    def check_messages(self) -> str:
        context = {"fleet_state": _fleet_state_view(self._fleet_state), "link": self._link.status()}
        return json.dumps(_drained_reply("messages", self._inbox.drain(), context, _COORDINATOR_ADVICE))

    def assign(
        self,
        worker_id: str,
        body: str,
        task_id=None,
        file_path: str | None = None,
        kind: str | None = None,
    ) -> str:
        """Publish a work assignment to one worker's own assign topic.
        Not retained — this is a polling architecture, so a coordinator
        that doesn't see the assignment acted on just re-sends it; retaining
        would mean a worker that reconnects long after finishing the
        assignment sees it again as if new."""
        _check_message_length(body, self.config.message_max_chars)
        directive = self._directive(worker_id, task_id, kind, "go")
        message_id = str(uuid.uuid4())
        now = time.time()
        payload = {
            "message_id": message_id,
            "from": self.config.client_id,
            "task_id": task_id,
            "body": body,
            "ts": now,
            **directive,
        }
        if file_path:
            payload["file"] = _encode_file(file_path, self.config.max_file_bytes)
        self._record_sent(message_id, worker_id, "assignment", directive, now)
        self._client.publish(self.config.assign_topic(worker_id), json.dumps(payload), qos=1)
        return self._sent_reply(message_id, directive)

    def list_sent(self, worker_id: str | None = None, unread_only: bool = False) -> str:
        with self._lock:
            entries = [
                dict(e) for e in self._sent.values() if worker_id is None or e["worker_id"] == worker_id
            ]
        for e in entries:
            e["status"] = "read" if e["read_at"] else "delivered" if e["delivered_at"] else "sent"
        if unread_only:
            entries = [e for e in entries if e["status"] != "read"]
        return json.dumps({"sent": entries, "link": self._link.status()})

    # -- fleet state --------------------------------------------------------

    def _publish_fleet_state(self, state: str, note: str | None, set_by: str) -> dict:
        payload = {"state": state, "note": note, "set_at": time.time(), "set_by": set_by}
        info = self._client.publish(self.config.fleet_state_topic, json.dumps(payload), qos=1, retain=True)
        with self._lock:
            self._fleet_state = payload
            self._fleet_state_known = True
            self._fleet_publish = (info, payload)
        return payload

    def _report_unsent(self) -> None:
        with self._lock:
            pending = self._fleet_publish
        if pending is not None and not pending[0].is_published():
            info, payload = pending
            try:
                info.wait_for_publish(timeout=2)
            except Exception:
                pass
            if not info.is_published():
                _warn(
                    f"exiting before fleet/state '{payload['state']}' (set_by {payload['set_by']}, "
                    f"at {_iso(payload['set_at'])}) reached the broker; it was NOT published"
                )

    def set_fleet_state(self, state: str, note: str | None = None) -> str:
        if state not in FLEET_STATES:
            raise RuntimeError(f"unknown fleet state '{state}' (expected {'|'.join(FLEET_STATES)})")
        payload = self._publish_fleet_state(state, note, self.config.client_id)
        reply = {**_fleet_state_view(payload), "deadman": self._deadman_view(), "link": self._link.status()}
        if reply["link"]["stale"]:
            reply["warning"] = "MQTT link is stale: workers may not see this until it recovers."
        return json.dumps(reply)

    def get_fleet_state(self) -> str:
        return json.dumps(
            {**_fleet_state_view(self._fleet_state), "deadman": self._deadman_view(), "link": self._link.status()}
        )

    # -- dead-man switch ----------------------------------------------------

    def _last_checkin(self) -> float | None:
        try:
            return self.config.checkin_path.stat().st_mtime
        except OSError:
            return None

    def _deadman_view(self) -> dict:
        """When the dead-man switch will act. The timer runs from the later
        of the last check-in and this process's start, so a fresh start
        (or a missing check-in file) never drains the fleet instantly."""
        drain_s, pause_s = self.config.deadman_drain_s, self.config.deadman_pause_s
        last = self._last_checkin()
        since = max(last or 0.0, self._started_at)
        return {
            "enabled": bool(drain_s or pause_s),
            "checkin_file": str(self.config.checkin_path),
            "last_checkin": last,
            "timer_from": since,
            "drains_at": since + drain_s if drain_s else None,
            "pauses_at": since + pause_s if pause_s else None,
        }

    def _on_heartbeat(self) -> None:
        self._deadman_tick()

    def _deadman_tick(self) -> None:
        """Runs on the heartbeat thread, so it works while the coordinator's
        agent is idle. Only ever moves the fleet toward draining/paused:
        never relaxes a state (a hand-set pause stays paused), and never
        resumes on a check-in, which only resets the timer."""
        view = self._deadman_view()
        if not view["enabled"]:
            return
        link = self._link.status()
        # Only act on a fleet state actually learned from the broker, over
        # a live link: a local None/stale copy could otherwise read as
        # "active" and a queued draining would overwrite, on reconnect, a
        # pause someone set by hand in the meantime.
        if not self._fleet_state_known or link["stale"] or link["error"]:
            return
        now = time.time()
        if view["pauses_at"] is not None and now >= view["pauses_at"]:
            target = "paused"
        elif view["drains_at"] is not None and now >= view["drains_at"]:
            target = "draining"
        else:
            return
        with self._lock:
            current = _fleet_state_view(self._fleet_state)["state"]
        if _STATE_RANK[target] <= _STATE_RANK[current]:
            return
        checked_in = view["last_checkin"] is not None and view["last_checkin"] >= self._started_at
        since = _iso(view["timer_from"]) if checked_in else f"coordinator start at {_iso(view['timer_from'])}"
        note = f"dead-man: no check-in since {since}"
        _warn(f"dead-man switch: setting fleet state {target} ({note})")
        self._publish_fleet_state(target, note, "deadman")

    def checkin(self, note: str | None = None) -> str:
        path = self.config.checkin_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"ts": time.time(), "note": note, "via": "checkin tool"}) + "\n")
        return json.dumps({"deadman": self._deadman_view(), "fleet_state": _fleet_state_view(self._fleet_state)})

    # -- broadcasts / presence ----------------------------------------------

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

    def delete_broadcast(self, message_id: str) -> str:
        """Clear a retained broadcast from the broker so new (re)connecting
        workers stop receiving it. Publishing a zero-length payload to a
        retained topic is the standard MQTT way to erase what's retained
        there. Currently-subscribed workers do receive that empty publish,
        and both sides drop empty payloads without parsing them.
        Has no effect on a broadcast that wasn't published with retain=True,
        and no effect on workers that already drained it."""
        self._client.publish(self.config.broadcast_topic(message_id), payload=None, qos=1, retain=True)
        return json.dumps({"message_id": message_id, "status": "deleted"})

    def list_workers(self) -> str:
        now = time.time()
        with self._lock:
            workers = [
                {"worker_id": w, **info, "age_s": round(now - info["ts"], 1)}
                for w, info in self._presence.items()
            ]
        return json.dumps({"workers": workers, "deadman": self._deadman_view(), "link": self._link.status()})


class WorkerService(_Node):
    """Never writes its own database directly. Every mutation becomes a
    request published to the coordinator; the local db is a disposable
    replica kept current by subscribing to the coordinator's snapshots.
    Also sends/receives direct messages, broadcasts, and assignments, and
    announces presence (an immediate "online" plus a periodic heartbeat and
    any self-reported work-state, backed by an MQTT Last-Will-Testament
    that fires "offline" on an unclean disconnect). Tracks the fleet state,
    the coordinator's presence and its own link health, and reports all
    three on every reply.
    """

    def __init__(self, config: Config, run):
        if not config.worker_db_path:
            raise RuntimeError("worker mode requires 'worker_db_path' in the MQTT config")
        self.config = config
        self.run = run  # the CLI, pinned to the local replica
        self._lock = threading.Lock()
        self._started_at = time.time()
        self._pending_subs: dict[int, str] = {}
        self._presence_mids: dict[int, str] = {}
        self._local: dict[str, dict] = {}
        self._last_synced: float | None = None
        self._work_state: str | None = None
        base = Path(config.worker_db_path)
        self._messages = Mailbox(base.with_suffix(".mqtt-messages.json"))
        self._broadcasts = Mailbox(base.with_suffix(".mqtt-broadcasts.json"))
        self._assignments = Mailbox(base.with_suffix(".mqtt-assignments.json"))
        # task_uuid -> the newest approve/hold/go directive seen for it.
        self._directives_path = base.with_suffix(".mqtt-directives.json")
        self._directives: dict[str, dict] = _load_json(self._directives_path, {})
        self._fleet_state: dict | None = None
        self._coordinator: dict | None = None
        self._link = LinkMonitor(config.stale_after_s)
        self._ensure_replica_initialized()
        will_payload = json.dumps(
            {"worker_id": config.client_id, "status": "offline", "work_state": None, "ts": time.time()}
        )
        self._client = _make_client(
            config,
            on_connect=self._on_connect,
            on_message=self._on_message,
            on_disconnect=self._on_disconnect,
            on_subscribe=self._on_subscribe,
            on_publish=self._on_publish,
            will_topic=config.presence_topic(config.client_id),
            will_payload=will_payload,
        )
        self._start_heartbeat()

    def _on_connect(self, client, _userdata, _connect_flags, reason_code, _properties):
        if reason_code.is_failure:
            return
        self._link.connected()
        cid = self.config.client_id
        self._subscribe(client, self.config.responses_topic(cid))
        self._subscribe(client, self.config.state_topic)
        self._subscribe(client, self.config.messages_to_worker_topic(cid))
        self._subscribe(client, self.config.assign_topic(cid))
        self._subscribe(client, self.config.broadcast_topic())
        self._subscribe(client, self.config.fleet_state_topic)
        self._subscribe(client, self.config.coordinator_presence_topic)
        # Our own presence leaf, read back as the link-liveness echo. An
        # ACL from before this version grants workers write-only here; the
        # denied SUBACK then shows up as link.error rather than as an
        # eternally stale link.
        self._subscribe(client, self.config.presence_topic(cid))
        self._publish_presence(client, "online")

    def _publish_presence(self, client, status: str):
        payload = json.dumps(
            {
                "worker_id": self.config.client_id,
                "status": status,
                "work_state": self._work_state,
                "ts": time.time(),
            }
        )
        return client.publish(self._own_presence_topic(), payload, qos=1, retain=True)

    def _own_presence_topic(self) -> str:
        return self.config.presence_topic(self.config.client_id)

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

    def _on_message(self, client, _userdata, msg):
        self._link.rx()
        cid = self.config.client_id
        if msg.topic == self.config.fleet_state_topic:
            with self._lock:
                self._fleet_state = json.loads(msg.payload.decode()) if msg.payload else None
            return
        if msg.topic == self.config.coordinator_presence_topic:
            if msg.payload:
                self._note_coordinator(json.loads(msg.payload.decode()), msg.retain)
            else:
                # Retained presence cleared (e.g. after rolling the
                # coordinator back to a version that doesn't publish it):
                # no information, so "unknown" rather than the last status.
                with self._lock:
                    self._coordinator = None
            return
        if not msg.payload:
            # A cleared retained topic (e.g. delete_broadcast) — nothing to parse.
            return
        if msg.topic == self.config.presence_topic(cid):
            self._link.echo()  # our own heartbeat echo
        elif msg.topic == self.config.responses_topic(cid):
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
            self._receive(client, self._messages, "message", msg)
        elif msg.topic == self.config.assign_topic(cid):
            self._receive(client, self._assignments, "assignment", msg)
        elif msg.topic.startswith(f"{self.config.topic_prefix}/broadcast/"):
            self._broadcasts.add(self._land_file(json.loads(msg.payload.decode())))

    def _receive(self, client, mailbox: Mailbox, channel: str, msg) -> None:
        item = self._land_file(json.loads(msg.payload.decode()))
        self._note_directive(item, channel)
        mailbox.add(item)
        self._publish_receipt(client, "delivered", channel, [item["message_id"]])

    def _publish_receipt(self, client, event: str, channel: str, message_ids: list[str]) -> None:
        payload = {
            "worker_id": self.config.client_id,
            "event": event,
            "channel": channel,
            "message_ids": message_ids,
            "ts": time.time(),
        }
        client.publish(self.config.receipts_topic(self.config.client_id), json.dumps(payload), qos=1)

    def _note_coordinator(self, payload: dict, retained: bool) -> None:
        """A live heartbeat is stamped with its receipt time (immune to
        clock skew between nodes); a retained one replayed on subscribe can
        only be aged by its own ts. An "offline" keeps the last online
        sighting as last_seen — a Last-Will's ts is fixed at connect time,
        so it says nothing about when the coordinator actually went away."""
        with self._lock:
            prev = self._coordinator or {}
            if payload.get("status") == "online":
                self._coordinator = {
                    "status": "online",
                    "last_seen": payload.get("ts") if retained else time.time(),
                    "heartbeat_interval_s": payload.get("heartbeat_interval_s"),
                }
            else:
                # When it went offline: now, for a live notice. A retained
                # one replayed on subscribe only has its payload ts (none at
                # all for a Last-Will), so fall back to now: that can only
                # delay the offline fallback, never trigger it early.
                offline_since = (payload.get("ts") if retained else None) or time.time()
                self._coordinator = {
                    "status": payload.get("status") or "offline",
                    "last_seen": prev.get("last_seen") or payload.get("ts"),
                    "heartbeat_interval_s": prev.get("heartbeat_interval_s"),
                    "offline_since": prev.get("offline_since") if prev.get("status") == "offline" else offline_since,
                }

    def _coordinator_view(self) -> dict:
        """online | offline | unknown, plus how long ago it was last heard
        from. "online" needs a heartbeat inside the stale window; an online
        whose heartbeats stopped (or a coordinator too old to publish
        presence at all) is "unknown", not assumed alive."""
        now = time.time()
        with self._lock:
            c = dict(self._coordinator) if self._coordinator else None
        if c is None:
            return {"status": "unknown", "last_seen": None, "age_s": None}
        last_seen = c.get("last_seen")
        age = round(now - last_seen, 1) if last_seen else None
        status = c["status"]
        if status == "online":
            interval = c.get("heartbeat_interval_s") or self.config.heartbeat_interval_s
            if age is None or age > interval * self.config.stale_after_heartbeats:
                status = "unknown"
        return {"status": status, "last_seen": last_seen, "age_s": age}

    def _coordinator_gone_since(self) -> float | None:
        """Since when the coordinator has been gone: an "offline" notice,
        or an "online" whose heartbeats stopped (which also covers this
        worker's own link being down). None if it's around, or if no
        presence was ever seen: an old coordinator that never publishes
        presence must not trip the offline fallback."""
        with self._lock:
            c = dict(self._coordinator) if self._coordinator else None
        if c is None:
            return None
        since = None
        if c["status"] != "online":
            since = c.get("offline_since") or c.get("last_seen") or time.time()
        else:
            last_seen = c.get("last_seen")
            interval = c.get("heartbeat_interval_s") or self.config.heartbeat_interval_s
            if last_seen is not None and time.time() - last_seen > interval * self.config.stale_after_heartbeats:
                since = last_seen
        # Floor at this process's start, like the dead-man timer: a worker
        # started long after the coordinator went away gets the full grace
        # period rather than deriving paused on its very first poll.
        return max(since, self._started_at) if since is not None else None

    def _effective_fleet_state(self) -> dict:
        """The retained fleet state, unless the coordinator has been gone
        past coordinator_offline_drain_after_s: then a locally derived
        draining (paused at twice that). Nothing is published, and the
        derived state lapses as soon as the coordinator is back."""
        with self._lock:
            retained = _fleet_state_view(self._fleet_state)
        drain_s = self.config.coordinator_offline_drain_s
        gone_since = self._coordinator_gone_since()
        if not drain_s or gone_since is None:
            return retained
        gone_for = time.time() - gone_since
        if gone_for < drain_s:
            return retained
        derived = "paused" if gone_for >= 2 * drain_s else "draining"
        if _STATE_RANK[derived] <= _STATE_RANK[retained["state"]]:
            return retained
        return {
            "state": derived,
            "note": f"coordinator gone since {_iso(gone_since)}",
            "set_at": None,
            "set_by": None,
            "source": "coordinator-offline",
            "since": gone_since,
            "retained": {k: v for k, v in retained.items() if k != "instruction"},
            "instruction": FLEET_INSTRUCTIONS[derived],
        }

    def _context(self) -> dict:
        return {
            "fleet_state": self._effective_fleet_state(),
            "coordinator": self._coordinator_view(),
            "link": self._link.status(),
        }

    def _note_directive(self, item: dict, channel: str) -> None:
        seq, task_uuid = item.get("seq"), item.get("task_uuid")
        if seq is None or task_uuid is None or item.get("kind") not in _ACTIONABLE_KINDS:
            return
        with self._lock:
            current = self._directives.get(task_uuid)
            if current is not None and current["seq"] >= seq:
                return  # a duplicate, or an older directive arriving late
            self._directives[task_uuid] = {
                "task_id": item.get("task_id"),
                "task_uuid": task_uuid,
                "seq": seq,
                "kind": item["kind"],
                "message_id": item["message_id"],
                "body": item["body"],
                "ts": item.get("ts"),
                "channel": channel,
            }
            _save_json(self._directives_path, self._directives)

    def _mark_superseded(self, items: list[dict]) -> None:
        """Flag each sequenced item as superseded or not — superseded means
        a newer approve/hold/go for the same task has arrived (on either
        channel; both share one per-task sequence). Unsequenced items (no
        task, or from an older coordinator) get no flag at all."""
        with self._lock:
            for item in items:
                if item.get("seq") is None or item.get("task_uuid") is None:
                    continue
                current = self._directives.get(item["task_uuid"])
                item["superseded"] = (
                    item.get("kind") in _ACTIONABLE_KINDS
                    and current is not None
                    and current["seq"] > item["seq"]
                )

    def _drain_directives(self, mailbox: Mailbox, key: str, channel: str) -> str:
        items = mailbox.drain()
        self._mark_superseded(items)
        if items:
            self._publish_receipt(self._client, "read", channel, [i["message_id"] for i in items])
        return json.dumps(_drained_reply(key, items, self._context(), _WORKER_ADVICE))

    def latest_directive(self, task_id) -> str:
        """The effective (newest approve/hold/go) directive for one task,
        meant to be re-checked immediately before an irreversible step. A
        stale link is an error here, not a stale answer: the hold that
        cancels a go may be exactly what hasn't arrived."""
        context = self._context()
        link = context["link"]
        if link["error"]:
            raise _acl_error(link)
        if link["stale"]:
            raise _stale_link_error(link, "a newer directive (e.g. a hold) may not have arrived", _WORKER_ADVICE)
        # Resolve through the replica and match on uuid only: a display id
        # stored when the directive arrived can have moved to another task
        # since (renumber_task), and must never hand that task's go to this one.
        # An exact uuid the ledger already holds needs no replica lookup (and
        # so still works if the replica lags behind the directive).
        key = str(task_id)
        with self._lock:
            ledger_entry = dict(self._directives[key]) if key in self._directives else None
        if ledger_entry is not None:
            task = {"id": ledger_entry.get("task_id"), "uuid": key}
        else:
            try:
                task = json.loads(self.run("show", key, "--format", "json"))
            except RuntimeError as e:
                raise RuntimeError(
                    f"can't resolve task {task_id} on this worker's replica ({e}): the replica has no row "
                    "for this task yet. Ask the coordinator to re-send the directive (that also refreshes "
                    "the replica), or pass the task's full uuid if you have it. Don't act until it resolves."
                ) from None
        with self._lock:
            effective = dict(self._directives[task["uuid"]]) if task["uuid"] in self._directives else None
        state = context["fleet_state"]["state"]
        return json.dumps(
            {
                "task_id": task["id"],
                "task_uuid": task["uuid"],
                "effective": effective,
                "ok_to_act": effective is not None
                and effective["kind"] in ("go", "approve")
                and state != "paused",
                "draining": state == "draining",
                **context,
            }
        )

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
        return {"request_id": request_id, "status": "pending", **self._context()}

    def check(self, request_id: str) -> dict:
        with self._lock:
            result = dict(self._local.get(request_id, {"status": "unknown"}))
        return {**result, **self._context()}

    def sync_state(self) -> dict:
        with self._lock:
            last_synced = self._last_synced
        return {"synced": last_synced is not None, "last_synced": last_synced, **self._context()}

    def fleet_state(self) -> dict:
        return self._context()

    def report_state(self, state: str) -> dict:
        """Update this worker's self-reported work-state (whatever
        convention the fleet agrees on, e.g. idle/busy/blocked) and
        republish presence immediately rather than waiting for the next
        heartbeat tick. Rides the same presence payload as connection
        status, so there's no separate retained "status" topic to manage."""
        self._work_state = state
        self._publish_presence(self._client, "online")
        return {"worker_id": self.config.client_id, "work_state": state, **self._context()}

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
        return json.dumps({"message_id": message_id, "status": "sent", **self._context()})

    def check_messages(self) -> str:
        return self._drain_directives(self._messages, "messages", "message")

    def check_assignments(self) -> str:
        return self._drain_directives(self._assignments, "assignments", "assignment")

    def check_broadcasts(self) -> str:
        return json.dumps(
            _drained_reply("broadcasts", self._broadcasts.drain(), self._context(), _WORKER_ADVICE, raise_if_empty=False)
        )
