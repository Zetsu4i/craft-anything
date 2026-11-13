use craft_anything_formats::dotnet::{meta::Metadata, meta::t, sig::GenCtx, sig::Names};

fn text(heap: &mut Vec<u8>, value: &str) -> u16 {
    let offset = heap.len() as u16;
    heap.extend_from_slice(value.as_bytes());
    heap.push(0);
    offset
}

fn metadata() -> Vec<u8> {
    let mut strings = vec![0];
    let mut rows = Vec::new();
    for (name, namespace) in [
        ("IsVolatile", "System.Runtime.CompilerServices"),
        ("IsVolatile", "Custom"),
        ("IsVolatileExtra", "System.Runtime.CompilerServices"),
        ("Marker", "Custom"),
    ] {
        let name = text(&mut strings, name);
        let namespace = text(&mut strings, namespace);
        for cell in [0u16, name, namespace] {
            rows.extend_from_slice(&cell.to_le_bytes());
        }
    }
    let mut tables = vec![0, 0, 0, 0, 2, 0, 0, 1];
    tables.extend_from_slice(&(1u64 << t::TYPE_REF).to_le_bytes());
    tables.extend_from_slice(&0u64.to_le_bytes());
    tables.extend_from_slice(&4u32.to_le_bytes());
    tables.extend_from_slice(&rows);
    let streams = [("#~", tables.as_slice()), ("#Strings", strings.as_slice()), ("#Blob", &[0][..])];
    let mut out = Vec::from(&b"BSJB\x01\0\x01\0\0\0\0\0\x04\0\0\0v4\0\0\0\0\x03\0"[..]);
    let mut offset = out.len() + streams.iter().map(|(name, _)| 8 + (name.len() + 1).div_ceil(4) * 4).sum::<usize>();
    for (name, bytes) in streams {
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        out.resize(out.len().div_ceil(4) * 4, 0);
        offset += bytes.len();
    }
    for (_, bytes) in streams {
        out.extend_from_slice(bytes);
    }
    out
}

#[test]
fn an_optional_marker_does_not_imply_volatile() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    assert!(md.warnings.is_empty(), "{:?}", md.warnings);
    let names = Names::new(&md);
    let g = GenCtx::default();
    assert_eq!(names.field_sig(&[6, 0x20, 5, 8], &g).as_deref(), Some("int"));
    let method = names.method_sig(&[0, 1, 0x1f, 5, 8, 0x20, 5, 8], &g).unwrap();
    assert_eq!(method.return_type, "volatile int");
    assert_eq!(method.params, ["int"]);
}

#[test]
fn the_marker_requires_the_exact_qualified_name() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    for reference in [9, 13] {
        assert_eq!(names.field_sig(&[6, 0x1f, reference, 8], &GenCtx::default()).as_deref(), Some("int"));
    }
}

#[test]
fn required_markers_after_many_other_modifiers_are_not_lost() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    for count in [8, 32] {
        let mut signature = vec![6];
        for _ in 0..count {
            signature.extend_from_slice(&[0x20, 17]);
        }
        signature.extend_from_slice(&[0x1f, 5, 8]);
        assert_eq!(names.field_sig(&signature, &GenCtx::default()).as_deref(), Some("volatile int"));
    }
}

#[test]
fn required_markers_remain_supported_in_nested_types() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    let g = GenCtx::default();
    assert_eq!(names.field_sig(&[6, 0x1f, 5, 8], &g).as_deref(), Some("volatile int"));
    assert_eq!(names.field_sig(&[6, 0x0f, 0x1f, 5, 8], &g).as_deref(), Some("volatile int*"));
    assert_eq!(names.field_sig(&[6, 0x1d, 0x1f, 5, 8], &g).as_deref(), Some("volatile int[]"));
}

#[test]
fn ordinary_types_and_unrelated_modifiers_stay_unchanged() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    let g = GenCtx::default();
    assert_eq!(names.field_sig(&[6, 8], &g).as_deref(), Some("int"));
    for kind in [0x1f, 0x20] {
        assert_eq!(names.field_sig(&[6, kind, 17, 8], &g).as_deref(), Some("int"));
    }
}

#[test]
fn a_recognized_marker_does_not_hide_a_malformed_modifier_tail() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    for tail in [&[0x20][..], &[0x20, 0x80], &[0x20, 3, 8], &[]] {
        let mut signature = vec![6, 0x1f, 5];
        signature.extend_from_slice(tail);
        assert_eq!(names.field_sig(&signature, &GenCtx::default()), None);
    }
}
