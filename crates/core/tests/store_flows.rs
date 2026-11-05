use vids_core::store::{NewFlow, Store};

fn sample_flow(host: &str, name: &str) -> NewFlow {
    NewFlow {
        site_id: None,
        host: host.to_owned(),
        name: name.to_owned(),
        start_url: format!("https://{host}/videos"),
        steps_json: serde_json::json!([{"label": "Play", "pattern": "button.play"}]),
        scopes_json: serde_json::json!(["this"]),
    }
}

#[test]
fn flow_insert_list_rename_and_delete() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .insert_flow(sample_flow("example.com", "Play"), 100)
        .unwrap();

    let flows = store.list_flows_by_host("example.com").unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].id, id);
    assert_eq!(flows[0].name, "Play");
    assert_eq!(flows[0].host, "example.com");
    assert_eq!(flows[0].created_at, 100);
    assert_eq!(flows[0].updated_at, 100);
    assert_eq!(flows[0].scopes_json, serde_json::json!(["this"]));
    assert!(
        store
            .list_flows_by_host("other.example")
            .unwrap()
            .is_empty()
    );

    store.rename_flow(id, "Open and play", 200).unwrap();
    let renamed = store.list_flows_by_host("example.com").unwrap();
    assert_eq!(renamed[0].name, "Open and play");
    assert_eq!(renamed[0].created_at, 100);
    assert_eq!(renamed[0].updated_at, 200);

    store.delete_flow(id).unwrap();
    assert!(store.list_flows_by_host("example.com").unwrap().is_empty());
    assert!(store.delete_flow(id).is_ok());
}
