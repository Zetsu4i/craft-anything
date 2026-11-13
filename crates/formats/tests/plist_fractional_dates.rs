use craft_anything_formats::{keyed_archive, plist};
use craft_anything_testkit::apple as fx;
use serde_json::json;

#[test]
fn binary_dates_keep_fractional_seconds_on_both_sides_of_the_epoch() {
    for (seconds, expected) in [
        (0.125, "2001-01-01T00:00:00.125Z"),
        (-0.25, "2000-12-31T23:59:59.75Z"),
        (86_400.000_001, "2001-01-02T00:00:00.000001Z"),
        (-978_307_200.5, "1969-12-31T23:59:59.5Z"),
    ] {
        let bytes = fx::to_bplist(&fx::P::Date(seconds));
        assert_eq!(plist::parse(&bytes).unwrap().to_json(), json!({"$date": expected}));
    }
}

#[test]
fn keyed_archive_dates_keep_their_subsecond_time() {
    let objects =
        vec![fx::s("$null"), fx::dict(vec![("$classname", fx::s("NSDate"))]), fx::dict(vec![("$class", fx::P::Uid(1)), ("NS.time", fx::P::Real(-0.125))])];
    let bytes = fx::to_bplist(&fx::dict(vec![("$objects", fx::P::Array(objects)), ("$top", fx::dict(vec![("root", fx::P::Uid(2))]))]));
    let archive = keyed_archive::parse(&bytes).unwrap();
    assert_eq!(archive.project(2), json!({"$date": "2000-12-31T23:59:59.875Z"}));
}

#[test]
fn rounding_carries_into_the_next_second_and_day() {
    assert_eq!(plist::apple_date_iso(86_399.999_999_6), "2001-01-02T00:00:00Z");
    assert_eq!(plist::apple_date_iso(-0.000_000_4), "2001-01-01T00:00:00Z");
    assert_eq!(plist::apple_date_iso(0.000_001), "2001-01-01T00:00:00.000001Z");
    assert_eq!(plist::apple_date_iso(0.000_000_001), "2001-01-01T00:00:00Z");
}

#[test]
fn modern_dates_do_not_display_float_noise_as_nanosecond_precision() {
    for (seconds, expected) in
        [(700_000_000.3, "2023-03-08T20:26:40.3Z"), (-700_000_000.3, "1978-10-27T03:33:19.7Z"), (800_000_000.123_456, "2026-05-09T06:13:20.123456Z")]
    {
        let bytes = fx::to_bplist(&fx::P::Date(seconds));
        assert_eq!(plist::parse(&bytes).unwrap().to_json(), json!({"$date": expected}));
    }
}

#[test]
fn whole_seconds_xml_and_non_finite_values_keep_their_existing_projection() {
    assert_eq!(plist::apple_date_iso(0.0), "2001-01-01T00:00:00Z");
    assert_eq!(plist::apple_date_iso(-1.0), "2000-12-31T23:59:59Z");
    assert_eq!(plist::apple_date_iso(f64::INFINITY), "inf");
    assert_eq!(plist::apple_date_iso(f64::NAN), "NaN");
    let xml = b"<plist><date>2001-01-01T00:00:00.125Z</date></plist>";
    assert_eq!(plist::parse(xml).unwrap().to_json(), json!({"$date": "2001-01-01T00:00:00.125Z"}));
}
