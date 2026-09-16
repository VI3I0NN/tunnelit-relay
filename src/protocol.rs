use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TunnelConfig {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub name: String,
    pub local_port: u16,
    pub protocol: String, // "tcp" or "udp"
    pub public_port: u16,
    pub subdomain: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum AgentMessage {
    /// Agent sends credentials to authenticate
    Auth {
        token: String,
        hostname: String,
    },
    /// First-time setup: request a web claim URL
    RequestClaim {
        hostname: String,
    },
    /// Data transfer
    Data {
        conn_id: Uuid,
        payload: String, // base64
    },
    /// Close an active proxy connection
    CloseConnection {
        conn_id: Uuid,
    },
    /// Ping keepalive
    Ping,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum RelayMessage {
    /// Authentication successful
    AuthSuccess {
        agent_id: Uuid,
        name: String,
        token: String,
    },
    /// Web claim code generated
    ClaimReady {
        code: String,
        claim_url: String,
    },
    /// Full sync of all configured tunnels for this agent
    SyncTunnels {
        tunnels: Vec<TunnelConfig>,
    },
    /// Command from web to start/enable a tunnel
    StartTunnel {
        tunnel: TunnelConfig,
    },
    /// Command from web to stop/disable a tunnel
    StopTunnel {
        tunnel_id: Uuid,
    },
    /// New incoming external client connection
    NewConnection {
        tunnel_id: Uuid,
        conn_id: Uuid,
    },
    /// Data transfer
    Data {
        conn_id: Uuid,
        payload: String, // base64
    },
    /// Close connection
    CloseConnection {
        conn_id: Uuid,
    },
    /// Pong keepalive
    Pong,
    /// Error message
    Error {
        message: String,
    },
}
