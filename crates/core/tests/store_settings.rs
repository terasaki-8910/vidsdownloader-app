//! Settings round-trip (including a struct), overwrite, and idempotent delete.

use serde::{Deserialize, Serialize};
use vids_core::store::Store;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Prefs {
    theme: String,
    concurrency: u32,
}

#[test]
fn settings_roundtrip_struct() {
    let store = Store::open_in_memory().unwrap();
    let prefs = Prefs {
        theme: "dark".to_owned(),
        concurrency: 3,
    };
    store.set_setting("prefs", &prefs).unwrap();

    let loaded: Option<Prefs> = store.get_setting("prefs").unwrap();
    assert_eq!(loaded, Some(prefs));
}

#[test]
fn get_setting_on_missing_key_returns_none() {
    let store = Store::open_in_memory().unwrap();
    let loaded: Option<Prefs> = store.get_setting("missing").unwrap();
    assert_eq!(loaded, None);
}

#[test]
fn set_setting_overwrites_the_previous_value() {
    let store = Store::open_in_memory().unwrap();
    store.set_setting("count", &1u32).unwrap();
    store.set_setting("count", &2u32).unwrap();
    let loaded: Option<u32> = store.get_setting("count").unwrap();
    assert_eq!(loaded, Some(2));
}

#[test]
fn delete_setting_removes_the_value_and_is_idempotent() {
    let store = Store::open_in_memory().unwrap();
    store.set_setting("x", &"y".to_owned()).unwrap();
    store.delete_setting("x").unwrap();
    let loaded: Option<String> = store.get_setting("x").unwrap();
    assert_eq!(loaded, None);
    assert!(store.delete_setting("x").is_ok());
}
