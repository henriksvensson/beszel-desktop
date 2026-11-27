use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use beszel_protocol::local::{ActiveAlert, ConnectionState, SystemSummary};
use beszel_protocol::upstream::{
    ALERTS_COLLECTION_TOPIC, AlertsListResponse, AlertsRealtimeEvent, AuthResponse, PbConnectEvent,
    RealtimeSubscribeRequest, RtMetricsPayload, SYSTEMS_COLLECTION_TOPIC,
    SystemDetailsListResponse, SystemsListResponse, SystemsRealtimeEvent, UpstreamAlertRecord,
    UpstreamSystemDetailsRecord, UpstreamSystemRecord, os_family_label, rt_metrics_topic,
};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use tokio::sync::watch;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::config::RelayConfig;
use crate::sse::{SseEvent, SseParser};
use crate::state::{RelayState, now_ms};

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const SSE_LIVE_IDLE_TIMEOUT: Duration = Duration::from_secs(20);
const SSE_PAUSED_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const IDLE_PAUSE_GRACE: Duration = Duration::from_secs(3);
const INITIAL_BACKOFF_SECS: u64 = 1;
const MAX_BACKOFF_SECS: u64 = 30;

/// Signals controlling the hub connection loop.
pub struct HubSignals {
    pub shutdown: watch::Receiver<bool>,
    pub wake: watch::Receiver<u64>,
}

/// Run the upstream Beszel hub connection loop until shutdown is signaled.
pub async fn run_hub_loop(config: RelayConfig, state: RelayState, mut signals: HubSignals) {
    let client = match Client::builder().connect_timeout(HTTP_TIMEOUT).build() {
        Ok(client) => client,
        Err(err) => {
            state.set_connection(ConnectionState::Disconnected, Some(err.to_string()), 0);
            return;
        }
    };

    let hub = HubClient {
        client: &client,
        config: &config,
        state: &state,
    };
    let mut token: Option<String> = None;
    let mut attempts: u32 = 0;

    while !*signals.shutdown.borrow() {
        let conn_state = if attempts == 0 {
            ConnectionState::Connecting
        } else {
            ConnectionState::Reconnecting
        };
        state.set_connection(conn_state, None, attempts);

        match hub.run_single_session(&mut token, &mut signals).await {
            Ok(()) => break,
            Err(SessionError::WakeReconnect) => {
                attempts = 0;
                info!("Reconnecting immediately after system wake");
                continue;
            }
            Err(SessionError::Auth(msg)) => {
                token = None;
                attempts = attempts.saturating_add(1);
                warn!(attempt = attempts, error = %msg, "Beszel authentication failed");
                state.set_connection(ConnectionState::AuthError, Some(msg), attempts);
            }
            Err(SessionError::Transient(msg)) => {
                attempts = attempts.saturating_add(1);
                warn!(attempt = attempts, error = %msg, "Beszel realtime session ended");
                state.set_connection(ConnectionState::Reconnecting, Some(msg), attempts);
            }
        }

        if wait_for_backoff(backoff_duration(attempts), &mut signals).await {
            break;
        }
    }

    state.set_connection(ConnectionState::Disconnected, None, 0);
}

struct HubClient<'a> {
    client: &'a Client,
    config: &'a RelayConfig,
    state: &'a RelayState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubscriptionMap {
    by_topic: HashMap<String, String>,
    topics: Vec<String>,
}

impl SubscriptionMap {
    fn from_systems(systems: &[SystemSummary]) -> Self {
        let mut by_topic = HashMap::with_capacity(systems.len());
        let mut topics = Vec::with_capacity(systems.len());
        for sys in systems {
            let topic = rt_metrics_topic(&sys.id);
            by_topic.insert(topic.clone(), sys.id.clone());
            topics.push(topic);
        }
        Self { by_topic, topics }
    }

    fn system_id_for(&self, event_name: &str) -> Option<&str> {
        if let Some(id) = self.by_topic.get(event_name) {
            return Some(id.as_str());
        }
        if !event_name.starts_with("rt_metrics") {
            return None;
        }
        self.by_topic
            .values()
            .find(|id| event_name.contains(id.as_str()))
            .map(String::as_str)
    }

    fn all_subscriptions(&self, include_metrics: bool) -> Vec<String> {
        let mut out = Vec::with_capacity(2 + self.topics.len());
        out.push(SYSTEMS_COLLECTION_TOPIC.to_string());
        out.push(ALERTS_COLLECTION_TOPIC.to_string());
        if include_metrics {
            out.extend(self.topics.iter().cloned());
        }
        out
    }
}

enum SessionError {
    Auth(String),
    Transient(String),
    WakeReconnect,
}

struct ActiveSession<'a> {
    hub: &'a HubClient<'a>,
    token: String,
    client_id: String,
    known_records: HashMap<String, UpstreamSystemRecord>,
    known_details: HashMap<String, UpstreamSystemDetailsRecord>,
    known_alerts: HashMap<String, UpstreamAlertRecord>,
    current_systems: Vec<SystemSummary>,
    subs: SubscriptionMap,
    streaming_metrics: bool,
}

impl<'a> ActiveSession<'a> {
    async fn sync_subscriptions(&mut self) -> std::result::Result<(), SessionError> {
        let topics = self.subs.all_subscriptions(self.streaming_metrics);
        self.hub
            .subscribe_topics(&self.token, &self.client_id, topics)
            .await?;

        let state = if self.streaming_metrics {
            ConnectionState::Connected
        } else {
            ConnectionState::Paused
        };
        self.hub.state.set_connection(state, None, 0);
        Ok(())
    }

    async fn set_streaming(&mut self, enabled: bool) -> std::result::Result<(), SessionError> {
        if self.streaming_metrics == enabled {
            return Ok(());
        }
        self.streaming_metrics = enabled;
        info!(
            streaming_metrics = enabled,
            systems = self.subs.topics.len(),
            "Updating Beszel live rt_metrics subscription state"
        );
        self.sync_subscriptions().await
    }

    async fn handle_event(&mut self, event: &SseEvent) -> std::result::Result<(), SessionError> {
        let Some(name) = event.event.as_deref() else {
            return Ok(());
        };
        if name == SYSTEMS_COLLECTION_TOPIC || name.starts_with("systems/") {
            return self.handle_systems_event(&event.data).await;
        }
        if name == ALERTS_COLLECTION_TOPIC || name.starts_with("alerts/") {
            self.handle_alerts_event(&event.data);
            return Ok(());
        }
        if let Some(system_id) = self.subs.system_id_for(name) {
            decode_and_record_sample(system_id, &event.data, self.hub.state);
        } else {
            debug!(event = %name, "Ignoring unrelated SSE event");
        }
        Ok(())
    }

    fn sync_alerts(&self) {
        let active = active_alerts_for_systems(&self.known_alerts, &self.current_systems);
        let counts = configured_alert_counts(&self.known_alerts, &self.current_systems);
        self.hub.state.set_alerts(active, counts);
    }

    fn handle_alerts_event(&mut self, data: &str) {
        let parsed: AlertsRealtimeEvent = match serde_json::from_str(data) {
            Ok(ev) => ev,
            Err(err) => {
                warn!(error = %err, "Failed to decode alerts/* realtime event");
                return;
            }
        };
        apply_alert_mutation(&mut self.known_alerts, parsed);
        self.sync_alerts();
    }

    async fn handle_systems_event(&mut self, data: &str) -> std::result::Result<(), SessionError> {
        let parsed: SystemsRealtimeEvent = match serde_json::from_str(data) {
            Ok(ev) => ev,
            Err(err) => {
                warn!(error = %err, "Failed to decode systems/* realtime event");
                return Ok(());
            }
        };

        apply_record_mutation(&mut self.known_records, parsed);
        let records: Vec<UpstreamSystemRecord> = self.known_records.values().cloned().collect();
        let next_systems = select_systems(
            &records,
            &self.known_details,
            self.hub.config.system.as_deref(),
        )
        .unwrap_or_default();

        if next_systems == self.current_systems {
            return Ok(());
        }

        let next_subs = SubscriptionMap::from_systems(&next_systems);
        let topics_changed = next_subs.topics != self.subs.topics;
        self.current_systems = next_systems.clone();
        self.subs = next_subs;
        self.hub.state.set_systems(next_systems);
        self.sync_alerts();

        if topics_changed && self.streaming_metrics {
            info!(
                systems = self.subs.topics.len(),
                "Resyncing rt_metrics subscriptions after systems collection change"
            );
            self.sync_subscriptions().await?;
        }
        Ok(())
    }
}

impl HubClient<'_> {
    async fn run_single_session(
        &self,
        cached_token: &mut Option<String>,
        signals: &mut HubSignals,
    ) -> std::result::Result<(), SessionError> {
        let token = self.ensure_token(cached_token).await?;
        let records = self
            .fetch_systems(&token)
            .await
            .inspect_err(|err| clear_token_on_auth_error(err, cached_token))?;
        let details = self.fetch_system_details(&token).await;
        let alerts = self.fetch_alerts(&token).await;

        let systems = select_systems(&records, &details, self.config.system.as_deref())
            .map_err(|e| SessionError::Transient(e.to_string()))?;
        let names: Vec<&str> = systems.iter().map(|s| s.name.as_str()).collect();
        info!(count = systems.len(), systems = ?names, "Discovered Beszel systems");
        self.state.set_systems(systems.clone());
        self.state.set_alerts(
            active_alerts_for_systems(&alerts, &systems),
            configured_alert_counts(&alerts, &systems),
        );

        let response = self.open_sse_stream().await?;
        let mut sse_stream = SseStream {
            byte_stream: response.bytes_stream(),
            parser: SseParser::default(),
        };

        let client_id = sse_stream.wait_for_pb_connect(signals).await?;
        let mut session = ActiveSession {
            hub: self,
            token,
            client_id,
            known_records: records.into_iter().map(|r| (r.id.clone(), r)).collect(),
            known_details: details,
            known_alerts: alerts,
            current_systems: systems.clone(),
            subs: SubscriptionMap::from_systems(&systems),
            streaming_metrics: self.state.client_count() > 0,
        };

        session
            .sync_subscriptions()
            .await
            .inspect_err(|err| clear_token_on_auth_error(err, cached_token))?;

        info!(
            streaming_metrics = session.streaming_metrics,
            systems = session.subs.topics.len(),
            "Beszel realtime session active"
        );

        let mut client_rx = self.state.subscribe_client_count();
        pump_session(&mut session, &mut sse_stream, &mut client_rx, signals)
            .await
            .inspect_err(|err| clear_token_on_auth_error(err, cached_token))
    }

    async fn ensure_token(
        &self,
        cached_token: &mut Option<String>,
    ) -> std::result::Result<String, SessionError> {
        if let Some(existing) = cached_token.clone() {
            return Ok(existing);
        }
        let fresh = self.authenticate().await?;
        *cached_token = Some(fresh.clone());
        Ok(fresh)
    }

    async fn authenticate(&self) -> std::result::Result<String, SessionError> {
        let url = format!(
            "{}/api/collections/users/auth-with-password",
            self.config.hub_url
        );
        let resp = self
            .client
            .post(&url)
            .timeout(HTTP_TIMEOUT)
            .json(&serde_json::json!({
                "identity": self.config.email,
                "password": self.config.password,
            }))
            .send()
            .await
            .map_err(|e| SessionError::Transient(format!("auth request failed: {e}")))?;

        let status = resp.status();
        if is_auth_rejection(status) {
            return Err(SessionError::Auth(format!(
                "hub rejected credentials (HTTP {status})"
            )));
        }
        if !status.is_success() {
            return Err(SessionError::Transient(format!(
                "auth endpoint returned HTTP {status}"
            )));
        }

        let body: AuthResponse = resp
            .json()
            .await
            .map_err(|e| SessionError::Transient(format!("invalid auth response: {e}")))?;
        Ok(body.token)
    }

    async fn fetch_systems(
        &self,
        token: &str,
    ) -> std::result::Result<Vec<UpstreamSystemRecord>, SessionError> {
        let url = format!(
            "{}/api/collections/systems/records?perPage=50",
            self.config.hub_url
        );
        let resp = self
            .client
            .get(&url)
            .timeout(HTTP_TIMEOUT)
            .header("Authorization", token)
            .send()
            .await
            .map_err(|e| SessionError::Transient(format!("systems discovery failed: {e}")))?;

        let status = resp.status();
        if is_unauthorized(status) {
            return Err(SessionError::Auth(format!(
                "systems discovery unauthorized (HTTP {status})"
            )));
        }
        if !status.is_success() {
            return Err(SessionError::Transient(format!(
                "systems discovery returned HTTP {status}"
            )));
        }

        let list: SystemsListResponse = resp
            .json()
            .await
            .map_err(|e| SessionError::Transient(format!("invalid systems list: {e}")))?;
        Ok(list.items)
    }

    async fn fetch_optional_json<T: DeserializeOwned>(&self, path: &str, token: &str) -> Option<T> {
        let url = format!("{}{}", self.config.hub_url, path);
        let resp = self
            .client
            .get(&url)
            .timeout(HTTP_TIMEOUT)
            .header("Authorization", token)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            debug!(path, status = %resp.status(), "Optional Beszel collection unavailable");
            return None;
        }
        resp.json::<T>().await.ok()
    }

    async fn fetch_system_details(
        &self,
        token: &str,
    ) -> HashMap<String, UpstreamSystemDetailsRecord> {
        let Some(list) = self
            .fetch_optional_json::<SystemDetailsListResponse>(
                "/api/collections/system_details/records?perPage=200",
                token,
            )
            .await
        else {
            return HashMap::new();
        };
        list.items
            .into_iter()
            .filter(|item| !item.system_key().is_empty())
            .map(|item| (item.system_key().to_string(), item))
            .collect()
    }

    async fn fetch_alerts(&self, token: &str) -> HashMap<String, UpstreamAlertRecord> {
        let Some(list) = self
            .fetch_optional_json::<AlertsListResponse>(
                "/api/collections/alerts/records?perPage=200&fields=id,name,system,value,min,triggered",
                token,
            )
            .await
        else {
            return HashMap::new();
        };
        list.items
            .into_iter()
            .filter(|item| !item.id.is_empty())
            .map(|item| (item.id.clone(), item))
            .collect()
    }

    async fn open_sse_stream(&self) -> std::result::Result<reqwest::Response, SessionError> {
        let url = format!("{}/api/realtime", self.config.hub_url);
        let resp = self
            .client
            .get(&url)
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .send()
            .await
            .map_err(|e| SessionError::Transient(format!("SSE connect failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(SessionError::Transient(format!(
                "SSE endpoint returned HTTP {}",
                resp.status()
            )));
        }
        Ok(resp)
    }

    async fn subscribe_topics(
        &self,
        token: &str,
        client_id: &str,
        subscriptions: Vec<String>,
    ) -> std::result::Result<(), SessionError> {
        let url = format!("{}/api/realtime", self.config.hub_url);
        let req_body = RealtimeSubscribeRequest {
            client_id: client_id.to_string(),
            subscriptions,
        };
        let resp = self
            .client
            .post(&url)
            .timeout(HTTP_TIMEOUT)
            .header("Authorization", token)
            .json(&req_body)
            .send()
            .await
            .map_err(|e| SessionError::Transient(format!("SSE subscribe failed: {e}")))?;

        let status = resp.status();
        if is_unauthorized(status) {
            return Err(SessionError::Auth(format!(
                "SSE subscribe unauthorized (HTTP {status})"
            )));
        }
        if !status.is_success() {
            return Err(SessionError::Transient(format!(
                "SSE subscribe returned HTTP {status}"
            )));
        }
        Ok(())
    }
}

async fn pump_session<S, B>(
    session: &mut ActiveSession<'_>,
    stream: &mut SseStream<S>,
    client_rx: &mut watch::Receiver<usize>,
    signals: &mut HubSignals,
) -> std::result::Result<(), SessionError>
where
    S: futures_util::Stream<Item = reqwest::Result<B>> + Unpin,
    B: AsRef<[u8]>,
{
    let mut pause_deadline: Option<Instant> = None;

    loop {
        let timeout = session_idle_timeout(session.streaming_metrics);

        tokio::select! {
            changed = signals.shutdown.changed() => {
                if changed.is_err() || *signals.shutdown.borrow() {
                    return Ok(());
                }
            }
            changed = signals.wake.changed() => {
                if changed.is_ok() {
                    return Err(SessionError::WakeReconnect);
                }
            }
            changed = client_rx.changed() => {
                if changed.is_ok() {
                    let clients = *client_rx.borrow();
                    handle_client_count_change(session, clients, &mut pause_deadline).await?;
                }
            }
            _ = sleep_until_optional(pause_deadline) => {
                pause_deadline = None;
                pause_if_idle(session, client_rx).await?;
            }
            item = tokio::time::timeout(timeout, stream.byte_stream.next()) => {
                process_sse_chunk(session, &mut stream.parser, item).await?;
            }
        }
    }
}

fn session_idle_timeout(streaming_metrics: bool) -> Duration {
    if streaming_metrics {
        SSE_LIVE_IDLE_TIMEOUT
    } else {
        SSE_PAUSED_IDLE_TIMEOUT
    }
}

async fn pause_if_idle(
    session: &mut ActiveSession<'_>,
    client_rx: &watch::Receiver<usize>,
) -> std::result::Result<(), SessionError> {
    let clients = *client_rx.borrow();
    if clients == 0 {
        return session.set_streaming(false).await;
    }
    Ok(())
}

async fn process_sse_chunk<B>(
    session: &mut ActiveSession<'_>,
    parser: &mut SseParser,
    item: std::result::Result<Option<reqwest::Result<B>>, tokio::time::error::Elapsed>,
) -> std::result::Result<(), SessionError>
where
    B: AsRef<[u8]>,
{
    let bytes = unpack_sse_chunk(item, session.streaming_metrics)?;
    for event in parser.push_chunk(bytes.as_ref()) {
        session.handle_event(&event).await?;
    }
    Ok(())
}

async fn handle_client_count_change(
    session: &mut ActiveSession<'_>,
    clients: usize,
    pause_deadline: &mut Option<Instant>,
) -> std::result::Result<(), SessionError> {
    if clients > 0 {
        *pause_deadline = None;
        return session.set_streaming(true).await;
    }
    if session.streaming_metrics && pause_deadline.is_none() {
        *pause_deadline = Some(Instant::now() + IDLE_PAUSE_GRACE);
    }
    Ok(())
}

async fn sleep_until_optional(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

fn unpack_sse_chunk<B>(
    item: std::result::Result<Option<reqwest::Result<B>>, tokio::time::error::Elapsed>,
    streaming_metrics: bool,
) -> std::result::Result<B, SessionError> {
    match item {
        Ok(Some(Ok(bytes))) => Ok(bytes),
        Ok(Some(Err(err))) => Err(SessionError::Transient(format!("SSE read error: {err}"))),
        Ok(None) => Err(SessionError::Transient("SSE stream closed by hub".into())),
        Err(_) if streaming_metrics => Err(SessionError::Transient(
            "SSE stream timed out waiting for data".into(),
        )),
        Err(_) => Err(SessionError::Transient(
            "paused SSE keepalive refresh".into(),
        )),
    }
}

struct SseStream<S> {
    byte_stream: S,
    parser: SseParser,
}

impl<S, B> SseStream<S>
where
    S: futures_util::Stream<Item = reqwest::Result<B>> + Unpin,
    B: AsRef<[u8]>,
{
    async fn wait_for_pb_connect(
        &mut self,
        signals: &mut HubSignals,
    ) -> std::result::Result<String, SessionError> {
        loop {
            let chunk = next_pb_connect_chunk(&mut self.byte_stream, signals).await?;
            for event in self.parser.push_chunk(chunk.as_ref()) {
                if event.event.as_deref() == Some("PB_CONNECT") {
                    let parsed: PbConnectEvent = serde_json::from_str(&event.data)
                        .map_err(|e| SessionError::Transient(format!("invalid PB_CONNECT: {e}")))?;
                    return Ok(parsed.client_id);
                }
            }
        }
    }
}

async fn next_pb_connect_chunk<S, B>(
    byte_stream: &mut S,
    signals: &mut HubSignals,
) -> std::result::Result<B, SessionError>
where
    S: futures_util::Stream<Item = reqwest::Result<B>> + Unpin,
{
    tokio::select! {
        changed = signals.shutdown.changed() => {
            let _ = changed;
            Err(SessionError::Transient("shutdown requested".into()))
        }
        changed = signals.wake.changed() => {
            let _ = changed;
            Err(SessionError::WakeReconnect)
        }
        item = tokio::time::timeout(SSE_LIVE_IDLE_TIMEOUT, byte_stream.next()) => {
            unpack_sse_chunk(item, true)
        }
    }
}

fn apply_collection_mutation<T>(map: &mut HashMap<String, T>, action: &str, id: String, record: T) {
    match action {
        "delete" => {
            map.remove(&id);
        }
        "create" | "update" if !id.is_empty() => {
            map.insert(id, record);
        }
        other => {
            debug!(action = %other, "Ignoring unknown collection realtime action");
        }
    }
}

fn apply_record_mutation(
    known_records: &mut HashMap<String, UpstreamSystemRecord>,
    event: SystemsRealtimeEvent,
) {
    let id = event.record.id.clone();
    apply_collection_mutation(known_records, &event.action, id, event.record);
}

fn apply_alert_mutation(
    known_alerts: &mut HashMap<String, UpstreamAlertRecord>,
    event: AlertsRealtimeEvent,
) {
    let id = event.record.id.clone();
    apply_collection_mutation(known_alerts, &event.action, id, event.record);
}

fn active_alerts_for_systems(
    known_alerts: &HashMap<String, UpstreamAlertRecord>,
    systems: &[SystemSummary],
) -> Vec<ActiveAlert> {
    let mut active: Vec<ActiveAlert> = known_alerts
        .values()
        .filter(|alert| systems.iter().any(|sys| sys.id == alert.system))
        .filter_map(UpstreamAlertRecord::to_active_alert)
        .collect();
    active.sort_by(|a, b| {
        a.system_id
            .cmp(&b.system_id)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    active
}

fn configured_alert_counts(
    known_alerts: &HashMap<String, UpstreamAlertRecord>,
    systems: &[SystemSummary],
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for alert in known_alerts.values() {
        if systems.iter().any(|sys| sys.id == alert.system) {
            *counts.entry(alert.system.clone()).or_insert(0) += 1;
        }
    }
    counts
}

fn decode_and_record_sample(system_id: &str, data: &str, state: &RelayState) {
    match serde_json::from_str::<RtMetricsPayload>(data) {
        Ok(payload) => {
            if let Some(sample) = payload.to_live_sample(now_ms()) {
                state.record_sample(system_id, sample);
            } else {
                warn!(
                    system_id,
                    "Received rt_metrics payload missing CPU or memory fields"
                );
            }
        }
        Err(err) => {
            warn!(system_id, error = %err, "Failed to decode rt_metrics payload");
        }
    }
}

fn clear_token_on_auth_error(err: &SessionError, cached_token: &mut Option<String>) {
    if matches!(err, SessionError::Auth(_)) {
        *cached_token = None;
    }
}

fn is_auth_rejection(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    )
}

fn is_unauthorized(status: StatusCode) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
}

fn select_systems(
    records: &[UpstreamSystemRecord],
    details: &HashMap<String, UpstreamSystemDetailsRecord>,
    requested: Option<&str>,
) -> Result<Vec<SystemSummary>> {
    if records.is_empty() {
        bail!("no systems accessible to this Beszel account");
    }

    let chosen: Vec<&UpstreamSystemRecord> = match requested {
        Some(filter) => match_requested_systems(records, filter)?,
        None => {
            let up: Vec<&UpstreamSystemRecord> =
                records.iter().filter(|r| r.status == "up").collect();
            if up.is_empty() {
                records.iter().collect()
            } else {
                up
            }
        }
    };

    let mut summaries: Vec<SystemSummary> = chosen
        .into_iter()
        .map(|record| to_summary(record, details.get(&record.id)))
        .collect();
    summaries.sort_by_key(|s| s.name.to_lowercase());
    Ok(summaries)
}

fn match_requested_systems<'a>(
    records: &'a [UpstreamSystemRecord],
    filter: &str,
) -> Result<Vec<&'a UpstreamSystemRecord>> {
    let mut matched = Vec::new();
    for raw_target in filter.split(',') {
        let target = raw_target.trim();
        if target.is_empty() {
            continue;
        }
        let record = records
            .iter()
            .find(|r| r.id == target || r.name.eq_ignore_ascii_case(target))
            .ok_or_else(|| anyhow!("configured system {target:?} not found on hub"))?;
        if !matched
            .iter()
            .any(|existing: &&UpstreamSystemRecord| existing.id == record.id)
        {
            matched.push(record);
        }
    }
    if matched.is_empty() {
        bail!("configured system filter {filter:?} matched no systems");
    }
    Ok(matched)
}

fn non_empty_trimmed(val: Option<&String>) -> Option<String> {
    val.map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn to_summary(
    record: &UpstreamSystemRecord,
    details: Option<&UpstreamSystemDetailsRecord>,
) -> SystemSummary {
    let info = record.info.as_ref();
    let os_code = details
        .and_then(|d| d.os)
        .or_else(|| info.and_then(|i| i.os));
    let os = os_code.and_then(os_family_label).map(str::to_string);
    let os_name = details.and_then(|d| non_empty_trimmed(d.os_name.as_ref()));
    let kernel = details
        .and_then(|d| non_empty_trimmed(d.kernel.as_ref()))
        .or_else(|| info.and_then(|i| non_empty_trimmed(i.k.as_ref())));
    let cpu_model = details
        .and_then(|d| non_empty_trimmed(d.cpu.as_ref()))
        .or_else(|| info.and_then(|i| non_empty_trimmed(i.m.as_ref())));
    let arch = details.and_then(|d| non_empty_trimmed(d.arch.as_ref()));

    SystemSummary {
        id: record.id.clone(),
        name: record.name.clone(),
        status: record.status.clone(),
        agent_version: info.and_then(|i| i.v.clone()),
        os_name,
        os,
        kernel,
        cpu_model,
        arch,
    }
}

fn backoff_duration(attempts: u32) -> Duration {
    let shift = attempts.saturating_sub(1).min(5);
    let secs = (INITIAL_BACKOFF_SECS << shift).min(MAX_BACKOFF_SECS);
    Duration::from_secs(secs)
}

async fn wait_for_backoff(delay: Duration, signals: &mut HubSignals) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        _ = signals.wake.changed() => false,
        _ = signals.shutdown.changed() => *signals.shutdown.borrow(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_records() -> Vec<UpstreamSystemRecord> {
        vec![
            UpstreamSystemRecord {
                id: "id_down".into(),
                name: "offline".into(),
                status: "down".into(),
                info: None,
            },
            UpstreamSystemRecord {
                id: "id_b".into(),
                name: "workstation".into(),
                status: "up".into(),
                info: None,
            },
            UpstreamSystemRecord {
                id: "id_a".into(),
                name: "server".into(),
                status: "up".into(),
                info: None,
            },
        ]
    }

    #[test]
    fn backoff_doubles_and_caps_at_thirty_seconds() {
        assert_eq!(backoff_duration(1), Duration::from_secs(1));
        assert_eq!(backoff_duration(2), Duration::from_secs(2));
        assert_eq!(backoff_duration(3), Duration::from_secs(4));
        assert_eq!(backoff_duration(4), Duration::from_secs(8));
        assert_eq!(backoff_duration(5), Duration::from_secs(16));
        assert_eq!(backoff_duration(6), Duration::from_secs(30));
        assert_eq!(backoff_duration(20), Duration::from_secs(30));
    }

    #[test]
    fn selects_all_up_systems_by_default_sorted_by_name() {
        let records = sample_records();
        let details = HashMap::from([(
            "id_a".to_string(),
            UpstreamSystemDetailsRecord {
                id: "id_a".into(),
                system: "id_a".into(),
                os: Some(0),
                os_name: Some("Ubuntu 24.04.2 LTS".into()),
                kernel: Some("6.8.0-55-generic".into()),
                cpu: Some("AMD EPYC 7763".into()),
                arch: Some("amd64".into()),
            },
        )]);
        let selected =
            select_systems(&records, &details, None).expect("default selection should succeed");
        let ids: Vec<&str> = selected.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["id_a", "id_b"]);
        assert_eq!(selected[0].os_name.as_deref(), Some("Ubuntu 24.04.2 LTS"));
        assert_eq!(selected[0].os.as_deref(), Some("linux"));
        assert_eq!(selected[0].cpu_model.as_deref(), Some("AMD EPYC 7763"));
    }

    #[test]
    fn selects_specific_or_comma_separated_systems() {
        let records = sample_records();
        let selected = select_systems(&records, &HashMap::new(), Some("WORKSTATION, offline"))
            .expect("comma filter should succeed");
        let names: Vec<&str> = selected.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["offline", "workstation"]);
    }

    #[test]
    fn routes_sse_events_to_matching_system_id() {
        let systems = select_systems(&sample_records(), &HashMap::new(), None).expect("systems");
        let subs = SubscriptionMap::from_systems(&systems);
        let topic_b = rt_metrics_topic("id_b");
        assert_eq!(subs.system_id_for(&topic_b), Some("id_b"));
        assert_eq!(subs.system_id_for("PB_CONNECT"), None);
    }

    #[test]
    fn paused_subscriptions_exclude_rt_metrics_topics() {
        let systems = select_systems(&sample_records(), &HashMap::new(), None).expect("systems");
        let subs = SubscriptionMap::from_systems(&systems);
        assert_eq!(
            subs.all_subscriptions(false),
            vec![
                SYSTEMS_COLLECTION_TOPIC.to_string(),
                ALERTS_COLLECTION_TOPIC.to_string(),
            ]
        );
        assert_eq!(subs.all_subscriptions(true).len(), 4);
    }

    #[test]
    fn alert_mutations_filter_triggered_alerts_to_subscribed_systems() {
        let systems = select_systems(&sample_records(), &HashMap::new(), None).expect("systems");
        let mut known = HashMap::new();
        apply_alert_mutation(
            &mut known,
            AlertsRealtimeEvent {
                action: "create".into(),
                record: UpstreamAlertRecord {
                    id: "a1".into(),
                    system: "id_a".into(),
                    name: "Disk".into(),
                    triggered: true,
                    value: Some(85.0),
                    min: Some(5),
                },
            },
        );
        apply_alert_mutation(
            &mut known,
            AlertsRealtimeEvent {
                action: "create".into(),
                record: UpstreamAlertRecord {
                    id: "a2".into(),
                    system: "id_b".into(),
                    name: "CPU".into(),
                    triggered: false,
                    value: Some(90.0),
                    min: Some(10),
                },
            },
        );
        apply_alert_mutation(
            &mut known,
            AlertsRealtimeEvent {
                action: "create".into(),
                record: UpstreamAlertRecord {
                    id: "a3".into(),
                    system: "id_down".into(),
                    name: "Status".into(),
                    triggered: true,
                    value: None,
                    min: None,
                },
            },
        );

        let active = active_alerts_for_systems(&known, &systems);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "a1");
        assert_eq!(active[0].name, "Disk");

        apply_alert_mutation(
            &mut known,
            AlertsRealtimeEvent {
                action: "delete".into(),
                record: UpstreamAlertRecord {
                    id: "a1".into(),
                    ..UpstreamAlertRecord::default()
                },
            },
        );
        assert!(active_alerts_for_systems(&known, &systems).is_empty());
    }

    #[test]
    fn record_mutations_update_and_delete_known_systems() {
        let mut known: HashMap<String, UpstreamSystemRecord> = sample_records()
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();

        apply_record_mutation(
            &mut known,
            SystemsRealtimeEvent {
                action: "update".into(),
                record: UpstreamSystemRecord {
                    id: "id_down".into(),
                    name: "offline".into(),
                    status: "up".into(),
                    info: None,
                },
            },
        );
        assert_eq!(known.get("id_down").unwrap().status, "up");

        apply_record_mutation(
            &mut known,
            SystemsRealtimeEvent {
                action: "delete".into(),
                record: UpstreamSystemRecord {
                    id: "id_down".into(),
                    name: "offline".into(),
                    status: "down".into(),
                    info: None,
                },
            },
        );
        assert!(!known.contains_key("id_down"));
    }
}
