use craft_anything_formats::fsread::{FsKind, Limits, extfs};

const HELLO_INODE: usize = 6400;
const HELLO_DIRENT: usize = 7168 + 24;

#[test]
fn special_inode_fields_are_not_exposed_as_file_contents() {
    for kind in [0x1000u16, 0x2000, 0x6000, 0xc000] {
        for hint in [0, 1, 2, 7] {
            let mut bytes = craft_anything_testkit::firmware::ext_image(false);
            bytes[HELLO_INODE..HELLO_INODE + 2].copy_from_slice(&(kind | 0o644).to_le_bytes());
            bytes[HELLO_DIRENT + 7] = hint;
            let result = extfs::read(&bytes, &Limits::default());
            let entry = result.entries.iter().find(|entry| entry.path == "hello.txt").unwrap();
            assert_eq!(entry.kind, FsKind::Other, "mode {kind:#x}, directory hint {hint}");
            assert!(entry.data.is_none());
            assert!(entry.symlink_target.is_none());
            assert!(result.complete, "{:?}", result.warnings);
            assert_eq!(result.entries.iter().find(|entry| entry.path == "sub/note").unwrap().data.as_deref(), Some(b"nested note".as_slice()));
        }
    }
}

#[test]
fn regular_inodes_remain_files_when_directory_type_hints_disagree() {
    for hint in [0, 2, 7] {
        let mut bytes = craft_anything_testkit::firmware::ext_image(false);
        bytes[HELLO_DIRENT + 7] = hint;
        let result = extfs::read(&bytes, &Limits::default());
        let entry = result.entries.iter().find(|entry| entry.path == "hello.txt").unwrap();
        assert_eq!(entry.kind, FsKind::File);
        assert_eq!(entry.data.as_deref(), Some(b"Hello ext filesystem!\n".as_slice()));
    }
}

#[test]
fn directory_and_symlink_inode_modes_remain_authoritative() {
    let mut bytes = craft_anything_testkit::firmware::ext_image(false);
    bytes[7168 + 44 + 7] = 1;
    bytes[HELLO_INODE..HELLO_INODE + 2].copy_from_slice(&0xa1ffu16.to_le_bytes());
    bytes[HELLO_INODE + 4..HELLO_INODE + 8].copy_from_slice(&4u32.to_le_bytes());
    bytes[HELLO_INODE + 40..HELLO_INODE + 44].copy_from_slice(b"dest");
    let result = extfs::read(&bytes, &Limits::default());
    let link = result.entries.iter().find(|entry| entry.path == "hello.txt").unwrap();
    assert_eq!(link.kind, FsKind::Symlink);
    assert_eq!(link.symlink_target.as_deref(), Some("dest"));
    assert!(result.entries.iter().any(|entry| entry.path == "sub" && entry.kind == FsKind::Dir));
    assert_eq!(result.entries.iter().find(|entry| entry.path == "sub/note").unwrap().data.as_deref(), Some(b"nested note".as_slice()));
}

#[test]
fn unknown_inode_types_do_not_default_to_regular_files() {
    let mut bytes = craft_anything_testkit::firmware::ext_image(false);
    bytes[HELLO_INODE..HELLO_INODE + 2].copy_from_slice(&0o644u16.to_le_bytes());
    let result = extfs::read(&bytes, &Limits::default());
    let entry = result.entries.iter().find(|entry| entry.path == "hello.txt").unwrap();
    assert_eq!(entry.kind, FsKind::Other);
    assert!(entry.data.is_none());
}
