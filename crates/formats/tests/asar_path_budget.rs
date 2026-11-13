use craft_anything_formats::asar::{self, AsarKind};
use serde_json::{Value, json};

const PATH_BUDGET: usize = 8 * 1024 * 1024;

fn archive(files: Value) -> Vec<u8> {
    let json = serde_json::to_vec(&json!({ "files": files })).unwrap();
    let padding = (4 - json.len() % 4) % 4;
    let payload = json.len() + 4 + padding;
    let mut bytes = 4u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&u32::try_from(payload + 4).unwrap().to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(payload).unwrap().to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(json.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&json);
    bytes.extend(std::iter::repeat_n(0, padding));
    bytes.push(b'x');
    bytes
}

fn shared_prefix(rejected: bool) -> (Vec<u8>, String) {
    let mut files: serde_json::Map<String, Value> = (0..4_000)
        .map(|index| {
            let name = if rejected { format!("bad/{index:05}") } else { format!("file-{index:05}") };
            (name, json!({ "size": 1, "offset": "0" }))
        })
        .collect();
    let mut prefix = String::new();
    for index in 0..15 {
        let name = format!("{index:02}{}", "d".repeat(238));
        prefix = format!("{name}/{prefix}");
        files = serde_json::Map::from_iter([(name, json!({ "files": files }))]);
    }
    (archive(Value::Object(files)), prefix)
}

#[test]
fn shared_directory_prefixes_cannot_amplify_retained_paths_without_a_byte_limit() {
    let (bytes, prefix) = shared_prefix(false);
    assert!(bytes.len() < 200_000);
    let listing = asar::parse(&bytes, 200_000).unwrap();
    assert!(listing.truncated);
    assert!(listing.entries.len() > 15);
    assert!(listing.entries.len() < 4_015);
    assert!(listing.entries.iter().map(|entry| entry.path.len()).sum::<usize>() <= PATH_BUDGET);
    for entry in listing.entries.iter().filter(|entry| entry.kind == AsarKind::File) {
        let leaf = entry.path.strip_prefix(&prefix).unwrap();
        assert_eq!(leaf.len(), "file-00000".len());
        assert!(leaf.starts_with("file-"));
        assert_eq!(asar::file_bytes(&bytes, &listing, entry), Some(&b"x"[..]));
    }
}

#[test]
fn rejected_names_share_the_expanded_path_budget() {
    let (bytes, prefix) = shared_prefix(true);
    let listing = asar::parse(&bytes, 200_000).unwrap();
    assert!(listing.truncated);
    assert_eq!(listing.entries.len(), 15);
    assert!(!listing.rejected_names.is_empty());
    assert!(listing.rejected_names.len() < 4_000);
    let kept = listing.entries.iter().map(|entry| entry.path.len()).sum::<usize>() + listing.rejected_names.iter().map(String::len).sum::<usize>();
    assert!(kept <= PATH_BUDGET);
    assert!(listing.rejected_names.iter().all(|name| name.starts_with(&prefix) && name.ends_with(|c: char| c.is_ascii_digit())));
}

#[test]
fn a_single_path_over_the_budget_is_omitted_whole() {
    let name = "é".repeat(PATH_BUDGET / 2 + 1);
    let bytes = archive(json!({ name: { "size": 1, "offset": "0" } }));
    let listing = asar::parse(&bytes, 200_000).unwrap();
    assert!(listing.truncated);
    assert!(listing.entries.is_empty());
    assert!(listing.rejected_names.is_empty());
}

#[test]
fn ordinary_unicode_paths_links_and_listing_limits_keep_their_meaning() {
    let bytes = archive(json!({
        "资料": { "files": {
            "file.txt": { "size": 1, "offset": "0" },
            "link": { "link": "资料/file.txt" },
            "unpacked": { "size": 10, "unpacked": true }
        } }
    }));
    let listing = asar::parse(&bytes, 4).unwrap();
    assert!(!listing.truncated);
    assert_eq!(listing.entries.len(), 4);
    assert_eq!(listing.get("资料/link").unwrap().link.as_deref(), Some("资料/file.txt"));
    assert_eq!(asar::file_bytes(&bytes, &listing, listing.get("资料/file.txt").unwrap()), Some(&b"x"[..]));
    assert!(listing.get("资料/unpacked").unwrap().unpacked);
    let limited = asar::parse(&bytes, 2).unwrap();
    assert!(limited.truncated);
    assert_eq!(limited.entries.len(), 2);
}
