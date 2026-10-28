//! The network side of a shared room: carries collaboration messages between
//! people over Logos Messaging (`delivery_module`).
//!
//! Two layers:
//! - [`Bridge`] is plain Rust. A background thread connects the engine's
//!   collaboration calls to a [`Net`] (publish what the engine wants sent,
//!   feed it what arrives). It is tested here against an in-memory network.
//! - [`LogosNet`] is the real [`Net`]: a thin binding to `delivery_module`
//!   through the Logos SDK. It only works inside Basecamp.
//!
//! The bridge never holds the engine lock while it calls the network, so a
//! slow or blocked network call can never freeze the chain methods.

use std::{
    collections::{HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Arc, Mutex, MutexGuard,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use crate::engine::Engine;

/// One message that arrived on a subscribed content topic.
#[derive(Debug, Clone)]
pub struct Incoming {
    /// The network's id for the message; used to drop duplicates. May be empty.
    pub hash: String,
    pub topic: String,
    pub payload: Vec<u8>,
}

/// What the bridge needs from a messaging network.
pub trait Net: Send + Sync + 'static {
    /// Brings the node up (idempotent). Called until it succeeds.
    fn prepare(&self) -> Result<(), String>;

    /// `Connected`, `PartiallyConnected`, `Disconnected` or `Unknown`.
    fn connection(&self) -> String;

    fn subscribe(&self, topic: &str) -> Result<(), String>;

    fn unsubscribe(&self, topic: &str);

    fn send(&self, topic: &str, payload: &[u8]) -> Result<(), String>;
}

/// How often the bridge does each of its jobs.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Longest wait for an incoming message before the other jobs run.
    pub tick: Duration,
    /// Host: how often to publish newly produced blocks.
    pub host_poll: Duration,
    /// Guest: how often to ask the host for the blocks we miss.
    pub guest_hello: Duration,
    /// How long to wait before trying `prepare` again after a failure.
    pub retry: Duration,
    /// How often to refresh the connection status.
    pub status: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            tick: Duration::from_millis(200),
            host_poll: Duration::from_millis(500),
            guest_hello: Duration::from_secs(5),
            retry: Duration::from_secs(3),
            status: Duration::from_secs(3),
        }
    }
}

/// Unsent messages kept while the network is not ready (oldest dropped).
const MAX_QUEUE: usize = 512;
/// Message ids remembered for duplicate suppression.
const SEEN_CAPACITY: usize = 4096;

#[derive(Default, Clone)]
struct State {
    phase: &'static str,
    connection: String,
    topic: Option<String>,
    error: Option<String>,
    note: Option<String>,
    sent: u64,
    received: u64,
    applied: Option<u64>,
}

/// The running bridge. Dropping it does not stop the thread; call [`stop`].
///
/// [`stop`]: Bridge::stop
pub struct Bridge {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
    thread: Option<JoinHandle<()>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Bridge {
    pub fn start(
        engine: Arc<Mutex<Engine>>,
        net: Arc<dyn Net>,
        incoming: Receiver<Incoming>,
        timing: Timing,
    ) -> Bridge {
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(State {
            phase: "starting",
            ..State::default()
        }));

        let worker = Worker {
            engine,
            net,
            incoming,
            timing,
            stop: stop.clone(),
            state: state.clone(),
            subscribed: None,
            ready: false,
            queue: VecDeque::new(),
            seen: Seen::default(),
        };

        let thread = thread::Builder::new()
            .name("solicamp-net".to_string())
            .spawn(move || worker.run())
            .ok();

        if thread.is_none() {
            let mut state = lock(&state);
            state.phase = "error";
            state.error = Some("could not start the network thread".to_string());
        }

        Bridge {
            stop,
            state,
            thread,
        }
    }

    pub fn status(&self) -> Value {
        let state = lock(&self.state).clone();

        json!({
            "phase": state.phase,
            "connection": state.connection,
            "topic": state.topic,
            "error": state.error,
            "note": state.note,
            "sent": state.sent,
            "received": state.received,
            "applied": state.applied,
        })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Remembers the last message ids to drop what the network delivers twice
/// (live, then again from Store catch-up).
#[derive(Default)]
struct Seen {
    order: VecDeque<String>,
    set: HashSet<String>,
}

impl Seen {
    /// True the first time an id is shown. An empty id is never a duplicate.
    fn first_time(&mut self, id: &str) -> bool {
        if id.is_empty() {
            return true;
        }

        if !self.set.insert(id.to_string()) {
            return false;
        }

        self.order.push_back(id.to_string());

        if self.order.len() > SEEN_CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }

        true
    }
}

struct Worker {
    engine: Arc<Mutex<Engine>>,
    net: Arc<dyn Net>,
    incoming: Receiver<Incoming>,
    timing: Timing,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
    /// The topic we are subscribed to on the network.
    subscribed: Option<String>,
    ready: bool,
    /// Messages waiting to be published, with their topic.
    queue: VecDeque<(String, Vec<u8>)>,
    seen: Seen,
}

fn decode_payloads(list: &Value) -> Vec<Vec<u8>> {
    list.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|text| hex::decode(text.trim_start_matches("0x")).ok())
                .collect()
        })
        .unwrap_or_default()
}

impl Worker {
    fn running(&self) -> bool {
        !self.stop.load(Ordering::SeqCst)
    }

    fn engine(&self) -> MutexGuard<'_, Engine> {
        lock(&self.engine)
    }

    fn set(&self, change: impl FnOnce(&mut State)) {
        change(&mut lock(&self.state));
    }

    fn enqueue(&mut self, topic: &str, payload: Vec<u8>) {
        if self.queue.len() >= MAX_QUEUE {
            self.queue.pop_front();
        }

        self.queue.push_back((topic.to_string(), payload));
    }

    fn sleep(&self, total: Duration) {
        let end = Instant::now() + total;

        while self.running() && Instant::now() < end {
            thread::sleep(Duration::from_millis(20).min(total));
        }
    }

    fn run(mut self) {
        let mut next_poll = Instant::now();
        let mut next_hello = Instant::now();
        let mut next_status = Instant::now();

        while self.running() {
            if !self.ready {
                match self.net.prepare() {
                    Ok(()) => {
                        self.ready = true;
                        self.set(|state| {
                            state.phase = "online";
                            state.error = None;
                        });
                    }
                    Err(error) => {
                        self.set(|state| {
                            state.phase = "starting";
                            state.error = Some(error);
                        });
                        self.sleep(self.timing.retry);
                        continue;
                    }
                }
            }

            // What arrived.
            match self.incoming.recv_timeout(self.timing.tick) {
                Ok(message) => self.handle(message),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.sleep(self.timing.tick),
            }

            while let Ok(message) = self.incoming.try_recv() {
                self.handle(message);
            }

            // Which room the network should serve.
            let room = self.engine().collab_room();

            if self.sync_topic(room.as_ref().map(|(topic, _)| topic.as_str())) {
                next_hello = Instant::now();
            }

            let now = Instant::now();

            if let (Some((topic, is_host)), Some(_)) = (&room, &self.subscribed) {
                if *is_host {
                    if now >= next_poll {
                        next_poll = now + self.timing.host_poll;
                        self.host_publish(topic);
                    }
                } else {
                    let outbox = self.engine().take_outbox();

                    for payload in outbox {
                        self.enqueue(topic, payload);
                    }

                    if now >= next_hello {
                        next_hello = now + self.timing.guest_hello;
                        self.guest_hello(topic);
                    }
                }
            }

            self.flush();

            if now >= next_status {
                next_status = now + self.timing.status;
                let connection = self.net.connection();
                self.set(|state| state.connection = connection);
            }
        }

        if let Some(topic) = self.subscribed.take() {
            self.net.unsubscribe(&topic);
        }

        self.set(|state| {
            state.phase = "stopped";
            state.topic = None;
        });
    }

    /// Follows the engine's open room: subscribes to its topic, leaves the
    /// old one. True when the topic changed.
    fn sync_topic(&mut self, wanted: Option<&str>) -> bool {
        if self.subscribed.as_deref() == wanted {
            return false;
        }

        if let Some(old) = self.subscribed.take() {
            self.net.unsubscribe(&old);
            self.queue.clear();
        }

        if let Some(topic) = wanted {
            match self.net.subscribe(topic) {
                Ok(()) => self.subscribed = Some(topic.to_string()),
                Err(error) => {
                    self.set(|state| {
                        state.error = Some(format!("could not subscribe: {error}"));
                    });
                    self.sleep(self.timing.tick);
                    return false;
                }
            }
        }

        let topic = self.subscribed.clone();
        self.set(|state| {
            state.topic = topic;
            state.error = None;
        });

        true
    }

    fn host_publish(&mut self, topic: &str) {
        let result = self.engine().collab_poll();

        match result {
            Ok(value) => {
                for payload in decode_payloads(&value["payloads"]) {
                    self.enqueue(topic, payload);
                }
            }
            Err(error) => self.set(|state| state.note = Some(error)),
        }
    }

    fn guest_hello(&mut self, topic: &str) {
        let result = self.engine().collab_hello();

        match result {
            Ok(value) => {
                if let Some(payload) = value["payload"]
                    .as_str()
                    .and_then(|text| hex::decode(text).ok())
                {
                    self.enqueue(topic, payload);
                }
            }
            Err(error) => self.set(|state| state.note = Some(error)),
        }
    }

    /// Publishes queued messages in order; stops at the first failure and
    /// tries again on the next round.
    fn flush(&mut self) {
        while let Some((topic, payload)) = self.queue.front() {
            if self.subscribed.as_deref() != Some(topic.as_str()) {
                self.queue.pop_front();
                continue;
            }

            match self.net.send(topic, payload) {
                Ok(()) => {
                    self.queue.pop_front();
                    self.set(|state| state.sent += 1);
                }
                Err(error) => {
                    self.set(|state| state.error = Some(format!("send failed: {error}")));
                    return;
                }
            }
        }

        // Everything went out: a leftover send error no longer applies.
        self.set(|state| {
            if state
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("send failed"))
            {
                state.error = None;
            }
        });
    }

    fn handle(&mut self, message: Incoming) {
        if self.subscribed.as_deref() != Some(message.topic.as_str()) {
            return;
        }

        if !self.seen.first_time(&message.hash) {
            return;
        }

        let result = self
            .engine()
            .collab_incoming(&hex::encode(&message.payload));

        self.set(|state| state.received += 1);

        match result {
            Ok(value) => {
                for payload in decode_payloads(&value["send"]) {
                    self.enqueue(&message.topic, payload);
                }

                let applied = value["applied"].as_u64();
                let note = value["note"].as_str().map(str::to_string);

                self.set(|state| {
                    if applied.is_some() {
                        state.applied = applied;
                    }

                    if note.is_some() {
                        state.note = note;
                    }
                });
            }
            Err(error) => self.set(|state| state.note = Some(error)),
        }
    }
}

// ---------------------------------------------------------------------------
// The real network: delivery_module, through the Logos SDK.
// ---------------------------------------------------------------------------

use logos_rust_sdk::{bytes, EventData, LogosModuleSDK, PluginProxy};

const DELIVERY: &str = "delivery_module";

/// Network preset and node mode. `logos.dev` has no RLN (the `logos.test`
/// preset needs the RLN modules loaded); override for experiments with the
/// `SOLICAMP_DELIVERY_CONFIG` environment variable (a createNode JSON).
const DEFAULT_CONFIG: &str = r#"{"mode":"Core","preset":"logos.dev"}"#;

/// Is a call's JSON result a failure? `delivery_module` methods return a
/// result object; accept the shapes it can take.
fn failure(value: &Value) -> Option<String> {
    match value {
        Value::Bool(false) => Some("the call failed".to_string()),
        Value::Object(map) => {
            let failed = ["success", "ok"]
                .iter()
                .any(|key| map.get(*key).and_then(Value::as_bool) == Some(false));

            let text = ["error", "message"].iter().find_map(|key| {
                map.get(*key)
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
            });

            match (failed, text) {
                (true, text) => Some(text.unwrap_or("the call failed").to_string()),
                (false, Some(text)) if map.contains_key("error") => Some(text.to_string()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn already(error: &str) -> bool {
    let error = error.to_lowercase();

    error.contains("already") || error.contains("running") || error.contains("exists")
}

/// A `delivery_module` event as a flat list of its arguments.
fn arguments(event: &EventData) -> Vec<Value> {
    match &event.data {
        Value::Array(items) => items.clone(),
        Value::Object(map) => map.values().cloned().collect(),
        Value::Null => Vec::new(),
        other => vec![other.clone()],
    }
}

/// `messageReceived(messageHash, contentTopic, payload, source, timestamp)`.
fn incoming_from(event: &EventData) -> Option<Incoming> {
    let (hash, topic, payload) = match &event.data {
        Value::Object(map) if map.contains_key("contentTopic") => (
            map.get("messageHash")?.as_str()?.to_string(),
            map.get("contentTopic")?.as_str()?.to_string(),
            bytes::decode_lenient(map.get("payload")?)?,
        ),
        _ => {
            let args = arguments(event);

            (
                args.first()?.as_str()?.to_string(),
                args.get(1)?.as_str()?.to_string(),
                bytes::decode_lenient(args.get(2)?)?,
            )
        }
    };

    Some(Incoming {
        hash,
        topic,
        payload,
    })
}

pub struct LogosNet {
    incoming: Sender<Incoming>,
    prepared: AtomicBool,
    connection: Arc<Mutex<String>>,
}

impl LogosNet {
    pub fn new() -> (Arc<LogosNet>, Receiver<Incoming>) {
        let (sender, receiver) = mpsc::channel();

        let net = Arc::new(LogosNet {
            incoming: sender,
            prepared: AtomicBool::new(false),
            connection: Arc::new(Mutex::new("Unknown".to_string())),
        });

        (net, receiver)
    }

    fn proxy() -> PluginProxy {
        LogosModuleSDK::new().plugin(DELIVERY)
    }

    fn call(method: &str, args: Value) -> Result<Value, String> {
        let value = Self::proxy()
            .call_json(method, &args)
            .map_err(|error| format!("{method}: {error}"))?;

        match failure(&value) {
            Some(error) => Err(format!("{method}: {error}")),
            None => Ok(value),
        }
    }

    /// Listens to one delivery_module event on its own thread.
    fn listen(
        &self,
        event: &str,
        handler: impl Fn(EventData) + Send + 'static,
    ) -> Result<(), String> {
        let subscription = Self::proxy()
            .on(event)
            .map_err(|error| format!("could not listen to {event}: {error}"))?;

        thread::Builder::new()
            .name(format!("solicamp-{event}"))
            .spawn(move || {
                for event in subscription {
                    handler(event);
                }
            })
            .map_err(|error| error.to_string())?;

        Ok(())
    }
}

impl Net for LogosNet {
    fn prepare(&self) -> Result<(), String> {
        if self.prepared.load(Ordering::SeqCst) {
            return Ok(());
        }

        let sender = self.incoming.clone();

        self.listen("messageReceived", move |event| {
            if let Some(message) = incoming_from(&event) {
                let _ = sender.send(message);
            }
        })?;

        let connection = self.connection.clone();

        self.listen("connectionStateChanged", move |event| {
            if let Some(status) = arguments(&event).first().and_then(Value::as_str) {
                *lock(&connection) = status.to_string();
            }
        })?;

        let config = std::env::var("SOLICAMP_DELIVERY_CONFIG")
            .ok()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CONFIG.to_string());

        // The node exists once per Logos instance: another module may have
        // made it already, which is fine as long as start works.
        let created = Self::call("createNode", json!([config]));
        let started = Self::call("start", json!([]));

        match (created, started) {
            (_, Ok(_)) => {}
            (_, Err(error)) if already(&error) => {}
            (created, Err(error)) => {
                return Err(match created {
                    Err(first) => format!("{first}; {error}"),
                    Ok(_) => error,
                });
            }
        }

        self.prepared.store(true, Ordering::SeqCst);

        Ok(())
    }

    fn connection(&self) -> String {
        if let Ok(value) = Self::call("getConnectionStatus", json!([])) {
            let status = match &value {
                Value::String(text) => Some(text.clone()),
                Value::Object(map) => ["value", "message"]
                    .iter()
                    .find_map(|key| map.get(*key).and_then(Value::as_str))
                    .map(str::to_string),
                _ => None,
            };

            if let Some(status) = status {
                *lock(&self.connection) = status;
            }
        }

        lock(&self.connection).clone()
    }

    fn subscribe(&self, topic: &str) -> Result<(), String> {
        Self::call("subscribe", json!([topic])).map(|_| ())
    }

    fn unsubscribe(&self, topic: &str) {
        let _ = Self::call("unsubscribe", json!([topic]));
    }

    fn send(&self, topic: &str, payload: &[u8]) -> Result<(), String> {
        Self::call("send", json!([topic, bytes::encode(payload)])).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, AtomicUsize},
    };

    const COUNTER_INITCODE: &str =
        include_str!("../vendor/solizone-evm/contracts/Counter.creation.hex");

    /// An in-memory network: everything sent on a topic reaches every node
    /// subscribed to it, the sender included (like a relay would).
    #[derive(Default)]
    struct Hub {
        nodes: Mutex<Vec<(usize, Arc<Mutex<HashSet<String>>>, Sender<Incoming>)>>,
        counter: AtomicU64,
        log: Mutex<Vec<(usize, String, usize)>>,
    }

    struct MockNet {
        id: usize,
        hub: Arc<Hub>,
        topics: Arc<Mutex<HashSet<String>>>,
        /// Sends that fail before one succeeds.
        fail_sends: AtomicUsize,
        /// `prepare` calls that fail before one succeeds.
        fail_prepare: AtomicUsize,
        prepares: AtomicUsize,
    }

    impl MockNet {
        fn join(hub: &Arc<Hub>) -> (Arc<MockNet>, Receiver<Incoming>) {
            let (sender, receiver) = mpsc::channel();
            let topics = Arc::new(Mutex::new(HashSet::new()));
            let id = lock(&hub.nodes).len();

            lock(&hub.nodes).push((id, topics.clone(), sender));

            let net = Arc::new(MockNet {
                id,
                hub: hub.clone(),
                topics,
                fail_sends: AtomicUsize::new(0),
                fail_prepare: AtomicUsize::new(0),
                prepares: AtomicUsize::new(0),
            });

            (net, receiver)
        }
    }

    impl Net for MockNet {
        fn prepare(&self) -> Result<(), String> {
            self.prepares.fetch_add(1, Ordering::SeqCst);

            if self
                .fail_prepare
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err("node not ready".to_string());
            }

            Ok(())
        }

        fn connection(&self) -> String {
            "Connected".to_string()
        }

        fn subscribe(&self, topic: &str) -> Result<(), String> {
            lock(&self.topics).insert(topic.to_string());
            Ok(())
        }

        fn unsubscribe(&self, topic: &str) {
            lock(&self.topics).remove(topic);
        }

        fn send(&self, topic: &str, payload: &[u8]) -> Result<(), String> {
            if self
                .fail_sends
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err("no peers".to_string());
            }

            let hash = format!("m{}", self.hub.counter.fetch_add(1, Ordering::SeqCst));
            lock(&self.hub.log).push((self.id, topic.to_string(), payload.len()));

            for (_, topics, sender) in lock(&self.hub.nodes).iter() {
                if lock(topics).contains(topic) {
                    let message = Incoming {
                        hash: hash.clone(),
                        topic: topic.to_string(),
                        payload: payload.to_vec(),
                    };

                    // Delivered twice, as Store catch-up can do.
                    let _ = sender.send(message.clone());
                    let _ = sender.send(message);
                }
            }

            Ok(())
        }
    }

    fn fast() -> Timing {
        Timing {
            tick: Duration::from_millis(10),
            host_poll: Duration::from_millis(20),
            guest_hello: Duration::from_millis(100),
            retry: Duration::from_millis(30),
            status: Duration::from_millis(50),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!(
            "solizone-net-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);

        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn engine(dir: &PathBuf) -> Arc<Mutex<Engine>> {
        let engine = Arc::new(Mutex::new(Engine::default()));
        lock(&engine).start(dir.to_str().unwrap()).unwrap();
        engine
    }

    fn head(engine: &Arc<Mutex<Engine>>) -> Value {
        lock(engine).status()["head"].clone()
    }

    #[test]
    fn two_people_share_a_room_over_the_network() {
        let hub = Arc::new(Hub::default());

        let host = engine(&temp_dir("host"));
        let guest = engine(&temp_dir("guest"));

        // The host makes and shares a room.
        let room = lock(&host).create_room("Team").unwrap();
        let id = room["id"].as_str().unwrap().to_string();
        lock(&host).open_room(&id).unwrap();
        let invite = lock(&host).share_room(&id).unwrap()["invite"]
            .as_str()
            .unwrap()
            .to_string();

        let (host_net, host_rx) = MockNet::join(&hub);
        let (guest_net, guest_rx) = MockNet::join(&hub);

        let host_bridge = Bridge::start(host.clone(), host_net.clone(), host_rx, fast());

        // The guest's node starts flaky: prepare fails twice, then works.
        guest_net.fail_prepare.store(2, Ordering::SeqCst);
        let guest_bridge = Bridge::start(guest.clone(), guest_net.clone(), guest_rx, fast());

        wait_until("the host to be online", || {
            host_bridge.status()["phase"] == "online" && host_bridge.status()["topic"].is_string()
        });

        // Joining is enough: the bridge asks for the chain by itself.
        lock(&guest).join_room(&invite).unwrap();

        wait_until("the guest to retry and come online", || {
            guest_bridge.status()["phase"] == "online"
        });
        assert!(guest_net.prepares.load(Ordering::SeqCst) >= 3);

        wait_until("the guest to receive the genesis block", || {
            head(&guest) == json!(0)
        });

        // The guest deploys; the bridge carries the transaction to the host
        // and the host's block back, even though the first sends fail.
        guest_net.fail_sends.store(2, Ordering::SeqCst);

        let sent = lock(&guest).deploy(0, COUNTER_INITCODE.trim(), "").unwrap();
        assert_eq!(sent["pending"], true);
        let hash = sent["txHash"].as_str().unwrap().to_string();

        wait_until("the host to order the transaction", || {
            head(&host) == json!(1)
        });
        wait_until("the guest to apply the block", || head(&guest) == json!(1));

        let receipt = lock(&guest).receipt(&hash).unwrap();
        assert_eq!(receipt["success"], true, "{receipt}");

        // Duplicate deliveries changed nothing: both chains agree, and the
        // host only ever saw the one transaction (one block, not two).
        assert_eq!(head(&host), head(&guest));
        thread::sleep(Duration::from_millis(300));
        assert_eq!(head(&host), json!(1));

        let status = guest_bridge.status();
        assert_eq!(status["connection"], "Connected");
        assert_eq!(status["applied"], json!(1));
        assert!(status["sent"].as_u64().unwrap() >= 2);

        // A guest that joins later catches up through hello alone.
        let late = engine(&temp_dir("late"));
        let (late_net, late_rx) = MockNet::join(&hub);
        let late_bridge = Bridge::start(late.clone(), late_net, late_rx, fast());
        lock(&late).join_room(&invite).unwrap();

        wait_until("the late guest to catch up", || head(&late) == json!(1));

        late_bridge.stop();
        guest_bridge.stop();
        host_bridge.stop();
    }

    #[test]
    fn the_bridge_follows_the_open_room_and_stops_cleanly() {
        let hub = Arc::new(Hub::default());
        let host = engine(&temp_dir("follow"));

        let (net, rx) = MockNet::join(&hub);
        let bridge = Bridge::start(host.clone(), net.clone(), rx, fast());

        // No shared room: online, but nothing subscribed.
        wait_until("online", || bridge.status()["phase"] == "online");
        assert!(bridge.status()["topic"].is_null());

        let first = lock(&host).create_room("One").unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let second = lock(&host).create_room("Two").unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();

        lock(&host).open_room(&first).unwrap();
        lock(&host).share_room(&first).unwrap();
        wait_until("the first topic", || {
            bridge.status()["topic"]
                .as_str()
                .is_some_and(|topic| topic.contains(&first))
        });

        // A second shared room replaces the first subscription.
        lock(&host).open_room(&second).unwrap();
        lock(&host).share_room(&second).unwrap();
        wait_until("the second topic", || {
            bridge.status()["topic"]
                .as_str()
                .is_some_and(|topic| topic.contains(&second))
        });
        assert_eq!(lock(&net.topics).len(), 1);

        // Back to Main (not shareable): the subscription goes away.
        lock(&host).open_room("main").unwrap();
        wait_until("no topic", || bridge.status()["topic"].is_null());
        assert!(lock(&net.topics).is_empty());

        bridge.stop();
    }

    #[test]
    fn duplicate_ids_are_dropped_and_the_memory_is_bounded() {
        let mut seen = Seen::default();

        assert!(seen.first_time("a"));
        assert!(!seen.first_time("a"));
        assert!(seen.first_time(""), "no id, no dedupe");
        assert!(seen.first_time(""));

        for index in 0..(SEEN_CAPACITY + 10) {
            seen.first_time(&format!("id{index}"));
        }

        assert!(seen.set.len() <= SEEN_CAPACITY);
        assert!(seen.first_time("a"), "old ids are eventually forgotten");
    }

    #[test]
    fn results_and_events_from_delivery_module_are_read_leniently() {
        // Results.
        assert!(failure(&json!(true)).is_none());
        assert!(failure(&json!("req-1")).is_none());
        assert!(failure(&json!({"success": true, "value": "req-1"})).is_none());
        assert_eq!(failure(&json!(false)).as_deref(), Some("the call failed"));
        assert_eq!(
            failure(&json!({"success": false, "error": "no node"})).as_deref(),
            Some("no node")
        );
        assert_eq!(failure(&json!({"error": "boom"})).as_deref(), Some("boom"));
        assert!(already("node already created"));
        assert!(!already("out of memory"));

        // messageReceived as an argument list, payload as tagged bytes.
        let list = EventData::new(
            "messageReceived",
            json!(["0xabc", "/t/1", bytes::encode(b"hi"), "live", 5]),
        );
        let message = incoming_from(&list).unwrap();
        assert_eq!(message.hash, "0xabc");
        assert_eq!(message.topic, "/t/1");
        assert_eq!(message.payload, b"hi");

        // As a named map.
        let map = EventData::new(
            "messageReceived",
            json!({"messageHash": "0x1", "contentTopic": "/t/2", "payload": bytes::encode(&[0u8, 255])}),
        );
        let message = incoming_from(&map).unwrap();
        assert_eq!(message.topic, "/t/2");
        assert_eq!(message.payload, vec![0u8, 255]);

        // Garbage is ignored, not a panic.
        assert!(incoming_from(&EventData::new("messageReceived", json!([]))).is_none());
        assert!(incoming_from(&EventData::new("messageReceived", json!(7))).is_none());
    }
}
