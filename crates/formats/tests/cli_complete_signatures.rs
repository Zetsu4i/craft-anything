use craft_anything_formats::dotnet::{
    self,
    meta::{Metadata, t},
    sig::{Cur, GenCtx, Names},
};

#[test]
fn padded_method_and_property_signatures_keep_their_types() {
    let bytes = craft_anything_testkit::managed::managed_assembly(1);
    let assembly = dotnet::open(&bytes).unwrap();
    for convention in [0, 8] {
        for tail in [0, 8, 0x41, 0xff] {
            let names = Names::new(&assembly.md);
            let mut signature = vec![convention, 1, 8, 14];
            let expected = names.method_sig(&signature, &GenCtx::default()).unwrap();
            assert!(names.signature_warning().is_none());
            signature.push(tail);
            assert_eq!(names.method_sig(&signature, &GenCtx::default()), Some(expected));
            assert!(names.signature_warning().is_some());
        }
    }
}

#[test]
fn padded_field_local_and_instantiation_signatures_keep_their_types() {
    let bytes = craft_anything_testkit::managed::managed_assembly(1);
    let assembly = dotnet::open(&bytes).unwrap();
    let names = Names::new(&assembly.md);
    let g = GenCtx::default();
    assert_eq!(names.field_sig(&[6, 8], &g).as_deref(), Some("int"));
    assert_eq!(names.locals_sig(&[7, 2, 8, 14], &g), Some(vec!["int".into(), "string".into()]));
    assert_eq!(names.method_spec_args(&[10, 1, 8], &g), Some(vec!["int".into()]));
    assert!(names.signature_warning().is_none());
    for tail in [0, 8, 0x41, 0xff] {
        let names = Names::new(&assembly.md);
        assert_eq!(names.field_sig(&[6, 8, tail], &g).as_deref(), Some("int"));
        assert!(names.signature_warning().is_some());
        let names = Names::new(&assembly.md);
        assert_eq!(names.locals_sig(&[7, 2, 8, 14, tail], &g), Some(vec!["int".into(), "string".into()]));
        assert!(names.signature_warning().is_some());
        let names = Names::new(&assembly.md);
        assert_eq!(names.method_spec_args(&[10, 1, 8, tail], &g), Some(vec!["int".into()]));
        assert!(names.signature_warning().is_some());
    }
    assert_eq!(names.locals_sig(&[7, 0], &g), Some(vec![]));
    assert!(names.signature_warning().is_none());
    assert_eq!(names.locals_sig(&[7, 0, 8], &g), Some(vec![]));
    assert!(names.signature_warning().is_some());
}

#[test]
fn nested_function_pointers_leave_the_enclosing_parameters_available() {
    let bytes = craft_anything_testkit::managed::managed_assembly(1);
    let assembly = dotnet::open(&bytes).unwrap();
    let names = Names::new(&assembly.md);
    let g = GenCtx::default();
    let signature = names.method_sig(&[0, 2, 1, 0x1b, 0, 1, 8, 8, 14], &g).unwrap();
    assert_eq!(signature.return_type, "void");
    assert_eq!(signature.params, ["delegate*<int, int>", "string"]);
    let mut cursor = Cur::new(&[8, 14]);
    assert_eq!(names.ty(&mut cursor, &g, 0).as_deref(), Some("int"));
    assert_eq!(cursor.pos, 1);
    assert_eq!(names.ty(&mut cursor, &g, 0).as_deref(), Some("string"));
    assert!(names.signature_warning().is_none());
}

fn type_spec_metadata(signature: &[u8]) -> Vec<u8> {
    let mut tables = vec![0, 0, 0, 0, 2, 0, 0, 1];
    tables.extend_from_slice(&(1u64 << t::TYPE_SPEC).to_le_bytes());
    tables.extend_from_slice(&0u64.to_le_bytes());
    tables.extend_from_slice(&1u32.to_le_bytes());
    tables.extend_from_slice(&1u16.to_le_bytes());
    let mut blobs = vec![0, signature.len() as u8];
    blobs.extend_from_slice(signature);
    let mut bytes = b"BSJB\x01\0\x01\0\0\0\0\0\x04\0\0\0v4\0\0\0\0\x02\0".to_vec();
    bytes.extend_from_slice(&52u32.to_le_bytes());
    bytes.extend_from_slice(&(tables.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"#~\0\0");
    bytes.extend_from_slice(&(52 + tables.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"#Blob\0\0\0");
    bytes.extend_from_slice(&tables);
    bytes.extend_from_slice(&blobs);
    bytes
}

#[test]
fn padded_standalone_type_specifications_keep_their_type() {
    for signature in [&[0x1d, 8][..], &[0x1d, 8, 14][..]] {
        let bytes = type_spec_metadata(signature);
        let md = Metadata::parse(&bytes).unwrap();
        assert!(md.warnings.is_empty(), "{:?}", md.warnings);
        let names = Names::new(&md);
        assert_eq!(names.type_name(t::TYPE_SPEC, 1, &GenCtx::default(), 0), "int[]");
        assert_eq!(names.signature_warning().is_some(), signature.len() > 2);
    }
}

#[test]
fn malformed_prefixes_are_not_accepted_as_padding() {
    let bytes = craft_anything_testkit::managed::managed_assembly(1);
    let assembly = dotnet::open(&bytes).unwrap();
    let names = Names::new(&assembly.md);
    let g = GenCtx::default();
    assert!(names.method_sig(&[0, 2, 8, 14], &g).is_none());
    assert!(names.field_sig(&[6, 0x1d], &g).is_none());
    assert!(names.locals_sig(&[7, 2, 8], &g).is_none());
    assert!(names.method_spec_args(&[10, 2, 8], &g).is_none());
    assert!(names.signature_warning().is_none());
    let metadata = type_spec_metadata(&[0x1d]);
    let md = Metadata::parse(&metadata).unwrap();
    let names = Names::new(&md);
    assert_eq!(names.type_name(t::TYPE_SPEC, 1, &g, 0), "?");
    assert!(names.signature_warning().is_none());
}

#[test]
fn public_views_retain_padded_signatures_and_report_the_limitation() {
    let bytes = craft_anything_testkit::managed::managed_assembly_with_padded_signatures(1);
    let assembly = dotnet::open(&bytes).unwrap();
    let inventory = dotnet::inventory(&assembly);
    assert_eq!(inventory.target_frameworks, [craft_anything_testkit::managed::TFM]);
    let members = dotnet::members(&assembly, &dotnet::MemberOptions { include_il: true, ..Default::default() });
    let calc = members.types.iter().find(|t| t.name == "Calculator").unwrap();
    assert_eq!(calc.fields[0].ty, "int");
    assert_eq!(calc.methods[0].signature, "public int Add(int a, int b)");
    let acc = calc.methods.iter().find(|m| m.name == "Accumulate").unwrap().body.as_ref().unwrap();
    assert_eq!(acc.locals, ["int"]);
    assert_eq!(acc.calls[0].target, "System.Console::WriteLine(string)");
    let native = dotnet::native_boundaries(&assembly);
    assert_eq!(native.pinvokes[0].signature, "public static extern int helper(int n)");
    for (view, warnings) in [("inventory", &inventory.warnings), ("members", &members.warnings), ("native boundaries", &native.warnings)] {
        assert_eq!(warnings.len(), 1, "{view}: {warnings:?}");
        assert!(warnings[0].contains("trailing bytes"), "{view}: {warnings:?}");
    }
}

#[test]
fn signature_note_survives_the_tools_warning_display_cap() {
    let bytes = craft_anything_testkit::managed::managed_assembly_with_padded_signatures(1);
    let mut assembly = dotnet::open(&bytes).unwrap();
    assembly.md.warnings = (0..21).map(|i| format!("earlier metadata warning {i}")).collect();
    let inventory = dotnet::inventory(&assembly);
    let members = dotnet::members(&assembly, &dotnet::MemberOptions::default());
    let native = dotnet::native_boundaries(&assembly);
    for warnings in [&inventory.warnings, &members.warnings, &native.warnings] {
        assert_eq!(warnings.len(), 22);
        assert!(warnings[0].contains("trailing bytes"));
        assert_eq!(warnings[1..], assembly.md.warnings);
    }
}
