use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use uuid::Uuid;

use crate::protocol::RelayMessage;

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub port_range: (u16, u16),
    pub allocated_ports: Arc<RwLock<HashSet<u16>>>,
    // session_token -> user_id
    pub user_sessions: Arc<RwLock<HashMap<String, String>>>,
    // active admin session tokens (password: 09090912Qw_)
    pub admin_sessions: Arc<RwLock<HashSet<String>>>,
    // agent_id -> WS sender channel
    pub agent_senders: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<RelayMessage>>>>,
    // claim_code -> WS sender channel
    pub pending_claims: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<RelayMessage>>>>,
    // conn_id -> TCP stream writer channel
    pub tcp_conn_senders: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<Vec<u8>>>>>,
    // tunnel_id -> background task JoinHandle
    pub tunnel_tasks: Arc<RwLock<HashMap<Uuid, tokio::task::JoinHandle<()>>>>,
    // tunnel_id -> (bytes_in, bytes_out) live telemetry
    pub live_traffic: Arc<RwLock<HashMap<Uuid, (u64, u64)>>>,
}

impl AppState {
    pub fn new(db_conn: Connection, port_range: (u16, u16)) -> Arc<Self> {
        Arc::new(Self {
            db: Arc::new(Mutex::new(db_conn)),
            port_range,
            allocated_ports: Arc::new(RwLock::new(HashSet::new())),
            user_sessions: Arc::new(RwLock::new(HashMap::new())),
            admin_sessions: Arc::new(RwLock::new(HashSet::new())),
            agent_senders: Arc::new(RwLock::new(HashMap::new())),
            pending_claims: Arc::new(RwLock::new(HashMap::new())),
            tcp_conn_senders: Arc::new(RwLock::new(HashMap::new())),
            tunnel_tasks: Arc::new(RwLock::new(HashMap::new())),
            live_traffic: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn allocate_port(&self, preferred: Option<u16>) -> Option<u16> {
        let mut allocated = self.allocated_ports.write().await;
        if let Some(p) = preferred {
            if p >= 1024 && !allocated.contains(&p) {
                allocated.insert(p);
                return Some(p);
            } else {
                return None;
            }
        }
        for p in self.port_range.0..=self.port_range.1 {
            if !allocated.contains(&p) {
                allocated.insert(p);
                return Some(p);
            }
        }
        None
    }

    pub async fn free_port(&self, port: u16) {
        let mut allocated = self.allocated_ports.write().await;
        allocated.remove(&port);
    }

    pub async fn record_traffic(&self, tunnel_id: Uuid, bytes_in: u64, bytes_out: u64) {
        let mut traffic = self.live_traffic.write().await;
        let entry = traffic.entry(tunnel_id).or_insert((0, 0));
        entry.0 += bytes_in;
        entry.1 += bytes_out;
    }
}
