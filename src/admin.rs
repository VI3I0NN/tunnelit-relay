use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;
use tracing::info;

use crate::db;
use crate::protocol::{RelayMessage, TunnelConfig};
use crate::state::AppState;
use crate::tunnel_manager::{start_tunnel_listener, stop_tunnel_listener};
use crate::ws_handler::sync_agent_tunnels;

const ADMIN_PASSWORD: &str = "09090912Qw_";
const ADMIN_HTML: &str = include_str!("admin.html");
const CLAIM_HTML: &str = include_str!("claim.html");

pub fn admin_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(serve_admin_page))
        .route("/admin", get(serve_admin_page))
        .route("/claim/:code", get(serve_claim_page))
        .route("/api/claim/:code", post(claim_agent))
        // Auth
        .route("/api/auth/register", post(register))
        .route("/api/auth/login", post(login))
        .route("/api/auth/me", get(get_me))
        .route("/api/auth/logout", post(logout))
        // Admin Master Auth
        .route("/api/admin/login", post(admin_login))
        .route("/api/admin/users", get(admin_list_users))
        .route("/api/admin/users/:id/ban", post(admin_ban_user))
        .route("/api/admin/users/:id/unban", post(admin_unban_user))
        // Agents & Tunnels
        .route("/api/agents", get(list_agents))
        .route("/api/agents/generate-token", post(generate_agent_token))
        .route("/api/agents/:id", delete(delete_agent))
        .route("/api/tunnels", get(list_tunnels).post(create_tunnel))
        .route("/api/tunnels/:id", delete(delete_tunnel).put(update_tunnel))
        .route("/api/tunnels/:id/toggle", post(toggle_tunnel))
        // Telemetry & 1-Click Launchers
        .route("/api/stats", get(get_live_stats))
        .route("/api/scripts/launcher.sh", get(get_launcher_sh))
        .route("/api/scripts/launcher.bat", get(get_launcher_bat))
}

async fn serve_admin_page() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

async fn serve_claim_page() -> Html<&'static str> {
    Html(CLAIM_HTML)
}

// Helpers to extract user_id or admin from headers
async fn get_current_user_id(headers: &HeaderMap, state: &AppState) -> Option<String> {
    let token = extract_token(headers)?;
    let sessions = state.user_sessions.read().await;
    sessions.get(&token).cloned()
}

async fn is_admin(headers: &HeaderMap, state: &AppState) -> bool {
    if let Some(token) = extract_token(headers) {
        let admins = state.admin_sessions.read().await;
        admins.contains(&token)
    } else {
        false
    }
}

fn extract_token(headers: &HeaderMap) -> Option<String> {
    if let Some(val) = headers.get("authorization") {
        if let Ok(s) = val.to_str() {
            if s.starts_with("Bearer ") {
                return Some(s[7..].trim().to_string());
            }
        }
    }
    if let Some(val) = headers.get("x-session-token") {
        if let Ok(s) = val.to_str() {
            return Some(s.trim().to_string());
        }
    }
    None
}

// -------------------------------------------------------------
// AUTH ENDPOINTS
// -------------------------------------------------------------

#[derive(Deserialize)]
struct AuthRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct AuthResponse {
    token: String,
    user: db::UserRecord,
}

async fn register(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AuthRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let username = body.username.trim();
    if username.len() < 3 {
        return Err((StatusCode::BAD_REQUEST, "Username must be at least 3 characters".into()));
    }
    if body.password.len() < 6 {
        return Err((StatusCode::BAD_REQUEST, "Password must be at least 6 characters".into()));
    }

    let user = {
        let db = state.db.lock().await;
        db::register_user(&db, username, &body.password, state.port_range)
            .map_err(|e| (StatusCode::CONFLICT, format!("Registration failed (username might be taken): {}", e)))?
    };

    let session_token = format!("usr_{}", Uuid::new_v4().to_string().replace('-', ""));
    {
        let mut sessions = state.user_sessions.write().await;
        sessions.insert(session_token.clone(), user.id.clone());
    }

    Ok((StatusCode::CREATED, Json(AuthResponse {
        token: session_token,
        user,
    })))
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AuthRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let user_opt = {
        let db = state.db.lock().await;
        db::verify_user(&db, body.username.trim(), &body.password)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?
    };

    let user = match user_opt {
        Some(u) => {
            if u.is_banned {
                return Err((StatusCode::FORBIDDEN, "This account is suspended".into()));
            }
            u
        }
        None => return Err((StatusCode::UNAUTHORIZED, "Invalid username or password".into())),
    };

    let session_token = format!("usr_{}", Uuid::new_v4().to_string().replace('-', ""));
    {
        let mut sessions = state.user_sessions.write().await;
        sessions.insert(session_token.clone(), user.id.clone());
    }

    Ok((StatusCode::OK, Json(AuthResponse {
        token: session_token,
        user,
    })))
}

async fn get_me(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let user_id = get_current_user_id(&headers, &state).await
        .ok_or((StatusCode::UNAUTHORIZED, "Not logged in".into()))?;

    let db = state.db.lock().await;
    let user = db::get_user_by_id(&db, &user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?
        .ok_or((StatusCode::NOT_FOUND, "User not found".into()))?;

    Ok((StatusCode::OK, Json(user)))
}

async fn logout(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Some(token) = extract_token(&headers) {
        let mut sessions = state.user_sessions.write().await;
        sessions.remove(&token);
    }
    StatusCode::OK
}

// -------------------------------------------------------------
// MASTER ADMIN ENDPOINTS (Password: 09090912Qw_)
// -------------------------------------------------------------

#[derive(Deserialize)]
struct AdminLoginRequest {
    password: String,
}

async fn admin_login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AdminLoginRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if body.password == ADMIN_PASSWORD {
        let token = format!("adm_{}", Uuid::new_v4().to_string().replace('-', ""));
        let mut admins = state.admin_sessions.write().await;
        admins.insert(token.clone());
        Ok((StatusCode::OK, Json(serde_json::json!({ "token": token }))))
    } else {
        Err((StatusCode::UNAUTHORIZED, "Invalid master admin password".into()))
    }
}

async fn admin_list_users(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !is_admin(&headers, &state).await {
        return Err((StatusCode::FORBIDDEN, "Admin access required".into()));
    }

    let db = state.db.lock().await;
    let users = db::list_all_users(&db)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?;

    Ok((StatusCode::OK, Json(users)))
}

async fn admin_ban_user(
    Path(id): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !is_admin(&headers, &state).await {
        return Err((StatusCode::FORBIDDEN, "Admin access required".into()));
    }

    let db = state.db.lock().await;
    let _ = db::toggle_user_ban(&db, &id, true);

    // Stop all tunnels of banned user
    let tunnels = db::list_tunnels_for_user(&db, &id).unwrap_or_default();
    for t in tunnels {
        if let Ok(tid) = Uuid::parse_str(&t.id) {
            stop_tunnel_listener(tid, state.clone()).await;
            state.free_port(t.public_port).await;
        }
    }

    Ok(StatusCode::OK)
}

async fn admin_unban_user(
    Path(id): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !is_admin(&headers, &state).await {
        return Err((StatusCode::FORBIDDEN, "Admin access required".into()));
    }

    let db = state.db.lock().await;
    let _ = db::toggle_user_ban(&db, &id, false);

    Ok(StatusCode::OK)
}

// -------------------------------------------------------------
// CLAIM AGENT (Linked to Current User if logged in)
// -------------------------------------------------------------

async fn claim_agent(
    Path(code): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, &'static str)> {
    let current_user_id = get_current_user_id(&headers, &state).await
        .ok_or((StatusCode::UNAUTHORIZED, "Registration or login is required to link devices"))?;

    let agent_record = {
        let db = state.db.lock().await;
        db::claim_agent_to_user(&db, &code, &current_user_id)
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

    let waiting_sender = {
        let mut pending = state.pending_claims.write().await;
        pending.remove(&code)
    };

    if let Some(sender) = waiting_sender {
        info!("Claim confirmed! Authenticating agent '{}'", agent.name);
        {
            let db = state.db.lock().await;
            let _ = db::set_agent_online(&db, &agent.id, true, None);
        }

        {
            let mut senders = state.agent_senders.write().await;
            senders.insert(agent_id, sender.clone());
        }

        let _ = sender.send(RelayMessage::AuthSuccess {
            agent_id,
            name: agent.name.clone(),
            token: agent.token.clone(),
        });

        sync_agent_tunnels(agent_id, state.clone(), sender).await;
    }

    Ok((StatusCode::OK, "Agent claimed successfully"))
}

// -------------------------------------------------------------
// AGENTS & TUNNELS
// -------------------------------------------------------------

async fn list_agents(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let is_adm = is_admin(&headers, &state).await;
    let current_uid = get_current_user_id(&headers, &state).await;

    if !is_adm && current_uid.is_none() {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!([]))).into_response();
    }

    let db = state.db.lock().await;
    let agents = if is_adm {
        db::list_all_agents(&db).unwrap_or_default()
    } else {
        db::list_agents_for_user(&db, current_uid.as_ref().unwrap()).unwrap_or_default()
    };

    (StatusCode::OK, Json(agents)).into_response()
}

async fn generate_agent_token(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let user_id = get_current_user_id(&headers, &state).await
        .ok_or((StatusCode::UNAUTHORIZED, "Please sign in first".into()))?;

    let db = state.db.lock().await;
    let existing_agents = db::list_agents_for_user(&db, &user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?;

    if let Some(first) = existing_agents.first() {
        return Ok((StatusCode::OK, Json(serde_json::json!({ "token": first.token, "id": first.id }))));
    }

    let new_agent = db::create_agent_for_user(&db, &user_id, "My Computer")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?;

    Ok((StatusCode::OK, Json(serde_json::json!({ "token": new_agent.token, "id": new_agent.id }))))
}


async fn delete_agent(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Ok(agent_id) = Uuid::parse_str(&id) {
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

async fn list_tunnels(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let is_adm = is_admin(&headers, &state).await;
    let current_uid = get_current_user_id(&headers, &state).await;

    if !is_adm && current_uid.is_none() {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!([]))).into_response();
    }

    let db = state.db.lock().await;
    let tunnels = if is_adm {
        db::list_all_tunnels(&db).unwrap_or_default()
    } else {
        db::list_tunnels_for_user(&db, current_uid.as_ref().unwrap()).unwrap_or_default()
    };

    (StatusCode::OK, Json(tunnels)).into_response()
}


#[derive(Deserialize)]
struct CreateTunnelRequest {
    agent_id: String,
    name: String,
    local_port: u16,
    protocol: String,
    #[allow(dead_code)]
    preferred_port: Option<u16>,
    subdomain: Option<String>,
}

async fn create_tunnel(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTunnelRequest>,
) -> Result<Response, (StatusCode, String)> {
    let current_uid = get_current_user_id(&headers, &state).await
        .ok_or((StatusCode::UNAUTHORIZED, "Registration or sign in is required to create tunnels".into()))?;

    let agent_id = match Uuid::parse_str(&body.agent_id) {
        Ok(id) => id,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "Invalid agent ID".into())),
    };

    // Subdomain uniqueness
    if let Some(sub) = &body.subdomain {
        let clean = sub.trim();
        if !clean.is_empty() {
            let db = state.db.lock().await;
            if db::is_subdomain_taken(&db, clean, None).unwrap_or(false) {
                return Err((StatusCode::CONFLICT, format!("Subdomain '{}' is already in use", clean)));
            }
        }
    }

    // Force public remote port to strictly be the user's dedicated_port
    let dedicated_port = {
        let db = state.db.lock().await;
        let user = db::get_user_by_id(&db, &current_uid)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?
            .ok_or((StatusCode::UNAUTHORIZED, "User account not found".into()))?;
        user.dedicated_port
    };

    let public_port = match state.allocate_port(Some(dedicated_port)).await {
        Some(p) if p == dedicated_port => p,
        _ => return Err((StatusCode::CONFLICT, format!("Ваш выделенный порт :{} уже используется другим активным туннелем. Отключите или удалите его.", dedicated_port))),
    };

    let clean_subdomain = body.subdomain.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());

    let tunnel_record = {
        let db = state.db.lock().await;
        db::create_tunnel(
            &db,
            &body.agent_id,
            Some(&current_uid),
            &body.name,
            body.local_port,
            &body.protocol,
            public_port,
            clean_subdomain,
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

    let senders = state.agent_senders.read().await;
    if let Some(agent_sender) = senders.get(&agent_id) {
        let _ = start_tunnel_listener(&config, agent_sender.clone(), state.clone()).await;
        let _ = agent_sender.send(RelayMessage::StartTunnel {
            tunnel: config.clone(),
        });
    }

    Ok((StatusCode::CREATED, Json(tunnel_record)).into_response())
}

#[derive(Deserialize)]
struct UpdateTunnelRequest {
    name: String,
    local_port: u16,
    protocol: String,
    subdomain: Option<String>,
}

async fn update_tunnel(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateTunnelRequest>,
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

    let clean_sub = body.subdomain.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());

    if let Some(sub) = clean_sub {
        let db = state.db.lock().await;
        if db::is_subdomain_taken(&db, sub, Some(&id)).unwrap_or(false) {
            return Err((StatusCode::CONFLICT, format!("Subdomain '{}' is already taken", sub)));
        }
    }

    {
        let db = state.db.lock().await;
        db::update_tunnel(&db, &id, &body.name, body.local_port, &body.protocol, clean_sub)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)))?;
    }

    if let Ok(agent_id) = Uuid::parse_str(&tunnel.agent_id) {
        let senders = state.agent_senders.read().await;
        if let Some(sender) = senders.get(&agent_id) {
            sync_agent_tunnels(agent_id, state.clone(), sender.clone()).await;
        }
    }

    Ok(StatusCode::OK)
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

// -------------------------------------------------------------
// TELEMETRY & 1-CLICK LAUNCHERS
// -------------------------------------------------------------

async fn get_live_stats(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let traffic = state.live_traffic.read().await;
    let mut map = std::collections::HashMap::new();
    for (k, v) in traffic.iter() {
        map.insert(k.to_string(), serde_json::json!({
            "bytes_in": v.0,
            "bytes_out": v.1,
            "total_mb": ((v.0 + v.1) as f64) / (1024.0 * 1024.0)
        }));
    }
    Json(map)
}

#[derive(Deserialize)]
struct ScriptQuery {
    token: Option<String>,
    session: Option<String>,
}

async fn resolve_token_for_script(query: &ScriptQuery, headers: &HeaderMap, state: &AppState) -> Option<String> {
    if let Some(ref t) = query.token {
        if !t.trim().is_empty() && t != "YOUR_TOKEN_HERE" {
            return Some(t.trim().to_string());
        }
    }

    let user_id = if let Some(ref sess) = query.session {
        let sessions = state.user_sessions.read().await;
        sessions.get(sess).cloned()
    } else {
        get_current_user_id(headers, state).await
    };

    if let Some(uid) = user_id {
        let db = state.db.lock().await;
        if let Ok(agents) = db::list_agents_for_user(&db, &uid) {
            if let Some(first) = agents.first() {
                return Some(first.token.clone());
            }
        }
        if let Ok(new_agent) = db::create_agent_for_user(&db, &uid, "My Computer") {
            return Some(new_agent.token);
        }
    }

    None
}

async fn get_launcher_sh(
    Query(query): Query<ScriptQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let token_arg = match resolve_token_for_script(&query, &headers, &state).await {
        Some(tok) => format!("--token \"{}\"", tok),
        None => "".to_string(),
    };

    let script = format!(
r#"#!/bin/bash
set -e
echo "⚡ Downloading tunnelit-agent..."
mkdir -p "$HOME/.tunnelit"
curl -sSL "https://github.com/visionn1488/tunnelit-agent/releases/download/latest/tunnelit-agent-linux-amd64" -o "$HOME/.tunnelit/tunnelit-agent"
chmod +x "$HOME/.tunnelit/tunnelit-agent"

echo "🚀 Starting tunnelit-agent..."
"$HOME/.tunnelit/tunnelit-agent" {} --relay "wss://ws.ezbchat.fun/ws"
"#,
        token_arg
    );

    (
        [
            ("content-type", "text/x-shellscript"),
            ("content-disposition", "attachment; filename=\"launch-tunnelit.sh\""),
        ],
        script,
    )
}

async fn get_launcher_bat(
    Query(query): Query<ScriptQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let token_arg = match resolve_token_for_script(&query, &headers, &state).await {
        Some(tok) => format!("--token \"{}\"", tok),
        None => "".to_string(),
    };

    let script = format!(
r#"@echo off
echo ⚡ Downloading tunnelit-agent for Windows...
powershell -Command "Invoke-WebRequest -Uri 'https://github.com/visionn1488/tunnelit-agent/releases/download/latest/tunnelit-agent-windows-amd64.exe' -OutFile 'tunnelit-agent.exe'"
echo 🚀 Starting tunnelit-agent...
tunnelit-agent.exe {} --relay "wss://ws.ezbchat.fun/ws"
pause
"#,
        token_arg
    );

    (
        [
            ("content-type", "text/plain"),
            ("content-disposition", "attachment; filename=\"launch-tunnelit.bat\""),
        ],
        script,
    )
}

