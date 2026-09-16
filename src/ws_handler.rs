use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;
use base64::{engine::general_purpose, Engine as _};
use tracing::{info, error, warn};
use tokio::net::{TcpListener, UdpSocket};

use crate::protocol::{AgentMessage, RelayMessage};
use crate::state::{AppState, TunnelInfo};
use crate::tcp_proxy::run_tcp_proxy;
use crate::udp_proxy::run_udp_proxy;
use crate::db;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let agent_id = Uuid::new_v4();
    info!("New agent connected: {}", agent_id);

    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<RelayMessage>();

    // Store sender in state
    {
        let mut senders = state.agent_senders.write().await;
        senders.insert(agent_id, tx.clone());
    }

    // Task to forward messages from channel to WebSocket
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });

    let state_clone = state.clone();
    
    // Task to handle incoming messages from WebSocket
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = receiver.next().await {
            match serde_json::from_str::<AgentMessage>(&text) {
                Ok(msg) => handle_agent_message(agent_id, msg, state_clone.clone(), tx.clone()).await,
                Err(e) => warn!("Failed to parse message from agent {}: {}", agent_id, e),
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    }

    // Cleanup on disconnect
    info!("Agent disconnected: {}", agent_id);
    cleanup_agent(agent_id, state).await;
}

async fn handle_agent_message(
    agent_id: Uuid,
    msg: AgentMessage,
    state: Arc<AppState>,
    tx: mpsc::UnboundedSender<RelayMessage>,
) {
    match msg {
        AgentMessage::CreateTunnel { local_port, protocol, preferred_port } => {
            let public_port = match state.allocate_port(preferred_port).await {
                Some(p) => p,
                None => {
                    let _ = tx.send(RelayMessage::Error { message: "No available ports".into() });
                    return;
                }
            };

            let tunnel_id = Uuid::new_v4();
            
            if protocol == "tcp" {
                match TcpListener::bind(format!("0.0.0.0:{}", public_port)).await {
                    Ok(listener) => {
                        let tunnel = TunnelInfo {
                            id: tunnel_id,
                            agent_id,
                            local_port,
                            protocol: protocol.clone(),
                            public_port,
                            created_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                        };
                        
                        {
                            let mut tunnels = state.tunnels.write().await;
                            tunnels.insert(tunnel_id, tunnel.clone());
                        }
                        
                        {
                            let db = state.db.lock().await;
                            let _ = db::save_tunnel(&db, tunnel.id, tunnel.agent_id, tunnel.local_port, &tunnel.protocol, tunnel.public_port, tunnel.created_at);
                        }

                        let _ = tx.send(RelayMessage::TunnelCreated { tunnel_id, public_port });
                        
                        let handle = tokio::spawn(run_tcp_proxy(listener, tunnel_id, agent_id, state.clone(), tx.clone()));
                        {
                            let mut tasks = state.tunnel_tasks.write().await;
                            tasks.insert(tunnel_id, handle);
                        }
                    }
                    Err(e) => {
                        error!("Failed to bind TCP port {}: {}", public_port, e);
                        state.free_port(public_port).await;
                        let _ = tx.send(RelayMessage::Error { message: "Failed to bind port".into() });
                    }
                }
            } else if protocol == "udp" {
                match UdpSocket::bind(format!("0.0.0.0:{}", public_port)).await {
                    Ok(socket) => {
                        let socket = Arc::new(socket);
                        let tunnel = TunnelInfo {
                            id: tunnel_id,
                            agent_id,
                            local_port,
                            protocol: protocol.clone(),
                            public_port,
                            created_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                        };
                        
                        {
                            let mut tunnels = state.tunnels.write().await;
                            tunnels.insert(tunnel_id, tunnel.clone());
                        }
                        
                        {
                            let db = state.db.lock().await;
                            let _ = db::save_tunnel(&db, tunnel.id, tunnel.agent_id, tunnel.local_port, &tunnel.protocol, tunnel.public_port, tunnel.created_at);
                        }

                        let _ = tx.send(RelayMessage::TunnelCreated { tunnel_id, public_port });
                        
                        let handle = tokio::spawn(run_udp_proxy(socket, tunnel_id, agent_id, state.clone(), tx.clone()));
                        {
                            let mut tasks = state.tunnel_tasks.write().await;
                            tasks.insert(tunnel_id, handle);
                        }
                    }
                    Err(e) => {
                        error!("Failed to bind UDP port {}: {}", public_port, e);
                        state.free_port(public_port).await;
                        let _ = tx.send(RelayMessage::Error { message: "Failed to bind port".into() });
                    }
                }
            }
        }
        AgentMessage::Data { conn_id, payload } => {
            if let Ok(data) = general_purpose::STANDARD.decode(&payload) {
                let senders = state.tcp_conn_senders.read().await;
                if let Some(sender) = senders.get(&conn_id) {
                    let _ = sender.send(data);
                }
            }
        }
        AgentMessage::CloseConnection { conn_id } => {
            let mut senders = state.tcp_conn_senders.write().await;
            senders.remove(&conn_id);
        }
        AgentMessage::RequestSubdomain { tunnel_id, desired_name } => {
            let db = state.db.lock().await;
            if let Err(e) = db::save_subdomain_request(&db, tunnel_id, &desired_name) {
                error!("Failed to save subdomain request: {}", e);
            }
        }
    }
}

async fn cleanup_agent(agent_id: Uuid, state: Arc<AppState>) {
    {
        let mut senders = state.agent_senders.write().await;
        senders.remove(&agent_id);
    }
    
    let mut ports_to_free = Vec::new();
    let mut tunnels_to_remove = Vec::new();
    
    {
        let mut tunnels = state.tunnels.write().await;
        tunnels.retain(|id, t| {
            if t.agent_id == agent_id {
                ports_to_free.push(t.public_port);
                tunnels_to_remove.push(*id);
                false
            } else {
                true
            }
        });
    }

    {
        let mut tasks = state.tunnel_tasks.write().await;
        for t_id in &tunnels_to_remove {
            if let Some(handle) = tasks.remove(t_id) {
                info!("Closing proxy listener for tunnel {}", t_id);
                handle.abort();
            }
        }
    }
    
    for port in ports_to_free {
        state.free_port(port).await;
    }
    
    let db = state.db.lock().await;
    for t_id in tunnels_to_remove {
        let _ = db::remove_tunnel(&db, t_id);
    }
}
