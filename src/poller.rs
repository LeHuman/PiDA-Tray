//! Shared app state plus the background task that talks to the server.
//!
//! The task sleeps on a `tokio::select!` over a command channel and an idle
//! timer, so it does zero work while nothing happens. The UI never blocks on
//! the network: it reads cheap snapshots and sends `Command`s.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Map, Value};
use tokio::{sync::mpsc, task::JoinSet};

use crate::{
    api::{ApiError, PidaClient, Schema},
    cache::{CacheData, icon_file, icons_dir},
    catalog::{DeviceGroup, apply_saved_order, build_catalog},
    config::{Settings, normalize_server_url},
};

pub type StateMap = Map<String, Value>;

/// Post-action settle passes, cumulative from the action: 0.5s, 1s, 3s, 8s.
/// A newer action restarts the cascade, matching the Android client.
const SETTLE_DELAYS: [Duration; 4] = [
    Duration::from_millis(500),
    Duration::from_millis(500),
    Duration::from_secs(2),
    Duration::from_secs(5),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    Unconfigured,
    Unknown,
    Online,
    Offline,
}

pub struct AppState {
    pub settings: Settings,
    pub cache: CacheData,
    pub catalog: Arc<Vec<DeviceGroup>>,
    /// Displayed state (includes optimistic edits); `cache.group_states` is the confirmed state.
    pub states: BTreeMap<String, StateMap>,
    pub connection: Connection,
    pub banner: Option<String>,
    /// In-flight dispatch counts keyed by `<action_path>` or `<action_path>:<param>`.
    pub in_flight: HashMap<String, u32>,
    /// Per-param active edit locks `(group_path, param)` while a dispatch is in flight.
    pub in_flight_params: HashMap<(String, String), u32>,
    /// Per-param active slider debounce locks `(group_path, param)`.
    pub pending_params: HashSet<(String, String)>,
    pub syncing: bool,
    pub icons: HashMap<String, Arc<Vec<u8>>>,
    /// Session-level negative cache: icon keys the server answered 404 for.
    pub missing_icons: HashSet<String>,
}

/// Cheap, owned copy of what the UI needs for one frame.
#[derive(Clone)]
pub struct Snapshot {
    pub settings: Settings,
    pub catalog: Arc<Vec<DeviceGroup>>,
    pub states: BTreeMap<String, StateMap>,
    pub connection: Connection,
    pub banner: Option<String>,
    pub in_flight: HashSet<String>,
    pub syncing: bool,
    pub icons: HashMap<String, Arc<Vec<u8>>>,
}

type RepaintFn = Arc<dyn Fn() + Send + Sync>;

pub struct Shared {
    state: Mutex<AppState>,
    repaint: Mutex<Option<RepaintFn>>,
    dir: PathBuf,
    visible: AtomicBool,
    /// Generation counter for the post-action settle cascade; a newer action
    /// supersedes any cascade still waiting on its delays.
    settle_generation: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    pub fn new(dir: PathBuf) -> Arc<Self> {
        let mut settings = Settings::load(&dir);
        let cache = CacheData::load(&dir);
        let mut catalog = cache.schema.as_ref().map(build_catalog).unwrap_or_default();
        let merged = apply_saved_order(&mut catalog, &settings.device_order);
        if merged != settings.device_order {
            settings.device_order = merged;
            let _ = settings.save(&dir);
        }

        // Cache-first: pull any icons already on disk so the first paint has them.
        let mut icons = HashMap::new();
        for g in &catalog {
            let found = g
                .icon_candidates()
                .into_iter()
                .find_map(|c| std::fs::read(icon_file(&dir, &c)).ok());
            if let Some(bytes) = found {
                icons.insert(g.path.clone(), Arc::new(bytes));
            }
        }

        let connection = if settings.server_url.is_empty() {
            Connection::Unconfigured
        } else {
            Connection::Unknown
        };
        let states = cache.group_states.clone();
        Arc::new(Self {
            state: Mutex::new(AppState {
                settings,
                cache,
                catalog: Arc::new(catalog),
                states,
                connection,
                banner: None,
                in_flight: HashMap::new(),
                in_flight_params: HashMap::new(),
                pending_params: HashSet::new(),
                syncing: false,
                icons,
                missing_icons: HashSet::new(),
            }),
            repaint: Mutex::new(None),
            dir,
            visible: AtomicBool::new(false),
            settle_generation: AtomicU64::new(0),
        })
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub fn set_repaint(&self, f: impl Fn() + Send + Sync + 'static) {
        *lock(&self.repaint) = Some(Arc::new(f));
    }

    pub fn set_visible(&self, v: bool) {
        self.visible.store(v, Ordering::Relaxed);
    }

    pub fn is_visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }

    fn begin_settle(&self) -> u64 {
        self.settle_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn settle_is_current(&self, generation: u64) -> bool {
        self.settle_generation.load(Ordering::SeqCst) == generation
    }

    /// Move a visible device one slot earlier/later in the saved order.
    pub fn move_device(&self, path: &str, delta: isize) -> bool {
        let (mut order, visible) = self.read(|s| {
            (
                s.settings.device_order.clone(),
                s.catalog
                    .iter()
                    .map(|group| group.path.clone())
                    .collect::<Vec<_>>(),
            )
        });
        let Some(position) = visible.iter().position(|candidate| candidate == path) else {
            return false;
        };
        let target = position as isize + delta;
        if target < 0 || target >= visible.len() as isize {
            return false;
        }
        let other = &visible[target as usize];
        let (Some(a), Some(b)) = (
            order.iter().position(|candidate| candidate == path),
            order.iter().position(|candidate| candidate == other),
        ) else {
            return false;
        };
        order.swap(a, b);
        self.update(|s| {
            s.settings.device_order = order.clone();
            let mut catalog = (*s.catalog).clone();
            apply_saved_order(&mut catalog, &order);
            s.catalog = Arc::new(catalog);
        });
        self.save_settings();
        true
    }

    fn request_repaint(&self) {
        let f = lock(&self.repaint).clone();
        if let Some(f) = f {
            f();
        }
    }

    /// Mutate state and wake the UI (which early-returns while hidden).
    pub fn update<R>(&self, f: impl FnOnce(&mut AppState) -> R) -> R {
        let r = {
            let mut s = lock(&self.state);
            f(&mut s)
        };
        self.request_repaint();
        r
    }

    /// Track/release a local debounced slider edit without forcing a repaint.
    pub fn set_pending_param(&self, group: &str, param: &str, active: bool) {
        let mut s = lock(&self.state);
        let key = (group.to_owned(), param.to_owned());
        if active {
            s.pending_params.insert(key);
        } else {
            s.pending_params.remove(&key);
        }
    }

    pub fn read<R>(&self, f: impl FnOnce(&AppState) -> R) -> R {
        f(&lock(&self.state))
    }

    pub fn snapshot(&self) -> Snapshot {
        self.read(|s| Snapshot {
            settings: s.settings.clone(),
            catalog: s.catalog.clone(),
            states: s.states.clone(),
            connection: s.connection,
            banner: s.banner.clone(),
            in_flight: s.in_flight.keys().cloned().collect(),
            syncing: s.syncing,
            icons: s.icons.clone(),
        })
    }

    pub fn settings(&self) -> Settings {
        self.read(|s| s.settings.clone())
    }

    pub fn save_settings(&self) {
        let settings = self.settings();
        let _ = settings.save(&self.dir);
    }

    fn save_cache(&self) {
        let cache = self.read(|s| s.cache.clone());
        let _ = cache.save(&self.dir);
    }

    fn set_connection(&self, c: Connection) {
        if self.read(|s| s.connection) != c {
            self.update(|s| s.connection = c);
        }
    }

    pub fn clear_cache(&self) {
        let _ = std::fs::remove_file(crate::cache::cache_path(&self.dir));
        let _ = std::fs::remove_dir_all(icons_dir(&self.dir));
        self.update(|s| {
            s.cache = CacheData::default();
            s.catalog = Arc::new(Vec::new());
            s.states.clear();
            s.icons.clear();
            s.missing_icons.clear();
        });
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------------

/// Merge server outputs into a state map.
pub fn merge_outputs(dst: &mut StateMap, src: &StateMap) {
    for (k, v) in src {
        dst.insert(k.clone(), v.clone());
    }
}

/// Merge server outputs into the displayed state while keeping any locally
/// active (`in_flight` or `pending` debounced) param values untouched.
pub fn merge_outputs_preserving_active(
    dst: &mut StateMap,
    src: &StateMap,
    group: &str,
    in_flight_params: &HashMap<(String, String), u32>,
    pending_params: &HashSet<(String, String)>,
) {
    for (k, v) in src {
        let key = (group.to_owned(), k.clone());
        if !in_flight_params.contains_key(&key) && !pending_params.contains(&key) {
            dst.insert(k.clone(), v.clone());
        }
    }
}

/// Roll one param in a group's displayed state back to its last confirmed value
/// (leaving sibling in-flight edits in the same group untouched).
pub fn revert_param(
    states: &mut BTreeMap<String, StateMap>,
    confirmed: &BTreeMap<String, StateMap>,
    group: &str,
    param: &str,
) {
    let prev = confirmed.get(group).and_then(|m| m.get(param)).cloned();
    match (states.get_mut(group), prev) {
        (Some(m), Some(v)) => {
            m.insert(param.to_owned(), v);
        }
        (Some(m), None) => {
            m.remove(param);
            if m.is_empty() {
                states.remove(group);
            }
        }
        (None, Some(v)) => {
            let mut m = StateMap::new();
            m.insert(param.to_owned(), v);
            states.insert(group.to_owned(), m);
        }
        (None, None) => {}
    }
}

/// Roll a group's displayed state back to the last confirmed one.
pub fn revert_group(
    states: &mut BTreeMap<String, StateMap>,
    confirmed: &BTreeMap<String, StateMap>,
    group: &str,
) {
    match confirmed.get(group) {
        Some(c) => {
            states.insert(group.to_owned(), c.clone());
        }
        None => {
            states.remove(group);
        }
    }
}

// ---------------------------------------------------------------------------
// Commands + task
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DispatchRequest {
    pub group_path: String,
    pub action_path: String,
    pub param: Option<String>,
    pub body: Value,
    pub key: String,
    /// `true` for set actions (state write-through); `false` for legacy taps.
    pub parameterized: bool,
}

#[derive(Debug, Clone)]
pub enum Command {
    /// Refresh schema + group states (flyout opened / manual refresh).
    Sync,
    /// The settings changed (server URL, poll interval): rebuild the client.
    SettingsChanged,
    /// System resumed from sleep.
    Wake,
    ClearCache,
    Dispatch(DispatchRequest),
}

#[derive(Clone)]
pub struct Poller {
    tx: mpsc::UnboundedSender<Command>,
}

impl Poller {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<Command>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }

    /// Optimistically apply `param = value`, then dispatch the partial delta
    /// (plus current values of any other `required` inputs on the action).
    pub fn set_param(
        &self,
        shared: &Shared,
        group: &str,
        action_path: &str,
        required_inputs: &[String],
        param: &str,
        value: Value,
    ) {
        let key = format!("{action_path}:{param}");
        let body = shared.update(|s| {
            let entry = s.states.entry(group.to_owned()).or_default();
            entry.insert(param.to_owned(), value.clone());

            let mut body = Map::new();
            body.insert(param.to_owned(), value);
            for req in required_inputs {
                if req != param
                    && let Some(existing) = entry.get(req)
                {
                    body.insert(req.clone(), existing.clone());
                }
            }

            *s.in_flight.entry(key.clone()).or_insert(0) += 1;
            *s.in_flight_params
                .entry((group.to_owned(), param.to_owned()))
                .or_insert(0) += 1;
            s.pending_params
                .remove(&(group.to_owned(), param.to_owned()));
            body
        });
        self.send(Command::Dispatch(DispatchRequest {
            group_path: group.to_owned(),
            action_path: action_path.to_owned(),
            param: Some(param.to_owned()),
            body: Value::Object(body),
            key,
            parameterized: true,
        }));
    }

    /// Fire a legacy one-shot action.
    pub fn press(&self, shared: &Shared, group: &str, action_path: &str) {
        let key = action_path.to_owned();
        shared.update(|s| *s.in_flight.entry(key.clone()).or_insert(0) += 1);
        self.send(Command::Dispatch(DispatchRequest {
            group_path: group.to_owned(),
            action_path: action_path.to_owned(),
            param: None,
            body: Value::Object(Map::new()),
            key,
            parameterized: false,
        }));
    }
}

fn make_client(shared: &Shared) -> Option<PidaClient> {
    let url = shared.settings().server_url;
    if url.trim().is_empty() {
        shared.set_connection(Connection::Unconfigured);
        return None;
    }
    match normalize_server_url(&url)
        .map_err(ApiError::Parse)
        .and_then(|u| PidaClient::new(&u))
    {
        Ok(c) => Some(c),
        Err(e) => {
            shared.update(|s| {
                s.connection = Connection::Offline;
                s.banner = Some(e.to_string());
            });
            None
        }
    }
}

pub async fn run(shared: Arc<Shared>, mut rx: mpsc::UnboundedReceiver<Command>) {
    let mut client = make_client(&shared);
    if let Some(c) = client.clone() {
        tokio::spawn(ping(c, shared.clone()));
    }

    loop {
        let secs = shared.settings().idle_poll_seconds;
        let idle = async move {
            if secs == 0 {
                std::future::pending::<()>().await;
            } else {
                tokio::time::sleep(Duration::from_secs(secs)).await;
            }
        };

        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    Command::SettingsChanged => {
                        client = make_client(&shared);
                        if let Some(c) = client.clone() {
                            tokio::spawn(ping(c, shared.clone()));
                        }
                    }
                    Command::Sync => start_sync(&client, &shared),
                    Command::Wake => {
                        if let Some(c) = client.clone() {
                            tokio::spawn(ping(c, shared.clone()));
                        }
                        if shared.is_visible() {
                            start_sync(&client, &shared);
                        }
                    }
                    Command::ClearCache => {
                        shared.clear_cache();
                        start_sync(&client, &shared);
                    }
                    Command::Dispatch(req) => match client.clone() {
                        Some(c) => { tokio::spawn(dispatch(c, shared.clone(), req)); }
                        None => finish_dispatch_error(&shared, &req, "Server URL is not configured".into(), false),
                    },
                }
            }
            _ = idle => {
                // Slow health ping only while the flyout is hidden.
                if !shared.is_visible()
                    && let Some(c) = client.clone()
                {
                    tokio::spawn(ping(c, shared.clone()));
                }
            }
        }
    }
}

/// Fill shared state with `count` synthetic devices for `--demo` runs.
///
/// Runs entirely in memory: no server URL, no cache writes, and no network.
/// Toggle/slider edits are optimistic and the demo task clears their in-flight
/// locks so the UI never sticks in a busy state.
pub fn seed_demo(shared: &Shared, count: usize) {
    let mut groups = Map::new();
    for i in 1..=count {
        let path = format!("demo/device{i}");
        let actions = match i % 3 {
            0 => serde_json::json!({
                "toggle": {},
                "set": {
                    "params": {"on": {"type": "bool"}},
                    "inputs": ["on"],
                    "outputs": ["on"]
                }
            }),
            1 => serde_json::json!({
                "set": {
                    "params": {"brightness": {"type": "int", "min": 0, "max": 100}},
                    "inputs": ["brightness"],
                    "outputs": ["brightness"]
                }
            }),
            _ => serde_json::json!({
                "toggle": {},
                "set": {
                    "params": {
                        "on": {"type": "bool"},
                        "brightness": {"type": "int", "min": 0, "max": 100}
                    },
                    "inputs": ["on", "brightness"],
                    "outputs": ["on", "brightness"]
                }
            }),
        };
        groups.insert(path, serde_json::json!({ "actions": actions }));
    }

    let schema = Schema::from_value(&serde_json::json!({ "groups": groups }))
        .expect("synthetic demo schema must parse");
    let catalog = build_catalog(&schema);
    let mut states = BTreeMap::new();
    for group in &catalog {
        let mut state = Map::new();
        if group
            .set
            .as_ref()
            .is_some_and(|set| set.toggle_param.is_some())
        {
            state.insert("on".into(), serde_json::json!(true));
        }
        if group.set.as_ref().is_some_and(|set| {
            set.range_params
                .iter()
                .any(|param| param.name == "brightness")
        }) {
            state.insert("brightness".into(), serde_json::json!(40));
        }
        states.insert(group.path.clone(), state);
    }

    shared.update(|s| {
        s.catalog = Arc::new(catalog);
        s.states = states;
        s.connection = Connection::Online;
        s.syncing = false;
        s.settings.server_url = "http://demo.local:9999".into();
    });
}

/// Command loop for `--demo`: acknowledges dispatches locally, no network.
pub async fn run_demo(shared: Arc<Shared>, mut rx: mpsc::UnboundedReceiver<Command>) {
    while let Some(command) = rx.recv().await {
        match command {
            Command::Dispatch(req) => shared.update(|s| dec_in_flight(s, &req)),
            _ => shared.update(|s| s.syncing = false),
        }
    }
}

fn start_sync(client: &Option<PidaClient>, shared: &Arc<Shared>) {
    match client {
        Some(c) => {
            tokio::spawn(sync(c.clone(), shared.clone()));
        }
        None => shared.set_connection(Connection::Unconfigured),
    }
}

async fn ping(client: PidaClient, shared: Arc<Shared>) {
    let conn = match client.health().await {
        Ok(h) if h.ok => Connection::Online,
        _ => Connection::Offline,
    };
    shared.set_connection(conn);
}

async fn sync(client: PidaClient, shared: Arc<Shared>) {
    if shared.read(|s| s.syncing) {
        return;
    }
    shared.update(|s| s.syncing = true);

    match client.schema().await {
        Ok(schema) => {
            apply_schema(&shared, schema);
            refresh_states(&client, &shared).await;
            refresh_icons(&client, &shared).await;
            shared.update(|s| s.connection = Connection::Online);
        }
        Err(e) => {
            // Keep cached devices; just flag offline.
            shared.update(|s| {
                s.connection = Connection::Offline;
                if !e.is_network() {
                    s.banner = Some(e.to_string());
                }
            });
        }
    }
    shared.update(|s| s.syncing = false);
}

fn apply_schema(shared: &Shared, schema: Schema) {
    let order = shared.read(|s| s.settings.device_order.clone());
    let mut catalog = build_catalog(&schema);
    let merged = apply_saved_order(&mut catalog, &order);
    shared.update(|s| {
        // Keep cached state for groups that are temporarily absent: the saved
        // order retains their slot, and a later sync updates the stale state.
        s.settings.device_order = merged;
        s.cache.schema = Some(schema);
        s.catalog = Arc::new(catalog);
        s.banner = None;
    });
    shared.save_settings();
    shared.save_cache();
}

async fn refresh_states(client: &PidaClient, shared: &Arc<Shared>) {
    // Only GET parameterized endpoints (`set` and `readouts`), never legacy actions.
    let targets: Vec<(String, String)> = shared.read(|s| {
        s.catalog
            .iter()
            .flat_map(|g| {
                g.set
                    .iter()
                    .map(|set| (g.path.clone(), set.action_path.clone()))
                    .chain(
                        g.readouts
                            .iter()
                            .map(|r| (g.path.clone(), r.action_path.clone())),
                    )
            })
            .collect()
    });

    let mut set = JoinSet::new();
    for (group, path) in targets {
        let c = client.clone();
        set.spawn(async move { (group, c.get_state(&path).await) });
    }
    while let Some(joined) = set.join_next().await {
        // A failed readback keeps the cached state for that group.
        if let Ok((group, Ok(outputs))) = joined {
            shared.update(|s| {
                merge_outputs(
                    s.cache.group_states.entry(group.clone()).or_default(),
                    &outputs,
                );
                let in_flight_params = s.in_flight_params.clone();
                let pending_params = s.pending_params.clone();
                let dst = s.states.entry(group.clone()).or_default();
                merge_outputs_preserving_active(
                    dst,
                    &outputs,
                    &group,
                    &in_flight_params,
                    &pending_params,
                );
            });
        }
    }
    shared.save_cache();
}

fn schedule_settle(client: PidaClient, shared: Arc<Shared>, generation: u64) {
    tokio::spawn(async move {
        for delay in SETTLE_DELAYS {
            tokio::time::sleep(delay).await;
            if !shared.settle_is_current(generation) {
                return;
            }
            refresh_states(&client, &shared).await;
        }
    });
}

async fn refresh_icons(client: &PidaClient, shared: &Arc<Shared>) {
    // Icons are served per endpoint, so each card tries its set action, readouts, then legacy actions.
    let targets: Vec<(String, Vec<String>)> = shared.read(|s| {
        s.catalog
            .iter()
            .filter(|g| !s.icons.contains_key(&g.path) && !s.missing_icons.contains(&g.path))
            .map(|g| (g.path.clone(), g.icon_candidates()))
            .collect()
    });
    for (group, candidates) in targets {
        let mut transient = false;
        let mut hit = None;
        for cand in &candidates {
            match client.icon(cand).await {
                Ok(Some(bytes)) => {
                    hit = Some((cand.clone(), bytes));
                    break;
                }
                Ok(None) => {}
                Err(_) => {
                    transient = true;
                    break;
                }
            }
        }
        match hit {
            Some((cand, bytes)) => {
                let path = icon_file(shared.dir(), &cand);
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(path, &bytes);
                shared.update(|s| {
                    s.icons.insert(group.clone(), Arc::new(bytes));
                });
            }
            // Every candidate answered 404: remember for this session only.
            None if !transient => {
                shared.update(|s| {
                    s.missing_icons.insert(group.clone());
                });
            }
            None => {}
        }
    }
}

async fn dispatch(client: PidaClient, shared: Arc<Shared>, req: DispatchRequest) {
    match client.dispatch(&req.action_path, &req.body).await {
        Ok(outputs) => {
            shared.update(|s| {
                dec_in_flight(s, &req);
                if req.parameterized {
                    merge_outputs(
                        s.cache
                            .group_states
                            .entry(req.group_path.clone())
                            .or_default(),
                        &outputs,
                    );
                    let in_flight_params = s.in_flight_params.clone();
                    let pending_params = s.pending_params.clone();
                    let dst = s.states.entry(req.group_path.clone()).or_default();
                    merge_outputs_preserving_active(
                        dst,
                        &outputs,
                        &req.group_path,
                        &in_flight_params,
                        &pending_params,
                    );
                }
                s.banner = None;
                s.connection = Connection::Online;
            });
            shared.save_cache();

            // Fan-out actions change other groups and some devices reflect
            // late. Re-read every group four times (0.5/1/3/8s), restarting
            // the cascade on each new successful action.
            let generation = shared.begin_settle();
            schedule_settle(client, shared, generation);
        }
        // A newer request in the same group queue replaced this one; its result will land.
        Err(e) if e.is_superseded() => shared.update(|s| dec_in_flight(s, &req)),
        Err(e) => finish_dispatch_error(&shared, &req, e.to_string(), e.is_network()),
    }
}

fn dec_in_flight(s: &mut AppState, req: &DispatchRequest) {
    if let Some(n) = s.in_flight.get_mut(&req.key) {
        *n = n.saturating_sub(1);
        if *n == 0 {
            s.in_flight.remove(&req.key);
        }
    }
    if let Some(param) = &req.param {
        let pkey = (req.group_path.clone(), param.clone());
        if let Some(n) = s.in_flight_params.get_mut(&pkey) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                s.in_flight_params.remove(&pkey);
            }
        }
    }
}

fn finish_dispatch_error(shared: &Shared, req: &DispatchRequest, message: String, offline: bool) {
    shared.update(|s| {
        if req.parameterized {
            let confirmed = s.cache.group_states.clone();
            if let Some(param) = &req.param {
                revert_param(&mut s.states, &confirmed, &req.group_path, param);
            } else {
                revert_group(&mut s.states, &confirmed, &req.group_path);
            }
        }
        s.banner = Some(message);
        if offline {
            s.connection = Connection::Offline;
        }
        dec_in_flight(s, req);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(v: Value) -> StateMap {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn merge_overwrites_and_adds() {
        let mut dst = map(json!({"power": false, "level": 1}));
        merge_outputs(&mut dst, &map(json!({"power": true, "mode": "warm"})));
        assert_eq!(dst, map(json!({"power": true, "level": 1, "mode": "warm"})));
    }

    #[test]
    fn merge_preserving_active_skips_in_flight_and_pending_keys() {
        let mut dst = map(json!({"power": false, "level": 75, "temp": 20}));
        let src = map(json!({"power": true, "level": 10, "temp": 25}));
        let in_flight = HashMap::from([(("a".to_owned(), "power".to_owned()), 1)]);
        let pending = HashSet::from([("a".to_owned(), "level".to_owned())]);
        merge_outputs_preserving_active(&mut dst, &src, "a", &in_flight, &pending);
        assert_eq!(dst, map(json!({"power": false, "level": 75, "temp": 25})));
    }

    #[test]
    fn revert_param_keeps_sibling_optimistic_edits() {
        let confirmed =
            BTreeMap::from([("a".to_owned(), map(json!({"power": true, "level": 10})))]);
        let mut states =
            BTreeMap::from([("a".to_owned(), map(json!({"power": false, "level": 80})))]);
        revert_param(&mut states, &confirmed, "a", "power");
        assert_eq!(states["a"], map(json!({"power": true, "level": 80})));
    }

    #[test]
    fn revert_restores_confirmed_or_removes() {
        let confirmed = BTreeMap::from([("a".to_owned(), map(json!({"power": true})))]);
        let mut states = BTreeMap::from([
            ("a".to_owned(), map(json!({"power": false}))),
            ("b".to_owned(), map(json!({"power": true}))),
        ]);
        revert_group(&mut states, &confirmed, "a");
        revert_group(&mut states, &confirmed, "b");
        assert_eq!(states["a"], map(json!({"power": true})));
        assert!(!states.contains_key("b"));
    }

    #[test]
    fn in_flight_counts_down_to_removal() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared::new(dir.path().to_path_buf());
        let req = DispatchRequest {
            group_path: "a".into(),
            action_path: "a/set".into(),
            param: Some("power".into()),
            body: json!({}),
            key: "k".into(),
            parameterized: true,
        };
        shared.update(|s| {
            s.in_flight.insert("k".into(), 2);
            s.in_flight_params.insert(("a".into(), "power".into()), 2);
            dec_in_flight(s, &req);
            assert_eq!(s.in_flight["k"], 1);
            dec_in_flight(s, &req);
            assert!(!s.in_flight.contains_key("k"));
            assert!(
                !s.in_flight_params
                    .contains_key(&("a".into(), "power".into()))
            );
        });
    }

    #[test]
    fn set_param_applies_optimistically_and_includes_required_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared::new(dir.path().to_path_buf());
        shared.update(|s| {
            s.states
                .insert("office/fan".into(), map(json!({"power": true})));
        });
        let (poller, mut rx) = Poller::new();
        let reqs = vec!["power".to_owned(), "speed".to_owned()];
        poller.set_param(
            &shared,
            "office/fan",
            "office/fan/set",
            &reqs,
            "speed",
            json!(3),
        );
        let snap = shared.snapshot();
        assert_eq!(snap.states["office/fan"]["speed"], json!(3));
        assert!(snap.in_flight.contains("office/fan/set:speed"));
        match rx.try_recv().unwrap() {
            Command::Dispatch(r) => {
                assert_eq!(r.body, json!({"speed": 3, "power": true}));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn starts_unconfigured_without_settings() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared::new(dir.path().to_path_buf());
        assert_eq!(shared.snapshot().connection, Connection::Unconfigured);
    }

    #[test]
    fn settlement_cascade_last_action_wins() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared::new(dir.path().to_path_buf());
        let first = shared.begin_settle();
        assert!(shared.settle_is_current(first));

        let second = shared.begin_settle();
        assert!(!shared.settle_is_current(first));
        assert!(shared.settle_is_current(second));
        assert_eq!(
            SETTLE_DELAYS.iter().sum::<Duration>(),
            Duration::from_secs(8)
        );
    }
}
