use std::path::Path;

use serde_json::{Map, Value};

use crate::models::Preset;

pub(crate) const ABSOLUTE_DEST_DIR_ERROR: &str = "destDir must be an absolute path";

pub(crate) fn validate_dest_dir(dest_dir: Option<&str>) -> Result<(), String> {
    if let Some(dir) = dest_dir
        && !Path::new(dir).is_absolute()
    {
        return Err(ABSOLUTE_DEST_DIR_ERROR.to_owned());
    }
    Ok(())
}

/// An absolute destination override, ignoring malformed persisted values.
pub(crate) fn overrides_dest_dir(overrides: &Value) -> Option<String> {
    overrides
        .get("destDir")
        .and_then(Value::as_str)
        .filter(|dir| Path::new(dir).is_absolute())
        .map(str::to_owned)
}

/// A known quality override, ignoring malformed persisted values.
pub(crate) fn overrides_preset(overrides: &Value) -> Option<Preset> {
    overrides
        .get("preset")
        .and_then(|preset| serde_json::from_value(preset.clone()).ok())
}

/// Set or remove the two method/source queue overrides, preserving unrelated keys.
pub(crate) fn with_options(
    overrides: Value,
    dest_dir: Option<&str>,
    preset: Option<Preset>,
) -> Value {
    let mut object = match overrides {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    match dest_dir {
        Some(dir) => object.insert("destDir".to_owned(), Value::String(dir.to_owned())),
        None => object.remove("destDir"),
    };
    match preset.and_then(|preset| serde_json::to_value(preset).ok()) {
        Some(preset) => object.insert("preset".to_owned(), preset),
        None => object.remove("preset"),
    };
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_destinations_use_the_shared_error() {
        assert_eq!(
            validate_dest_dir(Some("relative/folder")),
            Err(ABSOLUTE_DEST_DIR_ERROR.to_owned())
        );
    }
}
