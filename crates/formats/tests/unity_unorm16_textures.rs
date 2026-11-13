use craft_anything_formats::{FormatError, unity_texture};

fn samples(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[test]
fn multichannel_sixteen_bit_formats_keep_their_names_and_byte_widths() {
    for (format, name, bytes) in [(72, "RG32", 4), (73, "RGB48", 6), (74, "RGBA64", 8)] {
        assert_eq!(unity_texture::format_name(format), name);
        assert_eq!(unity_texture::mip0_size(format, 3, 2), Some(6 * bytes));
        assert_eq!(unity_texture::mip0_size(format, u32::MAX, u32::MAX), None);
    }
}

#[test]
fn channels_are_little_endian_and_normalized_without_float_gamma() {
    assert_eq!(unity_texture::decode(72, 1, 1, &samples(&[32768, 65535])).unwrap(), [128, 255, 0, 255]);
    assert_eq!(unity_texture::decode(73, 1, 1, &samples(&[65535, 32767, 0])).unwrap(), [255, 127, 0, 255]);
    assert_eq!(unity_texture::decode(74, 1, 1, &samples(&[0, 32768, 65535, 32768])).unwrap(), [0, 128, 255, 128]);
    assert_eq!(unity_texture::decode(74, 1, 1, &samples(&[128, 129, 65406, 65407])).unwrap(), [0, 1, 254, 255]);
}

#[test]
fn every_unsigned_channel_matches_the_existing_scalar_quantization() {
    for (value, grey) in [(0, 0), (128, 0), (129, 1), (32767, 127), (32768, 128), (65535, 255)] {
        assert_eq!(unity_texture::decode(9, 1, 1, &samples(&[value])).unwrap(), [grey, grey, grey, 255]);
        assert_eq!(unity_texture::decode(72, 1, 1, &samples(&[value; 2])).unwrap(), [grey, grey, 0, 255]);
        assert_eq!(unity_texture::decode(73, 1, 1, &samples(&[value; 3])).unwrap(), [grey, grey, grey, 255]);
        assert_eq!(unity_texture::decode(74, 1, 1, &samples(&[value; 4])).unwrap(), [grey; 4]);
    }
}

#[test]
fn all_channel_layouts_preserve_pixels_when_unity_rows_are_flipped() {
    for (format, channels) in [(72, 2), (73, 3), (74, 4)] {
        let mut values = vec![65535; channels * 2];
        values.extend(vec![0; channels * 2]);
        let top = match channels {
            2 | 3 => [0, 0, 0, 255],
            _ => [0; 4],
        };
        let bottom = if channels == 2 { [255, 255, 0, 255] } else { [255; 4] };
        let expected: Vec<u8> = top.repeat(2).into_iter().chain(bottom.repeat(2)).collect();
        assert_eq!(unity_texture::decode(format, 2, 2, &samples(&values)).unwrap(), expected);
    }
}

#[test]
fn short_pixels_and_output_budgets_fail_before_decoding() {
    for (format, channels) in [(72, 2), (73, 3), (74, 4)] {
        let pixel = samples(&vec![65535; channels]);
        for end in 0..pixel.len() {
            assert!(matches!(unity_texture::decode(format, 1, 1, &pixel[..end]), Err(FormatError::Truncated(_))));
        }
        assert!(matches!(unity_texture::decode(format, 0, 1, &[]), Err(FormatError::Malformed(_))));
        assert!(matches!(unity_texture::decode(format, 8193, 8193, &[]), Err(FormatError::TooLarge(_))));
        let mut next_mip = pixel.clone();
        next_mip.extend(vec![0; channels * 2]);
        assert_eq!(unity_texture::decode(format, 1, 1, &pixel).unwrap(), unity_texture::decode(format, 1, 1, &next_mip).unwrap());
    }
}

#[test]
fn serialized_unity_textures_reach_each_multichannel_decoder() {
    let version = "2022.3.12f1";
    for (format, channels) in [(72, 2), (73, 3), (74, 4)] {
        let pixel = samples(&vec![32768; channels]);
        let object = craft_anything_testkit::games::unity_texture2d(version, "DetailedChannels", 1, 1, format, Some(&pixel), None);
        let texture = unity_texture::parse(&object, version, false).unwrap();
        let (start, length) = texture.inline.unwrap();
        assert_eq!(unity_texture::mip0_size(texture.format, texture.width, texture.height), Some(length));
        let expected = match channels {
            2 => [128, 128, 0, 255],
            3 => [128, 128, 128, 255],
            _ => [128; 4],
        };
        assert_eq!(unity_texture::decode(texture.format, texture.width, texture.height, &object[start..start + length]).unwrap(), expected);
    }
}
