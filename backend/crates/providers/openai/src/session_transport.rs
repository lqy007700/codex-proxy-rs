//! 根会话的连续 WS 失败预算；耗尽后保持 HTTP，空闲过期后释放状态

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use gateway_core::account::ProviderAccountId;
use gateway_core::provider_ports::ProviderSessionAffinityKey;
use tokio::time::Instant;

const SESSION_IDLE_RETENTION: Duration = Duration::from_secs(8 * 60 * 60);
const MAX_SESSION_TRANSPORTS: usize = 16_384;

#[derive(Clone, Default)]
pub(crate) struct CodexSessionTransportRecovery {
    state: Arc<Mutex<TransportRecoveryState>>,
}

#[derive(Default)]
struct TransportRecoveryState {
    sessions: HashMap<AccountSessionTransportKey, SessionTransport>,
    account_generations: HashMap<ProviderAccountId, u64>,
}

struct SessionTransport {
    state: TransportState,
    last_used: Instant,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct AccountSessionTransportKey {
    account_id: ProviderAccountId,
    session: ProviderSessionAffinityKey,
}

#[derive(Clone)]
pub(crate) struct SessionTransportScope {
    key: AccountSessionTransportKey,
    account_generation: u64,
}

enum TransportState {
    WebSocket { failures: u32 },
    Http,
}

impl CodexSessionTransportRecovery {
    pub(crate) fn scope(
        &self,
        account_id: &ProviderAccountId,
        key: &ProviderSessionAffinityKey,
    ) -> SessionTransportScope {
        let state = self.lock();
        SessionTransportScope {
            key: AccountSessionTransportKey {
                account_id: account_id.clone(),
                session: key.clone(),
            },
            account_generation: state
                .account_generations
                .get(account_id)
                .copied()
                .unwrap_or_default(),
        }
    }

    pub(crate) fn uses_http(&self, scope: &SessionTransportScope) -> bool {
        let now = Instant::now();
        let mut state = self.lock();
        if !scope.is_current(&state) {
            return false;
        }
        let Some(session) = state.sessions.get_mut(&scope.key) else {
            return false;
        };
        if now.saturating_duration_since(session.last_used) >= SESSION_IDLE_RETENTION {
            state.sessions.remove(&scope.key);
            return false;
        }
        session.last_used = now;
        matches!(session.state, TransportState::Http)
    }

    /// 返回连续失败是否已耗尽首发加重试预算
    pub(crate) fn record_websocket_failure(
        &self,
        scope: &SessionTransportScope,
        max_retries: u32,
    ) -> bool {
        self.update(scope, |state| match state {
            TransportState::WebSocket { failures } if failures < max_retries => {
                TransportState::WebSocket {
                    failures: failures + 1,
                }
            }
            TransportState::WebSocket { .. } | TransportState::Http => TransportState::Http,
        })
    }

    pub(crate) fn disable_websocket(&self, scope: &SessionTransportScope) {
        self.update(scope, |_| TransportState::Http);
    }

    pub(crate) fn websocket_succeeded(&self, scope: &SessionTransportScope) {
        let mut state = self.lock();
        if scope.is_current(&state) {
            state.sessions.remove(&scope.key);
        }
    }

    pub(crate) fn account_is_degraded(&self, account_id: &ProviderAccountId) -> bool {
        let now = Instant::now();
        let mut state = self.lock();
        retain_active_sessions(&mut state.sessions, now);
        state.sessions.iter().any(|(key, session)| {
            key.account_id == *account_id && matches!(session.state, TransportState::Http)
        })
    }

    pub(crate) fn recover_account(&self, account_id: &ProviderAccountId) {
        let mut state = self.lock();
        let generation = state
            .account_generations
            .entry(account_id.clone())
            .or_default();
        *generation = generation.wrapping_add(1);
        state
            .sessions
            .retain(|key, _| key.account_id != *account_id);
    }

    fn update(
        &self,
        scope: &SessionTransportScope,
        update: impl FnOnce(TransportState) -> TransportState,
    ) -> bool {
        let now = Instant::now();
        let mut state = self.lock();
        if !scope.is_current(&state) {
            return false;
        }
        let previous = state
            .sessions
            .remove(&scope.key)
            .filter(|session| {
                now.saturating_duration_since(session.last_used) < SESSION_IDLE_RETENTION
            })
            .map_or(TransportState::WebSocket { failures: 0 }, |session| {
                session.state
            });
        if state.sessions.len() >= MAX_SESSION_TRANSPORTS {
            retain_active_sessions(&mut state.sessions, now);
            if state.sessions.len() >= MAX_SESSION_TRANSPORTS
                && let Some(oldest) = state
                    .sessions
                    .iter()
                    .min_by_key(|(_, session)| session.last_used)
                    .map(|(key, _)| key.clone())
            {
                state.sessions.remove(&oldest);
            }
        }
        let next_state = update(previous);
        let uses_http = matches!(next_state, TransportState::Http);
        state.sessions.insert(
            scope.key.clone(),
            SessionTransport {
                state: next_state,
                last_used: now,
            },
        );
        uses_http
    }

    fn lock(&self) -> MutexGuard<'_, TransportRecoveryState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SessionTransportScope {
    fn is_current(&self, state: &TransportRecoveryState) -> bool {
        state
            .account_generations
            .get(&self.key.account_id)
            .copied()
            .unwrap_or_default()
            == self.account_generation
    }
}

fn retain_active_sessions(
    sessions: &mut HashMap<AccountSessionTransportKey, SessionTransport>,
    now: Instant,
) {
    sessions.retain(|_, session| {
        now.saturating_duration_since(session.last_used) < SESSION_IDLE_RETENTION
    });
}
