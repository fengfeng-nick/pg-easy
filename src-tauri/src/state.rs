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

/// 一条数据库会话。每个工作区拥有自己的会话，一个标签页里打开的事务永远
/// 不会渗到另一个。
pub struct Session {
    pub id: String,
    #[allow(dead_code)]
    pub params: ConnectParams,
    pub database: String,
    pub user: String,
    pub server_version: String,
    pub conn: Connection,
    /// 已有一条语句交给这个会话且尚未返回。会话自身在整条语句期间都不会被
    /// 锁住，所以正是它告诉第二条命令该等待，而不是在协议上交错发送。
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

/// 与一条会话通信（或停止它）所需的全部内容。
pub struct SessionHandle {
    pub slot: Arc<Mutex<SessionSlot>>,
    /// 每条运行中的语句一个 token，语句开始时被替换。
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

    /// 把会话从表中取出并关闭。返回它之前是否是打开的。
    pub async fn remove(&self, id: &str) -> bool {
        let handle = self.sessions.lock().await.remove(id);
        match handle {
            Some(handle) => {
                // 先停掉正在跑的东西，再丢弃流，让服务器立刻看到断开。
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
