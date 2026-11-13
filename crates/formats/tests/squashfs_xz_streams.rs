use craft_anything_formats::fsread::{self, Limits};
use craft_anything_testkit::firmware::{XZ_FIXTURE, XZ_PLAIN, squashfs_image};

fn image_with_xz_readme(block: &[u8], file_size: usize) -> Vec<u8> {
    let mut data = squashfs_image();
    let inodes = u64::from_le_bytes(data[64..72].try_into().unwrap()) as usize;
    let start = data.len();
    data.extend_from_slice(block);
    let used = data.len() as u64;
    data[20..22].copy_from_slice(&4u16.to_le_bytes());
    data[40..48].copy_from_slice(&used.to_le_bytes());
    data[inodes + 18..inodes + 22].copy_from_slice(&(start as u32).to_le_bytes());
    data[inodes + 30..inodes + 34].copy_from_slice(&(file_size as u32).to_le_bytes());
    data[inodes + 34..inodes + 38].copy_from_slice(&(block.len() as u32).to_le_bytes());
    data
}

fn readme(block: &[u8], file_size: usize) -> Vec<u8> {
    let data = image_with_xz_readme(block, file_size);
    let image = fsread::squashfs::read(&data, &Limits::default());
    assert!(image.complete, "{:?}", image.warnings);
    image.entries.iter().find(|entry| entry.path == "readme").unwrap().data.clone().unwrap()
}

#[test]
fn squashfs_stops_after_the_first_xz_stream() {
    let block = [XZ_FIXTURE, XZ_FIXTURE].concat();
    assert_eq!(readme(&block, XZ_PLAIN.len()), XZ_PLAIN);
    assert_eq!(craft_anything_formats::decompress::xz(&block, 1024).unwrap(), [XZ_PLAIN, XZ_PLAIN].concat());
}

#[test]
fn squashfs_does_not_decode_trailing_xz_data() {
    for tail in [&b"junk"[..], &XZ_FIXTURE[..XZ_FIXTURE.len() - 1], &[0; 4]] {
        let block = [XZ_FIXTURE, tail].concat();
        assert_eq!(readme(&block, XZ_PLAIN.len()), XZ_PLAIN);
    }
    let mut large_tail = Vec::new();
    lzma_rs::xz_compress(&mut std::io::Cursor::new(vec![b'x'; 131_072]), &mut large_tail).unwrap();
    let block = [XZ_FIXTURE, &large_tail].concat();
    assert_eq!(readme(&block, XZ_PLAIN.len()), XZ_PLAIN);
}
