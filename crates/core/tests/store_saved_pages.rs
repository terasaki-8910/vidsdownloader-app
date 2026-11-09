//! Saved pages: add (stored as shown, deduplicated), list order, rename, remove and validation.

use vids_core::store::{MAX_SAVED_NAME_CHARS, MAX_SAVED_URL_CHARS, Store, StoreError};

const SEARCH_URL: &str = "https://x.com/search?q=%E7%8C%AB&src=typed_query&f=media";

fn invalid(result: Result<impl std::fmt::Debug, StoreError>) -> String {
    match result {
        Err(StoreError::Invalid(message)) => message,
        other => panic!("expected an Invalid error, got {other:?}"),
    }
}

#[test]
fn add_stores_the_url_as_shown_and_trims_the_name() {
    let store = Store::open_in_memory().unwrap();
    let page = store
        .add_saved_page(SEARCH_URL, "  猫 · メディア \n", 100)
        .unwrap();

    assert_eq!(page.url, SEARCH_URL);
    assert_eq!(page.name, "猫 · メディア");
    assert_eq!(page.created_at, 100);
    assert_eq!(store.list_saved_pages().unwrap(), vec![page]);
}

#[test]
fn add_keeps_the_query_and_drops_only_the_fragment() {
    let store = Store::open_in_memory().unwrap();
    let page = store
        .add_saved_page(
            "https://www.youtube.com/@Blender/videos?view=0&sort=dd#top",
            "Blender",
            1,
        )
        .unwrap();
    assert_eq!(
        page.url,
        "https://www.youtube.com/@Blender/videos?view=0&sort=dd"
    );

    // Not re-encoded or normalized: the case and the empty path stay as shown.
    let page = store
        .add_saved_page("HTTPS://Example.COM?a=b%20c", "x", 2)
        .unwrap();
    assert_eq!(page.url, "HTTPS://Example.COM?a=b%20c");
}

#[test]
fn adding_a_saved_url_returns_the_existing_row() {
    let store = Store::open_in_memory().unwrap();
    let first = store.add_saved_page(SEARCH_URL, "First name", 100).unwrap();
    let again = store
        .add_saved_page(&format!("{SEARCH_URL}#later"), "Another name", 200)
        .unwrap();

    assert_eq!(again, first);
    assert_eq!(store.list_saved_pages().unwrap().len(), 1);

    // The same page with another query is another page.
    let other = store
        .add_saved_page(
            "https://x.com/search?q=dog&src=typed_query&f=media",
            "Dog",
            300,
        )
        .unwrap();
    assert_ne!(other.id, first.id);
    assert_eq!(store.list_saved_pages().unwrap().len(), 2);
}

#[test]
fn list_is_in_the_order_pages_were_saved() {
    let store = Store::open_in_memory().unwrap();
    store
        .add_saved_page("https://a.example/1", "one", 10)
        .unwrap();
    store
        .add_saved_page("https://a.example/2", "two", 10)
        .unwrap();
    store
        .add_saved_page("https://a.example/0", "zero", 5)
        .unwrap();

    let names: Vec<String> = store
        .list_saved_pages()
        .unwrap()
        .into_iter()
        .map(|page| page.name)
        .collect();
    assert_eq!(names, ["zero", "one", "two"]);
}

#[test]
fn rename_trims_and_returns_the_updated_row() {
    let store = Store::open_in_memory().unwrap();
    let page = store.add_saved_page(SEARCH_URL, "Old", 100).unwrap();

    let renamed = store.rename_saved_page(page.id, "  New name  ").unwrap();
    assert_eq!(renamed.name, "New name");
    assert_eq!(renamed.url, SEARCH_URL);
    assert_eq!(renamed.created_at, 100);
    assert_eq!(store.list_saved_pages().unwrap(), vec![renamed]);

    let missing = store.rename_saved_page(999, "x");
    assert!(matches!(missing, Err(StoreError::NotFound { id: 999, .. })));
}

#[test]
fn remove_deletes_the_row_and_a_second_remove_is_a_no_op() {
    let store = Store::open_in_memory().unwrap();
    let keep = store
        .add_saved_page("https://a.example/keep", "keep", 1)
        .unwrap();
    let gone = store
        .add_saved_page("https://a.example/gone", "gone", 2)
        .unwrap();

    store.delete_saved_page(gone.id).unwrap();
    assert_eq!(store.list_saved_pages().unwrap(), vec![keep]);
    store.delete_saved_page(gone.id).unwrap();

    // The URL can be saved again once it was removed.
    let again = store
        .add_saved_page("https://a.example/gone", "gone", 3)
        .unwrap();
    assert_eq!(again.created_at, 3);
}

#[test]
fn urls_that_are_not_http_pages_are_refused() {
    let store = Store::open_in_memory().unwrap();
    for url in [
        "",
        "   ",
        "example.com/page",
        "ftp://example.com/file",
        "file:///C:/video.mp4",
        "javascript:alert(1)",
        "https://",
        "https://exa mple.com/",
        "https://example.com/a\nb",
        "#only-a-fragment",
    ] {
        invalid(store.add_saved_page(url, "name", 1));
    }
    assert!(store.list_saved_pages().unwrap().is_empty());
}

#[test]
fn the_url_length_limit_counts_characters_without_the_fragment() {
    let store = Store::open_in_memory().unwrap();
    let prefix = "https://example.com/";
    let at_limit = format!(
        "{prefix}{}",
        "あ".repeat(MAX_SAVED_URL_CHARS - prefix.chars().count())
    );
    assert_eq!(at_limit.chars().count(), MAX_SAVED_URL_CHARS);
    store.add_saved_page(&at_limit, "limit", 1).unwrap();

    let over = format!("{at_limit}a");
    let message = invalid(store.add_saved_page(&over, "over", 2));
    assert!(message.contains("2048"), "{message}");

    // A long fragment is dropped before the length is checked.
    let fragment = format!("https://example.com/short#{}", "f".repeat(5000));
    assert_eq!(
        store.add_saved_page(&fragment, "short", 3).unwrap().url,
        "https://example.com/short"
    );
}

#[test]
fn names_must_be_one_to_two_hundred_characters_after_trimming() {
    let store = Store::open_in_memory().unwrap();
    invalid(store.add_saved_page("https://a.example/", "", 1));
    invalid(store.add_saved_page("https://a.example/", " \t\u{3000}", 1));

    let longest = "名".repeat(MAX_SAVED_NAME_CHARS);
    let page = store
        .add_saved_page("https://a.example/", &format!(" {longest} "), 1)
        .unwrap();
    assert_eq!(page.name, longest);

    let too_long = "名".repeat(MAX_SAVED_NAME_CHARS + 1);
    let message = invalid(store.rename_saved_page(page.id, &too_long));
    assert!(message.contains("200"), "{message}");
    invalid(store.rename_saved_page(page.id, "   "));
    assert_eq!(store.list_saved_pages().unwrap()[0].name, longest);
    invalid(store.add_saved_page("https://b.example/", &too_long, 2));
}
