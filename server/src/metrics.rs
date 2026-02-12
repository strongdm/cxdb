// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use sysinfo::{Pid, System};

use crate::registry::Registry;
use crate::store::Store;

/// Information about a connected client session.
#[derive(Debug, Clone, Serialize)]
pub struct ClientSession {
    pub session_id: u64,
    pub client_tag: String,
    pub peer_addr: Option<String>, // Client IP:port from TCP connection
    pub connected_at: u64,         // unix_ms
    pub last_activity_at: u64,     // unix_ms
    pub contexts_created: Vec<u64>, // context IDs created by this session
}

/// Tracks connected client sessions and their metadata.
#[derive(Default)]
pub struct SessionTracker {
    sessions: RwLock<HashMap<u64, ClientSession>>,
    context_to_session: RwLock<HashMap<u64, u64>>,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            context_to_session: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new session with the given client tag and optional peer address.
    pub fn register(&self, session_id: u64, client_tag: String, peer_addr: Option<String>) {
        let now_ms = unix_ms();
        let session = ClientSession {
            session_id,
            client_tag,
            peer_addr,
            connected_at: now_ms,
            last_activity_at: now_ms,
            contexts_created: Vec::new(),
        };
        self.sessions.write().unwrap().insert(session_id, session);
    }

    /// Get the peer address for a session.
    pub fn get_peer_addr(&self, session_id: u64) -> Option<String> {
        self.sessions
            .read()
            .unwrap()
            .get(&session_id)
            .and_then(|s| s.peer_addr.clone())
    }

    /// Record activity for a session (updates last_activity_at).
    pub fn record_activity(&self, session_id: u64) {
        let now_ms = unix_ms();
        if let Some(session) = self.sessions.write().unwrap().get_mut(&session_id) {
            session.last_activity_at = now_ms;
        }
    }

    /// Associate a context with a session.
    pub fn add_context(&self, session_id: u64, context_id: u64) {
        self.context_to_session
            .write()
            .unwrap()
            .insert(context_id, session_id);
        if let Some(session) = self.sessions.write().unwrap().get_mut(&session_id) {
            if !session.contexts_created.contains(&context_id) {
                session.contexts_created.push(context_id);
            }
        }
    }

    /// Unregister a session and return its orphaned contexts.
    pub fn unregister(&self, session_id: u64) -> Vec<u64> {
        let session = self.sessions.write().unwrap().remove(&session_id);
        if let Some(session) = session {
            let mut ctx_map = self.context_to_session.write().unwrap();
            for ctx_id in &session.contexts_created {
                ctx_map.remove(ctx_id);
            }
            session.contexts_created
        } else {
            Vec::new()
        }
    }

    /// Get session info for a context (if session is still connected).
    pub fn get_session_for_context(&self, context_id: u64) -> Option<ClientSession> {
        let ctx_map = self.context_to_session.read().unwrap();
        let session_id = ctx_map.get(&context_id)?;
        let sessions = self.sessions.read().unwrap();
        sessions.get(session_id).cloned()
    }

    /// Get all active sessions.
    pub fn get_active_sessions(&self) -> Vec<ClientSession> {
        self.sessions.read().unwrap().values().cloned().collect()
    }

    /// Get all context IDs that have active sessions (are "live").
    pub fn get_live_context_ids(&self) -> std::collections::HashSet<u64> {
        self.context_to_session
            .read()
            .unwrap()
            .keys()
            .copied()
            .collect()
    }

    /// Get the client tag for a session.
    pub fn get_client_tag(&self, session_id: u64) -> Option<String> {
        self.sessions
            .read()
            .unwrap()
            .get(&session_id)
            .map(|s| s.client_tag.clone())
    }

    /// Check if a context is live (has an active session).
    pub fn is_context_live(&self, context_id: u64) -> bool {
        self.context_to_session
            .read()
            .unwrap()
            .contains_key(&context_id)
    }

    /// Get all unique client tags from active sessions.
    pub fn get_active_tags(&self) -> Vec<String> {
        let sessions = self.sessions.read().unwrap();
        let mut tags: HashSet<String> = HashSet::new();
        for session in sessions.values() {
            if !session.client_tag.is_empty() {
                tags.insert(session.client_tag.clone());
            }
        }
        tags.into_iter().collect()
    }
}

const MAX_LATENCY_SAMPLES: usize = 2048;
const MAX_ERROR_ENTRIES: usize = 256;

#[derive(Debug, Clone)]
pub struct MetricsConfig {
    pub budget_pct: f64,
    pub hard_cap_bytes: u64,
    pub warn_ratio: f64,
    pub hot_ratio: f64,
    pub critical_ratio: f64,
    pub idle_seconds: u64,
}

impl MetricsConfig {
    pub fn from_env() -> Self {
        let budget_pct = env_f64("CXDB_METRICS_BUDGET_PCT", 0.70).clamp(0.10, 0.85);
        let hard_cap_bytes = env_u64("CXDB_METRICS_HARD_CAP_BYTES", 24 * 1024 * 1024 * 1024);
        let warn_ratio = env_f64("CXDB_METRICS_WARN_RATIO", 0.60).clamp(0.10, 0.95);
        let hot_ratio = env_f64("CXDB_METRICS_HOT_RATIO", 0.80).clamp(0.10, 0.98);
        let critical_ratio = env_f64("CXDB_METRICS_CRITICAL_RATIO", 0.92).clamp(0.10, 0.999);
        let idle_seconds = env_u64("CXDB_METRICS_IDLE_SECONDS", 60);
        Self {
            budget_pct,
            hard_cap_bytes,
            warn_ratio,
            hot_ratio,
            critical_ratio,
            idle_seconds,
        }
    }
}

pub struct Metrics {
    config: MetricsConfig,
    start: Instant,
    pid: Pid,
    data_dir: PathBuf,

    sessions_total: AtomicU64,
    sessions_active: AtomicU64,
    last_session_activity_ms: AtomicU64,
    next_session_id: AtomicU64,
    session_activity: Mutex<HashMap<u64, u64>>,

    append_total: AtomicU64,
    get_last_total: AtomicU64,
    get_blob_total: AtomicU64,
    registry_ingest_total: AtomicU64,
    http_total: AtomicU64,
    http_errors_total: AtomicU64,
    errors_total: AtomicU64,
    errors_by_type: Mutex<HashMap<String, u64>>,
    recent_errors: Mutex<VecDeque<ErrorEntry>>,

    rates: Mutex<RateStore>,
    latencies: Mutex<LatencyStore>,
    system: Mutex<System>,
}

impl Metrics {
    pub fn new(data_dir: PathBuf) -> Self {
        let pid = Pid::from_u32(std::process::id());
        Self {
            config: MetricsConfig::from_env(),
            start: Instant::now(),
            pid,
            data_dir,
            sessions_total: AtomicU64::new(0),
            sessions_active: AtomicU64::new(0),
            last_session_activity_ms: AtomicU64::new(0),
            next_session_id: AtomicU64::new(1),
            session_activity: Mutex::new(HashMap::new()),
            append_total: AtomicU64::new(0),
            get_last_total: AtomicU64::new(0),
            get_blob_total: AtomicU64::new(0),
            registry_ingest_total: AtomicU64::new(0),
            http_total: AtomicU64::new(0),
            http_errors_total: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            errors_by_type: Mutex::new(HashMap::new()),
            recent_errors: Mutex::new(VecDeque::new()),
            rates: Mutex::new(RateStore::new()),
            latencies: Mutex::new(LatencyStore::new()),
            system: Mutex::new(System::new()),
        }
    }

    pub fn register_session(self: &Arc<Self>) -> SessionGuard {
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        self.sessions_total.fetch_add(1, Ordering::Relaxed);
        self.sessions_active.fetch_add(1, Ordering::Relaxed);
        let now_ms = unix_ms();
        self.last_session_activity_ms
            .store(now_ms, Ordering::Relaxed);
        self.session_activity
            .lock()
            .unwrap()
            .insert(session_id, now_ms);
        SessionGuard {
            session_id,
            metrics: Arc::clone(self),
        }
    }

    pub fn record_session_activity(&self, session_id: u64) {
        let now_ms = unix_ms();
        self.last_session_activity_ms
            .store(now_ms, Ordering::Relaxed);
        if let Some(last) = self.session_activity.lock().unwrap().get_mut(&session_id) {
            *last = now_ms;
        }
    }

    fn unregister_session(&self, session_id: u64) {
        self.sessions_active.fetch_sub(1, Ordering::Relaxed);
        self.session_activity.lock().unwrap().remove(&session_id);
    }

    pub fn record_append(&self, duration: Duration) {
        self.append_total.fetch_add(1, Ordering::Relaxed);
        self.latencies
            .lock()
            .unwrap()
            .append
            .push(duration_to_ms(duration));
    }

    pub fn record_get_last(&self, duration: Duration) {
        self.get_last_total.fetch_add(1, Ordering::Relaxed);
        self.latencies
            .lock()
            .unwrap()
            .get_last
            .push(duration_to_ms(duration));
    }

    pub fn record_get_blob(&self, duration: Duration) {
        self.get_blob_total.fetch_add(1, Ordering::Relaxed);
        self.latencies
            .lock()
            .unwrap()
            .get_blob
            .push(duration_to_ms(duration));
    }

    pub fn record_registry_ingest(&self) {
        self.registry_ingest_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_http(&self, status_code: u16, duration: Duration) {
        self.http_total.fetch_add(1, Ordering::Relaxed);
        if status_code >= 400 {
            self.http_errors_total.fetch_add(1, Ordering::Relaxed);
        }
        self.latencies
            .lock()
            .unwrap()
            .http
            .push(duration_to_ms(duration));
    }

    pub fn record_error(&self, kind: &str, status_code: u16, message: &str, path: Option<&str>) {
        self.errors_total.fetch_add(1, Ordering::Relaxed);
        {
            let mut map = self.errors_by_type.lock().unwrap();
            let count = map.entry(kind.to_string()).or_insert(0);
            *count += 1;
        }
        {
            let entry = ErrorEntry {
                timestamp_ms: unix_ms(),
                kind: kind.to_string(),
                status_code,
                message: message.to_string(),
                path: path.map(|s| s.to_string()),
            };
            let mut buf = self.recent_errors.lock().unwrap();
            if buf.len() >= MAX_ERROR_ENTRIES {
                buf.pop_front();
            }
            buf.push_back(entry);
        }
    }

    /// Returns the most recent errors, newest first. `limit` caps the result count.
    pub fn recent_errors(&self, limit: usize) -> Vec<ErrorEntry> {
        let buf = self.recent_errors.lock().unwrap();
        buf.iter().rev().take(limit).cloned().collect()
    }

    pub fn snapshot(&self, store: &mut Store, registry: &Registry) -> MetricsSnapshot {
        let now = Utc::now();
        let uptime_seconds = self.start.elapsed().as_secs_f64();

        let (memory, storage, objects) = self.collect_stats(store, registry);

        let append_total = self.append_total.load(Ordering::Relaxed);
        let get_last_total = self.get_last_total.load(Ordering::Relaxed);
        let get_blob_total = self.get_blob_total.load(Ordering::Relaxed);
        let registry_total = self.registry_ingest_total.load(Ordering::Relaxed);
        let http_total = self.http_total.load(Ordering::Relaxed);
        let http_errors = self.http_errors_total.load(Ordering::Relaxed);

        let mut rates = self.rates.lock().unwrap();
        let append_rates = rates.update_append(append_total);
        let get_last_rates = rates.update_get_last(get_last_total);
        let get_blob_rates = rates.update_get_blob(get_blob_total);
        let registry_rates = rates.update_registry(registry_total);
        let http_rates = rates.update_http(http_total);
        let http_error_rates = rates.update_http_errors(http_errors);

        let latencies = self.latencies.lock().unwrap();
        let append_latency = LatencySummary::from_samples(&latencies.append);
        let get_last_latency = LatencySummary::from_samples(&latencies.get_last);
        let get_blob_latency = LatencySummary::from_samples(&latencies.get_blob);
        let http_latency = LatencySummary::from_samples(&latencies.http);

        let sessions_active = self.sessions_active.load(Ordering::Relaxed);
        let sessions_total = self.sessions_total.load(Ordering::Relaxed);
        let last_activity_ms = self.last_session_activity_ms.load(Ordering::Relaxed);
        let now_ms = unix_ms();
        let idle_sessions = {
            let idle_cutoff = now_ms.saturating_sub(self.config.idle_seconds.saturating_mul(1000));
            let map = self.session_activity.lock().unwrap();
            map.values().filter(|v| **v < idle_cutoff).count() as u64
        };

        let errors_by_type = self.errors_by_type.lock().unwrap().clone();
        let errors_total = self.errors_total.load(Ordering::Relaxed);

        let store_stats = store.stats();
        let filesystem = FilesystemMetrics {
            snapshots_total: store_stats.fs_roots_total,
            index_bytes: store_stats.fs_roots_bytes,
            content_bytes: store_stats.fs_content_bytes,
        };

        MetricsSnapshot {
            ts: now.to_rfc3339_opts(SecondsFormat::Millis, true),
            uptime_seconds,
            memory,
            sessions: SessionMetrics {
                total: sessions_total,
                active: sessions_active,
                idle: idle_sessions,
                last_activity_unix_ms: last_activity_ms,
            },
            objects,
            storage,
            filesystem,
            perf: PerfMetrics {
                append_tps_1m: append_rates.rate_1m,
                append_tps_5m: append_rates.rate_5m,
                append_tps_history: append_rates.history,
                get_last_tps_1m: get_last_rates.rate_1m,
                get_last_tps_5m: get_last_rates.rate_5m,
                get_last_tps_history: get_last_rates.history,
                get_blob_tps_1m: get_blob_rates.rate_1m,
                get_blob_tps_5m: get_blob_rates.rate_5m,
                get_blob_tps_history: get_blob_rates.history,
                registry_ingest_tps_1m: registry_rates.rate_1m,
                registry_ingest_tps_5m: registry_rates.rate_5m,
                http_req_tps_1m: http_rates.rate_1m,
                http_req_tps_5m: http_rates.rate_5m,
                http_req_tps_history: http_rates.history,
                http_errors_tps_1m: http_error_rates.rate_1m,
                http_errors_tps_5m: http_error_rates.rate_5m,
                append_latency_ms: append_latency,
                get_last_latency_ms: get_last_latency,
                get_blob_latency_ms: get_blob_latency,
                http_latency_ms: http_latency,
            },
            errors: ErrorMetrics {
                total: errors_total,
                by_type: errors_by_type,
            },
        }
    }

    fn collect_stats(
        &self,
        store: &mut Store,
        registry: &Registry,
    ) -> (MemoryMetrics, StorageMetrics, ObjectMetrics) {
        let mut system = self.system.lock().unwrap();
        system.refresh_memory();
        system.refresh_process(self.pid);

        let total_bytes = system.total_memory().saturating_mul(1024);
        let available_bytes = system.available_memory().saturating_mul(1024);
        let free_bytes = system.free_memory().saturating_mul(1024);
        // cached_memory() was removed in sysinfo 0.30+, calculate as total - available - free
        let cached_bytes = total_bytes
            .saturating_sub(available_bytes)
            .saturating_sub(free_bytes);
        let swap_total_bytes = system.total_swap().saturating_mul(1024);
        let swap_free_bytes = system.free_swap().saturating_mul(1024);

        let process = system.process(self.pid);
        let process_rss_bytes = process.map(|p| p.memory().saturating_mul(1024));
        let process_vmem_bytes = process.map(|p| p.virtual_memory().saturating_mul(1024));

        let budget_bytes = std::cmp::min(
            (available_bytes as f64 * self.config.budget_pct) as u64,
            self.config.hard_cap_bytes,
        )
        .max(1);

        let rss = process_rss_bytes.unwrap_or(0);
        let pressure_ratio = (rss as f64) / (budget_bytes as f64);
        let pressure_level = if pressure_ratio >= self.config.critical_ratio {
            "CRITICAL"
        } else if pressure_ratio >= self.config.hot_ratio {
            "HOT"
        } else if pressure_ratio >= self.config.warn_ratio {
            "WARN"
        } else {
            "OK"
        };

        let spill_threshold_bytes = (budget_bytes as f64 * 0.85) as u64;
        let spill_critical_bytes = (budget_bytes as f64 * 0.95) as u64;

        let (disk_total, disk_free) = disk_space_for_path(&self.data_dir);

        let store_stats = store.stats();
        let registry_stats = registry.stats();

        let memory = MemoryMetrics {
            sys_total_bytes: total_bytes,
            sys_available_bytes: available_bytes,
            sys_free_bytes: free_bytes,
            sys_cached_bytes: cached_bytes,
            sys_swap_total_bytes: swap_total_bytes,
            sys_swap_free_bytes: swap_free_bytes,
            process_rss_bytes,
            process_vmem_bytes,
            process_heap_bytes: None,
            process_open_fds: None,
            budget_bytes,
            budget_pct: self.config.budget_pct,
            hard_cap_bytes: self.config.hard_cap_bytes,
            pressure_ratio,
            pressure_level: pressure_level.to_string(),
            spill_threshold_bytes,
            spill_critical_bytes,
        };

        let objects = ObjectMetrics {
            contexts_total: store_stats.contexts_total,
            turns_total: store_stats.turns_total,
            blobs_total: store_stats.blobs_total,
            registry_types_total: registry_stats.types_total,
            registry_bundles_total: registry_stats.bundles_total,
            heads_total: store_stats.heads_total,
        };

        let storage = StorageMetrics {
            turns_log_bytes: store_stats.turns_log_bytes,
            turns_index_bytes: store_stats.turns_index_bytes,
            turns_meta_bytes: store_stats.turns_meta_bytes,
            heads_table_bytes: store_stats.heads_table_bytes,
            blobs_pack_bytes: store_stats.blobs_pack_bytes,
            blobs_index_bytes: store_stats.blobs_index_bytes,
            data_dir_total_bytes: disk_total,
            data_dir_free_bytes: disk_free,
        };

        (memory, storage, objects)
    }
}

pub struct SessionGuard {
    session_id: u64,
    metrics: Arc<Metrics>,
}

impl SessionGuard {
    pub fn session_id(&self) -> u64 {
        self.session_id
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.metrics.unregister_session(self.session_id);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub ts: String,
    pub uptime_seconds: f64,
    pub memory: MemoryMetrics,
    pub sessions: SessionMetrics,
    pub objects: ObjectMetrics,
    pub storage: StorageMetrics,
    pub filesystem: FilesystemMetrics,
    pub perf: PerfMetrics,
    pub errors: ErrorMetrics,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryMetrics {
    pub sys_total_bytes: u64,
    pub sys_available_bytes: u64,
    pub sys_free_bytes: u64,
    pub sys_cached_bytes: u64,
    pub sys_swap_total_bytes: u64,
    pub sys_swap_free_bytes: u64,
    pub process_rss_bytes: Option<u64>,
    pub process_vmem_bytes: Option<u64>,
    pub process_heap_bytes: Option<u64>,
    pub process_open_fds: Option<u64>,
    pub budget_bytes: u64,
    pub budget_pct: f64,
    pub hard_cap_bytes: u64,
    pub pressure_ratio: f64,
    pub pressure_level: String,
    pub spill_threshold_bytes: u64,
    pub spill_critical_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionMetrics {
    pub total: u64,
    pub active: u64,
    pub idle: u64,
    pub last_activity_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObjectMetrics {
    pub contexts_total: usize,
    pub turns_total: usize,
    pub blobs_total: usize,
    pub registry_types_total: usize,
    pub registry_bundles_total: usize,
    pub heads_total: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageMetrics {
    pub turns_log_bytes: u64,
    pub turns_index_bytes: u64,
    pub turns_meta_bytes: u64,
    pub heads_table_bytes: u64,
    pub blobs_pack_bytes: u64,
    pub blobs_index_bytes: u64,
    pub data_dir_total_bytes: u64,
    pub data_dir_free_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PerfMetrics {
    pub append_tps_1m: f64,
    pub append_tps_5m: f64,
    pub append_tps_history: Vec<f64>,
    pub get_last_tps_1m: f64,
    pub get_last_tps_5m: f64,
    pub get_last_tps_history: Vec<f64>,
    pub get_blob_tps_1m: f64,
    pub get_blob_tps_5m: f64,
    pub get_blob_tps_history: Vec<f64>,
    pub registry_ingest_tps_1m: f64,
    pub registry_ingest_tps_5m: f64,
    pub http_req_tps_1m: f64,
    pub http_req_tps_5m: f64,
    pub http_req_tps_history: Vec<f64>,
    pub http_errors_tps_1m: f64,
    pub http_errors_tps_5m: f64,
    pub append_latency_ms: LatencySummary,
    pub get_last_latency_ms: LatencySummary,
    pub get_blob_latency_ms: LatencySummary,
    pub http_latency_ms: LatencySummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorMetrics {
    pub total: u64,
    pub by_type: HashMap<String, u64>,
}

/// A single recorded error with context for debugging.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorEntry {
    pub timestamp_ms: u64,
    pub kind: String,
    pub status_code: u16,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FilesystemMetrics {
    pub snapshots_total: usize,
    pub index_bytes: u64,
    pub content_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatencySummary {
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    pub max: Option<f64>,
    pub count: usize,
}

impl LatencySummary {
    fn from_samples(samples: &VecDeque<f64>) -> Self {
        if samples.is_empty() {
            return Self {
                p50: None,
                p95: None,
                p99: None,
                max: None,
                count: 0,
            };
        }
        let mut values: Vec<f64> = samples.iter().copied().collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p50 = percentile(&values, 0.50);
        let p95 = percentile(&values, 0.95);
        let p99 = percentile(&values, 0.99);
        let max = values.last().copied();
        Self {
            p50,
            p95,
            p99,
            max,
            count: values.len(),
        }
    }
}

struct LatencyStore {
    append: VecDeque<f64>,
    get_last: VecDeque<f64>,
    get_blob: VecDeque<f64>,
    http: VecDeque<f64>,
}

impl LatencyStore {
    fn new() -> Self {
        Self {
            append: VecDeque::with_capacity(MAX_LATENCY_SAMPLES),
            get_last: VecDeque::with_capacity(MAX_LATENCY_SAMPLES),
            get_blob: VecDeque::with_capacity(MAX_LATENCY_SAMPLES),
            http: VecDeque::with_capacity(MAX_LATENCY_SAMPLES),
        }
    }
}

trait SampleBuffer {
    fn push(&mut self, value: f64);
}

impl SampleBuffer for VecDeque<f64> {
    fn push(&mut self, value: f64) {
        if self.len() == MAX_LATENCY_SAMPLES {
            self.pop_front();
        }
        self.push_back(value);
    }
}

const MAX_RATE_HISTORY: usize = 30;

#[derive(Clone)]
struct RateCalc {
    last_total: u64,
    last_ts: Instant,
    rate_1m: f64,
    rate_5m: f64,
    history: VecDeque<f64>,
}

impl RateCalc {
    fn new(now: Instant) -> Self {
        Self {
            last_total: 0,
            last_ts: now,
            rate_1m: 0.0,
            rate_5m: 0.0,
            history: VecDeque::with_capacity(MAX_RATE_HISTORY),
        }
    }

    fn update(&mut self, total: u64) -> RateSnapshot {
        let now = Instant::now();
        let dt = now.duration_since(self.last_ts).as_secs_f64().max(0.001);
        let delta = total.saturating_sub(self.last_total) as f64;
        let instant_rate = delta / dt;

        let alpha_1m = alpha(dt, 60.0);
        let alpha_5m = alpha(dt, 300.0);
        self.rate_1m = self.rate_1m + alpha_1m * (instant_rate - self.rate_1m);
        self.rate_5m = self.rate_5m + alpha_5m * (instant_rate - self.rate_5m);

        self.last_total = total;
        self.last_ts = now;

        // Record history (keep last MAX_RATE_HISTORY samples)
        if self.history.len() >= MAX_RATE_HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(self.rate_1m);

        RateSnapshot {
            rate_1m: self.rate_1m,
            rate_5m: self.rate_5m,
            history: self.history.iter().copied().collect(),
        }
    }
}

struct RateStore {
    append: RateCalc,
    get_last: RateCalc,
    get_blob: RateCalc,
    registry: RateCalc,
    http: RateCalc,
    http_errors: RateCalc,
}

impl RateStore {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            append: RateCalc::new(now),
            get_last: RateCalc::new(now),
            get_blob: RateCalc::new(now),
            registry: RateCalc::new(now),
            http: RateCalc::new(now),
            http_errors: RateCalc::new(now),
        }
    }

    fn update_append(&mut self, total: u64) -> RateSnapshot {
        self.append.update(total)
    }

    fn update_get_last(&mut self, total: u64) -> RateSnapshot {
        self.get_last.update(total)
    }

    fn update_get_blob(&mut self, total: u64) -> RateSnapshot {
        self.get_blob.update(total)
    }

    fn update_registry(&mut self, total: u64) -> RateSnapshot {
        self.registry.update(total)
    }

    fn update_http(&mut self, total: u64) -> RateSnapshot {
        self.http.update(total)
    }

    fn update_http_errors(&mut self, total: u64) -> RateSnapshot {
        self.http_errors.update(total)
    }
}

#[derive(Debug, Clone)]
struct RateSnapshot {
    rate_1m: f64,
    rate_5m: f64,
    history: Vec<f64>,
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn duration_to_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn percentile(sorted: &[f64], pct: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (sorted.len() as f64 - 1.0) * pct;
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    if low == high {
        return sorted.get(low).copied();
    }
    let low_val = sorted[low];
    let high_val = sorted[high];
    let weight = rank - low as f64;
    Some(low_val + (high_val - low_val) * weight)
}

fn alpha(dt: f64, window_seconds: f64) -> f64 {
    1.0 - (-dt / window_seconds).exp()
}

/// Returns (total_bytes, free_bytes) for the filesystem containing `path`.
///
/// Uses libc::statvfs directly because the sysinfo crate multiplies block
/// counts by f_bsize instead of f_frsize, which inflates values by 256x on
/// VirtioFS mounts (Docker Desktop) where f_bsize=1MiB but f_frsize=4KiB.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // statvfs fields are u32 on macOS, u64 on Linux
fn disk_space_for_path(path: &Path) -> (u64, u64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = match CString::new(path.as_os_str().as_bytes()) {
        Ok(p) => p,
        Err(_) => return (0, 0),
    };
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
            let total = (stat.f_blocks as u64).saturating_mul(stat.f_frsize as u64);
            let free = (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64);
            (total, free)
        } else {
            (0, 0)
        }
    }
}

#[cfg(not(unix))]
fn disk_space_for_path(path: &Path) -> (u64, u64) {
    use sysinfo::Disks;
    let disks = Disks::new_with_refreshed_list();
    let mut best_match: Option<(u64, u64, usize)> = None;
    for disk in disks.list() {
        let mount = disk.mount_point();
        if path.starts_with(mount) {
            let match_len = mount.as_os_str().len();
            let total = disk.total_space();
            let free = disk.available_space();
            let candidate = (total, free, match_len);
            if best_match.as_ref().map(|b| match_len > b.2).unwrap_or(true) {
                best_match = Some(candidate);
            }
        }
    }
    if let Some((total, free, _)) = best_match {
        (total, free)
    } else {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_space_returns_sane_values() {
        let (total, free) = disk_space_for_path(Path::new("/tmp"));
        assert!(total > 0, "total disk space should be non-zero");
        assert!(free <= total, "free space should not exceed total");
        // Sanity: total should be less than 100 TiB (catches the VirtioFS inflation bug)
        let max_reasonable = 100 * 1024 * 1024 * 1024 * 1024_u64; // 100 TiB
        assert!(
            total < max_reasonable,
            "total {total} bytes exceeds 100 TiB — likely f_bsize/f_frsize confusion"
        );
    }

    #[test]
    fn disk_space_nonexistent_path_returns_zero() {
        let (total, free) = disk_space_for_path(Path::new("/nonexistent_path_xyz_12345"));
        // On Unix, statvfs on a nonexistent path returns an error → (0, 0)
        // On other platforms, sysinfo may still match "/" as a prefix
        #[cfg(unix)]
        {
            assert_eq!(total, 0);
            assert_eq!(free, 0);
        }
        #[cfg(not(unix))]
        {
            let _ = (total, free); // sysinfo might match root mount
        }
    }

    #[test]
    fn error_ring_buffer_stores_entries() {
        let m = Metrics::new(PathBuf::from("/tmp"));
        m.record_error("http", 404, "not found", Some("/v1/foo"));
        m.record_error("binary", 500, "corrupt", None);

        let recent = m.recent_errors(10);
        assert_eq!(recent.len(), 2);
        // Newest first
        assert_eq!(recent[0].kind, "binary");
        assert_eq!(recent[0].status_code, 500);
        assert_eq!(recent[1].kind, "http");
        assert_eq!(recent[1].path, Some("/v1/foo".to_string()));
    }

    #[test]
    fn error_ring_buffer_evicts_oldest() {
        let m = Metrics::new(PathBuf::from("/tmp"));
        for i in 0..MAX_ERROR_ENTRIES + 10 {
            m.record_error("http", 404, &format!("error-{i}"), None);
        }
        let recent = m.recent_errors(MAX_ERROR_ENTRIES);
        assert_eq!(recent.len(), MAX_ERROR_ENTRIES);
        // The oldest entries (0..9) should have been evicted
        assert!(recent.last().unwrap().message.contains("error-10"));
        assert!(recent
            .first()
            .unwrap()
            .message
            .contains(&format!("error-{}", MAX_ERROR_ENTRIES + 9)));
    }

    #[test]
    fn error_ring_buffer_respects_limit() {
        let m = Metrics::new(PathBuf::from("/tmp"));
        for i in 0..20 {
            m.record_error("http", 400, &format!("err-{i}"), None);
        }
        let recent = m.recent_errors(5);
        assert_eq!(recent.len(), 5);
        // Should be the 5 most recent
        assert!(recent[0].message.contains("err-19"));
        assert!(recent[4].message.contains("err-15"));
    }

    #[test]
    fn record_error_increments_counters() {
        let m = Metrics::new(PathBuf::from("/tmp"));
        m.record_error("http", 404, "not found", None);
        m.record_error("http", 500, "internal", None);
        m.record_error("binary", 422, "bad input", None);

        assert_eq!(m.errors_total.load(Ordering::Relaxed), 3);
        let by_type = m.errors_by_type.lock().unwrap();
        assert_eq!(by_type["http"], 2);
        assert_eq!(by_type["binary"], 1);
    }
}
