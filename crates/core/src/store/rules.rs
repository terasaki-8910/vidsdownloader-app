//! Saved collecting rules (URL match pattern plus rule data), referenced by
//! sources whose kind is [`super::SourceKind::Rule`].

use rusqlite::{OptionalExtension, Row, params};
use serde_json::Value;

use super::{Store, StoreError, require_row};

/// A row from `rules`. `rule_json` holds the rule itself (see SPEC's "Rules (draft
/// format)"); the store persists it as opaque JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: i64,
    pub name: String,
    pub match_pattern: String,
    pub rule_json: Value,
    pub options: Value,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

const RULE_COLUMNS: &str =
    "id, name, match_pattern, rule_json, options_json, last_used_at, created_at, updated_at";

struct RawRule {
    id: i64,
    name: String,
    match_pattern: String,
    rule_json: String,
    options_json: String,
    last_used_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<RawRule> {
    Ok(RawRule {
        id: row.get("id")?,
        name: row.get("name")?,
        match_pattern: row.get("match_pattern")?,
        rule_json: row.get("rule_json")?,
        options_json: row.get("options_json")?,
        last_used_at: row.get("last_used_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

impl RawRule {
    fn into_rule(self) -> Result<Rule, StoreError> {
        Ok(Rule {
            id: self.id,
            name: self.name,
            match_pattern: self.match_pattern,
            rule_json: serde_json::from_str(&self.rule_json)?,
            options: serde_json::from_str(&self.options_json)?,
            last_used_at: self.last_used_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

impl Store {
    /// Save a new rule.
    pub fn add_rule(
        &self,
        name: &str,
        match_pattern: &str,
        rule_json: Value,
        now: i64,
    ) -> Result<i64, StoreError> {
        let rule_json = serde_json::to_string(&rule_json)?;
        self.conn.execute(
            "INSERT INTO rules (name, match_pattern, rule_json, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![name, match_pattern, rule_json, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Replace a rule's name, match pattern and data, and touch `updated_at`.
    pub fn update_rule(
        &self,
        id: i64,
        name: &str,
        match_pattern: &str,
        rule_json: Value,
        now: i64,
    ) -> Result<(), StoreError> {
        let rule_json = serde_json::to_string(&rule_json)?;
        let changed = self.conn.execute(
            "UPDATE rules SET name = ?1, match_pattern = ?2, rule_json = ?3, \
             updated_at = ?4 WHERE id = ?5",
            params![name, match_pattern, rule_json, now, id],
        )?;
        require_row(changed, "rule", id)
    }

    /// Fetch one rule by id.
    pub fn get_rule(&self, id: i64) -> Result<Rule, StoreError> {
        let sql = format!("SELECT {RULE_COLUMNS} FROM rules WHERE id = ?1");
        let raw = self
            .conn
            .query_row(&sql, params![id], map_row)
            .optional()?
            .ok_or(StoreError::NotFound { entity: "rule", id })?;
        raw.into_rule()
    }

    /// List every rule, ordered by id.
    pub fn list_rules(&self) -> Result<Vec<Rule>, StoreError> {
        let sql = format!("SELECT {RULE_COLUMNS} FROM rules ORDER BY id ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], map_row)?;
        let mut rules = Vec::new();
        for row in rows {
            rules.push(row?.into_rule()?);
        }
        Ok(rules)
    }

    /// Replace a rule's queue options.
    pub fn set_rule_options(&self, id: i64, options: Value) -> Result<(), StoreError> {
        let options_json = serde_json::to_string(&options)?;
        let changed = self.conn.execute(
            "UPDATE rules SET options_json = ?1 WHERE id = ?2",
            params![options_json, id],
        )?;
        require_row(changed, "rule", id)
    }

    /// Record the last explicit use of a rule.
    pub fn set_rule_last_used(&self, id: i64, now: i64) -> Result<(), StoreError> {
        let changed = self.conn.execute(
            "UPDATE rules SET last_used_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        require_row(changed, "rule", id)
    }

    /// Rename both the indexed column and the copy embedded in the rule JSON.
    pub fn rename_rule(&self, id: i64, name: &str, now: i64) -> Result<(), StoreError> {
        let mut rule = self.get_rule(id)?;
        if let Some(object) = rule.rule_json.as_object_mut() {
            object.insert("name".to_owned(), Value::String(name.to_owned()));
        }
        let rule_json = serde_json::to_string(&rule.rule_json)?;
        let changed = self.conn.execute(
            "UPDATE rules SET name = ?1, rule_json = ?2, updated_at = ?3 WHERE id = ?4",
            params![name, rule_json, now, id],
        )?;
        require_row(changed, "rule", id)
    }

    /// Delete a rule. A no-op if it does not exist. Sources referencing it keep
    /// their row with `rule_id` set to `NULL` (`ON DELETE SET NULL`).
    pub fn delete_rule(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM rules WHERE id = ?1", params![id])?;
        Ok(())
    }
}
