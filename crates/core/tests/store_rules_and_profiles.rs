//! Basic CRUD coverage for rules and site profiles (not explicitly listed in the
//! acceptance criteria, but part of the public API surface).

use vids_core::store::Store;

#[test]
fn rule_crud_roundtrip() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .add_rule(
            "TED talks",
            "https://www.ted.com/talks*",
            serde_json::json!({"fetch": "static"}),
            100,
        )
        .unwrap();

    let rule = store.get_rule(id).unwrap();
    assert_eq!(rule.name, "TED talks");
    assert_eq!(rule.match_pattern, "https://www.ted.com/talks*");
    assert_eq!(rule.rule_json, serde_json::json!({"fetch": "static"}));
    assert_eq!(rule.options, serde_json::json!({}));
    assert_eq!(rule.last_used_at, None);
    assert_eq!(rule.created_at, 100);
    assert_eq!(rule.updated_at, 100);

    store
        .update_rule(
            id,
            "TED talks v2",
            "https://www.ted.com/talks/*",
            serde_json::json!({"fetch": "rendered"}),
            200,
        )
        .unwrap();
    let updated = store.get_rule(id).unwrap();
    assert_eq!(updated.name, "TED talks v2");
    assert_eq!(updated.created_at, 100);
    assert_eq!(updated.updated_at, 200);
    assert_eq!(updated.rule_json, serde_json::json!({"fetch": "rendered"}));

    store
        .set_rule_options(id, serde_json::json!({"destDir": "/videos"}))
        .unwrap();
    store.set_rule_last_used(id, 250).unwrap();
    store.rename_rule(id, "Renamed", 300).unwrap();
    let changed = store.get_rule(id).unwrap();
    assert_eq!(changed.name, "Renamed");
    assert_eq!(changed.rule_json["name"], "Renamed");
    assert_eq!(changed.options, serde_json::json!({"destDir": "/videos"}));
    assert_eq!(changed.last_used_at, Some(250));

    assert_eq!(store.list_rules().unwrap().len(), 1);

    store.delete_rule(id).unwrap();
    assert!(store.get_rule(id).is_err());
    assert!(store.delete_rule(id).is_ok());
    assert!(
        store
            .update_rule(id, "x", "y", serde_json::json!({}), 300)
            .is_err()
    );
}

#[test]
fn site_profile_upsert_and_crud() {
    let store = Store::open_in_memory().unwrap();
    let id1 = store
        .upsert_site_profile("youtube", serde_json::json!({"login": false}), 100)
        .unwrap();
    let loaded = store.get_site_profile("youtube").unwrap().unwrap();
    assert_eq!(loaded.id, id1);
    assert_eq!(loaded.profile_json, serde_json::json!({"login": false}));
    assert_eq!(loaded.updated_at, 100);

    let id2 = store
        .upsert_site_profile("youtube", serde_json::json!({"login": true}), 200)
        .unwrap();
    assert_eq!(id1, id2);
    let loaded2 = store.get_site_profile("youtube").unwrap().unwrap();
    assert_eq!(loaded2.profile_json, serde_json::json!({"login": true}));
    assert_eq!(loaded2.updated_at, 200);

    assert_eq!(store.list_site_profiles().unwrap().len(), 1);

    store.delete_site_profile("youtube").unwrap();
    assert!(store.get_site_profile("youtube").unwrap().is_none());
    assert!(store.delete_site_profile("youtube").is_ok());
}

#[test]
fn get_site_profile_for_unknown_name_is_none_not_an_error() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.get_site_profile("unknown").unwrap().is_none());
}
