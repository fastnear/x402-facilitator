use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection};
use tokio::sync::watch;
use zeroize::Zeroizing;

#[derive(Clone, Default)]
#[allow(missing_debug_implementations)]
pub struct ReadinessState {
    inner: Arc<ReadinessInner>,
}

#[derive(Default)]
struct ReadinessInner {
    // Three states distinguish "not observed yet" from the fail-closed false
    // value exposed by `snapshot`, so every gate's first observation is logged.
    database: AtomicU8,
    leadership: AtomicU8,
    reconciliation: AtomicU8,
    rpc: AtomicU8,
    relayer: AtomicU8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadinessGate {
    Database,
    Leadership,
    Reconciliation,
    Rpc,
    Relayer,
}

impl ReadinessGate {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::Leadership => "leadership",
            Self::Reconciliation => "reconciliation",
            Self::Rpc => "rpc",
            Self::Relayer => "relayer",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadinessGateState {
    Ready,
    NotReady,
}

impl ReadinessGateState {
    const UNKNOWN: u8 = 0;
    const NOT_READY: u8 = 1;
    const READY: u8 = 2;

    const fn from_ready(ready: bool) -> Self {
        if ready { Self::Ready } else { Self::NotReady }
    }

    const fn from_atomic(value: u8) -> Option<Self> {
        match value {
            Self::NOT_READY => Some(Self::NotReady),
            Self::READY => Some(Self::Ready),
            Self::UNKNOWN | 3..=u8::MAX => None,
        }
    }

    const fn as_atomic(self) -> u8 {
        match self {
            Self::Ready => Self::READY,
            Self::NotReady => Self::NOT_READY,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NotReady => "not_ready",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReadinessTransition {
    gate: ReadinessGate,
    state: ReadinessGateState,
}

impl ReadinessTransition {
    fn observed(gate: ReadinessGate, previous: u8, ready: bool) -> Option<Self> {
        let state = ReadinessGateState::from_ready(ready);
        if ReadinessGateState::from_atomic(previous) == Some(state) {
            None
        } else {
            Some(Self { gate, state })
        }
    }

    fn emit(self) {
        let gate = self.gate.as_str();
        let state = self.state.as_str();
        match self.state {
            ReadinessGateState::Ready => {
                tracing::info!(event = "readiness_gate_transition", gate, state);
            }
            ReadinessGateState::NotReady => {
                tracing::warn!(event = "readiness_gate_transition", gate, state);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
// Named readiness gates are intentionally independent and externally visible.
#[allow(clippy::struct_excessive_bools)]
pub struct ReadinessSnapshot {
    pub leadership: bool,
    pub reconciliation: bool,
    pub rpc: bool,
    pub relayer: bool,
}

#[allow(missing_debug_implementations)]
pub struct LeadershipHandle {
    shutdown: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl ReadinessState {
    pub fn set_database(&self, ready: bool) {
        self.set_gate(ReadinessGate::Database, ready);
    }

    pub fn set_leadership(&self, ready: bool) {
        self.set_gate(ReadinessGate::Leadership, ready);
        if !ready {
            self.set_reconciliation(false);
        }
    }

    pub fn set_reconciliation(&self, ready: bool) {
        self.set_gate(ReadinessGate::Reconciliation, ready);
    }

    pub fn set_rpc(&self, ready: bool) {
        self.set_gate(ReadinessGate::Rpc, ready);
    }

    pub fn set_relayer(&self, ready: bool) {
        self.set_gate(ReadinessGate::Relayer, ready);
    }

    fn set_gate(&self, gate: ReadinessGate, ready: bool) {
        let value = match gate {
            ReadinessGate::Database => &self.inner.database,
            ReadinessGate::Leadership => &self.inner.leadership,
            ReadinessGate::Reconciliation => &self.inner.reconciliation,
            ReadinessGate::Rpc => &self.inner.rpc,
            ReadinessGate::Relayer => &self.inner.relayer,
        };
        let state = ReadinessGateState::from_ready(ready);
        let previous = value.swap(state.as_atomic(), Ordering::AcqRel);
        if let Some(transition) = ReadinessTransition::observed(gate, previous, ready) {
            transition.emit();
        }
    }

    pub fn snapshot(&self) -> ReadinessSnapshot {
        ReadinessSnapshot {
            leadership: atomic_gate_ready(&self.inner.leadership),
            reconciliation: atomic_gate_ready(&self.inner.reconciliation),
            rpc: atomic_gate_ready(&self.inner.rpc),
            relayer: atomic_gate_ready(&self.inner.relayer),
        }
    }

    pub fn can_settle(&self) -> bool {
        let snapshot = self.snapshot();
        snapshot.leadership && snapshot.reconciliation && snapshot.rpc && snapshot.relayer
    }
}

fn atomic_gate_ready(value: &AtomicU8) -> bool {
    ReadinessGateState::from_atomic(value.load(Ordering::Acquire))
        == Some(ReadinessGateState::Ready)
}

impl LeadershipHandle {
    pub fn spawn(
        direct_database_url: Zeroizing<String>,
        network: &str,
        readiness: ReadinessState,
    ) -> Self {
        let (shutdown, receiver) = watch::channel(false);
        let lock_key = advisory_lock_key(network);
        let task = tokio::spawn(run_leadership(
            direct_database_url,
            lock_key,
            readiness,
            receiver,
        ));
        Self { shutdown, task }
    }

    pub async fn shutdown(self) {
        let _send_result = self.shutdown.send(true);
        let _join_result = self.task.await;
    }
}

async fn run_leadership(
    database_url: Zeroizing<String>,
    lock_key: i64,
    readiness: ReadinessState,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut retry = Duration::from_millis(250);
    while !*shutdown.borrow() {
        readiness.set_leadership(false);
        let connection = PgConnection::connect(database_url.as_str()).await;
        let Ok(mut connection) = connection else {
            tracing::warn!(event = "leadership_connection_failed");
            wait_or_shutdown(retry, &mut shutdown).await;
            retry = (retry * 2).min(Duration::from_secs(5));
            continue;
        };
        let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
            .bind(lock_key)
            .fetch_one(&mut connection)
            .await;
        match acquired {
            Ok(true) => {
                retry = Duration::from_millis(250);
                readiness.set_leadership(true);
                tracing::info!(event = "leadership_acquired");
                if hold_leadership(&mut connection, &readiness, &mut shutdown).await {
                    break;
                }
                tracing::warn!(event = "leadership_lost");
            }
            Ok(false) => {
                wait_or_shutdown(Duration::from_secs(2), &mut shutdown).await;
            }
            Err(_) => {
                tracing::warn!(event = "leadership_lock_failed");
                wait_or_shutdown(retry, &mut shutdown).await;
                retry = (retry * 2).min(Duration::from_secs(5));
            }
        }
    }
    readiness.set_leadership(false);
}

async fn hold_leadership(
    connection: &mut PgConnection,
    readiness: &ReadinessState,
    shutdown: &mut watch::Receiver<bool>,
) -> bool {
    let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if sqlx::query("SELECT 1").execute(&mut *connection).await.is_err() {
                    readiness.set_leadership(false);
                    return false;
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    readiness.set_leadership(false);
                    return true;
                }
            }
        }
    }
}

async fn wait_or_shutdown(duration: Duration, shutdown: &mut watch::Receiver<bool>) {
    tokio::select! {
        () = tokio::time::sleep(duration) => {}
        _ = shutdown.changed() => {}
    }
}

fn advisory_lock_key(network: &str) -> i64 {
    let digest: [u8; 32] =
        Sha256::digest(format!("x402-near-facilitator/leader/v1/{network}")).into();
    let mut key = [0_u8; 8];
    key.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(key)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Mutex;

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    #[derive(Clone)]
    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    struct CaptureGuard(Arc<Mutex<Vec<u8>>>);

    impl io::Write for CaptureGuard {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| io::Error::other("capture lock poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for CaptureWriter {
        type Writer = CaptureGuard;

        fn make_writer(&'writer self) -> Self::Writer {
            CaptureGuard(Arc::clone(&self.0))
        }
    }

    #[test]
    fn advisory_keys_are_stable_and_network_specific() {
        assert_eq!(
            advisory_lock_key("near:mainnet"),
            advisory_lock_key("near:mainnet")
        );
        assert_ne!(
            advisory_lock_key("near:mainnet"),
            advisory_lock_key("near:testnet")
        );
    }

    #[test]
    fn losing_leadership_also_invalidates_reconciliation() {
        let readiness = ReadinessState::default();
        readiness.set_leadership(true);
        readiness.set_reconciliation(true);
        readiness.set_rpc(true);
        readiness.set_relayer(true);
        assert!(readiness.can_settle());
        readiness.set_leadership(false);
        assert!(!readiness.snapshot().reconciliation);
        assert!(!readiness.can_settle());
    }

    #[test]
    fn readiness_transition_fields_are_bounded_and_only_emit_on_change() {
        let gate_names = [
            ReadinessGate::Database,
            ReadinessGate::Leadership,
            ReadinessGate::Reconciliation,
            ReadinessGate::Rpc,
            ReadinessGate::Relayer,
        ]
        .map(ReadinessGate::as_str);
        assert_eq!(
            gate_names,
            ["database", "leadership", "reconciliation", "rpc", "relayer"]
        );

        let initial_failure =
            ReadinessTransition::observed(ReadinessGate::Rpc, ReadinessGateState::UNKNOWN, false)
                .unwrap_or_else(|| std::process::abort());
        assert_eq!(initial_failure.gate.as_str(), "rpc");
        assert_eq!(initial_failure.state.as_str(), "not_ready");

        assert_eq!(
            ReadinessTransition::observed(ReadinessGate::Rpc, ReadinessGateState::NOT_READY, false,),
            None
        );

        let degraded =
            ReadinessTransition::observed(ReadinessGate::Rpc, ReadinessGateState::READY, false)
                .unwrap_or_else(|| std::process::abort());
        assert_eq!(degraded.gate.as_str(), "rpc");
        assert_eq!(degraded.state.as_str(), "not_ready");

        let recovered = ReadinessTransition::observed(
            ReadinessGate::Relayer,
            ReadinessGateState::NOT_READY,
            true,
        )
        .unwrap_or_else(|| std::process::abort());
        assert_eq!(recovered.gate.as_str(), "relayer");
        assert_eq!(recovered.state.as_str(), "ready");
    }

    #[test]
    fn all_readyz_gates_log_first_observation_and_only_later_changes() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_target(false)
            .with_writer(CaptureWriter(Arc::clone(&captured)))
            .finish();
        let readiness = ReadinessState::default();

        tracing::subscriber::with_default(subscriber, || {
            readiness.set_database(false);
            readiness.set_database(false);
            readiness.set_database(true);
            // Losing leadership also invalidates reconciliation. That first
            // fail-closed observation must be reported once for each gate.
            readiness.set_leadership(false);
            readiness.set_leadership(false);
            readiness.set_leadership(true);
            readiness.set_reconciliation(false);
            readiness.set_reconciliation(true);
            readiness.set_rpc(false);
            readiness.set_rpc(false);
            readiness.set_rpc(true);
            readiness.set_relayer(false);
            readiness.set_relayer(false);
            readiness.set_relayer(true);
        });

        let bytes = captured
            .lock()
            .map_or_else(|_| Vec::new(), |output| output.clone());
        let output = String::from_utf8(bytes).unwrap_or_default();
        let events = output
            .lines()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .unwrap_or_else(|_| std::process::abort())
            })
            .collect::<Vec<_>>();
        let expected = [
            ("database", "not_ready"),
            ("database", "ready"),
            ("leadership", "not_ready"),
            ("reconciliation", "not_ready"),
            ("leadership", "ready"),
            ("reconciliation", "ready"),
            ("rpc", "not_ready"),
            ("rpc", "ready"),
            ("relayer", "not_ready"),
            ("relayer", "ready"),
        ];
        assert_eq!(events.len(), expected.len());
        for (event, (gate, state)) in events.iter().zip(expected) {
            let fields = event
                .get("fields")
                .and_then(serde_json::Value::as_object)
                .unwrap_or_else(|| std::process::abort());
            assert_eq!(fields.len(), 3);
            assert_eq!(
                fields.get("event").and_then(serde_json::Value::as_str),
                Some("readiness_gate_transition")
            );
            assert_eq!(
                fields.get("gate").and_then(serde_json::Value::as_str),
                Some(gate)
            );
            assert_eq!(
                fields.get("state").and_then(serde_json::Value::as_str),
                Some(state)
            );
        }
    }

    #[test]
    fn unobserved_settlement_gates_remain_fail_closed() {
        let readiness = ReadinessState::default();
        let snapshot = readiness.snapshot();
        assert!(!snapshot.leadership);
        assert!(!snapshot.reconciliation);
        assert!(!snapshot.rpc);
        assert!(!snapshot.relayer);
        assert!(!readiness.can_settle());
    }
}

#[cfg(test)]
#[path = "leadership_postgres_tests.rs"]
mod postgres_tests;
