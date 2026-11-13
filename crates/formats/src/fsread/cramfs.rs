//! cramfs (`Compressed ROMFS`, magic `0x28cd3d45`): a compressed read-only filesystem in either
//! byte order. Inodes are 12 bytes; a
//! directory's children sit consecutively in its data span; a regular file's data is a run of
//! 4 KiB blocks, stored as zlib streams, uncompressed bytes or zero-filled holes, preceded by
//! end-offset pointers or direct start pointers with optional compressed lengths.
//! Direct blocks may be shared. Bounded against cycles, huge sizes and corrupt offsets.

use super::{FsEntry, FsImage, FsKind, Limits, join};
use crate::bytes::Reader;

const BLK: usize = 4096;
const UNCOMPRESSED: u32 = 0x8000_0000;
const DIRECT_PTR: u32 = 0x4000_0000;

struct Inode {
    mode: u16,
    size: usize,
    namelen: usize,
    offset: usize,
}

fn inode_at(r: &Reader, off: usize, big_endian: bool) -> Option<Inode> {
    let mode = if big_endian { r.u16_be(off) } else { r.u16_le(off) }?;
    let sz = if big_endian { r.u32_be(off + 4) } else { r.u32_le(off + 4) }?;
    let no = if big_endian { r.u32_be(off + 8) } else { r.u32_le(off + 8) }?;
    let size = if big_endian { sz >> 8 } else { sz & 0x00ff_ffff };
    let (namelen, offset) = if big_endian { (no >> 26, no & 0x03ff_ffff) } else { (no & 0x3f, no >> 6) };
    Some(Inode { mode, size: size as usize, namelen: namelen as usize * 4, offset: offset as usize * 4 })
}

pub fn read(data: &[u8], limits: &Limits) -> FsImage {
    let mut img = FsImage::new("cramfs");
    let r = Reader::new(data);
    let big_endian = if r.u32_le(0) == Some(0x28cd_3d45) {
        false
    } else if r.u32_be(0) == Some(0x28cd_3d45) {
        true
    } else {
        img.warn("not a cramfs image");
        img.complete = false;
        return img;
    };
    let word = |off| if big_endian { r.u32_be(off) } else { r.u32_le(off) };
    let data = if word(8).is_some_and(|flags| flags & 1 != 0) {
        let Some(size) = word(4).and_then(|n| usize::try_from(n).ok()).filter(|&n| n >= 76) else {
            img.warn("cramfs: invalid version-two filesystem size");
            img.complete = false;
            return img;
        };
        if size > data.len() {
            img.warn("cramfs: image ends before its declared filesystem size");
            img.complete = false;
        }
        data.get(..size).unwrap_or(data)
    } else {
        data
    };
    let r = Reader::new(data);
    let Some(root) = inode_at(&r, 64, big_endian) else {
        img.warn("root inode unreadable");
        img.complete = false;
        return img;
    };
    walk(data, &root, "", limits, &mut img, 0, big_endian);
    img
}

fn trim(name: &[u8]) -> String {
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    String::from_utf8_lossy(name.get(..end).unwrap_or(&[])).into_owned()
}

fn walk(data: &[u8], dir: &Inode, parent: &str, limits: &Limits, img: &mut FsImage, depth: usize, big_endian: bool) {
    if depth > limits.max_depth {
        img.warn("directory nesting limit reached");
        img.complete = false;
        return;
    }
    let rest = data.get(dir.offset..).unwrap_or_default();
    let directory = rest.get(..dir.size).unwrap_or(rest);
    let r = Reader::new(directory);
    let mut pos = 0;
    while pos < directory.len() {
        if img.entries.len() >= limits.max_entries {
            img.complete = false;
            return;
        }
        let Some(ino) = inode_at(&r, pos, big_endian) else { break };
        let name_off = pos + 12;
        let Some(full_name) = r.slice(name_off, ino.namelen) else { break };
        let name = trim(full_name.get(..ino.namelen.min(limits.max_name.max(1))).unwrap_or_default());
        let advance = 12usize.checked_add(ino.namelen);
        let Some(advance) = advance.filter(|&a| a > 12) else { break };
        let path = if name.is_empty() { parent.to_string() } else { join(parent, &name) };
        let typ = ino.mode & 0xf000;
        if !name.is_empty() {
            match typ {
                0x4000 => {
                    img.entries.push(FsEntry::dir(path.clone()));
                    walk(data, &ino, &path, limits, img, depth + 1, big_endian);
                }
                0x8000 => {
                    let bytes = file_data(data, &ino, limits, img, big_endian);
                    img.entries.push(FsEntry::file(path, ino.size as u64, bytes));
                }
                0xa000 => {
                    let target = file_data(data, &ino, limits, img, big_endian).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                    img.entries.push(FsEntry::symlink(path, target));
                }
                _ => img.entries.push(FsEntry { path, kind: FsKind::Other, size: ino.size as u64, symlink_target: None, data: None }),
            }
        }
        pos = match pos.checked_add(advance) {
            Some(n) if n > pos => n,
            _ => break,
        };
    }
    if pos != dir.size {
        img.warn("cramfs: incomplete directory entry");
        img.complete = false;
    }
}

/// Recover a regular file's (or symlink's) block run.
fn file_data(data: &[u8], ino: &Inode, limits: &Limits, img: &mut FsImage, big_endian: bool) -> Option<Vec<u8>> {
    if ino.size == 0 {
        return Some(Vec::new());
    }
    if ino.size as u64 > limits.max_file_bytes || img.bytes.saturating_add(ino.size as u64) > limits.max_total_bytes {
        img.warn("file skipped (size budget)");
        img.complete = false;
        return None;
    }
    let r = Reader::new(data);
    let nblocks = ino.size.div_ceil(BLK);
    let mut out = Vec::with_capacity(ino.size);
    let mut block_start = ino.offset.checked_add(nblocks.checked_mul(4)?)?;
    for i in 0..nblocks {
        let pointer_offset = ino.offset.checked_add(i.checked_mul(4)?)?;
        let pointer = if big_endian { r.u32_be(pointer_offset) } else { r.u32_le(pointer_offset) };
        let Some(pointer) = pointer else {
            img.warn("cramfs block pointer table is unavailable");
            img.complete = false;
            return None;
        };
        let expected = ino.size.saturating_sub(i.saturating_mul(BLK)).min(BLK);
        let address = pointer & !(UNCOMPRESSED | DIRECT_PTR);
        let end = if pointer & DIRECT_PTR != 0 {
            let range = usize::try_from(address).ok().and_then(|start| start.checked_mul(4)).and_then(|start| {
                if pointer & UNCOMPRESSED != 0 {
                    Some((start, start.checked_add(expected)?))
                } else {
                    let length = if big_endian { r.u16_be(start) } else { r.u16_le(start) }?;
                    let payload = start.checked_add(2)?;
                    Some((payload, payload.checked_add(usize::from(length))?))
                }
            });
            let Some((start, end)) = range else {
                img.warn("cramfs direct block header is unavailable");
                img.complete = false;
                return None;
            };
            block_start = start;
            end
        } else {
            address as usize
        };
        if end < block_start || end > data.len() {
            img.warn("corrupt cramfs block pointer");
            img.complete = false;
            return None;
        }
        if end == block_start {
            out.resize(out.len().checked_add(expected)?, 0);
            continue;
        }
        let block = data.get(block_start..end)?;
        if pointer & UNCOMPRESSED != 0 {
            if block.len() != expected {
                img.warn("cramfs uncompressed block does not match the file size");
                img.complete = false;
                return None;
            }
            out.extend_from_slice(block);
            block_start = end;
            continue;
        }
        match crate::firmware::inflate(block, true, BLK as u64, true) {
            Ok(inf) => {
                if inf.produced != expected as u64 {
                    img.warn("cramfs block output does not match the file size");
                    img.complete = false;
                    return None;
                }
                out.extend_from_slice(&inf.data.unwrap_or_default());
            }
            Err(_) => {
                img.warn("cramfs block did not inflate");
                img.complete = false;
                return None;
            }
        }
        block_start = end;
    }
    out.truncate(ino.size);
    img.bytes = img.bytes.saturating_add(out.len() as u64);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_small_image() {
        let img = read(&craft_anything_testkit::firmware::cramfs_image(), &Limits::default());
        assert!(img.complete, "{:?}", img.warnings);
        assert_eq!(img.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.as_deref(), Some(b"Hello from cramfs!\n".as_slice()));
        assert!(img.entries.iter().any(|e| e.path == "dir" && e.kind == FsKind::Dir));
        assert_eq!(img.entries.iter().find(|e| e.path == "dir/inner").unwrap().data.as_deref(), Some(b"nested".as_slice()));
    }

    #[test]
    fn hostile_never_panics() {
        let base = craft_anything_testkit::firmware::cramfs_image();
        for m in craft_anything_testkit::mutations(&base) {
            let _ = read(&m, &Limits::default());
        }
        for len in 0..base.len().min(400) {
            let _ = read(&base[..len], &Limits::default());
        }
    }
}
