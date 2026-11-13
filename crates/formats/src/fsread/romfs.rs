//! romfs (`-rom1fs-`): a tiny read-only filesystem. Big-endian throughout. Each file header is
//! 16-byte aligned: next-header pointer (with the type in its low 4 bits), spec info, size,
//! checksum, then a NUL-terminated 16-aligned name, then the file data. We walk the directory tree
//! from the root, following sibling (`next`) and child (`spec` for directories) pointers, bounded
//! against cycles and runaway sizes.

use super::{FsEntry, FsImage, Limits, is_dot, join};
use crate::bytes::Reader;
use std::collections::BTreeSet;

fn align16(n: usize) -> Option<usize> {
    n.checked_add(15).map(|v| v & !15)
}

/// Name field length (NUL-terminated, rounded up to 16), and the trimmed name.
fn name_at(data: &[u8], off: usize, max_name: usize) -> Option<(String, usize)> {
    let rest = data.get(off..)?;
    let end = rest.iter().take(max_name.min(1024) + 1).position(|&b| b == 0)?;
    let name = String::from_utf8_lossy(rest.get(..end)?).into_owned();
    Some((name, align16(end + 1)?))
}

/// Read a ROMFS image within the caller's recovery limits.
pub fn read(data: &[u8], limits: &Limits) -> FsImage {
    let mut img = FsImage::new("romfs");
    let r = Reader::new(data);
    if r.slice(0, 8) != Some(b"-rom1fs-") {
        img.warn("not a romfs image");
        img.complete = false;
        return img;
    }
    let Some(size) = r.u32_be(8).filter(|&size| size >= 16) else {
        img.warn("romfs: invalid image size");
        img.complete = false;
        return img;
    };
    if size as usize > data.len() {
        img.warn("romfs: image ends before its declared size");
        img.complete = false;
    }
    let data = data.get(..size as usize).unwrap_or(data);
    let r = Reader::new(data);
    // Root directory's first entry sits just after the volume name.
    let Some((_, vol_len)) = name_at(data, 16, 128) else {
        img.warn("volume name unreadable");
        img.complete = false;
        return img;
    };
    let Some(root) = 16usize.checked_add(vol_len) else {
        img.complete = false;
        return img;
    };
    let mut visited = BTreeSet::new();
    // A dot-named root is the container itself, not a child to skip with `.` and `..`.
    let root_container = r.u32_be(root).is_some_and(|kind| kind & 7 == 1)
        && root.checked_add(16).and_then(|at| name_at(data, at, limits.max_name.max(1))).is_some_and(|(name, _)| name == "." || name.is_empty());
    if root_container {
        visited.insert(root);
        let first = root.checked_add(4).and_then(|at| r.u32_be(at)).unwrap_or(0) as usize;
        walk(data, first, "", limits, &mut img, &mut visited, 0);
    } else {
        walk(data, root, "", limits, &mut img, &mut visited, 0);
    }
    img
}

#[allow(clippy::too_many_arguments)]
fn walk(data: &[u8], first: usize, parent: &str, limits: &Limits, img: &mut FsImage, visited: &mut BTreeSet<usize>, depth: usize) {
    if depth > limits.max_depth {
        img.warn("directory nesting limit reached");
        img.complete = false;
        return;
    }
    let r = Reader::new(data);
    let mut off = first;
    let mut siblings = 0usize;
    while off != 0 {
        if !off.is_multiple_of(16) || r.slice(off, 16).is_none() {
            img.warn("romfs: header pointer outside the image or unaligned");
            img.complete = false;
            break;
        }
        if img.entries.len() >= limits.max_entries {
            img.complete = false;
            return;
        }
        if !visited.insert(off) {
            img.warn("romfs header cycle detected");
            img.complete = false;
            return;
        }
        siblings += 1;
        if siblings > limits.max_entries {
            img.complete = false;
            return;
        }
        let Some(next_raw) = r.u32_be(off) else { break };
        let ftype = next_raw & 0x7;
        let next = (next_raw & 0xffff_fff0) as usize;
        let spec = r.u32_be(off + 4).unwrap_or(0) as usize;
        let size = r.u32_be(off + 8).unwrap_or(0) as usize;
        let Some((name, name_len)) = name_at(data, off + 16, limits.max_name) else {
            img.warn("romfs: unreadable entry name");
            img.complete = false;
            break;
        };
        if !is_dot(&name) && !name.is_empty() {
            let path = join(parent, &name);
            match ftype {
                1 => {
                    // Directory: recurse into its first child at `spec`.
                    img.entries.push(FsEntry::dir(path.clone()));
                    if spec != 0 {
                        walk(data, spec, &path, limits, img, visited, depth + 1);
                    }
                }
                2 | 3 => {
                    let payload = (size as u64 <= limits.max_file_bytes && img.bytes.saturating_add(size as u64) <= limits.max_total_bytes)
                        .then_some(off + 16)
                        .and_then(|start| start.checked_add(name_len))
                        .and_then(|start| r.slice(start, size));
                    if payload.is_some() {
                        img.bytes = img.bytes.saturating_add(size as u64);
                    } else {
                        img.warn(format!("{path}: entry data not recovered (size/bounds)"));
                        img.complete = false;
                    }
                    if ftype == 2 {
                        img.entries.push(FsEntry::file(path, size as u64, payload.map(<[u8]>::to_vec)));
                    } else {
                        let symlink_target = payload.map(|bytes| String::from_utf8_lossy(bytes).into_owned());
                        img.entries.push(FsEntry { path, kind: super::FsKind::Symlink, size: size as u64, symlink_target, data: None });
                    }
                }
                _ => img.entries.push(FsEntry { path, kind: super::FsKind::Other, size: size as u64, symlink_target: None, data: None }),
            }
        }
        off = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zero name budget rejects stored names while still recognizing root containers.
    #[test]
    fn zero_name_limit_preserves_root_recognition() {
        let limits = Limits { max_name: 0, ..Limits::default() };
        let mut file = craft_anything_testkit::firmware::romfs_image();
        file[32..36].copy_from_slice(&2u32.to_be_bytes());
        file[36..44].fill(0);
        file[48..50].copy_from_slice(b"a\0");
        let image = read(&file, &limits);
        assert!(image.entries.is_empty());
        assert!(!image.complete);
        for name in [b".\0".as_slice(), b"\0".as_slice()] {
            let mut directory = file.clone();
            directory[32..36].copy_from_slice(&1u32.to_be_bytes());
            directory[48..48 + name.len()].copy_from_slice(name);
            let image = read(&directory, &limits);
            assert!(image.entries.is_empty());
            assert!(image.complete, "{:?}", image.warnings);
        }
    }

    fn wide_image(count: usize, cycle: Option<u32>) -> Vec<u8> {
        fn put(data: &mut [u8], at: usize, value: u32) {
            data[at..at + 4].copy_from_slice(&value.to_be_bytes());
        }
        fn checksum(data: &mut [u8], start: usize, length: usize, at: usize) {
            let sum = data[start..start + length].as_chunks::<4>().0.iter().fold(0u32, |sum, word| sum.wrapping_add(u32::from_be_bytes(*word)));
            put(data, at, 0u32.wrapping_sub(sum));
        }
        let size = 96 + count * 48;
        let mut data = vec![0; size.next_multiple_of(1024)];
        data[..8].copy_from_slice(b"-rom1fs-");
        put(&mut data, 8, size as u32);
        data[16..21].copy_from_slice(b"tests");
        put(&mut data, 32, 1);
        put(&mut data, 36, 64);
        data[48] = b'.';
        checksum(&mut data, 32, 32, 44);
        put(&mut data, 64, 96);
        put(&mut data, 68, 32);
        data[80..82].copy_from_slice(b"..");
        checksum(&mut data, 64, 32, 76);
        for i in 0..count {
            let at = 96 + i * 48;
            let next = if i + 1 < count { (at + 48) as u32 } else { cycle.unwrap_or(0) };
            put(&mut data, at, next | 2);
            put(&mut data, at + 8, 1);
            let name = format!("item{i:05}");
            data[at + 16..at + 16 + name.len()].copy_from_slice(name.as_bytes());
            checksum(&mut data, at, 32, at + 12);
            data[at + 32] = i as u8;
        }
        checksum(&mut data, 0, size.min(512), 12);
        data
    }

    #[test]
    fn wide_sibling_walk_keeps_order_limits_and_cycle_detection() {
        let bytes = wide_image(4096, None);
        let image = read(&bytes, &Limits::default());
        assert!(image.complete, "{:?}", image.warnings);
        assert_eq!(image.entries.len(), 4096);
        assert_eq!(image.bytes, 4096);
        for (i, entry) in image.entries.iter().enumerate() {
            assert_eq!(entry.path, format!("item{i:05}"));
            assert_eq!(entry.data.as_deref(), Some(&[i as u8][..]));
        }
        let limited = read(&bytes, &Limits { max_entries: 17, ..Limits::default() });
        assert!(!limited.complete);
        // The existing sibling-work limit also counts the skipped `..` header.
        assert_eq!(limited.entries, image.entries[..16]);
        assert_eq!(limited.bytes, 16);
        for target in [32, 96] {
            let cyclic = read(&wide_image(4096, Some(target)), &Limits::default());
            assert!(!cyclic.complete);
            assert_eq!(cyclic.entries, image.entries);
            assert!(cyclic.warnings.iter().any(|warning| warning.contains("header cycle")));
        }
    }

    /// Names at the configured byte limit fit while longer names stop recovery.
    #[test]
    fn name_limit_counts_name_bytes_without_terminator() {
        let mut data = craft_anything_testkit::firmware::romfs_image();
        let limits = Limits { max_name: 12, ..Limits::default() };
        data[80..92].copy_from_slice(b"readme12.txt");
        data[92] = 0;
        let image = read(&data, &limits);
        assert!(image.complete, "{:?}", image.warnings);
        assert!(image.entries.iter().any(|entry| entry.path == "readme12.txt"));
        data[92] = b'x';
        data[93] = 0;
        assert!(!read(&data, &limits).complete);
    }

    #[test]
    fn reads_a_small_image() {
        let img = read(&craft_anything_testkit::firmware::romfs_image(), &Limits::default());
        assert!(img.complete, "{:?}", img.warnings);
        let f = img.entries.iter().find(|e| e.path == "etc/hostname").expect("etc/hostname");
        assert_eq!(f.data.as_deref(), Some(b"craft-router\n".as_slice()));
        assert!(img.entries.iter().any(|e| e.path == "etc" && e.kind == super::super::FsKind::Dir));
        assert_eq!(img.entries.iter().find(|e| e.path == "readme.txt").unwrap().data.as_deref(), Some(b"hello romfs".as_slice()));
    }

    #[test]
    fn hostile_never_panics() {
        let base = craft_anything_testkit::firmware::romfs_image();
        for m in craft_anything_testkit::mutations(&base) {
            let _ = read(&m, &Limits::default());
        }
        for len in 0..base.len().min(400) {
            let _ = read(&base[..len], &Limits::default());
        }
        for seed in 0..32 {
            let _ = read(&craft_anything_testkit::noise(seed, 300), &Limits::default());
        }
    }
}
