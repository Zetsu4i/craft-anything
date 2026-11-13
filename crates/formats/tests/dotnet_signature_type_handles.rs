use craft_anything_formats::dotnet::{meta::Metadata, meta::t, sig::Cur, sig::GenCtx, sig::Names};

fn metadata() -> Vec<u8> {
    let strings = b"\0Container`2\0Tests\0";
    let mut blobs = vec![0];
    let mut specs = Vec::new();
    let mut recursive = vec![0x15, 0x12, 5, 64];
    for _ in 0..64 {
        recursive.extend_from_slice(&[0x12, 14]); // CLASS TypeSpec row 3
    }
    for blob in [&[0x15, 0x12, 5, 2, 8, 14][..], &[0x1d, 8], recursive.as_slice()] {
        specs.extend_from_slice(&(blobs.len() as u16).to_le_bytes());
        if blob.len() < 128 {
            blobs.push(blob.len() as u8);
        } else {
            blobs.extend_from_slice(&[0x80 | ((blob.len() >> 8) as u8), blob.len() as u8]);
        }
        blobs.extend_from_slice(blob);
    }
    let mut tables = vec![0, 0, 0, 0, 2, 0, 0, 1];
    tables.extend_from_slice(&((1u64 << t::TYPE_REF) | (1u64 << t::TYPE_SPEC)).to_le_bytes());
    tables.extend_from_slice(&0u64.to_le_bytes());
    tables.extend_from_slice(&1u32.to_le_bytes());
    tables.extend_from_slice(&3u32.to_le_bytes());
    for cell in [0u16, 1, 13] {
        tables.extend_from_slice(&cell.to_le_bytes());
    }
    tables.extend_from_slice(&specs);

    let streams = [("#~", tables.as_slice()), ("#Strings", strings.as_slice()), ("#Blob", blobs.as_slice())];
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
fn class_and_value_type_handles_must_not_reference_type_specs() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    for kind in [0x11, 0x12] {
        for reference in [6, 10] {
            assert_eq!(names.field_sig(&[6, kind, reference], &GenCtx::default()), None);
        }
    }
}

#[test]
fn a_generic_constructor_must_name_a_definition_or_reference() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    for kind in [0x11, 0x12] {
        assert_eq!(names.field_sig(&[6, 0x15, kind, 6, 1, 8], &GenCtx::default()), None);
    }
}

#[test]
fn nested_signatures_preserve_the_type_handle_rule() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    let g = GenCtx::default();
    assert_eq!(names.field_sig(&[6, 0x1d, 0x12, 6], &g), None);
    assert_eq!(names.method_sig(&[0, 1, 1, 0x12, 6], &g), None);
    assert_eq!(names.locals_sig(&[7, 1, 0x12, 6], &g), None);
    assert_eq!(names.method_spec_args(&[0x0a, 1, 0x12, 6], &g), None);
    assert_eq!(names.ty(&mut Cur::new(&[0x12, 6]), &g, 0), None);
}

#[test]
fn standalone_specs_and_spec_custom_modifiers_remain_supported() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    assert!(md.warnings.is_empty(), "{:?}", md.warnings);
    let names = Names::new(&md);
    let g = GenCtx::default();
    assert_eq!(names.type_name(t::TYPE_SPEC, 1, &g, 0), "Tests.Container<int, string>");
    assert_eq!(names.type_name(t::TYPE_SPEC, 2, &g, 0), "int[]");
    for modifier in [0x1f, 0x20] {
        assert_eq!(names.field_sig(&[6, modifier, 6, 8], &g).as_deref(), Some("int"));
    }
    assert_eq!(names.field_sig(&[6, 0x15, 0x12, 5, 2, 8, 14], &g).as_deref(), Some("Tests.Container<int, string>"));
    assert_eq!(names.field_sig(&[6, 0x12, 5], &g).as_deref(), Some("Tests.Container`2"));
}

#[test]
#[ignore = "run separately with a process timeout against the recursive baseline"]
fn invalid_type_spec_recursion_stops_before_expanding_its_branches() {
    let bytes = metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    assert_eq!(names.type_name(t::TYPE_SPEC, 3, &GenCtx::default(), 0), "?");
}
