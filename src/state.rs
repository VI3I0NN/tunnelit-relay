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
    // agent_id -> WS sender channel
    pub agent_senders: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<RelayMessage>>>>,
    // claim_code -> WS sender channel (for agents waiting for web claim confirmation)
    pub pending_claims: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<RelayMessage>>>>,
    // conn_id -> TCP stream writer channel
    pub tcp_conn_senders: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<Vec<u8>>>>>,
    // tunnel_id -> background task JoinHandle (to close socket on disable/disconnect)
    pub tunnel_tasks: Arc<RwLock<HashMap<Uuid, tokio::task::JoinHandle<()>>>>,
}

impl AppState {
    pub fn new(db_conn: Connection, port_range: (u16, u16)) -> Arc<Self> {
        Arc::new(Self {
            db: Arc::new(Mutex::new(db_conn)),
            port_range,
            allocated_ports: Arc::new(RwLock::new(HashSet::new())),
            agent_senders: Arc::new(RwLock::new(HashMap::new())),
            pending_claims: Arc::new(RwLock::new(HashMap::new())),
            tcp_conn_senders: Arc::new(RwLock::new(HashMap::new())),
            tunnel_tasks: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn allocate_port(&self, preferred: Option<u16>) -> Option<u16> {
        let mut allocated = self.allocated_ports.write().await;
        if let Some(p) = preferred {
            if p >= self.port_range.0 && p <= self.port_range.1 && !allocated.contains(&p) {
                allocated.insert(p);
                return Some(p);
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
}
