use craft_anything_formats::{FormatError, unity_texture};

#[test]
fn etc1_has_its_name_and_eight_byte_blocks() {
    assert_eq!(unity_texture::format_name(34), "ETC_RGB4");
    for (w, h, bytes) in [(1, 1, 8), (4, 4, 8), (5, 5, 32), (8, 1, 16)] {
        assert_eq!(unity_texture::mip0_size(34, w, h), Some(bytes));
    }
}

#[test]
fn individual_mode_keeps_both_partition_directions_and_unity_row_order() {
    for flip in [0, 1] {
        let block = [0xf0, 0x0f, 0x00, flip, 0, 0, 0, 0];
        let rgba = unity_texture::decode(34, 4, 4, &block).unwrap();
        for y in 0..4 {
            for x in 0..4 {
                let red = if flip == 0 { x < 2 } else { y >= 2 };
                assert_eq!(&rgba[(y * 4 + x) * 4..(y * 4 + x + 1) * 4], if red { &[255, 2, 2, 255] } else { &[2, 255, 2, 255] });
            }
        }
    }
}

#[test]
fn every_modifier_table_and_selector_bit_plane_uses_column_major_indices() {
    for (table, small, large) in [(0u8, 2i16, 8i16), (1, 5, 17), (2, 9, 29), (3, 13, 42), (4, 18, 60), (5, 24, 80), (6, 33, 106), (7, 47, 183)] {
        let block = [0x88, 0x88, 0x88, (table << 5) | (table << 2), 0xcc, 0xcc, 0xaa, 0xaa];
        let rgba = unity_texture::decode(34, 4, 4, &block).unwrap();
        for (row, delta) in [-large, -small, large, small].into_iter().enumerate() {
            let value = (136 + delta).clamp(0, 255) as u8;
            assert_eq!(&rgba[row * 16..(row + 1) * 16], [value, value, value, 255].repeat(4));
        }
    }
}

#[test]
fn differential_mode_sign_extends_before_expanding_rgb555() {
    // (28,4,3) + (-4,2,0) gives (24,6,3), then five-to-eight bit expansion.
    let rgba = unity_texture::decode(34, 4, 4, &[0xe4, 0x22, 0x18, 0x02, 0, 0, 0, 0]).unwrap();
    for y in 0..4 {
        assert_eq!(&rgba[y * 16..y * 16 + 8], [233, 35, 26, 255].repeat(2));
        assert_eq!(&rgba[y * 16 + 8..(y + 1) * 16], [200, 51, 26, 255].repeat(2));
    }
}

#[test]
fn out_of_range_differential_bases_are_not_reported_as_etc1_colours() {
    for channel in 0..3 {
        for invalid in [0x04, 0xfb] {
            let mut block = [0x40, 0x40, 0x40, 0x02, 0, 0, 0, 0];
            block[channel] = invalid;
            assert!(matches!(unity_texture::decode(34, 4, 4, &block), Err(FormatError::Malformed(_))));
        }
    }
}

#[test]
fn clipped_edge_blocks_mips_and_output_limits_use_the_shared_pipeline() {
    let blocks: Vec<u8> = [0x00, 0x44, 0x88, 0xcc].into_iter().flat_map(|base| [base, base, base, 0, 0, 0, 0, 0]).collect();
    let rgba = unity_texture::decode(34, 5, 5, &blocks).unwrap();
    assert_eq!(rgba.len(), 100);
    for y in 0..5 {
        for x in 0..5 {
            let value = if y == 0 {
                if x == 4 { 206 } else { 138 }
            } else if x == 4 {
                70
            } else {
                2
            };
            assert_eq!(&rgba[(y * 5 + x) * 4..(y * 5 + x + 1) * 4], [value, value, value, 255]);
        }
    }
    let mut with_mip = blocks.clone();
    with_mip.extend([0xff; 8]);
    assert_eq!(unity_texture::decode(34, 5, 5, &with_mip).unwrap(), rgba);
    assert!(matches!(unity_texture::decode(34, 5, 5, &blocks[..31]), Err(FormatError::Truncated(_))));
    assert!(matches!(unity_texture::decode(34, 0, 1, &[]), Err(FormatError::Malformed(_))));
    assert!(matches!(unity_texture::decode(34, 8193, 8193, &[]), Err(FormatError::TooLarge(_))));
}

#[test]
fn serialized_unity_texture_reaches_the_etc1_decoder() {
    let block = [0x88, 0x88, 0x88, 0, 0, 0, 0, 0];
    let version = "2022.3.12f1";
    let object = craft_anything_testkit::games::unity_texture2d(version, "MobileTexture", 4, 4, 34, Some(&block), None);
    let texture = unity_texture::parse(&object, version, false).unwrap();
    let (start, len) = texture.inline.unwrap();
    assert_eq!(unity_texture::decode(texture.format, texture.width, texture.height, &object[start..start + len]).unwrap(), [138, 138, 138, 255].repeat(16));
}
