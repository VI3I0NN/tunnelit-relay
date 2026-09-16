use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{delete, get, post},
    Json, Router,
};
use std::sync::Arc;
use serde::Deserialize;

use crate::state::AppState;
use crate::db;
use crate::protocol::RelayMessage;

const ADMIN_HTML: &str = include_str!("admin.html");

pub fn admin_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(serve_admin_page))
        .route("/api/tunnels", get(list_tunnels))
        .route("/api/tunnels/:id", delete(close_tunnel))
        .route("/api/requests", get(list_requests))
        .route("/api/requests/:id/approve", post(approve_request))
        .route("/api/requests/:id/reject", post(reject_request))
}

async fn serve_admin_page() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

async fn list_tunnels(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db::list_tunnels(&db) {
        Ok(tunnels) => (StatusCode::OK, Json(tunnels)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn close_tunnel(
    Path(id): Path<uuid::Uuid>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // In a real app we'd close the proxy tasks, but here we'll just remove it from DB and state.
    // The agent will eventually notice or we could send a message.
    let mut tunnels = state.tunnels.write().await;
    if let Some(t) = tunnels.remove(&id) {
        state.free_port(t.public_port).await;
        let db = state.db.lock().await;
        let _ = db::remove_tunnel(&db, id);
    }
    StatusCode::OK
}

async fn list_requests(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db::list_subdomain_requests(&db) {
        Ok(reqs) => (StatusCode::OK, Json(reqs)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
struct ApproveBody {
    subdomain: String,
}

async fn approve_request(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<ApproveBody>,
) -> impl IntoResponse {
    let db = state.db.lock().await;
    if let Ok(req) = db::get_subdomain_request(&db, &id) {
        let _ = db::update_subdomain_request_status(&db, &id, "approved");
        
        if let Ok(tunnel_id) = uuid::Uuid::parse_str(&req.tunnel_id) {
            let tunnels = state.tunnels.read().await;
            if let Some(tunnel) = tunnels.get(&tunnel_id) {
                let senders = state.agent_senders.read().await;
                if let Some(sender) = senders.get(&tunnel.agent_id) {
                    let _ = sender.send(RelayMessage::SubdomainAssigned {
                        tunnel_id,
                        subdomain: body.subdomain,
                    });
                }
            }
        }
    }
    StatusCode::OK
}

async fn reject_request(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let db = state.db.lock().await;
    let _ = db::update_subdomain_request_status(&db, &id, "rejected");
    StatusCode::OK
}
