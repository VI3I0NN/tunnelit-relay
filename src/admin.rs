use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;
use tracing::info;

use crate::db;
use crate::protocol::{RelayMessage, TunnelConfig};
use crate::state::AppState;
use crate::tunnel_manager::{start_tunnel_listener, stop_tunnel_listener};
use crate::ws_handler::sync_agent_tunnels;

const ADMIN_HTML: &str = include_str!("admin.html");
const CLAIM_HTML: &str = include_str!("claim.html");

pub fn admin_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(serve_admin_page))
        .route("/claim/:code", get(serve_claim_page))
        .route("/api/claim/:code", post(claim_agent))
        .route("/api/agents", get(list_agents))
        .route("/api/agents/:id", delete(delete_agent))
        .route("/api/tunnels", get(list_tunnels).post(create_tunnel))
        .route("/api/tunnels/:id", delete(delete_tunnel))
        .route("/api/tunnels/:id/toggle", post(toggle_tunnel))
}

async fn serve_admin_page() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

async fn serve_claim_page() -> Html<&'static str> {
    Html(CLAIM_HTML)
}

async fn claim_agent(
    Path(code): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, &'static str)> {
    let agent_record = {
        let db = state.db.lock().await;
        db::claim_agent_by_code(&db, &code)
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error"))?
    };

    let agent = match agent_record {
        Some(a) => a,
        None => return Err((StatusCode::NOT_FOUND, "Invalid or expired claim code")),
    };

    let agent_id = match Uuid::parse_str(&agent.id) {
        Ok(id) => id,
        Err(_) => return Err((StatusCode::INTERNAL_SERVER_ERROR, "Invalid agent ID")),
    };

    // If the agent is currently connected and waiting for this claim code:
    let waiting_sender = {
        let mut pending = state.pending_claims.write().await;
        pending.remove(&code)
    };

    if let Some(sender) = waiting_sender {
        info!("Claim confirmed on web! Authenticating waiting agent '{}'", agent.name);
        // Mark online in DB
        {
            let db = state.db.lock().await;
            let _ = db::set_agent_online(&db, &agent.id, true, None);
        }

        // Register in active senders
        {
            let mut senders = state.agent_senders.write().await;
            senders.insert(agent_id, sender.clone());
        }

        // Send AuthSuccess to agent so it knows its permanent token!
        let _ = sender.send(RelayMessage::AuthSuccess {
            agent_id,
            name: agent.name.clone(),
            token: agent.token.clone(),
        });

        // Sync tunnels
        sync_agent_tunnels(agent_id, state.clone(), sender).await;
    }

    Ok((StatusCode::OK, "Agent claimed successfully"))
}

async fn list_agents(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db::list_agents(&db) {
        Ok(agents) => (StatusCode::OK, Json(agents)).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Failed to load agents").into_response(),
    }
}

async fn delete_agent(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Ok(agent_id) = Uuid::parse_str(&id) {
        // Disconnect if online
        let mut senders = state.agent_senders.write().await;
        if let Some(sender) = senders.remove(&agent_id) {
            let _ = sender.send(RelayMessage::Error {
                message: "Agent deleted from dashboard".into(),
            });
        }
    }

    let db = state.db.lock().await;
    let _ = db::delete_agent(&db, &id);
    StatusCode::OK
}

async fn list_tunnels(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db::list_all_tunnels(&db) {
        Ok(tunnels) => (StatusCode::OK, Json(tunnels)).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Failed to load tunnels").into_response(),
    }
}

#[derive(Deserialize)]
struct CreateTunnelRequest {
    agent_id: String,
    name: String,
    local_port: u16,
    protocol: String,
    preferred_port: Option<u16>,
    subdomain: Option<String>,
}

async fn create_tunnel(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTunnelRequest>,
) -> Result<Response, (StatusCode, String)> {
    let agent_id = match Uuid::parse_str(&body.agent_id) {
        Ok(id) => id,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "Invalid agent ID".into())),
    };

    let public_port = match state.allocate_port(body.preferred_port).await {
        Some(p) => p,
        None => return Err((StatusCode::CONFLICT, "No ports available or requested port is already in use".into())),
    };

    let tunnel_record = {
        let db = state.db.lock().await;
        db::create_tunnel(
            &db,
            &body.agent_id,
            &body.name,
            body.local_port,
            &body.protocol,
            public_port,
            body.subdomain.as_deref(),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?
    };

    let tunnel_id = match Uuid::parse_str(&tunnel_record.id) {
        Ok(id) => id,
        Err(_) => return Err((StatusCode::INTERNAL_SERVER_ERROR, "UUID parse error".into())),
    };

    let config = TunnelConfig {
        id: tunnel_id,
        agent_id,
        name: tunnel_record.name.clone(),
        local_port: tunnel_record.local_port,
        protocol: tunnel_record.protocol.clone(),
        public_port,
        subdomain: tunnel_record.subdomain.clone(),
        enabled: true,
    };

    // If agent is online, start listener and notify agent
    let senders = state.agent_senders.read().await;
    if let Some(agent_sender) = senders.get(&agent_id) {
        let _ = start_tunnel_listener(&config, agent_sender.clone(), state.clone()).await;
        let _ = agent_sender.send(RelayMessage::StartTunnel {
            tunnel: config.clone(),
        });
    }

    Ok((StatusCode::CREATED, Json(tunnel_record)).into_response())
}

async fn toggle_tunnel(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, (StatusCode, String)> {
    let tunnel = {
        let db = state.db.lock().await;
        db::get_tunnel_by_id(&db, &id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?
    };

    let tunnel = match tunnel {
        Some(t) => t,
        None => return Err((StatusCode::NOT_FOUND, "Tunnel not found".into())),
    };

    let tunnel_id = match Uuid::parse_str(&tunnel.id) {
        Ok(id) => id,
        Err(_) => return Err((StatusCode::INTERNAL_SERVER_ERROR, "Invalid ID".into())),
    };

    let agent_id = match Uuid::parse_str(&tunnel.agent_id) {
        Ok(id) => id,
        Err(_) => return Err((StatusCode::INTERNAL_SERVER_ERROR, "Invalid agent ID".into())),
    };

    let new_enabled = !tunnel.enabled;

    {
        let db = state.db.lock().await;
        let _ = db::set_tunnel_enabled(&db, &id, new_enabled);
    }

    let senders = state.agent_senders.read().await;
    let agent_sender_opt = senders.get(&agent_id);

    if new_enabled {
        // Turning ON
        let mut allocated = state.allocated_ports.write().await;
        allocated.insert(tunnel.public_port);

        let config = TunnelConfig {
            id: tunnel_id,
            agent_id,
            name: tunnel.name,
            local_port: tunnel.local_port,
            protocol: tunnel.protocol,
            public_port: tunnel.public_port,
            subdomain: tunnel.subdomain,
            enabled: true,
        };

        if let Some(agent_sender) = agent_sender_opt {
            let _ = start_tunnel_listener(&config, agent_sender.clone(), state.clone()).await;
            let _ = agent_sender.send(RelayMessage::StartTunnel { tunnel: config });
        }
    } else {
        // Turning OFF
        stop_tunnel_listener(tunnel_id, state.clone()).await;
        state.free_port(tunnel.public_port).await;

        if let Some(agent_sender) = agent_sender_opt {
            let _ = agent_sender.send(RelayMessage::StopTunnel { tunnel_id });
        }
    }

    Ok(StatusCode::OK)
}

async fn delete_tunnel(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, (StatusCode, String)> {
    let tunnel = {
        let db = state.db.lock().await;
        db::get_tunnel_by_id(&db, &id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?
    };

    if let Some(t) = tunnel {
        if let Ok(tunnel_id) = Uuid::parse_str(&t.id) {
            stop_tunnel_listener(tunnel_id, state.clone()).await;
            state.free_port(t.public_port).await;

            if let Ok(agent_id) = Uuid::parse_str(&t.agent_id) {
                let senders = state.agent_senders.read().await;
                if let Some(sender) = senders.get(&agent_id) {
                    let _ = sender.send(RelayMessage::StopTunnel { tunnel_id });
                }
            }
        }

        let db = state.db.lock().await;
        let _ = db::delete_tunnel(&db, &id);
    }

    Ok(StatusCode::OK)
}
