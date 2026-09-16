use rusqlite::{params, Connection, Result};
use uuid::Uuid;

pub fn init_db(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS tunnels (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            local_port INTEGER NOT NULL,
            protocol TEXT NOT NULL,
            public_port INTEGER NOT NULL,
            created_at INTEGER NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS subdomain_requests (
            id TEXT PRIMARY KEY,
            tunnel_id TEXT NOT NULL,
            desired_name TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        [],
    )?;

    Ok(conn)
}

pub fn save_tunnel(
    conn: &Connection,
    id: Uuid,
    agent_id: Uuid,
    local_port: u16,
    protocol: &str,
    public_port: u16,
    created_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO tunnels (id, agent_id, local_port, protocol, public_port, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id.to_string(),
            agent_id.to_string(),
            local_port,
            protocol,
            public_port,
            created_at
        ],
    )?;
    Ok(())
}

pub fn remove_tunnel(conn: &Connection, id: Uuid) -> Result<()> {
    conn.execute("DELETE FROM tunnels WHERE id = ?1", params![id.to_string()])?;
    Ok(())
}

#[derive(serde::Serialize)]
pub struct DbTunnel {
    pub id: String,
    pub agent_id: String,
    pub local_port: u16,
    pub protocol: String,
    pub public_port: u16,
    pub created_at: i64,
}

pub fn list_tunnels(conn: &Connection) -> Result<Vec<DbTunnel>> {
    let mut stmt = conn.prepare("SELECT id, agent_id, local_port, protocol, public_port, created_at FROM tunnels")?;
    let tunnel_iter = stmt.query_map([], |row| {
        Ok(DbTunnel {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            local_port: row.get(2)?,
            protocol: row.get(3)?,
            public_port: row.get(4)?,
            created_at: row.get(5)?,
        })
    })?;

    let mut tunnels = Vec::new();
    for t in tunnel_iter {
        tunnels.push(t?);
    }
    Ok(tunnels)
}

#[derive(serde::Serialize)]
pub struct SubdomainRequest {
    pub id: String,
    pub tunnel_id: String,
    pub desired_name: String,
    pub status: String,
    pub created_at: i64,
}

pub fn save_subdomain_request(
    conn: &Connection,
    tunnel_id: Uuid,
    desired_name: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO subdomain_requests (id, tunnel_id, desired_name, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            Uuid::new_v4().to_string(),
            tunnel_id.to_string(),
            desired_name,
            "pending",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
        ],
    )?;
    Ok(())
}

pub fn list_subdomain_requests(conn: &Connection) -> Result<Vec<SubdomainRequest>> {
    let mut stmt = conn.prepare("SELECT id, tunnel_id, desired_name, status, created_at FROM subdomain_requests")?;
    let req_iter = stmt.query_map([], |row| {
        Ok(SubdomainRequest {
            id: row.get(0)?,
            tunnel_id: row.get(1)?,
            desired_name: row.get(2)?,
            status: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;

    let mut reqs = Vec::new();
    for r in req_iter {
        reqs.push(r?);
    }
    Ok(reqs)
}

pub fn update_subdomain_request_status(conn: &Connection, id: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE subdomain_requests SET status = ?1 WHERE id = ?2",
        params![status, id],
    )?;
    Ok(())
}

pub fn get_subdomain_request(conn: &Connection, id: &str) -> Result<SubdomainRequest> {
    let mut stmt = conn.prepare("SELECT id, tunnel_id, desired_name, status, created_at FROM subdomain_requests WHERE id = ?1")?;
    let req = stmt.query_row(params![id], |row| {
        Ok(SubdomainRequest {
            id: row.get(0)?,
            tunnel_id: row.get(1)?,
            desired_name: row.get(2)?,
            status: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    Ok(req)
}
