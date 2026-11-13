//! A minimal read-only ext2/3/4 reader: the superblock, block-group descriptors (32- or 64-byte),
//! inodes, directory entries, and regular-file data via either classic block pointers
//! (direct, single-, double- and triple-indirect) or ext4 extent trees (depth 0 and 1).
//! It extracts regular files (including small ext4 inline files), directories and fast symlinks
//! from the root inode down, strictly bounded.
//!
//! Deliberately NOT handled (reported in warnings): larger inline files and inline directories, directory
//! HTree index specifics (directories are read as linear entry lists, which is layout-compatible),
//! encryption, and 64-bit block numbers beyond the low 32 bits. These make a file appear as
//! unrecovered rather than producing wrong bytes.

use super::{FsEntry, FsImage, FsKind, Limits, is_dot, join};
use crate::bytes::Reader;
use std::collections::BTreeSet;

struct Sb {
    block_size: u64,
    inodes_per_group: u32,
    inode_size: u64,
    desc_size: u64,
    incompat: u32,
    kind: &'static str,
}

fn superblock(data: &[u8]) -> Option<Sb> {
    let r = Reader::new(data);
    let s = 1024usize;
    if r.u16_le(s + 56)? != 0xef53 {
        return None;
    }
    let log = r.u32_le(s + 24)?;
    if log > 6 {
        return None;
    }
    let block_size = 1024u64 << log;
    let inodes_per_group = r.u32_le(s + 40)?;
    let rev = r.u32_le(s + 76)?;
    let inode_size = if rev >= 1 { u64::from(r.u16_le(s + 88)?) } else { 128 };
    let incompat = r.u32_le(s + 96)?;
    let compat = r.u32_le(s + 92)?;
    let desc_size = if incompat & 0x80 != 0 { u64::from(r.u16_le(s + 0xfe).filter(|&v| v >= 32)?) } else { 32 };
    if inodes_per_group == 0 || inode_size < 128 {
        return None;
    }
    let kind = if incompat & (0x40 | 0x80 | 0x200 | 0x8000) != 0 {
        "ext4"
    } else if compat & 0x4 != 0 {
        "ext3"
    } else {
        "ext2"
    };
    Some(Sb { block_size, inodes_per_group, inode_size, desc_size, incompat, kind })
}

/// Byte offset of the group-descriptor table: the block after the one holding the superblock.
fn gdt_offset(block_size: u64) -> u64 {
    if block_size == 1024 { 2048 } else { block_size }
}

/// Byte offset of inode `ino` (1-based).
fn inode_offset(data: &[u8], sb: &Sb, ino: u32) -> Option<u64> {
    let group = (ino.checked_sub(1)?) / sb.inodes_per_group;
    let index = u64::from((ino - 1) % sb.inodes_per_group);
    let desc = gdt_offset(sb.block_size).checked_add(u64::from(group).checked_mul(sb.desc_size)?)?;
    let r = Reader::new(data);
    let table_lo = r.u32_le(usize::try_from(desc).ok()?.checked_add(8)?)?;
    let table_hi = if sb.desc_size >= 64 { r.u32_le(usize::try_from(desc).ok()?.checked_add(40)?).unwrap_or(0) } else { 0 };
    let table = u64::from(table_lo) | (u64::from(table_hi) << 32);
    table.checked_mul(sb.block_size)?.checked_add(index.checked_mul(sb.inode_size)?)
}

/// Return a complete physical block; an out-of-image block is unavailable, not a sparse hole.
fn block_bytes<'a>(data: &'a [u8], sb: &Sb, block: u64) -> Option<&'a [u8]> {
    let start = usize::try_from(block.checked_mul(sb.block_size)?).ok()?;
    let end = start.checked_add(usize::try_from(sb.block_size).ok()?)?;
    data.get(start..end)
}

/// Collect the physical block numbers for logical blocks 0..n of an inode, via block pointers or
/// an extent tree. Unmapped logical blocks are `None` (sparse → zeros); unreadable indirect
/// metadata makes the whole map unavailable rather than turning unknown mappings into holes.
fn file_block_map(data: &[u8], sb: &Sb, inode_off: u64, flags: u32, nblocks: u64, img: &mut FsImage) -> Option<Vec<Option<u64>>> {
    let r = Reader::new(data);
    let ib = usize::try_from(inode_off).ok().and_then(|o| o.checked_add(40))?;
    let n = usize::try_from(nblocks.min(1 << 20)).unwrap_or(0);
    let mut map = vec![None; n];
    if flags & 0x8_0000 != 0 {
        // ext4 extent tree rooted in i_block.
        let mut remaining = 1_000_000usize;
        if extents(data, sb, ib, &mut map, None, &mut remaining).is_none() {
            img.warn("ext4: extent mapping is unavailable or invalid; file data not recovered");
            img.complete = false;
            return None;
        }
    } else {
        // Classic indirect block pointers.
        let per = (sb.block_size / 4).max(1);
        let mut logical = 0u64;
        let set = |logical: &mut u64, phys: u32, map: &mut Vec<Option<u64>>| {
            if let Ok(i) = usize::try_from(*logical)
                && i < map.len()
                && phys != 0
            {
                map[i] = Some(u64::from(phys));
            }
            *logical += 1;
        };
        for i in 0..12 {
            if let Some(p) = r.u32_le(ib + i * 4) {
                set(&mut logical, p, &mut map);
            }
        }
        // Each root owns its full logical span, including when its pointer is zero.
        for (level, slot, first) in [(1, 48, 12), (2, 52, 12 + per), (3, 56, 12 + per + per * per)] {
            logical = first;
            if let Some(root) = r.u32_le(ib + slot).filter(|&p| p != 0)
                && let Err(block) = read_indirect(data, sb, u64::from(root), level, per, &mut logical, &mut map)
            {
                img.warn(format!("ext: indirect block {block} is unavailable; file data not recovered"));
                img.complete = false;
                return None;
            }
        }
    }
    Some(map)
}

fn read_indirect(data: &[u8], sb: &Sb, block: u64, level: u8, per: u64, logical: &mut u64, map: &mut Vec<Option<u64>>) -> Result<(), u64> {
    if usize::try_from(*logical).ok().is_none_or(|i| i >= map.len()) {
        return Ok(());
    }
    let b = block_bytes(data, sb, block).ok_or(block)?;
    let r = Reader::new(b);
    for i in 0..per {
        if usize::try_from(*logical).ok().is_none_or(|i| i >= map.len()) {
            return Ok(());
        }
        let off = usize::try_from(i.saturating_mul(4)).map_err(|_| block)?;
        let ptr = r.u32_le(off).ok_or(block)?;
        if ptr == 0 {
            let span = per.checked_pow(u32::from(level.saturating_sub(1))).ok_or(block)?;
            *logical = logical.checked_add(span).ok_or(block)?;
            continue;
        }
        if level == 1 {
            if let Ok(idx) = usize::try_from(*logical)
                && idx < map.len()
            {
                map[idx] = Some(u64::from(ptr));
            }
            *logical += 1;
        } else {
            read_indirect(data, sb, u64::from(ptr), level - 1, per, logical, map)?;
        }
    }
    Ok(())
}

/// Read a complete extent node. Child depths decrease exactly once per level, and the whole
/// traversal shares a work budget; damaged metadata must never turn into invented sparse bytes.
fn extents(data: &[u8], sb: &Sb, node_off: usize, map: &mut [Option<u64>], expected_depth: Option<u16>, remaining: &mut usize) -> Option<()> {
    *remaining = remaining.checked_sub(1)?;
    let length = if expected_depth.is_none() { 60 } else { usize::try_from(sb.block_size).ok()? };
    let node = data.get(node_off..node_off.checked_add(length)?)?;
    let r = Reader::new(node);
    if r.u16_le(0)? != 0xf30a {
        return None;
    }
    let entries = usize::from(r.u16_le(2)?);
    let maximum = usize::from(r.u16_le(4)?);
    let depth = r.u16_le(6)?;
    if entries > maximum || maximum > length.saturating_sub(12) / 12 || depth > 8 || expected_depth.is_some_and(|want| want != depth) {
        return None;
    }
    for i in 0..entries {
        *remaining = remaining.checked_sub(1)?;
        let entry = 12 + i * 12;
        if depth == 0 {
            let logical = u64::from(r.u32_le(entry)?);
            let raw_len = r.u16_le(entry + 4)?;
            let start_hi = r.u16_le(entry + 6)?;
            let start_lo = r.u32_le(entry + 8)?;
            let physical = u64::from(start_lo) | (u64::from(start_hi) << 32);
            let unwritten = raw_len > 32768;
            let count = if unwritten { raw_len - 32768 } else { raw_len };
            if count == 0 {
                return None;
            }
            let end = logical.checked_add(u64::from(count))?.min(map.len() as u64);
            for block in logical..end {
                *remaining = remaining.checked_sub(1)?;
                let slot = map.get_mut(usize::try_from(block).ok()?)?;
                *slot = if unwritten { None } else { Some(physical.checked_add(block - logical)?) };
            }
        } else {
            let child_lo = r.u32_le(entry + 4)?;
            let child_hi = r.u16_le(entry + 8)?;
            let child = u64::from(child_lo) | (u64::from(child_hi) << 32);
            let offset = usize::try_from(child.checked_mul(sb.block_size)?).ok()?;
            extents(data, sb, offset, map, Some(depth - 1), remaining)?;
        }
    }
    Some(())
}

fn file_data(data: &[u8], sb: &Sb, inode_off: u64, flags: u32, size: u64, limits: &Limits, img: &mut FsImage) -> Option<Vec<u8>> {
    if size == 0 {
        return Some(Vec::new());
    }
    if size > limits.max_file_bytes || img.bytes.saturating_add(size) > limits.max_total_bytes {
        img.warn("ext: file skipped (size budget)");
        img.complete = false;
        return None;
    }
    if flags & 0x1000_0000 != 0 {
        if sb.incompat & 0x8000 == 0 || size > 60 {
            img.warn("ext4: inline file needs unsupported metadata; file data not recovered");
            img.complete = false;
            return None;
        }
        let start = usize::try_from(inode_off).ok()?.checked_add(40)?;
        let end = start.checked_add(usize::try_from(size).ok()?)?;
        let bytes = data.get(start..end)?.to_vec();
        img.bytes = img.bytes.saturating_add(size);
        return Some(bytes);
    }
    let nblocks = size.div_ceil(sb.block_size);
    let map = file_block_map(data, sb, inode_off, flags, nblocks, img)?;
    let mut out = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    for phys in &map {
        match phys {
            Some(p) => match block_bytes(data, sb, *p) {
                Some(b) => out.extend_from_slice(b),
                None => {
                    img.warn(format!("ext: physical block {p} lies outside the image; file data not recovered"));
                    img.complete = false;
                    return None;
                }
            },
            None => out.resize(out.len().saturating_add(usize::try_from(sb.block_size).unwrap_or(0)), 0),
        }
    }
    out.truncate(usize::try_from(size).unwrap_or(0));
    img.bytes = img.bytes.saturating_add(out.len() as u64);
    Some(out)
}

#[allow(clippy::too_many_arguments)]
fn walk(data: &[u8], sb: &Sb, ino: u32, parent: &str, limits: &Limits, img: &mut FsImage, depth: usize, seen: &mut BTreeSet<u32>) {
    if depth > limits.max_depth || seen.len() > limits.max_entries {
        img.complete = false;
        return;
    }
    if !seen.insert(ino) {
        return;
    }
    let Some(inode_off) = inode_offset(data, sb, ino) else { return };
    let r = Reader::new(data);
    let io = usize::try_from(inode_off).ok().filter(|&o| o.checked_add(128).is_some_and(|end| end <= data.len()));
    let Some(io) = io else { return };
    let size = u64::from(r.u32_le(io + 4).unwrap_or(0)) | (u64::from(r.u32_le(io + 108).unwrap_or(0)) << 32);
    let flags = r.u32_le(io + 32).unwrap_or(0);
    if flags & 0x1000_0000 != 0 {
        img.warn(format!("{parent}: inline directory not read"));
        img.complete = false;
        return;
    }
    // Read the directory's data (a run of entry blocks) and parse entries.
    let dir_bytes = file_data(data, sb, inode_off, flags, size, &Limits { max_file_bytes: 64 << 20, ..*limits }, img).unwrap_or_default();
    let mut pos = 0usize;
    while pos + 8 <= dir_bytes.len() {
        if img.entries.len() >= limits.max_entries {
            img.complete = false;
            return;
        }
        let dr = Reader::new(&dir_bytes);
        let child = dr.u32_le(pos).unwrap_or(0);
        let rec_len = dr.u16_le(pos + 4).unwrap_or(0) as usize;
        let name_len = dr.u8(pos + 6).unwrap_or(0) as usize;
        if rec_len < 8 || pos + rec_len > dir_bytes.len() {
            break;
        }
        if child != 0 && name_len > 0 && name_len <= limits.max_name {
            let name = String::from_utf8_lossy(dir_bytes.get(pos + 8..pos + 8 + name_len).unwrap_or(&[])).into_owned();
            if !is_dot(&name) {
                let path = join(parent, &name);
                child_entry(data, sb, child, path, limits, img, depth, seen);
            }
        }
        pos += rec_len;
    }
}

/// A short target can still live in a data block. Only allocation accounted for by
/// external attributes may coexist with an inline target in the classic inode layout.
fn inline_symlink(r: &Reader, sb: &Sb, io: usize, size: u64, flags: u32) -> bool {
    if size >= 60 || flags & 0x1000_0000 != 0 {
        return false;
    }
    if sb.incompat & 0x400 != 0 {
        // EA_INODE makes i_blocks include external attribute values as well.
        return size != 0;
    }
    let ro_compat = r.u32_le(1024 + 100).unwrap_or(0);
    let huge_files = ro_compat & 8 != 0;
    let high = if huge_files { u64::from(r.u16_le(io + 116).unwrap_or(0)) << 32 } else { 0 };
    let blocks = u64::from(r.u32_le(io + 28).unwrap_or(0)) | high;
    let unit = if huge_files && flags & 0x4_0000 != 0 { sb.block_size / 512 } else { 1 };
    let has_attributes = r.u32_le(io + 104).unwrap_or(0) != 0 || (sb.incompat & 0x80 != 0 && r.u16_le(io + 118).unwrap_or(0) != 0);
    let attribute_sectors = if has_attributes {
        let cluster_size = if ro_compat & 0x200 != 0 {
            let Some(size) = r.u32_le(1024 + 28).and_then(|log| log.checked_add(10)).and_then(|shift| 1u64.checked_shl(shift)) else {
                return false;
            };
            size
        } else {
            sb.block_size
        };
        cluster_size / 512
    } else {
        0
    };
    blocks.checked_mul(unit) == Some(attribute_sectors)
}

#[allow(clippy::too_many_arguments)]
fn child_entry(data: &[u8], sb: &Sb, ino: u32, path: String, limits: &Limits, img: &mut FsImage, depth: usize, seen: &mut BTreeSet<u32>) {
    let Some(io) = inode_offset(data, sb, ino).and_then(|o| usize::try_from(o).ok()).filter(|&o| o.checked_add(128).is_some_and(|end| end <= data.len()))
    else {
        return;
    };
    let r = Reader::new(data);
    let mode = r.u16_le(io).unwrap_or(0) & 0xf000;
    // The inode owns its kind. Directory-entry type hints may be absent or inconsistent;
    // in particular, a device's i_block fields are device numbers, not file block pointers.
    if mode == 0x4000 {
        img.entries.push(FsEntry::dir(path.clone()));
        walk(data, sb, ino, &path, limits, img, depth + 1, seen);
    } else if mode == 0xa000 {
        let size = u64::from(r.u32_le(io + 4).unwrap_or(0));
        let flags = r.u32_le(io + 32).unwrap_or(0);
        let target = if flags & 0x1000_0000 != 0 {
            img.warn(format!("{path}: inline-data symlink not read"));
            img.complete = false;
            None
        } else if inline_symlink(&r, sb, io, size, flags) {
            data.get(io + 40..io + 40 + size as usize).map(|b| String::from_utf8_lossy(b).into_owned())
        } else {
            file_data(data, sb, io as u64, flags, size, limits, img).map(|b| String::from_utf8_lossy(&b).into_owned())
        };
        img.entries.push(FsEntry { path, kind: FsKind::Symlink, size, symlink_target: target, data: None });
    } else if mode == 0x8000 {
        let size = u64::from(r.u32_le(io + 4).unwrap_or(0)) | (u64::from(r.u32_le(io + 108).unwrap_or(0)) << 32);
        let flags = r.u32_le(io + 32).unwrap_or(0);
        let bytes = file_data(data, sb, io as u64, flags, size, limits, img);
        img.entries.push(FsEntry::file(path, size, bytes));
    } else {
        img.entries.push(FsEntry { path, kind: FsKind::Other, size: u64::from(r.u32_le(io + 4).unwrap_or(0)), symlink_target: None, data: None });
    }
}

pub fn read(data: &[u8], limits: &Limits) -> FsImage {
    let Some(sb) = superblock(data) else {
        let mut img = FsImage::new("ext");
        img.warn("not an ext2/3/4 superblock");
        img.complete = false;
        return img;
    };
    let mut img = FsImage::new(sb.kind);
    if sb.incompat & 0x1_0000 != 0 {
        img.warn("filesystem is encrypted; file contents are not decrypted");
        img.complete = false;
    }
    let mut seen = BTreeSet::new();
    walk(data, &sb, 2, "", limits, &mut img, 0, &mut seen);
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(extents: bool) {
        let img = read(&craft_anything_testkit::firmware::ext_image(extents), &Limits::default());
        assert!(img.complete, "extents={extents}: {:?}", img.warnings);
        assert_eq!(img.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.as_deref(), Some(b"Hello ext filesystem!\n".as_slice()));
        assert!(img.entries.iter().any(|e| e.path == "sub" && e.kind == super::super::FsKind::Dir));
        assert_eq!(img.entries.iter().find(|e| e.path == "sub/note").unwrap().data.as_deref(), Some(b"nested note".as_slice()));
    }

    #[test]
    fn reads_ext2_block_pointers() {
        check(false);
    }

    #[test]
    fn reads_ext4_extents() {
        check(true);
    }

    #[test]
    fn unwritten_ext4_extents_read_as_zeros() {
        let mut base = craft_anything_testkit::firmware::ext_image(true);
        // Inode 11 ("hello.txt", 22 bytes) starts at 5120 + (11 - 1) * 128 = 6400; its first extent's ee_len is at +56.
        base[6456..6458].copy_from_slice(&32769u16.to_le_bytes());
        let img = read(&base, &Limits::default());
        assert!(img.complete, "{:?}", img.warnings);
        let hello = img.entries.iter().find(|e| e.path == "hello.txt").expect("hello.txt entry");
        assert_eq!(hello.size, 22);
        assert_eq!(hello.data.as_deref(), Some([0u8; 22].as_slice()));
        assert_eq!(img.entries.iter().find(|e| e.path == "sub/note").unwrap().data.as_deref(), Some(b"nested note".as_slice()));
    }

    /// Switch to 64-bit group descriptors and send `group`'s inode table to the last
    /// 1 KiB block of the address space, so an inode there starts within 128 bytes of `usize::MAX`.
    fn point_table_at_the_end(img: &mut [u8], group: usize) {
        img[1024 + 96..1024 + 100].copy_from_slice(&0x80u32.to_le_bytes()); // incompat: 64-bit
        img[1024 + 0xfe..1024 + 0x100].copy_from_slice(&64u16.to_le_bytes()); // descriptor size
        let d = 2048 + group * 64;
        img[d + 8..d + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        img[d + 40..d + 44].copy_from_slice(&0x003f_ffffu32.to_le_bytes());
    }

    #[test]
    fn root_inode_at_the_end_of_the_address_space_is_refused() {
        let mut base = craft_anything_testkit::firmware::ext_image(false);
        base[1024 + 88..1024 + 90].copy_from_slice(&1000u16.to_le_bytes()); // inode size
        point_table_at_the_end(&mut base, 0);
        let img = read(&base, &Limits::default());
        assert!(img.entries.is_empty(), "{:?}", img.entries);
    }

    #[test]
    fn child_inode_at_the_end_of_the_address_space_is_skipped() {
        let mut base = craft_anything_testkit::firmware::ext_image(false);
        point_table_at_the_end(&mut base, 1);
        // Point "hello.txt" at inode 24: group 1, slot 7, which starts 128 bytes before the end.
        let at = base.windows(9).position(|w| w == b"hello.txt").expect("the fixture lists hello.txt");
        base[at - 8..at - 4].copy_from_slice(&24u32.to_le_bytes());
        let img = read(&base, &Limits::default());
        assert!(img.entries.iter().all(|e| e.path != "hello.txt"), "{:?}", img.entries);
        assert!(img.entries.iter().any(|e| e.path == "sub"), "the rest of the directory is still read");
    }

    #[test]
    fn hostile_never_panics() {
        let base = craft_anything_testkit::firmware::ext_image(false);
        for m in craft_anything_testkit::mutations(&base) {
            let _ = read(&m, &Limits::default());
        }
        for len in (0..base.len()).step_by(7) {
            let _ = read(&base[..len], &Limits::default());
        }
        for seed in 0..16 {
            let _ = read(&craft_anything_testkit::noise(seed, 4096), &Limits::default());
        }
    }
}
