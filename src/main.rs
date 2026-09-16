use axum::{routing::get, Router};
use clap::Parser;
use std::net::SocketAddr;
use tracing::{info, Level};
use tower_http::cors::CorsLayer;

mod protocol;
mod state;
mod ws_handler;
mod tcp_proxy;
mod udp_proxy;
mod db;
mod admin;

use state::AppState;
use ws_handler::ws_handler;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 9090)]
    ws_port: u16,

    #[arg(long, default_value_t = 8080)]
    http_port: u16,

    #[arg(long, default_value_t = 10000)]
    port_range_start: u16,

    #[arg(long, default_value_t = 60000)]
    port_range_end: u16,

    #[arg(long, default_value = "tunnelit.db")]
    db_path: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .init();

    let args = Args::parse();
    info!("Starting Tunnelit Relay Server...");

    let db_conn = db::init_db(&args.db_path)?;
    info!("Database initialized at {}", args.db_path);

    let state = AppState::new(
        db_conn,
        (args.port_range_start, args.port_range_end),
    );

    // WS Server
    let ws_state = state.clone();
    let ws_app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(ws_state)
        .layer(CorsLayer::permissive());

    let ws_addr = SocketAddr::from(([0, 0, 0, 0], args.ws_port));
    info!("WebSocket server listening on {}", ws_addr);
    
    // HTTP Admin Server
    let http_state = state.clone();
    let http_app = admin::admin_routes()
        .with_state(http_state)
        .layer(CorsLayer::permissive());

    let http_addr = SocketAddr::from(([0, 0, 0, 0], args.http_port));
    info!("HTTP Admin server listening on {}", http_addr);

    // Run both servers
    tokio::select! {
        _ = axum::Server::bind(&ws_addr).serve(ws_app.into_make_service()) => {
            info!("WebSocket server stopped");
        }
        _ = axum::Server::bind(&http_addr).serve(http_app.into_make_service()) => {
            info!("HTTP server stopped");
        }
    }

    Ok(())
}
