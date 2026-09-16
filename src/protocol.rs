use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum AgentMessage {
    CreateTunnel {
        local_port: u16,
        protocol: String,
        preferred_port: Option<u16>,
    },
    Data {
        conn_id: Uuid,
        payload: String, // base64
    },
    CloseConnection {
        conn_id: Uuid,
    },
    RequestSubdomain {
        tunnel_id: Uuid,
        desired_name: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum RelayMessage {
    TunnelCreated {
        tunnel_id: Uuid,
        public_port: u16,
    },
    NewConnection {
        tunnel_id: Uuid,
        conn_id: Uuid,
    },
    Data {
        conn_id: Uuid,
        payload: String, // base64
    },
    CloseConnection {
        conn_id: Uuid,
    },
    SubdomainAssigned {
        tunnel_id: Uuid,
        subdomain: String,
    },
    Error {
        message: String,
    },
}
