use craft_anything_formats::dotnet::{
    self,
    sig::{Cur, GenCtx, Names},
};

fn type_name(signature: &[u8]) -> Option<String> {
    let bytes = craft_anything_testkit::managed::managed_assembly(1);
    let assembly = dotnet::open(&bytes).unwrap();
    Names::new(&assembly.md).ty(&mut Cur::new(signature), &GenCtx::default(), 0)
}

#[test]
fn rank_one_arrays_keep_their_identity_separate_from_vectors() {
    assert_eq!(type_name(&[0x1d, 8]).as_deref(), Some("int[]"));
    assert_eq!(type_name(&[0x14, 8, 1, 0, 0]).as_deref(), Some("int[*]"));
    assert_eq!(type_name(&[0x14, 8, 1, 0, 1, 0]).as_deref(), Some("int[0...]"));
}

#[test]
fn dimensions_retain_sizes_and_signed_lower_bounds() {
    for (signature, expected) in [
        (&[0x14, 8, 1, 1, 3, 0][..], "int[0...2]"),
        (&[0x14, 8, 2, 2, 2, 3, 2, 2, 12][..], "int[1...2,6...8]"),
        (&[0x14, 8, 3, 1, 3, 2, 4, 0x7f][..], "int[2...4,-1...,]"),
        (&[0x14, 8, 2, 0, 0][..], "int[,]"),
        (&[0x14, 8, 1, 1, 0, 1, 0][..], "int[0...-1]"),
    ] {
        assert_eq!(type_name(signature).as_deref(), Some(expected));
    }
}

#[test]
fn nested_array_kinds_keep_their_order() {
    assert_eq!(type_name(&[0x1d, 0x14, 8, 1, 0, 0]).as_deref(), Some("int[*][]"));
    assert_eq!(type_name(&[0x14, 0x1d, 8, 1, 0, 0]).as_deref(), Some("int[][*]"));
}

#[test]
fn signature_projections_keep_the_same_array_shape() {
    let bytes = craft_anything_testkit::managed::managed_assembly(1);
    let assembly = dotnet::open(&bytes).unwrap();
    let names = Names::new(&assembly.md);
    let g = GenCtx::default();
    assert_eq!(names.field_sig(&[6, 0x14, 8, 1, 0, 0], &g).as_deref(), Some("int[*]"));
    assert_eq!(names.locals_sig(&[7, 1, 0x14, 8, 1, 0, 0], &g), Some(vec!["int[*]".into()]));
    let method = names.method_sig(&[0, 1, 1, 0x14, 8, 1, 0, 0], &g).unwrap();
    assert_eq!(method.params, ["int[*]"]);
}

#[test]
fn shape_counts_and_truncated_bounds_remain_bounded() {
    for signature in
        [&[0x14, 8, 0, 0, 0][..], &[0x14, 8, 33, 0, 0][..], &[0x14, 8, 1, 2, 3, 4, 0][..], &[0x14, 8, 1, 0, 2, 0, 0][..], &[0x14, 8, 1, 0, 1, 0x80][..]]
    {
        assert_eq!(type_name(signature), None);
    }
    let largest = [0x14, 8, 1, 1, 0xdf, 0xff, 0xff, 0xff, 1, 0xdf, 0xff, 0xff, 0xfe];
    assert_eq!(type_name(&largest).as_deref(), Some("int[268435455...805306365]"));
}
