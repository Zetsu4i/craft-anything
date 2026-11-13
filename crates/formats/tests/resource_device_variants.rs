use craft_anything_formats::{arsc::config_qualifiers, bytes::Reader};

fn config(f: impl FnOnce(&mut [u8])) -> (String, String) {
    let mut cfg = [0; 64];
    cfg[0] = 64;
    f(&mut cfg);
    config_qualifiers(&Reader::new(&cfg), 0)
}

#[test]
fn carrier_zero_is_distinct_from_unspecified() {
    assert_eq!(config(|c| c[6..8].fill(255)).0, "mnc0");
    assert_eq!(config(|_| {}).0, "default");
}

#[test]
fn round_screen_variants_are_distinct() {
    assert_eq!(config(|c| c[48] = 2).0, "round");
    assert_eq!(config(|c| c[48] = 1).0, "notround");
}

#[test]
fn color_capabilities_precede_orientation() {
    assert_eq!(
        config(|c| {
            c[12] = 2;
            c[49] = 10;
        })
        .0,
        "widecg-highdr-land"
    );
    assert_eq!(config(|c| c[49] = 5).0, "nowidecg-lowdr");
}

#[test]
fn keyboard_and_navigation_availability_keep_selector_order() {
    assert_eq!(
        config(|c| {
            c[16] = 2;
            c[17] = 2;
            c[18] = 11;
        })
        .0,
        "keyssoft-qwerty-navhidden-dpad"
    );
}

#[test]
fn inflection_changes_the_resource_config_but_not_its_locale() {
    assert_eq!(
        config(|c| {
            c[8..10].copy_from_slice(b"fr");
            c[19] = 2;
        }),
        ("fr-feminine".into(), "fr".into())
    );
}

#[test]
fn undeclared_configuration_tail_does_not_create_selectors() {
    assert_eq!(
        config(|c| {
            c[0] = 36;
            c[48] = 2;
            c[49] = 10;
        })
        .0,
        "default"
    );
}
