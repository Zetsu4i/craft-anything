use craft_anything_formats::swift_demangle::{Status, demangle};

fn rejects_expansion(symbol: &str) {
    let result = demangle(symbol);
    assert_eq!(result.status, Status::Invalid);
    assert_eq!(result.reason.as_deref(), Some("symbol expands too much"));
    assert_eq!(result.demangled, None);
}

#[test]
fn nested_nominal_substitution_copies_use_the_budget() {
    rejects_expansion(&format!("$s1M{}Ma", "1AV".repeat(400)));
}

#[test]
fn optional_substitution_copies_use_the_budget() {
    rejects_expansion(&format!("$s1M1AV{}Ma", "Sg".repeat(300)));
}

#[test]
fn ordinary_nested_types_and_repeated_substitutions_still_decode() {
    for symbol in ["$s1M1AV1BVMa", "$s4main6WidgetC6sharedACvpZ", "_$sxSg8ShelfKit13ObserverValueABMc"] {
        let result = demangle(symbol);
        assert_eq!(result.status, Status::Demangled, "{symbol}: {:?}", result.reason);
        assert!(result.reason.is_none());
    }
    let symbol = format!("$s1M{}Ma", "1AV".repeat(40));
    let result = demangle(&symbol);
    assert_eq!(result.status, Status::Demangled);
    assert_eq!(result.demangled.as_deref(), Some(format!("type metadata accessor for M{}", ".A".repeat(40)).as_str()));
}
