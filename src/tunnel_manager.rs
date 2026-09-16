use std::sync::Arc;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tracing::{error, info};
use uuid::Uuid;

use crate::protocol::{RelayMessage, TunnelConfig};
use crate::state::AppState;
use crate::tcp_proxy::run_tcp_proxy;
use crate::udp_proxy::run_udp_proxy;

pub async fn start_tunnel_listener(
    tunnel: &TunnelConfig,
    agent_sender: mpsc::UnboundedSender<RelayMessage>,
    state: Arc<AppState>,
) -> Result<(), String> {
    // If a listener task already exists for this tunnel, abort it first
    stop_tunnel_listener(tunnel.id, state.clone()).await;

    let public_port = tunnel.public_port;

    if tunnel.protocol.to_lowercase() == "tcp" {
        match TcpListener::bind(format!("0.0.0.0:{}", public_port)).await {
            Ok(listener) => {
                info!("Binding TCP listener on port {} for tunnel {}", public_port, tunnel.id);
                let handle = tokio::spawn(run_tcp_proxy(
                    listener,
                    tunnel.id,
                    tunnel.agent_id,
                    state.clone(),
                    agent_sender,
                ));
                let mut tasks = state.tunnel_tasks.write().await;
                tasks.insert(tunnel.id, handle);
                Ok(())
            }
            Err(e) => {
                let err = format!("Failed to bind TCP port {}: {}", public_port, e);
                error!("{}", err);
                Err(err)
            }
        }
    } else {
        match UdpSocket::bind(format!("0.0.0.0:{}", public_port)).await {
            Ok(socket) => {
                info!("Binding UDP socket on port {} for tunnel {}", public_port, tunnel.id);
                let socket = Arc::new(socket);
                let handle = tokio::spawn(run_udp_proxy(
                    socket,
                    tunnel.id,
                    tunnel.agent_id,
                    state.clone(),
                    agent_sender,
                ));
                let mut tasks = state.tunnel_tasks.write().await;
                tasks.insert(tunnel.id, handle);
                Ok(())
            }
            Err(e) => {
                let err = format!("Failed to bind UDP port {}: {}", public_port, e);
                error!("{}", err);
                Err(err)
            }
        }
    }
}

pub async fn stop_tunnel_listener(tunnel_id: Uuid, state: Arc<AppState>) {
    let mut tasks = state.tunnel_tasks.write().await;
    if let Some(handle) = tasks.remove(&tunnel_id) {
        info!("Stopping listener for tunnel {}", tunnel_id);
        handle.abort();
    }
}
