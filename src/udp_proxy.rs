use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use std::collections::HashMap;
use std::net::SocketAddr;
use uuid::Uuid;
use base64::{engine::general_purpose, Engine as _};
use tracing::{info, error};

use crate::protocol::RelayMessage;
use crate::state::AppState;

pub async fn run_udp_proxy(
    socket: Arc<UdpSocket>,
    tunnel_id: Uuid,
    _agent_id: Uuid,
    state: Arc<AppState>,
    agent_sender: mpsc::UnboundedSender<RelayMessage>,
) {
    info!("Starting UDP proxy for tunnel {} on {:?}", tunnel_id, socket.local_addr());
    
    let mut addr_to_conn: HashMap<SocketAddr, Uuid> = HashMap::new();
    let mut conn_to_addr: HashMap<Uuid, SocketAddr> = HashMap::new();
    let mut buf = vec![0u8; 65536];

    // Channel for incoming data from agent intended for this UDP tunnel
    let (tx, mut rx) = mpsc::unbounded_channel::<(Uuid, Vec<u8>)>();
    
    // We register a separate unbounded sender per connection, or we could have one for the whole tunnel.
    // To fit into tcp_conn_senders model, we'll spawn a receiver for each UDP "connection" that forwards to the main loop,
    // or just register individual senders in tcp_conn_senders that write to the same UDP socket.
    
    loop {
        tokio::select! {
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, addr)) => {
                        let conn_id = *addr_to_conn.entry(addr).or_insert_with(|| {
                            let id = Uuid::new_v4();
                            conn_to_addr.insert(id, addr);
                            
                            let _ = agent_sender.send(RelayMessage::NewConnection {
                                tunnel_id,
                                conn_id: id,
                            });
                            
                            // Register in state so ws_handler can route to us
                            let tx_clone = tx.clone();
                            let (conn_tx, mut conn_rx) = mpsc::unbounded_channel::<Vec<u8>>();
                            let state_clone = state.clone();
                            
                            tokio::spawn(async move {
                                {
                                    let mut senders = state_clone.tcp_conn_senders.write().await;
                                    senders.insert(id, conn_tx);
                                }
                                while let Some(data) = conn_rx.recv().await {
                                    let _ = tx_clone.send((id, data));
                                }
                            });
                            
                            id
                        });
                        
                        let payload = general_purpose::STANDARD.encode(&buf[..len]);
                        let _ = agent_sender.send(RelayMessage::Data {
                            conn_id,
                            payload,
                        });
                    }
                    Err(e) => {
                        error!("UDP recv_from error: {}", e);
                    }
                }
            }
            Some((conn_id, data)) = rx.recv() => {
                if let Some(addr) = conn_to_addr.get(&conn_id) {
                    let _ = socket.send_to(&data, addr).await;
                }
            }
        }
    }
}
