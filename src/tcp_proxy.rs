use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use uuid::Uuid;
use base64::{engine::general_purpose, Engine as _};
use tracing::{info, error};

use crate::protocol::RelayMessage;
use crate::state::AppState;

pub async fn run_tcp_proxy(
    listener: TcpListener,
    tunnel_id: Uuid,
    _agent_id: Uuid,
    state: Arc<AppState>,
    agent_sender: mpsc::UnboundedSender<RelayMessage>,
) {
    info!("Starting TCP proxy for tunnel {} on {:?}", tunnel_id, listener.local_addr());
    loop {
        if agent_sender.is_closed() {
            info!("Agent disconnected, closing TCP listener for tunnel {}", tunnel_id);
            break;
        }
        match listener.accept().await {
            Ok((mut stream, addr)) => {
                info!("New TCP connection from {} for tunnel {}", addr, tunnel_id);
                let conn_id = Uuid::new_v4();
                
                // Notify agent
                if let Err(e) = agent_sender.send(RelayMessage::NewConnection {
                    tunnel_id,
                    conn_id,
                }) {
                    error!("Failed to notify agent of new connection: {}", e);
                    break;
                }

                let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
                {
                    let mut senders = state.tcp_conn_senders.write().await;
                    senders.insert(conn_id, tx);
                }

                let agent_sender_clone = agent_sender.clone();
                let state_clone = state.clone();

                tokio::spawn(async move {
                    let (mut read_half, mut write_half) = stream.split();

                    let mut buf = vec![0u8; 8192];
                    
                    loop {
                        tokio::select! {
                            result = read_half.read(&mut buf) => {
                                match result {
                                    Ok(0) => {
                                        info!("TCP connection closed by client: {}", conn_id);
                                        break;
                                    }
                                    Ok(n) => {
                                        let payload = general_purpose::STANDARD.encode(&buf[..n]);
                                        if let Err(e) = agent_sender_clone.send(RelayMessage::Data {
                                            conn_id,
                                            payload,
                                        }) {
                                            error!("Failed to send data to agent: {}", e);
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        error!("Error reading from TCP stream: {}", e);
                                        break;
                                    }
                                }
                            }
                            Some(data) = rx.recv() => {
                                if let Err(e) = write_half.write_all(&data).await {
                                    error!("Error writing to TCP stream: {}", e);
                                    break;
                                }
                            }
                        }
                    }

                    // Clean up
                    let _ = agent_sender_clone.send(RelayMessage::CloseConnection { conn_id });
                    {
                        let mut senders = state_clone.tcp_conn_senders.write().await;
                        senders.remove(&conn_id);
                    }
                });
            }
            Err(e) => {
                error!("Error accepting TCP connection: {}", e);
            }
        }
    }
}
