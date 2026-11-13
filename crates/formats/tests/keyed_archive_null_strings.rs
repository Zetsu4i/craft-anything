use craft_anything_formats::keyed_archive::{self, KeyedArchive};
use craft_anything_testkit::apple::{self as fx, P};
use serde_json::json;

fn archives(objects: Vec<P>, root: u64) -> Vec<KeyedArchive> {
    let value = fx::dict(vec![
        ("$archiver", fx::s("NSKeyedArchiver")),
        ("$version", P::Int(100_000)),
        ("$objects", P::Array(objects)),
        ("$top", fx::dict(vec![("root", P::Uid(root))])),
    ]);
    [fx::to_bplist(&value), fx::to_xml(&value).into_bytes()].into_iter().map(|bytes| keyed_archive::parse(&bytes).unwrap()).collect()
}

#[test]
fn stored_null_marker_text_keeps_its_value_through_uids_and_aliases() {
    for archive in archives(vec![fx::s("$null"), fx::s("$null"), P::Array(vec![P::Uid(0), P::Uid(1), fx::s("$null"), P::Uid(3)]), P::Uid(1)], 2) {
        assert_eq!(archive.project(2), json!([null, "$null", "$null", "$null"]));
        assert_eq!(archive.project(1), "$null");
    }
}

#[test]
fn object_rows_reserve_null_kind_for_the_zero_sentinel() {
    for archive in archives(vec![fx::s("$null"), fx::s("$null")], 1) {
        let rows = archive.rows();
        assert_eq!(rows[0].kind, "null");
        assert_eq!(rows[1].kind, "string");
        assert_eq!(archive.rows_page(1, 1)[0].kind, "string");
    }
}

#[test]
fn dictionary_projection_preserves_a_literal_null_marker_key() {
    for archive in archives(
        vec![
            fx::s("$null"),
            fx::s("$null"),
            fx::s("kept"),
            fx::dict(vec![("$class", P::Uid(4)), ("NS.keys", P::Array(vec![P::Uid(1)])), ("NS.objects", P::Array(vec![P::Uid(2)]))]),
            fx::dict(vec![("$classname", fx::s("NSDictionary"))]),
        ],
        3,
    ) {
        let projected = archive.project(3);
        assert_eq!(projected["entries"], json!({"$null": "kept"}));
    }
}

#[test]
fn the_zero_sentinel_and_unrelated_strings_keep_their_existing_meaning() {
    for archive in archives(vec![fx::s("$null"), fx::s("ordinary")], 0) {
        assert_eq!(archive.project(0), json!(null));
        assert_eq!(archive.project(1), "ordinary");
    }
    for archive in archives(vec![fx::s("ordinary")], 0) {
        assert_eq!(archive.project(0), "ordinary");
        assert_eq!(archive.rows()[0].kind, "string");
    }
}
