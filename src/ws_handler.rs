use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;
use base64::{engine::general_purpose, Engine as _};
use tracing::{info, error, warn};

use crate::db;
use crate::protocol::{AgentMessage, RelayMessage, TunnelConfig};
use crate::state::AppState;
use crate::tunnel_manager::{start_tunnel_listener, stop_tunnel_listener};

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let session_id = Uuid::new_v4();
    info!("New agent connection initiated: {}", session_id);

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<RelayMessage>();

    // Forward outgoing messages from internal channel to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });

    let mut authenticated_agent_id: Option<Uuid> = None;
    let mut current_claim_code: Option<String> = None;
    let state_clone = state.clone();
    let tx_clone = tx.clone();

    // Process incoming messages from agent
    while let Some(msg_result) = ws_receiver.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<AgentMessage>(&text) {
                    Ok(AgentMessage::RequestClaim { hostname }) => {
                        let db = state.db.lock().await;
                        match db::create_agent_claim(&db, &hostname) {
                            Ok((record, code)) => {
                                info!("Claim code {} generated for agent '{}'", code, hostname);
                                current_claim_code = Some(code.clone());
                                {
                                    let mut pending = state.pending_claims.write().await;
                                    pending.insert(code.clone(), tx_clone.clone());
                                }
                                let _ = tx_clone.send(RelayMessage::ClaimReady {
                                    code: code.clone(),
                                    claim_url: format!("/claim/{}", code),
                                    token: Some(record.token),
                                });
                            }
                            Err(e) => {
                                error!("Failed to create agent claim: {}", e);
                                let _ = tx_clone.send(RelayMessage::Error {
                                    message: "Failed to generate claim code".into(),
                                });
                            }
                        }
                    }
                    Ok(AgentMessage::Auth { token, hostname }) => {
                        let agent_opt = {
                            let db = state.db.lock().await;
                            db::get_agent_by_token(&db, &token)
                        };

                        match agent_opt {
                            Ok(Some(agent)) => {
                                // If agent still has an active claim code, keep waiting for web confirmation
                                if let Some(code) = agent.claim_code {
                                    info!("Agent '{}' token is pending web claim with code {}", agent.name, code);
                                    current_claim_code = Some(code.clone());
                                    {
                                        let mut pending = state.pending_claims.write().await;
                                        pending.insert(code.clone(), tx_clone.clone());
                                    }
                                    let _ = tx_clone.send(RelayMessage::ClaimReady {
                                        code: code.clone(),
                                        claim_url: format!("/claim/{}", code),
                                        token: Some(token),
                                    });
                                    continue;
                                }

                                let agent_id = match Uuid::parse_str(&agent.id) {
                                    Ok(id) => id,
                                    Err(_) => {
                                        let _ = tx_clone.send(RelayMessage::Error {
                                            message: "Invalid agent ID format in database".into(),
                                        });
                                        continue;
                                    }
                                };

                                authenticated_agent_id = Some(agent_id);

                                // Mark agent as online
                                {
                                    let db = state.db.lock().await;
                                    let _ = db::set_agent_online(&db, &agent.id, true, Some(&hostname));
                                }

                                // Register active sender
                                {
                                    let mut senders = state.agent_senders.write().await;
                                    senders.insert(agent_id, tx_clone.clone());
                                }

                                info!("Agent '{}' ({}) successfully authenticated!", agent.name, agent_id);

                                let _ = tx_clone.send(RelayMessage::AuthSuccess {
                                    agent_id,
                                    name: agent.name.clone(),
                                    token: agent.token.clone(),
                                });

                                // Load and start all configured tunnels for this agent
                                sync_agent_tunnels(agent_id, state.clone(), tx_clone.clone()).await;
                            }
                            Ok(None) => {
                                warn!("Authentication failed: invalid token");
                                let _ = tx_clone.send(RelayMessage::Error {
                                    message: "Invalid or expired token".into(),
                                });
                            }
                            Err(e) => {
                                error!("Database error during authentication: {}", e);
                                let _ = tx_clone.send(RelayMessage::Error {
                                    message: "Database authentication error".into(),
                                });
                            }
                        }
                    }
                    Ok(AgentMessage::Data { conn_id, payload }) => {
                        if let Ok(data) = general_purpose::STANDARD.decode(&payload) {
                            let senders = state.tcp_conn_senders.read().await;
                            if let Some(sender) = senders.get(&conn_id) {
                                let _ = sender.send(data);
                            }
                        }
                    }
                    Ok(AgentMessage::CloseConnection { conn_id }) => {
                        let mut senders = state.tcp_conn_senders.write().await;
                        senders.remove(&conn_id);
                    }
                    Ok(AgentMessage::Ping) => {
                        let _ = tx_clone.send(RelayMessage::Pong);
                    }
                    Err(e) => {
                        warn!("Failed to parse AgentMessage: {} (payload: {})", e, text);
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("Agent requested WebSocket close");
                break;
            }
            Err(e) => {
                warn!("WebSocket read error: {}", e);
                break;
            }
            _ => {}
        }
    }

    send_task.abort();

    // Clean up on disconnect
    if let Some(code) = current_claim_code {
        let mut pending = state_clone.pending_claims.write().await;
        pending.remove(&code);
    }

    if let Some(agent_id) = authenticated_agent_id {
        info!("Agent {} disconnected. Cleaning up tunnels...", agent_id);
        cleanup_agent(agent_id, state_clone).await;
    }
}

pub async fn sync_agent_tunnels(
    agent_id: Uuid,
    state: Arc<AppState>,
    agent_sender: mpsc::UnboundedSender<RelayMessage>,
) {
    let db_tunnels = {
        let db = state.db.lock().await;
        db::list_tunnels_for_agent(&db, &agent_id.to_string()).unwrap_or_default()
    };

    let mut tunnel_configs = Vec::new();

    for t in db_tunnels {
        if let Ok(tid) = Uuid::parse_str(&t.id) {
            let config = TunnelConfig {
                id: tid,
                agent_id,
                name: t.name,
                local_port: t.local_port,
                protocol: t.protocol,
                public_port: t.public_port,
                subdomain: t.subdomain,
                enabled: t.enabled,
            };

            if config.enabled {
                // Reserve port in memory
                {
                    let mut allocated = state.allocated_ports.write().await;
                    allocated.insert(config.public_port);
                }
                // Start listener
                let _ = start_tunnel_listener(&config, agent_sender.clone(), state.clone()).await;
            }

            tunnel_configs.push(config);
        }
    }

    let _ = agent_sender.send(RelayMessage::SyncTunnels {
        tunnels: tunnel_configs,
    });
}

async fn cleanup_agent(agent_id: Uuid, state: Arc<AppState>) {
    // 1. Remove agent_sender
    {
        let mut senders = state.agent_senders.write().await;
        senders.remove(&agent_id);
    }

    // 2. Mark offline in DB
    {
        let db = state.db.lock().await;
        let _ = db::set_agent_online(&db, &agent_id.to_string(), false, None);
    }

    // 3. Stop all tunnel listeners for this agent
    let db_tunnels = {
        let db = state.db.lock().await;
        db::list_tunnels_for_agent(&db, &agent_id.to_string()).unwrap_or_default()
    };

    for t in db_tunnels {
        if let Ok(tid) = Uuid::parse_str(&t.id) {
            stop_tunnel_listener(tid, state.clone()).await;
            state.free_port(t.public_port).await;
        }
    }
}
