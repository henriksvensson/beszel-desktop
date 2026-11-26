use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use beszel_protocol::LOCAL_PROTOCOL_VERSION;
use beszel_protocol::local::{
    ConnectionState, ConnectionStatus, LiveSample, RelayEvent, SystemLiveState, SystemSummary,
};
use tokio::sync::{broadcast, watch};

const BROADCAST_CAPACITY: usize = 256;

/// Shared live metric store and broadcast hub for connected Unix socket clients.
#[derive(Clone)]
pub struct RelayState {
    inner: Arc<Mutex<InnerState>>,
    tx: broadcast::Sender<RelayEvent>,
    clients_tx: Arc<watch::Sender<usize>>,
}

struct InnerState {
    history_capacity: usize,
    hub_url: Option<String>,
    connection: ConnectionStatus,
    system_order: Vec<String>,
    systems: HashMap<String, SystemEntry>,
}

struct SystemEntry {
    summary: SystemSummary,
    samples: VecDeque<LiveSample>,
}

/// RAII guard tracking an active local Unix socket client connection.
pub struct ClientGuard {
    clients_tx: Arc<watch::Sender<usize>>,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.clients_tx.send_modify(|count| {
            *count = count.saturating_sub(1);
        });
    }
}

impl RelayState {
    pub fn new(history_capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (clients_tx, _) = watch::channel(0usize);
        let now = now_ms();
        let inner = InnerState {
            history_capacity: history_capacity.max(1),
            hub_url: None,
            connection: ConnectionStatus {
                state: ConnectionState::Connecting,
                since_ms: now,
                last_sample_ms: None,
                last_error: None,
                reconnect_attempts: 0,
            },
            system_order: Vec::new(),
            systems: HashMap::new(),
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
            tx,
            clients_tx: Arc::new(clients_tx),
        }
    }

    /// Attach the configured Beszel hub URL so snapshots can link to host pages.
    pub fn with_hub_url(self, hub_url: String) -> Self {
        let mut guard = self.inner.lock().expect("relay state lock poisoned");
        let trimmed = hub_url.trim_end_matches('/').to_string();
        if !trimmed.is_empty() {
            guard.hub_url = Some(trimmed);
        }
        drop(guard);
        self
    }

    /// Increment active local client count until the returned guard is dropped.
    pub fn register_client(&self) -> ClientGuard {
        self.clients_tx.send_modify(|count| {
            *count = count.saturating_add(1);
        });
        ClientGuard {
            clients_tx: Arc::clone(&self.clients_tx),
        }
    }

    /// Current number of connected local Unix socket clients.
    pub fn client_count(&self) -> usize {
        *self.clients_tx.borrow()
    }

    /// Watch receiver notified whenever local client count changes.
    pub fn subscribe_client_count(&self) -> watch::Receiver<usize> {
        self.clients_tx.subscribe()
    }

    /// Atomically subscribe to live events and capture the current snapshot.
    pub fn subscribe_with_snapshot(&self) -> (RelayEvent, broadcast::Receiver<RelayEvent>) {
        let guard = self.inner.lock().expect("relay state lock poisoned");
        let rx = self.tx.subscribe();
        let snapshot = build_snapshot(&guard, now_ms());
        (snapshot, rx)
    }

    /// Capture a fresh snapshot (used after slow-client broadcast lag).
    pub fn snapshot(&self) -> RelayEvent {
        let guard = self.inner.lock().expect("relay state lock poisoned");
        build_snapshot(&guard, now_ms())
    }

    /// Update the set of subscribed systems while preserving existing rolling history.
    pub fn set_systems(&self, summaries: Vec<SystemSummary>) {
        let mut guard = self.inner.lock().expect("relay state lock poisoned");
        let capacity = guard.history_capacity;
        let mut next_order = Vec::with_capacity(summaries.len());
        let mut next_systems = HashMap::with_capacity(summaries.len());

        for summary in &summaries {
            let samples = guard
                .systems
                .remove(&summary.id)
                .map(|entry| entry.samples)
                .unwrap_or_else(|| VecDeque::with_capacity(capacity));
            next_order.push(summary.id.clone());
            next_systems.insert(
                summary.id.clone(),
                SystemEntry {
                    summary: summary.clone(),
                    samples,
                },
            );
        }

        guard.system_order = next_order;
        guard.systems = next_systems;
        let _ = self
            .tx
            .send(RelayEvent::SystemsChanged { systems: summaries });
    }

    /// Append a live sample into the bounded rolling window and broadcast it.
    pub fn record_sample(&self, system_id: &str, sample: LiveSample) {
        let mut guard = self.inner.lock().expect("relay state lock poisoned");
        let capacity = guard.history_capacity;
        let sample_ts = sample.timestamp_ms;

        if let Some(entry) = guard.systems.get_mut(system_id) {
            if let Some(prev) = entry.samples.back_mut() {
                compact_historical_sample(prev);
            }
            if entry.samples.len() >= capacity {
                entry.samples.pop_front();
            }
            entry.samples.push_back(sample.clone());
        }
        guard.connection.last_sample_ms = Some(sample_ts);

        let _ = self.tx.send(RelayEvent::Sample {
            system_id: system_id.to_string(),
            sample: Box::new(sample),
        });
    }

    /// Update connection lifecycle state and broadcast the new status.
    pub fn set_connection(
        &self,
        state: ConnectionState,
        last_error: Option<String>,
        reconnect_attempts: u32,
    ) {
        let mut guard = self.inner.lock().expect("relay state lock poisoned");
        let now = now_ms();
        if guard.connection.state != state {
            guard.connection.since_ms = now;
        }
        guard.connection.state = state;
        guard.connection.last_error = last_error;
        guard.connection.reconnect_attempts = reconnect_attempts;

        let _ = self.tx.send(RelayEvent::Connection {
            connection: guard.connection.clone(),
        });
    }
}

/// Strip latest-only heavy fields from historical samples so snapshots stay compact.
fn compact_historical_sample(sample: &mut LiveSample) {
    sample.cpu_cores = None;
    sample.containers = None;
    sample.unhealthy_pools = None;
    sample.extra_disks = None;
}

fn build_snapshot(inner: &InnerState, generated_at_ms: i64) -> RelayEvent {
    let systems = inner
        .system_order
        .iter()
        .filter_map(|id| inner.systems.get(id))
        .map(|entry| SystemLiveState {
            system: entry.summary.clone(),
            history_capacity: inner.history_capacity,
            samples: entry.samples.iter().cloned().collect(),
        })
        .collect();

    RelayEvent::Snapshot {
        protocol_version: LOCAL_PROTOCOL_VERSION,
        generated_at_ms,
        hub_url: inner.hub_url.clone(),
        connection: inner.connection.clone(),
        systems,
    }
}

/// Current local wall-clock time in milliseconds since Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ts: i64, cpu: f64) -> LiveSample {
        LiveSample {
            timestamp_ms: ts,
            cpu_pct: cpu,
            mem_pct: 25.0,
            ..LiveSample::default()
        }
    }

    #[test]
    fn rolling_history_evicts_oldest_samples_beyond_capacity() {
        let state = RelayState::new(3);
        state.set_systems(vec![SystemSummary {
            id: "sys1".into(),
            name: "host-a".into(),
            status: "up".into(),
            agent_version: Some("0.21.2".into()),
            ..SystemSummary::default()
        }]);

        for i in 1..=5 {
            state.record_sample("sys1", sample(i * 1000, i as f64));
        }

        let RelayEvent::Snapshot {
            systems,
            connection,
            ..
        } = state.snapshot()
        else {
            panic!("expected Snapshot");
        };

        assert_eq!(connection.last_sample_ms, Some(5000));
        assert_eq!(systems.len(), 1);
        let timestamps: Vec<i64> = systems[0].samples.iter().map(|s| s.timestamp_ms).collect();
        assert_eq!(timestamps, vec![3000, 4000, 5000]);
    }

    #[test]
    fn set_systems_preserves_history_for_existing_system() {
        let state = RelayState::new(10);
        let summary = SystemSummary {
            id: "sys1".into(),
            name: "host-a".into(),
            status: "up".into(),
            agent_version: Some("0.21.2".into()),
            ..SystemSummary::default()
        };
        state.set_systems(vec![summary.clone()]);
        state.record_sample("sys1", sample(1000, 10.0));
        state.set_systems(vec![summary]);

        let RelayEvent::Snapshot { systems, .. } = state.snapshot() else {
            panic!("expected Snapshot");
        };
        assert_eq!(systems[0].samples.len(), 1);
    }

    #[test]
    fn client_guard_tracks_active_connections() {
        let state = RelayState::new(5);
        assert_eq!(state.client_count(), 0);
        let g1 = state.register_client();
        let g2 = state.register_client();
        assert_eq!(state.client_count(), 2);
        drop(g1);
        assert_eq!(state.client_count(), 1);
        drop(g2);
        assert_eq!(state.client_count(), 0);
    }
}
