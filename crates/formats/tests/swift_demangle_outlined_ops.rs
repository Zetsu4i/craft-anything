use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

// Real symbols are from macOS 15.7 and Xcode 26 system binaries; constructed ones are marked.
// Every expected string is what `swift-demangle` prints.

/// One real outlined operation per letter, each on a type operand before `WO`.
#[test]
fn each_outlined_letter_decodes_on_a_real_symbol() {
    assert_eq!(dm("_$s10ACPToolLib11ResetSchemaVWOh"), "outlined destroy of ACPToolLib.ResetSchema");
    assert_eq!(dm("_$s10ACPToolLib11ResetSchemaVWOc"), "outlined init with copy of ACPToolLib.ResetSchema");
    assert_eq!(dm("_$s10ACPToolLib15APIRequestErrorOWOe"), "outlined consume of ACPToolLib.APIRequestError");
    assert_eq!(dm("_$s10ACPToolLib15APIRequestErrorOWOy"), "outlined copy of ACPToolLib.APIRequestError");
    assert_eq!(dm("_$s10DVTIconKit16GeneratedIconSet_pWOb"), "outlined init with take of DVTIconKit.GeneratedIconSet");
    assert_eq!(dm("_$s10Foundation10URLRequestVSgWOd"), "outlined assign with take of Foundation.URLRequest?");
    assert_eq!(dm("_$s10Foundation3URLVSgWOf"), "outlined assign with copy of Foundation.URL?");
    assert_eq!(dm("_$s11AppThinning0B6TraitsVSgWOg"), "outlined enum get tag of AppThinning.ThinningTraits?");
    assert_eq!(dm("_$s11SwiftMacros13FunctionPartsVWOs"), "outlined release of SwiftMacros.FunctionParts");
    assert_eq!(dm("_$s11SwiftMacros13FunctionPartsVWOr"), "outlined retain of SwiftMacros.FunctionParts");
}

/// `WOi` and `WOj` take an index after the letter, which `swift-demangle` does not print.
#[test]
fn indexed_outlined_enum_operations_decode() {
    assert_eq!(dm("_$s11AppThinning0B6TraitsVSgWOi0_"), "outlined enum tag store of AppThinning.ThinningTraits?");
    assert_eq!(dm("_$s11SWBProtocol9PIFObjectOWOj0_"), "outlined enum project data for load of SWBProtocol.PIFObject");
}

/// A merged outlined operation keeps the `merged` prefix.
#[test]
fn merged_outlined_operation_decodes() {
    assert_eq!(dm("_$s10ACPToolLib12CreateRecordVWOhTm"), "merged outlined destroy of ACPToolLib.CreateRecord");
    assert_eq!(dm("_$s11SWBProtocol18ExternalToolResultOWOeTm"), "merged outlined consume of SWBProtocol.ExternalToolResult");
}

/// Every letter `swift-demangle` accepts, on a constructed `main.S`, with an index where one is
/// needed.
#[test]
fn every_outlined_letter_has_its_phrase() {
    let codes = [
        ("h", "destroy"),
        ("c", "init with copy"),
        ("e", "consume"),
        ("y", "copy"),
        ("b", "init with take"),
        ("d", "assign with take"),
        ("f", "assign with copy"),
        ("g", "enum get tag"),
        ("s", "release"),
        ("r", "retain"),
        ("i_", "enum tag store"),
        ("i0_", "enum tag store"),
        ("j_", "enum project data for load"),
        ("j3_", "enum project data for load"),
    ];
    for (code, phrase) in codes {
        assert_eq!(dm(&format!("$s4main1SVWO{code}")), format!("outlined {phrase} of main.S"), "{code}");
    }
}

/// `swift-demangle` decodes none of these: a module or a global before `WO`, no letter, a letter
/// it does not know, an `i` with no index or an unterminated one, or an outlined operation on
/// another outlined operation.
#[test]
fn outlined_operation_without_a_known_letter_or_a_type_is_invalid() {
    for symbol in ["$s4mainWOh", "$s4main1SVWO", "$s4main1SVWOz", "$s4main1SVWOi", "$s4main1SVWOi0", "$s4main1SVWOiTm", "$s4main1SVMaWOh", "$s4main1SVWOhWOh"] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}
