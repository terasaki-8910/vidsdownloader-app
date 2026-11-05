//! Checks the no-login niconico comment route against the live site, which the unit tests
//! cannot do. Run it by hand when comments stop arriving, to tell a site change from an
//! app bug:
//!
//! ```text
//! cargo run -p vids-core --example nico_guest_live -- sm9 sm500873
//! ```

fn main() {
    let ids: Vec<String> = std::env::args().skip(1).collect();
    let ids = if ids.is_empty() {
        vec!["sm9".to_owned()]
    } else {
        ids
    };
    for id in ids {
        match vids_core::niconico_guest::fetch_comments_json(&id) {
            Ok(json) => match serde_json::from_str::<Vec<serde_json::Value>>(&json) {
                Ok(list) => println!("{id}: {} comments, {} bytes", list.len(), json.len()),
                Err(error) => println!("{id}: unreadable answer: {error}"),
            },
            Err(error) => println!("{id}: failed: {error}"),
        }
    }
}
