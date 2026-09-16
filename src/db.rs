use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentRecord {
    pub id: String,
    pub token: String,
    pub claim_code: Option<String>,
    pub name: String,
    pub created_at: i64,
    pub last_seen: i64,
    pub is_online: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TunnelRecord {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub local_port: u16,
    pub protocol: String,
    pub public_port: u16,
    pub subdomain: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
}

pub fn init_db(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS agents (
            id TEXT PRIMARY KEY,
            token TEXT NOT NULL UNIQUE,
            claim_code TEXT,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_seen INTEGER NOT NULL,
            is_online INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS tunnels (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            name TEXT NOT NULL,
            local_port INTEGER NOT NULL,
            protocol TEXT NOT NULL,
            public_port INTEGER NOT NULL,
            subdomain TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE
        )",
        [],
    )?;

    // Auto-migrate from older schema if columns are missing:
    let _ = conn.execute("ALTER TABLE tunnels ADD COLUMN name TEXT NOT NULL DEFAULT 'Server'", []);
    let _ = conn.execute("ALTER TABLE tunnels ADD COLUMN subdomain TEXT", []);
    let _ = conn.execute("ALTER TABLE tunnels ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1", []);

    Ok(conn)
}

pub fn create_agent_claim(conn: &Connection, name: &str) -> Result<(AgentRecord, String)> {
    let id = Uuid::new_v4().to_string();
    let token = format!("tk_{}", Uuid::new_v4().to_string().replace('-', ""));
    let claim_code = format!("{:06}", (Uuid::new_v4().as_u128() % 1_000_000) as u32);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;

    conn.execute(
        "INSERT INTO agents (id, token, claim_code, name, created_at, last_seen, is_online)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
        params![id, token, claim_code, name, now, now],
    )?;

    let record = AgentRecord {
        id,
        token,
        claim_code: Some(claim_code.clone()),
        name: name.to_string(),
        created_at: now,
        last_seen: now,
        is_online: false,
    };

    Ok((record, claim_code))
}

pub fn claim_agent_by_code(conn: &Connection, code: &str) -> Result<Option<AgentRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, token, claim_code, name, created_at, last_seen, is_online
         FROM agents WHERE claim_code = ?1"
    )?;

    let mut rows = stmt.query(params![code])?;
    if let Some(row) = rows.next()? {
        let agent = AgentRecord {
            id: row.get(0)?,
            token: row.get(1)?,
            claim_code: row.get(2)?,
            name: row.get(3)?,
            created_at: row.get(4)?,
            last_seen: row.get(5)?,
            is_online: row.get::<_, i64>(6)? != 0,
        };

        // Clear claim_code once claimed
        conn.execute(
            "UPDATE agents SET claim_code = NULL WHERE id = ?1",
            params![agent.id],
        )?;

        Ok(Some(agent))
    } else {
        Ok(None)
    }
}

pub fn get_agent_by_token(conn: &Connection, token: &str) -> Result<Option<AgentRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, token, claim_code, name, created_at, last_seen, is_online
         FROM agents WHERE token = ?1"
    )?;

    let mut rows = stmt.query(params![token])?;
    if let Some(row) = rows.next()? {
        Ok(Some(AgentRecord {
            id: row.get(0)?,
            token: row.get(1)?,
            claim_code: row.get(2)?,
            name: row.get(3)?,
            created_at: row.get(4)?,
            last_seen: row.get(5)?,
            is_online: row.get::<_, i64>(6)? != 0,
        }))
    } else {
        Ok(None)
    }
}

pub fn get_agent_by_id(conn: &Connection, id: &str) -> Result<Option<AgentRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, token, claim_code, name, created_at, last_seen, is_online
         FROM agents WHERE id = ?1"
    )?;

    let mut rows = stmt.query(params![id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(AgentRecord {
            id: row.get(0)?,
            token: row.get(1)?,
            claim_code: row.get(2)?,
            name: row.get(3)?,
            created_at: row.get(4)?,
            last_seen: row.get(5)?,
            is_online: row.get::<_, i64>(6)? != 0,
        }))
    } else {
        Ok(None)
    }
}

pub fn set_agent_online(conn: &Connection, id: &str, is_online: bool, name: Option<&str>) -> Result<()> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let online_val = if is_online { 1 } else { 0 };

    if let Some(n) = name {
        conn.execute(
            "UPDATE agents SET is_online = ?1, last_seen = ?2, name = ?3 WHERE id = ?4",
            params![online_val, now, n, id],
        )?;
    } else {
        conn.execute(
            "UPDATE agents SET is_online = ?1, last_seen = ?2 WHERE id = ?3",
            params![online_val, now, id],
        )?;
    }
    Ok(())
}

pub fn list_agents(conn: &Connection) -> Result<Vec<AgentRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, token, claim_code, name, created_at, last_seen, is_online
         FROM agents ORDER BY created_at DESC"
    )?;
    let iter = stmt.query_map([], |row| {
        Ok(AgentRecord {
            id: row.get(0)?,
            token: row.get(1)?,
            claim_code: row.get(2)?,
            name: row.get(3)?,
            created_at: row.get(4)?,
            last_seen: row.get(5)?,
            is_online: row.get::<_, i64>(6)? != 0,
        })
    })?;

    let mut list = Vec::new();
    for a in iter {
        list.push(a?);
    }
    Ok(list)
}

pub fn delete_agent(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM tunnels WHERE agent_id = ?1", params![id])?;
    conn.execute("DELETE FROM agents WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn create_tunnel(
    conn: &Connection,
    agent_id: &str,
    name: &str,
    local_port: u16,
    protocol: &str,
    public_port: u16,
    subdomain: Option<&str>,
) -> Result<TunnelRecord> {
    let id = Uuid::new_v4().to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;

    conn.execute(
        "INSERT INTO tunnels (id, agent_id, name, local_port, protocol, public_port, subdomain, enabled, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8)",
        params![id, agent_id, name, local_port, protocol, public_port, subdomain, now],
    )?;

    Ok(TunnelRecord {
        id,
        agent_id: agent_id.to_string(),
        name: name.to_string(),
        local_port,
        protocol: protocol.to_string(),
        public_port,
        subdomain: subdomain.map(|s| s.to_string()),
        enabled: true,
        created_at: now,
    })
}

pub fn list_tunnels_for_agent(conn: &Connection, agent_id: &str) -> Result<Vec<TunnelRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, name, local_port, protocol, public_port, subdomain, enabled, created_at
         FROM tunnels WHERE agent_id = ?1 ORDER BY created_at ASC"
    )?;
    let iter = stmt.query_map(params![agent_id], |row| {
        Ok(TunnelRecord {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            name: row.get(2)?,
            local_port: row.get(3)?,
            protocol: row.get(4)?,
            public_port: row.get(5)?,
            subdomain: row.get(6)?,
            enabled: row.get::<_, i64>(7)? != 0,
            created_at: row.get(8)?,
        })
    })?;

    let mut list = Vec::new();
    for t in iter {
        list.push(t?);
    }
    Ok(list)
}

pub fn list_all_tunnels(conn: &Connection) -> Result<Vec<TunnelRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, name, local_port, protocol, public_port, subdomain, enabled, created_at
         FROM tunnels ORDER BY created_at DESC"
    )?;
    let iter = stmt.query_map([], |row| {
        Ok(TunnelRecord {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            name: row.get(2)?,
            local_port: row.get(3)?,
            protocol: row.get(4)?,
            public_port: row.get(5)?,
            subdomain: row.get(6)?,
            enabled: row.get::<_, i64>(7)? != 0,
            created_at: row.get(8)?,
        })
    })?;

    let mut list = Vec::new();
    for t in iter {
        list.push(t?);
    }
    Ok(list)
}

pub fn get_tunnel_by_id(conn: &Connection, id: &str) -> Result<Option<TunnelRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, name, local_port, protocol, public_port, subdomain, enabled, created_at
         FROM tunnels WHERE id = ?1"
    )?;
    let mut rows = stmt.query(params![id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(TunnelRecord {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            name: row.get(2)?,
            local_port: row.get(3)?,
            protocol: row.get(4)?,
            public_port: row.get(5)?,
            subdomain: row.get(6)?,
            enabled: row.get::<_, i64>(7)? != 0,
            created_at: row.get(8)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn set_tunnel_enabled(conn: &Connection, id: &str, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE tunnels SET enabled = ?1 WHERE id = ?2",
        params![if enabled { 1 } else { 0 }, id],
    )?;
    Ok(())
}

pub fn update_tunnel(
    conn: &Connection,
    id: &str,
    name: &str,
    local_port: u16,
    protocol: &str,
    subdomain: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE tunnels SET name = ?1, local_port = ?2, protocol = ?3, subdomain = ?4 WHERE id = ?5",
        params![name, local_port, protocol, subdomain, id],
    )?;
    Ok(())
}

pub fn is_subdomain_taken(conn: &Connection, subdomain: &str, exclude_id: Option<&str>) -> Result<bool> {
    if let Some(ex_id) = exclude_id {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tunnels WHERE LOWER(subdomain) = LOWER(?1) AND id != ?2",
            params![subdomain, ex_id],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    } else {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tunnels WHERE LOWER(subdomain) = LOWER(?1)",
            params![subdomain],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }
}

pub fn delete_tunnel(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM tunnels WHERE id = ?1", params![id])?;
    Ok(())
}
