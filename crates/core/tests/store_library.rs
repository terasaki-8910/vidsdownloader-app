//! Library add/search/update behavior: Japanese and ASCII text search, site and
//! date filters, LIKE wildcard escaping, missing-item handling, and ordering.

use vids_core::store::{LibraryQuery, NewLibraryItem, Store};

fn sample_item(path: &str) -> NewLibraryItem {
    NewLibraryItem {
        job_id: None,
        site: None,
        video_id: None,
        url: None,
        title: None,
        uploader: None,
        published_at: None,
        duration_s: None,
        path: path.to_owned(),
        comments_path: None,
        size_bytes: None,
    }
}

fn base_query() -> LibraryQuery {
    LibraryQuery {
        text: None,
        site: None,
        published_from: None,
        published_to: None,
        include_missing: false,
        limit: 50,
        offset: 0,
    }
}

#[test]
fn search_matches_japanese_substring_in_title() {
    let store = Store::open_in_memory().unwrap();
    let mut a = sample_item("/lib/a.mp4");
    a.title = Some("新・豪血寺一族 -煩悩解放- レッツゴー！陰陽師".to_owned());
    store.add_library_item(a, 1).unwrap();
    let mut b = sample_item("/lib/b.mp4");
    b.title = Some("別の動画".to_owned());
    store.add_library_item(b, 2).unwrap();

    let mut query = base_query();
    query.text = Some("陰陽師".to_owned());
    let results = store.search_library(&query).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].path, "/lib/a.mp4");
}

#[test]
fn search_text_is_ascii_case_insensitive() {
    let store = Store::open_in_memory().unwrap();
    let mut item = sample_item("/lib/rust.mp4");
    item.title = Some("Rust Tutorial".to_owned());
    let id = store.add_library_item(item, 1).unwrap();

    let mut query = base_query();
    query.text = Some("RUST".to_owned());
    assert_eq!(ids_of(&store, &query), vec![id]);

    query.text = Some("rust".to_owned());
    assert_eq!(ids_of(&store, &query), vec![id]);

    query.text = Some("tutorial".to_owned());
    assert_eq!(ids_of(&store, &query), vec![id]);
}

#[test]
fn search_text_matches_uploader_too() {
    let store = Store::open_in_memory().unwrap();
    let mut item = sample_item("/lib/u.mp4");
    item.title = Some("Untitled".to_owned());
    item.uploader = Some("Example Channel".to_owned());
    let id = store.add_library_item(item, 1).unwrap();

    let mut query = base_query();
    query.text = Some("channel".to_owned());
    assert_eq!(ids_of(&store, &query), vec![id]);
}

#[test]
fn search_escapes_like_wildcards_in_text() {
    let store = Store::open_in_memory().unwrap();
    let mut underscore_literal = sample_item("/lib/underscore_literal.mp4");
    underscore_literal.title = Some("xx a_b yy".to_owned());
    let id_underscore = store.add_library_item(underscore_literal, 1).unwrap();

    let mut underscore_wildcard = sample_item("/lib/underscore_wildcard.mp4");
    underscore_wildcard.title = Some("xx aXb yy".to_owned());
    store.add_library_item(underscore_wildcard, 2).unwrap();

    let mut percent_literal = sample_item("/lib/percent_literal.mp4");
    percent_literal.title = Some("xx c%d yy".to_owned());
    let id_percent = store.add_library_item(percent_literal, 3).unwrap();

    let mut percent_wildcard = sample_item("/lib/percent_wildcard.mp4");
    percent_wildcard.title = Some("xx cXXXd yy".to_owned());
    store.add_library_item(percent_wildcard, 4).unwrap();

    let mut query = base_query();
    query.text = Some("a_b".to_owned());
    assert_eq!(ids_of(&store, &query), vec![id_underscore]);

    query.text = Some("c%d".to_owned());
    assert_eq!(ids_of(&store, &query), vec![id_percent]);
}

#[test]
fn search_filters_by_site_and_inclusive_date_range() {
    let store = Store::open_in_memory().unwrap();
    let mut a = sample_item("/lib/a.mp4");
    a.site = Some("youtube".to_owned());
    a.published_at = Some(1_000);
    let id_a = store.add_library_item(a, 1).unwrap();

    let mut b = sample_item("/lib/b.mp4");
    b.site = Some("youtube".to_owned());
    b.published_at = Some(2_000);
    let id_b = store.add_library_item(b, 2).unwrap();

    let mut c = sample_item("/lib/c.mp4");
    c.site = Some("niconico".to_owned());
    c.published_at = Some(1_500);
    store.add_library_item(c, 3).unwrap();

    let mut query = base_query();
    query.site = Some("youtube".to_owned());
    query.published_from = Some(1_000);
    query.published_to = Some(1_000);
    assert_eq!(ids_of(&store, &query), vec![id_a]);

    let mut query = base_query();
    query.site = Some("youtube".to_owned());
    query.published_from = Some(1_000);
    query.published_to = Some(2_000);
    let mut ids = ids_of(&store, &query);
    ids.sort_unstable();
    let mut expected = vec![id_a, id_b];
    expected.sort_unstable();
    assert_eq!(ids, expected);
}

#[test]
fn search_excludes_missing_unless_asked() {
    let store = Store::open_in_memory().unwrap();
    let id_present = store
        .add_library_item(sample_item("/lib/present.mp4"), 1)
        .unwrap();
    let id_missing = store
        .add_library_item(sample_item("/lib/missing.mp4"), 2)
        .unwrap();
    store.set_missing(id_missing, true).unwrap();

    let mut query = base_query();
    assert_eq!(ids_of(&store, &query), vec![id_present]);

    query.include_missing = true;
    let mut ids = ids_of(&store, &query);
    ids.sort_unstable();
    let mut expected = vec![id_present, id_missing];
    expected.sort_unstable();
    assert_eq!(ids, expected);
}

#[test]
fn search_orders_by_added_at_desc_then_id_desc() {
    let store = Store::open_in_memory().unwrap();
    let id1 = store
        .add_library_item(sample_item("/lib/1.mp4"), 1_000)
        .unwrap();
    let id2 = store
        .add_library_item(sample_item("/lib/2.mp4"), 1_000)
        .unwrap();
    let id3 = store
        .add_library_item(sample_item("/lib/3.mp4"), 2_000)
        .unwrap();

    let ids = ids_of(&store, &base_query());
    assert_eq!(ids, vec![id3, id2, id1]);
}

#[test]
fn search_respects_limit_and_offset() {
    let store = Store::open_in_memory().unwrap();
    let ids: Vec<i64> = (0..5i64)
        .map(|index| {
            store
                .add_library_item(sample_item(&format!("/lib/{index}.mp4")), 1_000 + index)
                .unwrap()
        })
        .collect();

    let mut query = base_query();
    query.limit = 2;
    assert_eq!(ids_of(&store, &query), vec![ids[4], ids[3]]);

    query.offset = 2;
    assert_eq!(ids_of(&store, &query), vec![ids[2], ids[1]]);
}

#[test]
fn set_comments_path_sets_and_clears() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .add_library_item(sample_item("/lib/a.mp4"), 1)
        .unwrap();

    store.set_comments_path(id, Some("/lib/a.ass")).unwrap();
    assert_eq!(
        store.get_library_item(id).unwrap().comments_path,
        Some("/lib/a.ass".to_owned())
    );

    store.set_comments_path(id, None).unwrap();
    assert_eq!(store.get_library_item(id).unwrap().comments_path, None);
}

#[test]
fn delete_library_item_removes_the_row() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .add_library_item(sample_item("/lib/a.mp4"), 1)
        .unwrap();
    store.delete_library_item(id).unwrap();
    assert!(store.get_library_item(id).is_err());
    assert!(store.delete_library_item(id).is_ok());
}

fn video_item(path: &str, site: &str, video_id: &str) -> NewLibraryItem {
    let mut item = sample_item(path);
    item.site = Some(site.to_owned());
    item.video_id = Some(video_id.to_owned());
    item
}

#[test]
fn latest_library_item_returns_the_newest_row_for_the_pair() {
    let store = Store::open_in_memory().unwrap();
    store
        .add_library_item(video_item("/lib/old.mp4", "Youtube", "abc123"), 1_000)
        .unwrap();
    let newest = store
        .add_library_item(video_item("/lib/new.mp4", "Youtube", "abc123"), 2_000)
        .unwrap();
    store
        .add_library_item(video_item("/lib/other-id.mp4", "Youtube", "zzz999"), 3_000)
        .unwrap();
    store
        .add_library_item(
            video_item("/lib/other-site.mp4", "Niconico", "abc123"),
            4_000,
        )
        .unwrap();

    let item = store
        .latest_library_item("Youtube", "abc123")
        .unwrap()
        .expect("the pair is in the library");
    assert_eq!(item.id, newest);
    assert_eq!(item.path, "/lib/new.mp4");
    assert_eq!(item.added_at, 2_000);
    assert_eq!(store.latest_library_item("Youtube", "nope").unwrap(), None);
    assert_eq!(store.latest_library_item("Twitch", "abc123").unwrap(), None);
}

#[test]
fn latest_library_item_breaks_a_tie_by_id_and_keeps_missing_rows() {
    let store = Store::open_in_memory().unwrap();
    store
        .add_library_item(video_item("/lib/1.mp4", "Youtube", "abc123"), 1_000)
        .unwrap();
    let second = store
        .add_library_item(video_item("/lib/2.mp4", "Youtube", "abc123"), 1_000)
        .unwrap();
    assert_eq!(
        store
            .latest_library_item("Youtube", "abc123")
            .unwrap()
            .unwrap()
            .id,
        second
    );

    store.set_missing(second, true).unwrap();
    let item = store
        .latest_library_item("Youtube", "abc123")
        .unwrap()
        .expect("a missing row is still returned");
    assert_eq!(item.id, second);
    assert!(item.missing);
}

#[test]
fn get_library_item_on_missing_id_errors() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.get_library_item(999).is_err());
}

#[test]
fn add_library_item_rejects_duplicate_path() {
    let store = Store::open_in_memory().unwrap();
    store
        .add_library_item(sample_item("/lib/a.mp4"), 1)
        .unwrap();
    assert!(
        store
            .add_library_item(sample_item("/lib/a.mp4"), 2)
            .is_err()
    );
}

fn ids_of(store: &Store, query: &LibraryQuery) -> Vec<i64> {
    store
        .search_library(query)
        .unwrap()
        .iter()
        .map(|item| item.id)
        .collect()
}
