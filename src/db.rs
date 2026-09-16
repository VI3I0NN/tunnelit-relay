use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserRecord {
    pub id: String,
    pub username: String,
    pub dedicated_port: u16,
    pub is_banned: bool,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentRecord {
    pub id: String,
    pub user_id: Option<String>,
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
    pub user_id: Option<String>,
    pub name: String,
    pub local_port: u16,
    pub protocol: String,
    pub public_port: u16,
    pub subdomain: Option<String>,
    pub enabled: bool,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub created_at: i64,
}

pub fn init_db(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)?;

    // 1. Users table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            dedicated_port INTEGER NOT NULL UNIQUE,
            is_banned INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL
        )",
        [],
    )?;

    // 2. Agents table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS agents (
            id TEXT PRIMARY KEY,
            user_id TEXT,
            token TEXT NOT NULL UNIQUE,
            claim_code TEXT,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_seen INTEGER NOT NULL,
            is_online INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
        )",
        [],
    )?;

    // 3. Tunnels table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tunnels (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            user_id TEXT,
            name TEXT NOT NULL,
            local_port INTEGER NOT NULL,
            protocol TEXT NOT NULL,
            public_port INTEGER NOT NULL,
            subdomain TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            bytes_in INTEGER NOT NULL DEFAULT 0,
            bytes_out INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE,
            FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
        )",
        [],
    )?;

    // Auto-migrations for existing databases
    let _ = conn.execute("ALTER TABLE agents ADD COLUMN user_id TEXT", []);
    let _ = conn.execute("ALTER TABLE tunnels ADD COLUMN user_id TEXT", []);
    let _ = conn.execute("ALTER TABLE tunnels ADD COLUMN bytes_in INTEGER NOT NULL DEFAULT 0", []);
    let _ = conn.execute("ALTER TABLE tunnels ADD COLUMN bytes_out INTEGER NOT NULL DEFAULT 0", []);
    let _ = conn.execute("ALTER TABLE tunnels ADD COLUMN name TEXT NOT NULL DEFAULT 'Server'", []);
    let _ = conn.execute("ALTER TABLE tunnels ADD COLUMN subdomain TEXT", []);
    let _ = conn.execute("ALTER TABLE tunnels ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1", []);

    Ok(conn)
}

// Simple hash for passwords (SHA256 equivalent via simple salt+digest or sha2)
pub fn hash_password(password: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    format!("salt_090909_{}", password).hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub fn generate_unique_dedicated_port(conn: &Connection, port_range: (u16, u16)) -> Result<u16> {
    for _ in 0..1000 {
        let port = (Uuid::new_v4().as_u128() % ((port_range.1 - port_range.0) as u128)) as u16 + port_range.0;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM users WHERE dedicated_port = ?1",
            params![port],
            |r| r.get(0),
        )?;
        if count == 0 {
            return Ok(port);
        }
    }
    // Fallback sequential
    for p in port_range.0..=port_range.1 {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM users WHERE dedicated_port = ?1",
            params![p],
            |r| r.get(0),
        )?;
        if count == 0 {
            return Ok(p);
        }
    }
    Err(rusqlite::Error::QueryReturnedNoRows)
}

pub fn register_user(conn: &Connection, username: &str, password: &str, port_range: (u16, u16)) -> Result<UserRecord> {
    let id = Uuid::new_v4().to_string();
    let password_hash = hash_password(password);
    let dedicated_port = generate_unique_dedicated_port(conn, port_range)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;

    conn.execute(
        "INSERT INTO users (id, username, password_hash, dedicated_port, is_banned, created_at)
         VALUES (?1, ?2, ?3, ?4, 0, ?5)",
        params![id, username, password_hash, dedicated_port, now],
    )?;

    Ok(UserRecord {
        id,
        username: username.to_string(),
        dedicated_port,
        is_banned: false,
        created_at: now,
    })
}

pub fn verify_user(conn: &Connection, username: &str, password: &str) -> Result<Option<UserRecord>> {
    let hash = hash_password(password);
    let mut stmt = conn.prepare(
        "SELECT id, username, dedicated_port, is_banned, created_at
         FROM users WHERE username = ?1 AND password_hash = ?2"
    )?;

    let mut rows = stmt.query(params![username, hash])?;
    if let Some(row) = rows.next()? {
        Ok(Some(UserRecord {
            id: row.get(0)?,
            username: row.get(1)?,
            dedicated_port: row.get(2)?,
            is_banned: row.get::<_, i64>(3)? != 0,
            created_at: row.get(4)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn get_user_by_id(conn: &Connection, id: &str) -> Result<Option<UserRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, username, dedicated_port, is_banned, created_at
         FROM users WHERE id = ?1"
    )?;
    let mut rows = stmt.query(params![id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(UserRecord {
            id: row.get(0)?,
            username: row.get(1)?,
            dedicated_port: row.get(2)?,
            is_banned: row.get::<_, i64>(3)? != 0,
            created_at: row.get(4)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn list_all_users(conn: &Connection) -> Result<Vec<UserRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, username, dedicated_port, is_banned, created_at
         FROM users ORDER BY created_at DESC"
    )?;
    let iter = stmt.query_map([], |row| {
        Ok(UserRecord {
            id: row.get(0)?,
            username: row.get(1)?,
            dedicated_port: row.get(2)?,
            is_banned: row.get::<_, i64>(3)? != 0,
            created_at: row.get(4)?,
        })
    })?;

    let mut list = Vec::new();
    for u in iter {
        list.push(u?);
    }
    Ok(list)
}

pub fn toggle_user_ban(conn: &Connection, id: &str, banned: bool) -> Result<()> {
    conn.execute(
        "UPDATE users SET is_banned = ?1 WHERE id = ?2",
        params![if banned { 1 } else { 0 }, id],
    )?;
    Ok(())
}

pub fn create_agent_claim(conn: &Connection, name: &str) -> Result<(AgentRecord, String)> {
    let id = Uuid::new_v4().to_string();
    let token = format!("tk_{}", Uuid::new_v4().to_string().replace('-', ""));
    let claim_code = format!("{:06}", (Uuid::new_v4().as_u128() % 1_000_000) as u32);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;

    conn.execute(
        "INSERT INTO agents (id, user_id, token, claim_code, name, created_at, last_seen, is_online)
         VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, 0)",
        params![id, token, claim_code, name, now, now],
    )?;

    let record = AgentRecord {
        id,
        user_id: None,
        token,
        claim_code: Some(claim_code.clone()),
        name: name.to_string(),
        created_at: now,
        last_seen: now,
        is_online: false,
    };

    Ok((record, claim_code))
}

pub fn claim_agent_to_user(conn: &Connection, code: &str, user_id: &str) -> Result<Option<AgentRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, token, claim_code, name, created_at, last_seen, is_online
         FROM agents WHERE claim_code = ?1"
    )?;

    let mut rows = stmt.query(params![code])?;
    if let Some(row) = rows.next()? {
        let agent = AgentRecord {
            id: row.get(0)?,
            user_id: Some(user_id.to_string()),
            token: row.get(2)?,
            claim_code: row.get(3)?,
            name: row.get(4)?,
            created_at: row.get(5)?,
            last_seen: row.get(6)?,
            is_online: row.get::<_, i64>(7)? != 0,
        };

        conn.execute(
            "UPDATE agents SET claim_code = NULL, user_id = ?1 WHERE id = ?2",
            params![user_id, agent.id],
        )?;

        Ok(Some(agent))
    } else {
        Ok(None)
    }
}

pub fn get_agent_by_token(conn: &Connection, token: &str) -> Result<Option<AgentRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, token, claim_code, name, created_at, last_seen, is_online
         FROM agents WHERE token = ?1"
    )?;

    let mut rows = stmt.query(params![token])?;
    if let Some(row) = rows.next()? {
        Ok(Some(AgentRecord {
            id: row.get(0)?,
            user_id: row.get(1)?,
            token: row.get(2)?,
            claim_code: row.get(3)?,
            name: row.get(4)?,
            created_at: row.get(5)?,
            last_seen: row.get(6)?,
            is_online: row.get::<_, i64>(7)? != 0,
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

pub fn list_agents_for_user(conn: &Connection, user_id: &str) -> Result<Vec<AgentRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, token, claim_code, name, created_at, last_seen, is_online
         FROM agents WHERE user_id = ?1 ORDER BY created_at DESC"
    )?;
    let iter = stmt.query_map(params![user_id], |row| {
        Ok(AgentRecord {
            id: row.get(0)?,
            user_id: row.get(1)?,
            token: row.get(2)?,
            claim_code: row.get(3)?,
            name: row.get(4)?,
            created_at: row.get(5)?,
            last_seen: row.get(6)?,
            is_online: row.get::<_, i64>(7)? != 0,
        })
    })?;

    let mut list = Vec::new();
    for a in iter {
        list.push(a?);
    }
    Ok(list)
}

pub fn list_all_agents(conn: &Connection) -> Result<Vec<AgentRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, token, claim_code, name, created_at, last_seen, is_online
         FROM agents ORDER BY created_at DESC"
    )?;
    let iter = stmt.query_map([], |row| {
        Ok(AgentRecord {
            id: row.get(0)?,
            user_id: row.get(1)?,
            token: row.get(2)?,
            claim_code: row.get(3)?,
            name: row.get(4)?,
            created_at: row.get(5)?,
            last_seen: row.get(6)?,
            is_online: row.get::<_, i64>(7)? != 0,
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
    user_id: Option<&str>,
    name: &str,
    local_port: u16,
    protocol: &str,
    public_port: u16,
    subdomain: Option<&str>,
) -> Result<TunnelRecord> {
    let id = Uuid::new_v4().to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;

    conn.execute(
        "INSERT INTO tunnels (id, agent_id, user_id, name, local_port, protocol, public_port, subdomain, enabled, bytes_in, bytes_out, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 0, 0, ?8)",
        params![id, agent_id, user_id, name, local_port, protocol, public_port, subdomain, now],
    )?;

    Ok(TunnelRecord {
        id,
        agent_id: agent_id.to_string(),
        user_id: user_id.map(|s| s.to_string()),
        name: name.to_string(),
        local_port,
        protocol: protocol.to_string(),
        public_port,
        subdomain: subdomain.map(|s| s.to_string()),
        enabled: true,
        bytes_in: 0,
        bytes_out: 0,
        created_at: now,
    })
}

pub fn list_tunnels_for_user(conn: &Connection, user_id: &str) -> Result<Vec<TunnelRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, user_id, name, local_port, protocol, public_port, subdomain, enabled, bytes_in, bytes_out, created_at
         FROM tunnels WHERE user_id = ?1 ORDER BY created_at ASC"
    )?;
    let iter = stmt.query_map(params![user_id], |row| {
        Ok(TunnelRecord {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            user_id: row.get(2)?,
            name: row.get(3)?,
            local_port: row.get(4)?,
            protocol: row.get(5)?,
            public_port: row.get(6)?,
            subdomain: row.get(7)?,
            enabled: row.get::<_, i64>(8)? != 0,
            bytes_in: row.get(9)?,
            bytes_out: row.get(10)?,
            created_at: row.get(11)?,
        })
    })?;

    let mut list = Vec::new();
    for t in iter {
        list.push(t?);
    }
    Ok(list)
}

pub fn list_tunnels_for_agent(conn: &Connection, agent_id: &str) -> Result<Vec<TunnelRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, user_id, name, local_port, protocol, public_port, subdomain, enabled, bytes_in, bytes_out, created_at
         FROM tunnels WHERE agent_id = ?1 ORDER BY created_at ASC"
    )?;
    let iter = stmt.query_map(params![agent_id], |row| {
        Ok(TunnelRecord {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            user_id: row.get(2)?,
            name: row.get(3)?,
            local_port: row.get(4)?,
            protocol: row.get(5)?,
            public_port: row.get(6)?,
            subdomain: row.get(7)?,
            enabled: row.get::<_, i64>(8)? != 0,
            bytes_in: row.get(9)?,
            bytes_out: row.get(10)?,
            created_at: row.get(11)?,
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
        "SELECT id, agent_id, user_id, name, local_port, protocol, public_port, subdomain, enabled, bytes_in, bytes_out, created_at
         FROM tunnels ORDER BY created_at DESC"
    )?;
    let iter = stmt.query_map([], |row| {
        Ok(TunnelRecord {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            user_id: row.get(2)?,
            name: row.get(3)?,
            local_port: row.get(4)?,
            protocol: row.get(5)?,
            public_port: row.get(6)?,
            subdomain: row.get(7)?,
            enabled: row.get::<_, i64>(8)? != 0,
            bytes_in: row.get(9)?,
            bytes_out: row.get(10)?,
            created_at: row.get(11)?,
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
        "SELECT id, agent_id, user_id, name, local_port, protocol, public_port, subdomain, enabled, bytes_in, bytes_out, created_at
         FROM tunnels WHERE id = ?1"
    )?;
    let mut rows = stmt.query(params![id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(TunnelRecord {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            user_id: row.get(2)?,
            name: row.get(3)?,
            local_port: row.get(4)?,
            protocol: row.get(5)?,
            public_port: row.get(6)?,
            subdomain: row.get(7)?,
            enabled: row.get::<_, i64>(8)? != 0,
            bytes_in: row.get(9)?,
            bytes_out: row.get(10)?,
            created_at: row.get(11)?,
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

pub fn add_traffic_stats(conn: &Connection, id: &str, bytes_in: u64, bytes_out: u64) -> Result<()> {
    conn.execute(
        "UPDATE tunnels SET bytes_in = bytes_in + ?1, bytes_out = bytes_out + ?2 WHERE id = ?3",
        params![bytes_in as i64, bytes_out as i64, id],
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
