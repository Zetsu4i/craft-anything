use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

fn invalid(symbol: &str) {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Invalid, "{symbol} should be invalid, got {:?}", d.demangled);
}

// Symbols from macOS 15.7 and Xcode 26.0 beta system binaries, expected text from `swift-demangle`.
// Symbols starting `$s4main` or `$sSi` were built with `swiftc` or written by hand and checked
// with `swift-demangle`.

/// `MN` after a generic type's operands is the noncanonical metadata of a specialized generic type.
/// The three system symbols with this letter are `_$sSRys6UInt16VGMN`, `_$sSRys6UInt32VGMN`
/// and `_$sScMN`; the last one is a bare `N` after the name `Swift.MainActor`, not `MN`.
#[test]
fn noncanonical_specialized_metadata_decodes() {
    assert_eq!(dm("_$sSRys6UInt16VGMN"), "noncanonical specialized generic type metadata for Swift.UnsafeBufferPointer<Swift.UInt16>");
    assert_eq!(dm("_$sSRys6UInt32VGMN"), "noncanonical specialized generic type metadata for Swift.UnsafeBufferPointer<Swift.UInt32>");
    assert_eq!(dm("$s4main1SVMN"), "noncanonical specialized generic type metadata for main.S");
    assert_eq!(dm("$sSiMN"), "noncanonical specialized generic type metadata for Swift.Int");
    assert_eq!(dm("_$sScMN"), "type metadata for Swift.MainActor");
}

/// `Mj` names the key of an opaque type's descriptor accessor, after the type it is for.
#[test]
fn opaque_type_descriptor_accessor_key_decodes() {
    assert_eq!(dm("$s4main1SVMj"), "opaque type descriptor accessor key for main.S");
    assert_eq!(dm("$sSiMj"), "opaque type descriptor accessor key for Swift.Int");
}

/// `Mz` is the flag the runtime reads to load a canonical specialized generic type's metadata.
#[test]
fn canonical_specialized_metadata_flag_decodes() {
    assert_eq!(dm("$s4main1SVMz"), "flag for loading of canonical specialized generic type metadata for main.S");
    assert_eq!(dm("$sSiMz"), "flag for loading of canonical specialized generic type metadata for Swift.Int");
}

/// The three kinds above need a whole type after them. A protocol, a struct member and a conformance
/// descriptor's name are not types for these kinds, so `swift-demangle` leaves the symbols raw.
#[test]
fn metadata_kinds_reject_operands_that_are_not_types() {
    invalid("$s4main1PMN");
    invalid("$s4main1PMz");
    invalid("$s4main1SV3fooMN");
    invalid("$sSC4FooVMN");
    invalid("$s4main5MyActorVMN");
}
