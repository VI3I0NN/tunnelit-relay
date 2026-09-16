use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use uuid::Uuid;

use crate::protocol::RelayMessage;

#[derive(Debug, Clone)]
pub struct TunnelInfo {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub local_port: u16,
    pub protocol: String,
    pub public_port: u16,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub id: Uuid,
    pub tunnel_id: Uuid,
    pub agent_id: Uuid,
}

pub struct AppState {
    pub tunnels: Arc<RwLock<HashMap<Uuid, TunnelInfo>>>,
    pub connections: Arc<RwLock<HashMap<Uuid, ConnectionInfo>>>,
    pub agent_senders: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<RelayMessage>>>>,
    pub tcp_conn_senders: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<Vec<u8>>>>>,
    pub port_range: (u16, u16),
    pub allocated_ports: Arc<RwLock<HashSet<u16>>>,
    pub db: Arc<Mutex<Connection>>,
}

impl AppState {
    pub fn new(db_conn: Connection, port_range: (u16, u16)) -> Arc<Self> {
        Arc::new(Self {
            tunnels: Arc::new(RwLock::new(HashMap::new())),
            connections: Arc::new(RwLock::new(HashMap::new())),
            agent_senders: Arc::new(RwLock::new(HashMap::new())),
            tcp_conn_senders: Arc::new(RwLock::new(HashMap::new())),
            port_range,
            allocated_ports: Arc::new(RwLock::new(HashSet::new())),
            db: Arc::new(Mutex::new(db_conn)),
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
