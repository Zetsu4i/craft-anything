use craft_anything_formats::FormatError;
use craft_anything_formats::pixels::{self, Layout};
use craft_anything_formats::unreal_texture::{self, MipData};
use craft_anything_testkit::games::unreal_texture_package;

fn decoded_package(format: &str, width: i32, height: i32, bytes: &[u8]) -> Vec<u8> {
    let (asset, export) = unreal_texture_package("T_Float", format, width, height, bytes, None);
    let package = unreal_texture::package(&asset).unwrap();
    let entry = &package.exports[0];
    let offset = (entry.serial_offset - package.header_size) as usize;
    let data = &export[offset..offset + entry.serial_size as usize];
    let texture = unreal_texture::texture(data, &package.names).unwrap();
    let mip = &texture.mips[0];
    let MipData::Inline(start, size) = mip.data else { panic!("inline fixture") };
    pixels::decode(unreal_texture::layout(&texture.pixel_format).unwrap(), mip.width, mip.height, &data[start..start + size]).unwrap()
}

#[test]
fn cooked_half_and_full_precision_formats_reach_the_pixel_decoder() {
    let half: Vec<_> = [0x3800u16, 0x3400, 0x3c00, 0x3800].into_iter().flat_map(u16::to_le_bytes).collect();
    let full: Vec<_> = [0.5f32, 0.25, 1.0, 0.5].into_iter().flat_map(f32::to_le_bytes).collect();
    for (names, bytes, stride) in [(["PF_R16F", "PF_G16R16F", "PF_FloatRGBA"], &half, 2), (["PF_R32_FLOAT", "PF_G32R32F", "PF_A32B32G32R32F"], &full, 4)] {
        for (name, (channels, expected)) in names.into_iter().zip([(1, [188, 188, 188, 255]), (2, [188, 137, 0, 255]), (4, [188, 137, 255, 128])]) {
            assert_eq!(pixels::size(unreal_texture::layout(name).unwrap(), 1, 1), Some(channels * stride));
            assert_eq!(decoded_package(name, 1, 1, &bytes[..channels * stride]), expected);
        }
    }
    assert_eq!(unreal_texture::layout("PF_R16F_FILTER"), Some(Layout::RHalf));
    assert_eq!(unreal_texture::layout("PF_G16R16F_FILTER"), Some(Layout::RgHalf));
    assert_eq!(unreal_texture::layout("PF_FloatR11G11B10"), Some(pixels::Layout::Rgb111110Float));
}

#[test]
fn nonfinite_and_out_of_range_channels_clip_without_changing_alpha_semantics() {
    let half: Vec<_> = [0xfc00u16, 0x7e00, 0x7c00, 0x7c00].into_iter().flat_map(u16::to_le_bytes).collect();
    let full: Vec<_> = [-2.0f32, f32::NAN, 2.0, f32::INFINITY].into_iter().flat_map(f32::to_le_bytes).collect();
    assert_eq!(decoded_package("PF_FloatRGBA", 1, 1, &half), [0, 0, 255, 255]);
    assert_eq!(decoded_package("PF_A32B32G32R32F", 1, 1, &full), [0, 0, 255, 255]);
    let alpha_nan: Vec<_> = [1.0f32, 0.0, 0.0, f32::NAN].into_iter().flat_map(f32::to_le_bytes).collect();
    assert_eq!(decoded_package("PF_A32B32G32R32F", 1, 1, &alpha_nan), [255, 0, 0, 0]);
}

#[test]
fn unreal_rows_keep_their_stored_order() {
    let bytes: Vec<_> = [1.0f32, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.5].into_iter().flat_map(f32::to_le_bytes).collect();
    assert_eq!(decoded_package("PF_A32B32G32R32F", 1, 2, &bytes), [255, 0, 0, 255, 0, 255, 0, 128]);
}

#[test]
fn new_formats_retain_truncation_and_output_budget_checks() {
    for name in ["PF_R16F", "PF_G16R16F", "PF_FloatRGBA", "PF_R32_FLOAT", "PF_G32R32F", "PF_A32B32G32R32F"] {
        let layout = unreal_texture::layout(name).unwrap();
        let size = pixels::size(layout, 1, 1).unwrap();
        assert!(matches!(pixels::decode(layout, 1, 1, &vec![0; size - 1]), Err(FormatError::Truncated(_))));
        assert!(matches!(pixels::decode(layout, 0, 1, &[]), Err(FormatError::Malformed(_))));
        assert!(matches!(pixels::decode_with_limit(layout, 1, 1, &vec![0; size], 3), Err(FormatError::TooLarge(_))));
        assert!(matches!(pixels::decode(layout, u32::MAX, u32::MAX, &[]), Err(FormatError::TooLarge(_))));
    }
}
