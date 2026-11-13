//! EAC's 11-bit channel blocks, reduced to display bytes after reconstruction and clamping.
//! The signed layouts map -1..1 to 0..255 so negative values remain visible; zero is 128.
//! Layout and modifier tables: <https://registry.khronos.org/DataFormat/specs/1.4/dataformat.1.4.html>.

// Positive halves of the Khronos EAC modifier tables. Negative entries are -positive - 1.
const MODIFIERS: [[i16; 4]; 16] = [
    [2, 5, 8, 14],
    [2, 6, 9, 12],
    [1, 4, 7, 12],
    [1, 3, 5, 12],
    [2, 5, 7, 11],
    [2, 6, 8, 10],
    [3, 6, 7, 10],
    [2, 4, 7, 10],
    [1, 5, 7, 9],
    [1, 4, 7, 9],
    [1, 3, 7, 9],
    [1, 4, 6, 9],
    [2, 3, 6, 9],
    [0, 1, 2, 9],
    [3, 5, 7, 8],
    [2, 4, 6, 8],
];

fn channel(block: &[u8; 8], signed: bool) -> [u8; 16] {
    let bits = u64::from_be_bytes(*block);
    let base = if signed { i16::from((block[0] as i8).max(-127)) * 8 } else { i16::from(block[0]) * 8 + 4 };
    let scale = (i16::from(block[1] >> 4) * 8).max(1);
    let table = MODIFIERS.get(usize::from(block[1] & 15)).copied().unwrap_or_default();
    std::array::from_fn(|pixel| {
        let (x, y) = (pixel % 4, pixel / 4);
        let selector = ((bits >> (45 - 3 * (x * 4 + y))) & 7) as usize;
        let magnitude = table.get(selector & 3).copied().unwrap_or_default();
        let modifier = if selector < 4 { -magnitude - 1 } else { magnitude };
        let value = base + modifier * scale;
        if signed {
            let value = u32::try_from(value.clamp(-1023, 1023) + 1023).unwrap_or_default();
            ((value * 255 + 1023) / 2046) as u8
        } else {
            let value = u32::try_from(value.clamp(0, 2047)).unwrap_or_default();
            ((value * 255 + 1023) / 2047) as u8
        }
    })
}

pub(super) fn scalar(block: &[u8; 8], signed: bool) -> [[u8; 4]; 16] {
    channel(block, signed).map(|value| [value, value, value, 255])
}

pub(super) fn dual(block: &[u8; 16], signed: bool) -> [[u8; 4]; 16] {
    let (red, green) = super::halves(block);
    let (red, green) = (channel(&red, signed), channel(&green, signed));
    std::array::from_fn(|pixel| [red[pixel], green[pixel], 0, 255])
}
