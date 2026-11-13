use craft_anything_formats::{
    FormatError, pixels,
    unreal_texture::{self, MipData},
};

fn cooked(format: &str, width: i32, height: i32, payload: &[u8]) -> Vec<u8> {
    let (asset, exports) = craft_anything_testkit::games::unreal_texture_package("T_Packed", format, width, height, payload, None);
    let package = unreal_texture::package(&asset).unwrap();
    let export = &package.exports[0];
    let start = (export.serial_offset - package.header_size) as usize;
    let object = &exports[start..start + export.serial_size as usize];
    let texture = unreal_texture::texture(object, &package.names).unwrap();
    let mip = &texture.mips[0];
    let MipData::Inline(start, length) = mip.data else { panic!("inline fixture") };
    pixels::decode(unreal_texture::layout(&texture.pixel_format).unwrap(), mip.width, mip.height, &object[start..start + length]).unwrap()
}

#[test]
fn normalized_ten_bit_channels_keep_their_order_and_two_bit_alpha() {
    for alpha in 0..4u32 {
        let packed = 1023 | (512 << 10) | (alpha << 30);
        assert_eq!(cooked("PF_A2B10G10R10", 1, 1, &packed.to_le_bytes()), [255, 128, 0, alpha as u8 * 85]);
    }
    let packed = (1023u32 << 20) | (3 << 30);
    assert_eq!(cooked("PF_A2B10G10R10", 1, 1, &packed.to_le_bytes()), [0, 0, 255, 255]);
}

#[test]
fn packed_float_channels_use_their_individual_mantissa_widths() {
    // R=0.5 and G=0.25 use six mantissa bits; B=1 uses five.
    let packed = (14u32 << 6) | ((13 << 6) << 11) | ((15 << 5) << 22);
    assert_eq!(cooked("PF_FloatR11G11B10", 1, 1, &packed.to_le_bytes()), [188, 137, 255, 255]);
    // R=0.375, G=0.75, B=0.09375 each exercise a nonzero mantissa.
    let packed = ((13u32 << 6) | 32) | (((14 << 6) | 32) << 11) | (((11 << 5) | 16) << 22);
    assert_eq!(cooked("PF_FloatR11G11B10", 1, 1, &packed.to_le_bytes()), [165, 225, 86, 255]);
}

#[test]
fn packed_float_zero_subnormals_and_nonfinite_values_use_the_existing_display_rules() {
    let layout = unreal_texture::layout("PF_FloatR11G11B10").unwrap();
    for packed in [0u32, 1 | (1 << 11) | (1 << 22), 63 | (63 << 11) | (31 << 22)] {
        assert_eq!(pixels::decode(layout, 1, 1, &packed.to_le_bytes()).unwrap(), [0, 0, 0, 255]);
    }
    // +infinity, NaN and a finite value above one.
    let packed = (31u32 << 6) | (((31 << 6) | 1) << 11) | ((16 << 5) << 22);
    assert_eq!(pixels::decode(layout, 1, 1, &packed.to_le_bytes()).unwrap(), [255, 0, 255, 255]);
}

#[test]
fn cooked_packed_textures_keep_unreal_row_order() {
    let words = [1023u32 | (3 << 30), (1023 << 10) | (2 << 30)];
    let bytes = words.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
    assert_eq!(cooked("PF_A2B10G10R10", 1, 2, &bytes), [255, 0, 0, 255, 0, 255, 0, 170]);
}

#[test]
fn packed_layouts_retain_input_and_output_bounds() {
    for format in ["PF_A2B10G10R10", "PF_FloatR11G11B10"] {
        let layout = unreal_texture::layout(format).unwrap();
        assert_eq!(pixels::size(layout, 3, 2), Some(24));
        assert!(matches!(pixels::decode(layout, 1, 1, &[0; 3]), Err(FormatError::Truncated(_))));
        assert!(matches!(pixels::decode(layout, 0, 1, &[]), Err(FormatError::Malformed(_))));
        assert!(matches!(pixels::decode_with_limit(layout, 1, 1, &[0; 4], 3), Err(FormatError::TooLarge(_))));
        assert!(matches!(pixels::decode(layout, u32::MAX, u32::MAX, &[]), Err(FormatError::TooLarge(_))));
        let mut data = vec![0; 4];
        let expected = pixels::decode(layout, 1, 1, &data).unwrap();
        data.extend_from_slice(&[255; 4]);
        assert_eq!(pixels::decode(layout, 1, 1, &data).unwrap(), expected);
    }
}
