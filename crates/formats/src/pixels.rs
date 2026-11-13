//! Turning game textures' stored pixels into RGBA: uncompressed layouts and the 4×4 block formats
//! (DXT1/BC1, DXT3/BC2, DXT5/BC3, BC4, BC5, BC6H, BC7, ETC1 and EAC). Shared by the Unity and Unreal readers; rows come out in the
//! order they're stored (Unity stores the bottom row first, Unreal the top).

mod eac;

use crate::bptc;
use crate::common::FormatError;

mod packed;

/// Default RGBA output budget: 256 MiB, matching the texture exporters' 8192×8192 pixel limit.
pub const DEFAULT_MAX_RGBA_BYTES: u64 = 256 << 20;

/// How a texture's pixels are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Alpha only (shown as white with that alpha).
    Alpha8,
    /// One grey channel.
    R8,
    /// One unsigned 16-bit channel, shown as grey.
    R16,
    /// Two unsigned 8-bit channels; blue is zero and alpha is opaque.
    Rg8,
    /// Two unsigned 16-bit channels; blue is zero and alpha is opaque.
    Rg16,
    /// Three unsigned 16-bit channels; alpha is opaque.
    Rgb16,
    /// Four unsigned 16-bit channels, including alpha.
    Rgba16,
    RHalf,
    RgHalf,
    RgbaHalf,
    RFloat,
    RgFloat,
    RgbaFloat,
    /// Nine-bit RGB mantissas with a shared five-bit exponent, displayed as opaque sRGB.
    Rgb9e5,
    /// Packed unsigned 10-bit RGB with two-bit alpha, low bits first.
    Rgba1010102,
    /// Packed unsigned 11/11/10-bit floating-point RGB, displayed as opaque sRGB.
    Rgb111110Float,
    Rgb24,
    Rgba32,
    Argb32,
    Bgra32,
    Rgb565,
    Rgba4444,
    Argb4444,
    Dxt1,
    /// Four-colour blocks with one explicit four-bit alpha value per texel.
    Dxt3,
    Dxt5,
    /// One channel in blocks (shown as grey).
    Bc4,
    /// Two channels in blocks, usually a normal map (blue is rebuilt from red and green).
    Bc5,
    /// High dynamic range RGB in blocks (unsigned half floats), tone-mapped to 8-bit sRGB.
    Bc6h,
    /// BC6H with signed half floats.
    Bc6hSigned,
    Bc7,
    /// ETC1 RGB, with individual or differential base colours.
    Etc1,
    /// One EAC channel, shown as grey.
    EacR,
    /// One signed EAC channel; -1..1 maps to 0..255.
    EacRSigned,
    /// Two EAC channels; blue is zero and alpha is opaque.
    EacRg,
    /// Two signed EAC channels; -1..1 maps to 0..255.
    EacRgSigned,
}

/// Bytes of a `width`×`height` picture in `layout`.
pub fn size(layout: Layout, width: u32, height: u32) -> Option<usize> {
    let (w, h) = (width as usize, height as usize);
    let blocks = w.div_ceil(4).checked_mul(h.div_ceil(4))?;
    match layout {
        Layout::Alpha8 | Layout::R8 => w.checked_mul(h),
        Layout::Rgb565 | Layout::Rgba4444 | Layout::Argb4444 | Layout::RHalf | Layout::R16 | Layout::Rg8 => w.checked_mul(h)?.checked_mul(2),
        Layout::Rgb24 => w.checked_mul(h)?.checked_mul(3),
        Layout::Rg16 => w.checked_mul(h)?.checked_mul(4),
        Layout::Rgb16 => w.checked_mul(h)?.checked_mul(6),
        Layout::Rgba16 => w.checked_mul(h)?.checked_mul(8),
        Layout::Rgba32 | Layout::Argb32 | Layout::Bgra32 | Layout::RgHalf | Layout::RFloat | Layout::Rgb9e5 | Layout::Rgba1010102 | Layout::Rgb111110Float => {
            w.checked_mul(h)?.checked_mul(4)
        }
        Layout::RgbaHalf | Layout::RgFloat => w.checked_mul(h)?.checked_mul(8),
        Layout::RgbaFloat => w.checked_mul(h)?.checked_mul(16),
        Layout::Dxt1 | Layout::Bc4 | Layout::Etc1 | Layout::EacR | Layout::EacRSigned => blocks.checked_mul(8),
        Layout::Dxt3 | Layout::Dxt5 | Layout::Bc5 | Layout::Bc6h | Layout::Bc6hSigned | Layout::Bc7 | Layout::EacRg | Layout::EacRgSigned => {
            blocks.checked_mul(16)
        }
    }
}

fn unorm8(value: u16) -> u8 {
    ((u32::from(value) * 255 + 32767) / 65535) as u8
}

/// Quantize unsigned channels; missing blue is zero and missing alpha is opaque.
fn unorm16_pixels(samples: impl Iterator<Item = u16>, channels: usize) -> impl Iterator<Item = [u8; 4]> {
    let mut samples = samples.map(unorm8);
    std::iter::from_fn(move || {
        let (r, g) = (samples.next()?, samples.next()?);
        let b = if channels > 2 { samples.next()? } else { 0 };
        let a = if channels > 3 { samples.next()? } else { 255 };
        Some([r, g, b, a])
    })
}

/// Treat RGB as linear light, alpha as coverage, and show a scalar texture in grey.
fn float_pixels(mut samples: impl Iterator<Item = f32>, channels: usize) -> impl Iterator<Item = [u8; 4]> {
    std::iter::from_fn(move || {
        let r = bptc::srgb8(samples.next()?);
        if channels == 1 {
            return Some([r, r, r, 255]);
        }
        let g = bptc::srgb8(samples.next()?);
        if channels == 2 {
            return Some([r, g, 0, 255]);
        }
        let b = bptc::srgb8(samples.next()?);
        let a = (samples.next()?.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some([r, g, b, a])
    })
}

fn expand5(v: u16) -> u8 {
    let v = (v & 31) as u8;
    (v << 3) | (v >> 2)
}

fn expand6(v: u16) -> u8 {
    let v = (v & 63) as u8;
    (v << 2) | (v >> 4)
}

fn rgb565(v: u16) -> [u8; 4] {
    [expand5(v >> 11), expand6(v >> 5), expand5(v), 255]
}

fn nibbles(v: u16, order: [u32; 4]) -> [u8; 4] {
    order.map(|shift| ((v >> shift) & 15) as u8 * 17)
}

/// The four colours of a DXT colour block; `punch` allows the transparent fourth (DXT1 only).
fn dxt_colours(b: &[u8; 8], punch: bool) -> [[u8; 4]; 4] {
    let (c0, c1) = (u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]));
    let (p, q) = (rgb565(c0), rgb565(c1));
    let mix = |a: u8, b: u8, wa: u16, wb: u16| ((u16::from(a) * wa + u16::from(b) * wb) / (wa + wb)) as u8;
    let blend = |wa: u16, wb: u16| [mix(p[0], q[0], wa, wb), mix(p[1], q[1], wa, wb), mix(p[2], q[2], wa, wb), 255];
    if c0 > c1 || !punch { [p, q, blend(2, 1), blend(1, 2)] } else { [p, q, blend(1, 1), [0, 0, 0, 0]] }
}

/// The 16 texels of a DXT colour block, row by row.
fn dxt_texels(b: &[u8; 8], punch: bool) -> [[u8; 4]; 16] {
    let colours = dxt_colours(b, punch);
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    std::array::from_fn(|k| colours[((bits >> (2 * k)) & 3) as usize])
}

/// ETC1 uses big-endian blocks and column-major selector bits, independently of the split.
/// Differential bases outside RGB555 are undefined by the format and are refused.
fn etc1(block: &[u8; 8]) -> Option<[[u8; 4]; 16]> {
    const MODIFIERS: [[i16; 2]; 8] = [[2, 8], [5, 17], [9, 29], [13, 42], [18, 60], [24, 80], [33, 106], [47, 183]];
    let bits = u64::from_be_bytes(*block);
    let differential = bits & (1 << 33) != 0;
    let horizontal = bits & (1 << 32) != 0;
    let mut bases = [[0i16; 3]; 2];
    for (channel, shift) in [56, 48, 40].into_iter().enumerate() {
        let value = ((bits >> shift) & 255) as i16;
        let (first, second) = if differential {
            let first = value >> 3;
            let delta = ((value & 7) ^ 4) - 4;
            let second = first + delta;
            if !(0..=31).contains(&second) {
                return None;
            }
            ((first << 3) | (first >> 2), (second << 3) | (second >> 2))
        } else {
            ((value >> 4) * 17, (value & 15) * 17)
        };
        bases[0][channel] = first;
        bases[1][channel] = second;
    }
    let first_table = MODIFIERS.get(((bits >> 37) & 7) as usize)?;
    let second_table = MODIFIERS.get(((bits >> 34) & 7) as usize)?;
    Some(std::array::from_fn(|pixel| {
        let (x, y) = (pixel % 4, pixel / 4);
        let second = if horizontal { y >= 2 } else { x >= 2 };
        let (base, table) = if second { (bases[1], second_table) } else { (bases[0], first_table) };
        let selector = x * 4 + y;
        let magnitude = if bits & (1 << selector) == 0 { table[0] } else { table[1] };
        let delta = if bits & (1 << (selector + 16)) == 0 { magnitude } else { -magnitude };
        let rgb = base.map(|channel| (channel + delta).clamp(0, 255) as u8);
        [rgb[0], rgb[1], rgb[2], 255]
    }))
}

/// The 16 values of an 8-byte single-channel block (DXT5's alpha, BC4, each half of BC5).
fn channel_block(b: &[u8; 8]) -> [u8; 16] {
    let (a0, a1) = (u16::from(b[0]), u16::from(b[1]));
    let values: [u8; 8] = if a0 > a1 {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            i => (((8 - i as u16) * a0 + (i as u16 - 1) * a1) / 7) as u8,
        })
    } else {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            6 => 0,
            7 => 255,
            i => (((6 - i as u16) * a0 + (i as u16 - 1) * a1) / 5) as u8,
        })
    };
    let bits = b.iter().skip(2).rev().fold(0u64, |n, x| (n << 8) | u64::from(*x));
    std::array::from_fn(|k| values[((bits >> (3 * k)) & 7) as usize])
}

fn halves(b: &[u8; 16]) -> ([u8; 8], [u8; 8]) {
    let mut first = [0u8; 8];
    let mut second = [0u8; 8];
    for (i, x) in b.iter().enumerate() {
        if i < 8 {
            first[i] = *x;
        } else {
            second[i - 8] = *x;
        }
    }
    (first, second)
}

fn dxt3(b: &[u8; 16]) -> [[u8; 4]; 16] {
    let (alpha, colour) = halves(b);
    let mut texels = dxt_texels(&colour, false);
    for (texel, value) in texels.iter_mut().zip(alpha.iter().flat_map(|byte| [byte & 15, byte >> 4])) {
        texel[3] = value * 17;
    }
    texels
}

fn dxt5(b: &[u8; 16]) -> [[u8; 4]; 16] {
    let (alpha, colour) = halves(b);
    let mut texels = dxt_texels(&colour, false);
    for (t, a) in texels.iter_mut().zip(channel_block(&alpha)) {
        t[3] = a;
    }
    texels
}

fn bc4(b: &[u8; 8]) -> [[u8; 4]; 16] {
    channel_block(b).map(|r| [r, r, r, 255])
}

fn bc5(b: &[u8; 16]) -> [[u8; 4]; 16] {
    let (red, green) = halves(b);
    let (r, g) = (channel_block(&red), channel_block(&green));
    std::array::from_fn(|k| {
        // A unit normal's z from its x and y, each stored as 0..255 for -1..1.
        let (x, y) = (f32::from(r[k]) / 127.5 - 1.0, f32::from(g[k]) / 127.5 - 1.0);
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        [r[k], g[k], ((z + 1.0) * 127.5) as u8, 255]
    })
}

/// Place 4×4 blocks (left to right, then row by row) into a `w`×`h` RGBA picture.
fn blocks(out: &mut [u8], w: usize, h: usize, texels: impl Iterator<Item = [[u8; 4]; 16]>) {
    let across = w.div_ceil(4);
    for (i, block) in texels.enumerate() {
        let (bx, by) = (i % across, i / across);
        for (k, p) in block.iter().enumerate() {
            let (x, y) = (bx * 4 + k % 4, by * 4 + k / 4);
            if x < w && y < h {
                let at = (y * w + x) * 4;
                if let Some(dst) = out.get_mut(at..at + 4) {
                    dst.copy_from_slice(p);
                }
            }
        }
    }
}

/// Decode a `width`×`height` picture (`data` starts with it) into RGBA, rows in stored order.
///
/// Caps the output at [`DEFAULT_MAX_RGBA_BYTES`]; use [`decode_with_limit`] for a different budget.
pub fn decode(layout: Layout, width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>, FormatError> {
    decode_with_limit(layout, width, height, data, DEFAULT_MAX_RGBA_BYTES)
}

/// Decode into RGBA, allowing at most `max_bytes` of output.
///
/// The budget applies to RGBA bytes, not the encoded input, and is checked before reading pixels
/// or reserving the output. Output reservation failures are reported as [`FormatError::TooLarge`].
pub fn decode_with_limit(layout: Layout, width: u32, height: u32, data: &[u8], max_bytes: u64) -> Result<Vec<u8>, FormatError> {
    let need = size(layout, width, height).ok_or_else(|| FormatError::TooLarge(format!("a {width}×{height} picture")))?;
    let (w, h) = (width as usize, height as usize);
    let out_len = w
        .checked_mul(h)
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|bytes| *bytes <= isize::MAX as usize)
        .ok_or_else(|| FormatError::TooLarge(format!("a {width}×{height} RGBA picture")))?;
    if out_len as u64 > max_bytes {
        return Err(FormatError::TooLarge(format!("a {width}×{height} RGBA picture exceeds {max_bytes} bytes")));
    }
    let data = data.get(..need).ok_or_else(|| FormatError::Truncated(format!("{} bytes of pixels, {need} needed", data.len())))?;
    if w == 0 || h == 0 {
        return Err(FormatError::Malformed("an empty picture".into()));
    }
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve_exact(out_len).map_err(|_| FormatError::TooLarge(format!("a {width}×{height} RGBA picture")))?;
    out.resize(out_len, 0);
    let mut put = |px: &mut dyn Iterator<Item = [u8; 4]>| {
        for (dst, p) in out.as_chunks_mut::<4>().0.iter_mut().zip(px) {
            *dst = p;
        }
    };
    let u16s = data.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b));
    let floats = data.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b));
    match layout {
        Layout::RHalf => put(&mut float_pixels(u16s.map(bptc::half), 1)),
        Layout::RgHalf => put(&mut float_pixels(u16s.map(bptc::half), 2)),
        Layout::RgbaHalf => put(&mut float_pixels(u16s.map(bptc::half), 4)),
        Layout::RFloat => put(&mut float_pixels(floats, 1)),
        Layout::RgFloat => put(&mut float_pixels(floats, 2)),
        Layout::RgbaFloat => put(&mut float_pixels(floats, 4)),
        Layout::Rgba1010102 => put(&mut data.as_chunks::<4>().0.iter().map(packed::rgba1010102)),
        Layout::Rgb111110Float => put(&mut data.as_chunks::<4>().0.iter().map(packed::rgb111110)),
        Layout::Rgb9e5 => put(&mut data.as_chunks::<4>().0.iter().map(|bytes| {
            let packed = u32::from_le_bytes(*bytes);
            let scale = 2.0f32.powi((packed >> 27) as i32 - 24);
            let channel = |shift: u32| bptc::srgb8(((packed >> shift) & 0x1ff) as f32 * scale);
            [channel(0), channel(9), channel(18), 255]
        })),
        Layout::Alpha8 => put(&mut data.iter().map(|a| [255, 255, 255, *a])),
        Layout::R8 => put(&mut data.iter().map(|r| [*r, *r, *r, 255])),
        Layout::R16 => put(&mut u16s.map(|value| {
            let grey = unorm8(value);
            [grey, grey, grey, 255]
        })),
        Layout::Rg8 => put(&mut data.as_chunks::<2>().0.iter().map(|[r, g]| [*r, *g, 0, 255])),
        Layout::Rg16 => put(&mut unorm16_pixels(u16s, 2)),
        Layout::Rgb16 => put(&mut unorm16_pixels(u16s, 3)),
        Layout::Rgba16 => put(&mut unorm16_pixels(u16s, 4)),
        Layout::Rgb24 => put(&mut data.as_chunks::<3>().0.iter().map(|[r, g, b]| [*r, *g, *b, 255])),
        Layout::Rgba32 => put(&mut data.as_chunks::<4>().0.iter().copied()),
        Layout::Argb32 => put(&mut data.as_chunks::<4>().0.iter().map(|[a, r, g, b]| [*r, *g, *b, *a])),
        Layout::Bgra32 => put(&mut data.as_chunks::<4>().0.iter().map(|[b, g, r, a]| [*r, *g, *b, *a])),
        Layout::Rgb565 => put(&mut u16s.map(rgb565)),
        Layout::Rgba4444 => put(&mut u16s.map(|v| nibbles(v, [12, 8, 4, 0]))),
        Layout::Argb4444 => put(&mut u16s.map(|v| nibbles(v, [8, 4, 0, 12]))),
        Layout::Dxt1 => blocks(&mut out, w, h, data.as_chunks::<8>().0.iter().map(|b| dxt_texels(b, true))),
        Layout::Dxt3 => blocks(&mut out, w, h, data.as_chunks::<16>().0.iter().map(dxt3)),
        Layout::Dxt5 => blocks(&mut out, w, h, data.as_chunks::<16>().0.iter().map(dxt5)),
        Layout::Bc4 => blocks(&mut out, w, h, data.as_chunks::<8>().0.iter().map(bc4)),
        Layout::Bc5 => blocks(&mut out, w, h, data.as_chunks::<16>().0.iter().map(bc5)),
        Layout::Bc6h => blocks(&mut out, w, h, data.as_chunks::<16>().0.iter().map(|b| bptc::bc6h(b, false))),
        Layout::Bc6hSigned => blocks(&mut out, w, h, data.as_chunks::<16>().0.iter().map(|b| bptc::bc6h(b, true))),
        Layout::Bc7 => blocks(&mut out, w, h, data.as_chunks::<16>().0.iter().map(bptc::bc7)),
        Layout::EacR | Layout::EacRSigned => {
            blocks(&mut out, w, h, data.as_chunks::<8>().0.iter().map(|block| eac::scalar(block, layout == Layout::EacRSigned)))
        }
        Layout::EacRg | Layout::EacRgSigned => {
            blocks(&mut out, w, h, data.as_chunks::<16>().0.iter().map(|block| eac::dual(block, layout == Layout::EacRgSigned)))
        }
        Layout::Etc1 => {
            let mut valid = true;
            let texels = data.as_chunks::<8>().0.iter().map_while(|block| {
                let pixels = etc1(block);
                valid &= pixels.is_some();
                pixels
            });
            blocks(&mut out, w, h, texels);
            if !valid {
                return Err(FormatError::Malformed("ETC1 differential colour is outside its five-bit range".into()));
            }
        }
    }
    Ok(out)
}

/// Flip a `width`-wide RGBA picture upside down (Unity stores the bottom row first).
pub fn flip_rows(rgba: &[u8], width: u32) -> Vec<u8> {
    let row = (width as usize).saturating_mul(4).max(1);
    rgba.chunks(row).rev().flatten().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncompressed_layouts() {
        assert_eq!(decode(Layout::Rgba32, 1, 2, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap(), [1, 2, 3, 4, 5, 6, 7, 8], "stored order");
        assert_eq!(decode(Layout::Argb32, 1, 1, &[9, 1, 2, 3]).unwrap(), [1, 2, 3, 9]);
        assert_eq!(decode(Layout::Bgra32, 1, 1, &[3, 2, 1, 9]).unwrap(), [1, 2, 3, 9]);
        assert_eq!(decode(Layout::Rgb24, 1, 1, &[1, 2, 3]).unwrap(), [1, 2, 3, 255]);
        assert_eq!(decode(Layout::Alpha8, 1, 1, &[7]).unwrap(), [255, 255, 255, 7]);
        assert_eq!(decode(Layout::R8, 1, 1, &[7]).unwrap(), [7, 7, 7, 255]);
        assert_eq!(decode(Layout::Rgb565, 1, 1, &0xf800u16.to_le_bytes()).unwrap(), [255, 0, 0, 255]);
        assert_eq!(decode(Layout::Rgba4444, 1, 1, &0xf00fu16.to_le_bytes()).unwrap(), [255, 0, 0, 255]);
        assert_eq!(decode(Layout::Argb4444, 1, 1, &0xff00u16.to_le_bytes()).unwrap(), [255, 0, 0, 255]);
        assert_eq!(flip_rows(&[1, 1, 1, 1, 2, 2, 2, 2], 1), [2, 2, 2, 2, 1, 1, 1, 1]);
        assert!(matches!(decode(Layout::Rgba32, 2, 2, &[0; 15]), Err(FormatError::Truncated(_))));
        assert!(decode(Layout::Rgba32, 0, 1, &[]).is_err());
    }

    #[test]
    fn block_layouts() {
        // DXT1: colour 0 red, colour 1 blue; the first texel uses index 1, the rest index 0.
        let px = decode(Layout::Dxt1, 4, 4, &[0x00, 0xf8, 0x1f, 0x00, 0b01, 0, 0, 0]).unwrap();
        assert_eq!((&px[..4], &px[4..8]), (&[0, 0, 255, 255][..], &[255, 0, 0, 255][..]));
        // DXT5: alpha 255 → 0; the first texel index 1 (alpha 0), the rest index 0 (255).
        let mut b = vec![255, 0, 1, 0, 0, 0, 0, 0];
        b.extend_from_slice(&[0x00, 0xf8, 0x1f, 0x00, 0, 0, 0, 0]);
        let px = decode(Layout::Dxt5, 4, 4, &b).unwrap();
        assert_eq!((px[3], px[7]), (0, 255));
        // BC4: grey from one channel block; BC5: red and green, blue rebuilt (flat normal → 255).
        assert_eq!(&decode(Layout::Bc4, 4, 4, &[200, 10, 0, 0, 0, 0, 0, 0]).unwrap()[..4], &[200, 200, 200, 255]);
        let mut n = vec![128, 128, 0, 0, 0, 0, 0, 0];
        n.extend_from_slice(&[128, 128, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&decode(Layout::Bc5, 4, 4, &n).unwrap()[..4], &[128, 128, 254, 255]);
        // 5×5 needs 4 blocks and is cropped.
        assert_eq!(decode(Layout::Dxt1, 5, 5, &[0; 32]).unwrap().len(), 5 * 5 * 4);
    }
}
