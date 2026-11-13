use craft_anything_formats::packed::{Compression, PackedFormat};
use craft_anything_formats::pyinstaller::{PyInstaller, PyzArchive};
use craft_anything_testkit::packed::{pyinstaller, pyz};

const PATH_BYTES: usize = 8 * 1024 * 1024;
const BODY: &[u8] = b"N";

fn long_prefix(last: &str) -> String {
    let directories: Vec<String> = (0..15).map(|i| format!("{i:02}{}", "d".repeat(238))).collect();
    format!("{}/{last}", directories.join("/"))
}

fn archive(count: usize, mixed: bool) -> Vec<u8> {
    let names: Vec<String> = (0..count).map(|i| format!("module_{i:05}")).collect();
    let modules: Vec<(&str, i32, &[u8])> =
        names.iter().enumerate().map(|(i, name)| (name.as_str(), if mixed { [0, 1, 2, 7][i % 4] } else { 0 }, BODY)).collect();
    pyz(3531, &modules, false, true)
}

#[test]
fn expanded_module_resource_and_unknown_paths_share_the_retained_byte_limit() {
    let inner = archive(3_000, true);
    let prefix = long_prefix("modules.pyz");
    let data = pyinstaller(&[(&prefix, b'z', &inner, false)], true, 312);
    assert!(data.len() < 250_000);
    let listing = PyInstaller.list(&data).unwrap();
    assert!(listing.truncated);
    assert_eq!(listing.facts["pyz_archives"][0]["modules"], 3_000);
    let nested = &listing.entries[1..];
    assert!(!nested.is_empty());
    assert!(nested.len() < 3_000);
    assert!(nested.iter().map(|entry| entry.path.len()).sum::<usize>() <= PATH_BYTES);
    for (i, entry) in nested.iter().enumerate() {
        let name = format!("module_{i:05}");
        let expected = match i % 4 {
            0 => format!("{prefix}.modules/{name}.pyc"),
            1 => format!("{prefix}.modules/{name}/__init__.pyc"),
            2 => format!("{prefix}.data/{name}"),
            _ => format!("{prefix}.members/{name}"),
        };
        assert_eq!(entry.path, expected, "retain the complete identity");
    }
    assert_eq!(&PyInstaller.read(&data, &nested[0], 17).unwrap()[16..], BODY);
    assert_eq!(PyInstaller.read(&data, &nested[2], 1).unwrap(), BODY);
    assert_eq!(nested[3].compression, Compression::Unreadable);
}

#[test]
fn multiple_pyz_members_spend_one_shared_expanded_path_budget() {
    let inner = archive(1_700, false);
    let first = long_prefix("first.pyz");
    let second = long_prefix("second.pyz");
    let data = pyinstaller(&[(&first, b'z', &inner, false), (&second, b'z', &inner, false)], true, 312);
    let listing = PyInstaller.list(&data).unwrap();
    assert!(listing.truncated);
    assert_eq!(listing.facts["pyz_archives"][0]["modules"], 1_700);
    assert_eq!(listing.facts["pyz_archives"][1]["modules"], 1_700);
    let nested = &listing.entries[2..];
    assert!(nested.len() > 1_700);
    assert!(nested.len() < 3_400);
    assert!(nested.iter().map(|entry| entry.path.len()).sum::<usize>() <= PATH_BYTES);
    assert_eq!(nested.iter().filter(|entry| entry.path.starts_with(&format!("{first}.modules/"))).count(), 1_700);
    assert_eq!(&PyInstaller.read(&data, nested.last().unwrap(), 17).unwrap()[16..], BODY);
}

#[test]
fn standalone_namespace_metadata_and_entry_paths_share_a_byte_budget() {
    let prefix = "包.".repeat(600);
    let names: Vec<String> = (0..4_000).map(|i| format!("{prefix}m{i:05}")).collect();
    let modules: Vec<(&str, i32, &[u8])> =
        names.iter().enumerate().map(|(i, name)| (name.as_str(), if i % 2 == 0 { 3 } else { 0 }, if i % 2 == 0 { b"" } else { BODY })).collect();
    let data = pyz(3531, &modules, false, true);
    let listing = PyzArchive.list(&data).unwrap();
    assert!(listing.truncated);
    assert_eq!(listing.facts["modules"], 4_000);
    let namespaces = listing.facts["namespace_packages"].as_array().unwrap();
    assert!(!namespaces.is_empty());
    assert!(!listing.entries.is_empty());
    assert!(namespaces.len() + listing.entries.len() < 4_000);
    let kept = namespaces.iter().map(|name| name.as_str().unwrap().len()).sum::<usize>() + listing.entries.iter().map(|entry| entry.path.len()).sum::<usize>();
    assert!(kept <= PATH_BYTES);
    assert_eq!(namespaces[0], names[0]);
    assert_eq!(listing.entries[0].path, format!("pyz.modules/{}.pyc", names[1].replace('.', "/")));
    assert_eq!(&PyzArchive.read(&data, &listing.entries[0], 17).unwrap()[16..], BODY);
}

#[test]
fn ordinary_unicode_identities_and_member_bytes_are_preserved() {
    let data = pyz(3531, &[("包.模块", 0, BODY), ("包", 1, BODY), ("资料/read.me", 2, b"contents"), ("包.空间", 3, b"")], false, true);
    let listing = PyzArchive.list(&data).unwrap();
    assert!(!listing.truncated);
    assert_eq!(listing.facts["modules"], 4);
    assert_eq!(listing.facts["namespace_packages"][0], "包.空间");
    assert_eq!(listing.entries[0].path, "pyz.modules/包/模块.pyc");
    assert_eq!(listing.entries[1].path, "pyz.modules/包/__init__.pyc");
    assert_eq!(listing.entries[2].path, "pyz.data/资料/read.me");
    assert_eq!(&PyzArchive.read(&data, &listing.entries[0], 17).unwrap()[16..], BODY);
    assert_eq!(PyzArchive.read(&data, &listing.entries[2], 8).unwrap(), b"contents");
}
