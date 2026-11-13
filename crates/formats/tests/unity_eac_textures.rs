use craft_anything_formats::{FormatError, unity_texture};

fn block(base: u8, multiplier: u8, table: u8, selectors: [u8; 16]) -> [u8; 8] {
    let mut bits = (u64::from(base) << 56) | (u64::from((multiplier << 4) | table) << 48);
    for (index, selector) in selectors.into_iter().enumerate() {
        let (x, y) = (index % 4, index / 4);
        bits |= u64::from(selector) << (45 - 3 * (x * 4 + y));
    }
    bits.to_be_bytes()
}

#[test]
fn eac_formats_have_names_and_block_sizes() {
    for (format, name, stride) in [(41, "EAC_R", 8), (42, "EAC_R_SIGNED", 8), (43, "EAC_RG", 16), (44, "EAC_RG_SIGNED", 16)] {
        assert_eq!(unity_texture::format_name(format), name);
        for (width, height, count) in [(1, 1, 1), (4, 4, 1), (5, 5, 4), (8, 1, 2)] {
            assert_eq!(unity_texture::mip0_size(format, width, height), Some(count * stride));
        }
    }
}

#[test]
fn reference_values_include_the_special_zero_multiplier() {
    for (format, base, multiplier, expected) in [(41, 103, 2, 83), (41, 103, 0, 102), (42, 60, 2, 167), (42, 60, 0, 186)] {
        let rgba = unity_texture::decode(format, 4, 4, &block(base, multiplier, 13, [3; 16])).unwrap();
        assert_eq!(rgba, [expected, expected, expected, 255].repeat(16));
    }
}

#[test]
fn modifier_tables_and_column_major_indices_survive_unity_row_flipping() {
    // Golden bytes for a base of 128 and multiplier 1, after 11-bit UNORM quantization.
    let values = [
        [125, 122, 119, 113, 130, 133, 136, 142],
        [125, 121, 118, 115, 130, 134, 137, 140],
        [126, 123, 120, 115, 129, 132, 135, 140],
        [126, 124, 122, 115, 129, 131, 133, 140],
        [125, 122, 120, 116, 130, 133, 135, 139],
        [125, 121, 119, 117, 130, 134, 136, 138],
        [124, 121, 120, 117, 131, 134, 135, 138],
        [125, 123, 120, 117, 130, 132, 135, 138],
        [126, 122, 120, 118, 129, 133, 135, 137],
        [126, 123, 120, 118, 129, 132, 135, 137],
        [126, 124, 120, 118, 129, 131, 135, 137],
        [126, 123, 121, 118, 129, 132, 134, 137],
        [125, 124, 121, 118, 130, 131, 134, 137],
        [127, 126, 125, 118, 128, 129, 130, 137],
        [124, 122, 120, 119, 131, 133, 135, 136],
        [125, 123, 121, 119, 130, 132, 134, 136],
    ];
    let selectors = std::array::from_fn(|index| ((index * 5) % 8) as u8);
    for (table, values) in values.into_iter().enumerate() {
        let rgba = unity_texture::decode(41, 4, 4, &block(128, 1, table as u8, selectors)).unwrap();
        for y in 0..4 {
            for x in 0..4 {
                let value = values[selectors[(3 - y) * 4 + x] as usize];
                assert_eq!(&rgba[(y * 4 + x) * 4..(y * 4 + x + 1) * 4], [value, value, value, 255]);
            }
        }
    }
}

#[test]
fn signed_bases_and_clamping_have_an_explicit_display_range() {
    for (format, base, multiplier, selector, expected) in
        [(41, 0, 15, 3, 0), (41, 255, 15, 7, 255), (42, 128, 0, 4, 1), (42, 129, 0, 4, 1), (42, 128, 15, 3, 0), (42, 127, 15, 7, 255), (42, 0, 0, 4, 128)]
    {
        let rgba = unity_texture::decode(format, 4, 4, &block(base, multiplier, 13, [selector; 16])).unwrap();
        assert_eq!(rgba, [expected, expected, expected, 255].repeat(16));
    }
}

#[test]
fn dual_channel_blocks_keep_red_and_green_independent() {
    for (format, red, green, expected) in [
        (43, block(103, 2, 13, [3; 16]), block(255, 15, 13, [7; 16]), [83, 255, 0, 255]),
        (44, block(60, 2, 13, [3; 16]), block(196, 0, 13, [4; 16]), [167, 68, 0, 255]),
    ] {
        let data = [red, green].concat();
        assert_eq!(unity_texture::decode(format, 4, 4, &data).unwrap(), expected.repeat(16));
    }
}

#[test]
fn edge_blocks_later_mips_and_budgets_use_the_shared_pipeline() {
    let bytes = [0, 64, 128, 192].into_iter().flat_map(|base| block(base, 0, 13, [4; 16])).collect::<Vec<_>>();
    let rgba = unity_texture::decode(41, 5, 5, &bytes).unwrap();
    for y in 0..5 {
        for x in 0..5 {
            let value = match (y == 0, x == 4) {
                (false, false) => 0,
                (false, true) => 64,
                (true, false) => 128,
                (true, true) => 192,
            };
            assert_eq!(&rgba[(y * 5 + x) * 4..(y * 5 + x + 1) * 4], [value, value, value, 255]);
        }
    }
    let mut mips = bytes.clone();
    mips.extend_from_slice(&[255; 8]);
    assert_eq!(unity_texture::decode(41, 5, 5, &mips).unwrap(), rgba);
    for format in 41..=44 {
        let size = unity_texture::mip0_size(format, 4, 4).unwrap();
        assert!(matches!(unity_texture::decode(format, 4, 4, &vec![0; size - 1]), Err(FormatError::Truncated(_))));
        assert!(matches!(unity_texture::decode(format, 0, 1, &[]), Err(FormatError::Malformed(_))));
        assert!(matches!(unity_texture::decode(format, 8193, 8193, &[]), Err(FormatError::TooLarge(_))));
    }
}

#[test]
fn serialized_texture_objects_reach_all_four_eac_decoders() {
    let version = "2022.3.12f1";
    for format in 41..=44 {
        let one = block(0, 0, 13, [4; 16]);
        let data = if format < 43 { one.to_vec() } else { [one, one].concat() };
        let object = craft_anything_testkit::games::unity_texture2d(version, "ChannelTexture", 4, 4, format, Some(&data), None);
        let texture = unity_texture::parse(&object, version, false).unwrap();
        let (start, size) = texture.inline.unwrap();
        let decoded = unity_texture::decode(texture.format, texture.width, texture.height, &object[start..start + size]).unwrap();
        let expected = match format {
            41 => [0, 0, 0, 255],
            42 => [128, 128, 128, 255],
            43 => [0, 0, 0, 255],
            _ => [128, 128, 0, 255],
        };
        assert_eq!(decoded, expected.repeat(16));
    }
}
