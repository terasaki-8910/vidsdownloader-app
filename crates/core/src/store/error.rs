//! Errors produced by any [`super::Store`] operation.

/// Failures from any [`super::Store`] operation.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A SQLite error, including constraint violations (unique, foreign key, check).
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// A JSON column failed to parse, or a value failed to serialize to JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A TEXT column held a value that is not one of the expected enum variants.
    #[error("unknown {type_name} value: {value:?}")]
    InvalidEnumValue {
        type_name: &'static str,
        value: String,
    },
    /// No row exists for the given id.
    #[error("{entity} {id} not found")]
    NotFound { entity: &'static str, id: i64 },
    /// The on-disk schema version is newer than this build of the store understands.
    #[error("database schema version {0} is newer than supported")]
    UnsupportedSchemaVersion(i64),
    /// A job tied to a library item has not finished (Done, Failed or Canceled), so the
    /// item's records were left as they are.
    #[error("job {id} has not finished ({state:?}); cancel it or wait until it ends")]
    JobNotFinished {
        id: i64,
        state: crate::types::JobState,
    },
}
