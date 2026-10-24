use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::connection::ConnectParams;
use crate::error::AppError;
use crate::pg::Connection;
use crate::protocol::ConnectionStatus;
use crate::settings::Settings;

/// One database session. Each workspace owns its own session, so an open
/// transaction in one tab never leaks into another.
pub struct Session {
    pub id: String,
    #[allow(dead_code)]
    pub params: ConnectParams,
    pub database: String,
    pub user: String,
    pub server_version: String,
    pub conn: Connection,
    /// A statement has been handed to this session and has not returned yet.
    /// The session itself is never locked for the whole statement, so this is
    /// what tells a second command to wait instead of interleaving on the wire.
    pub busy: bool,
    pub backend_pid: i32,
    pub idle_since: Instant,
}

impl Session {
    pub fn status(&self) -> ConnectionStatus {
        let tx_status = match self.conn.status() {
            b'T' => "intransaction",
            b'E' => "inerror",
            _ => "idle",
        };
        ConnectionStatus {
            id: self.id.clone(),
            tx_status: tx_status.to_string(),
            backend_pid: self.backend_pid,
            server_version: self.server_version.clone(),
        }
    }
}

/// Everything needed to talk to (or stop) one session.
pub struct SessionHandle {
    pub slot: Arc<Mutex<SessionSlot>>,
    /// One token per running statement, replaced when a statement starts.
    pub cancel: Arc<Mutex<CancellationToken>>,
    pub target: (String, u16),
}

pub struct SessionSlot {
    pub session: Session,
}

pub struct AppState {
    sessions: Mutex<HashMap<String, SessionHandle>>,
    settings: RwLock<Settings>,
}

impl AppState {
    pub fn new(settings: Settings) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            settings: RwLock::new(settings),
        }
    }

    pub async fn settings(&self) -> Settings {
        self.settings.read().await.clone()
    }

    pub async fn set_settings(&self, settings: Settings) {
        *self.settings.write().await = settings;
    }

    pub async fn insert(&self, session: Session, target: (String, u16)) -> SessionHandle {
        let handle = SessionHandle {
            slot: Arc::new(Mutex::new(SessionSlot { session })),
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
            target,
        };
        self.sessions
            .lock()
            .await
            .insert(handle.session_id(), handle.clone());
        handle
    }

    /// Takes the session out of the table and closes it. Returns whether it
    /// was open.
    pub async fn remove(&self, id: &str) -> bool {
        let handle = self.sessions.lock().await.remove(id);
        match handle {
            Some(handle) => {
                // Stops whatever is running, then drops the stream so the
                // server sees the disconnect right away.
                handle.cancel.lock().await.cancel();
                drop(handle.slot.lock().await);
                true
            }
            None => false,
        }
    }

    pub async fn handle(&self, id: &str) -> Result<SessionHandle, AppError> {
        self.sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::new("session", "session_closed"))
    }

    pub async fn peek_status(&self, id: &str) -> Option<ConnectionStatus> {
        let handle = self.sessions.lock().await.get(id).cloned()?;
        let slot = handle.slot.try_lock().ok()?;
        Some(slot.session.status())
    }
}

impl SessionHandle {
    pub fn try_cancel(&self) {
        if let Ok(token) = self.cancel.try_lock() {
            token.cancel();
        }
    }

    fn session_id(&self) -> String {
        self.slot
            .try_lock()
            .expect("a session is not locked while it is being registered")
            .session
            .id
            .clone()
    }
}

impl Clone for SessionHandle {
    fn clone(&self) -> Self {
        Self {
            slot: self.slot.clone(),
            cancel: self.cancel.clone(),
            target: self.target.clone(),
        }
    }
}
