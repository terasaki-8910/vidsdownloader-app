//! Recorded browser flows, found by their exact starting host and optionally
//! associated with a registered site for Settings.

use rusqlite::{Row, params};
use serde_json::Value;

use super::{Store, StoreError, require_row};

#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    pub id: i64,
    pub site_id: Option<i64>,
    pub host: String,
    pub name: String,
    pub start_url: String,
    pub steps_json: Value,
    pub scopes_json: Value,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewFlow {
    pub site_id: Option<i64>,
    pub host: String,
    pub name: String,
    pub start_url: String,
    pub steps_json: Value,
    pub scopes_json: Value,
}

const FLOW_COLUMNS: &str =
    "id, site_id, host, name, start_url, steps_json, scopes_json, created_at, updated_at";

struct RawFlow {
    id: i64,
    site_id: Option<i64>,
    host: String,
    name: String,
    start_url: String,
    steps_json: String,
    scopes_json: String,
    created_at: i64,
    updated_at: i64,
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<RawFlow> {
    Ok(RawFlow {
        id: row.get("id")?,
        site_id: row.get("site_id")?,
        host: row.get("host")?,
        name: row.get("name")?,
        start_url: row.get("start_url")?,
        steps_json: row.get("steps_json")?,
        scopes_json: row.get("scopes_json")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

impl RawFlow {
    fn into_flow(self) -> Result<Flow, StoreError> {
        Ok(Flow {
            id: self.id,
            site_id: self.site_id,
            host: self.host,
            name: self.name,
            start_url: self.start_url,
            steps_json: serde_json::from_str(&self.steps_json)?,
            scopes_json: serde_json::from_str(&self.scopes_json)?,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

impl Store {
    pub fn insert_flow(&self, flow: NewFlow, now: i64) -> Result<i64, StoreError> {
        let steps_json = serde_json::to_string(&flow.steps_json)?;
        let scopes_json = serde_json::to_string(&flow.scopes_json)?;
        self.conn.execute(
            "INSERT INTO flows (site_id, host, name, start_url, steps_json, scopes_json, \
             created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                flow.site_id,
                flow.host,
                flow.name,
                flow.start_url,
                steps_json,
                scopes_json,
                now
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_flows_by_host(&self, host: &str) -> Result<Vec<Flow>, StoreError> {
        self.list_flows("host = ?1", params![host])
    }

    pub fn list_flows_by_site(&self, site_id: i64) -> Result<Vec<Flow>, StoreError> {
        self.list_flows("site_id = ?1", params![site_id])
    }

    fn list_flows<P>(&self, predicate: &str, params: P) -> Result<Vec<Flow>, StoreError>
    where
        P: rusqlite::Params,
    {
        let sql = format!(
            "SELECT {FLOW_COLUMNS} FROM flows WHERE {predicate} ORDER BY updated_at DESC, id DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params, map_row)?;
        let mut flows = Vec::new();
        for row in rows {
            flows.push(row?.into_flow()?);
        }
        Ok(flows)
    }

    pub fn count_flows_by_site(&self, site_id: i64) -> Result<usize, StoreError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM flows WHERE site_id = ?1",
            params![site_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    pub fn rename_flow(&self, id: i64, name: &str, now: i64) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE flows SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, now, id],
        )?;
        require_row(changed, "flow", id)
    }

    pub fn delete_flow(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM flows WHERE id = ?1", params![id])?;
        Ok(())
    }
}
