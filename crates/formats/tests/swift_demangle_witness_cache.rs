use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

// Symbols from macOS 15.7.4 and Xcode 26.0 beta system binaries, expected text from `swift-demangle`.

/// `WL` names a type and then a conformance, in the same order and with the same operands as `Wl`.
#[test]
fn a_cache_variable_names_its_type_and_its_conformance() {
    assert_eq!(
        dm("_$s7SwiftUI4EdgeO3SetVAESQAAWL"),
        "lazy protocol witness table cache variable for type SwiftUI.Edge.Set and conformance SwiftUI.Edge.Set : Swift.Equatable in SwiftUI"
    );
    assert_eq!(
        dm("_$s013CompilerSwiftA21PluginMessageHandling06HostTocD0O04LoadC17LibraryCodingKeys015_27E8E1F2E9F975n14C8237BC9B33F40M0LLOAFs0J3KeyAAWL"),
        "lazy protocol witness table cache variable for type CompilerSwiftCompilerPluginMessageHandling.HostToPluginMessage.(LoadPluginLibraryCodingKeys in _27E8E1F2E9F975F2C8237BC9B33F40E1) and conformance CompilerSwiftCompilerPluginMessageHandling.HostToPluginMessage.(LoadPluginLibraryCodingKeys in _27E8E1F2E9F975F2C8237BC9B33F40E1) : Swift.CodingKey in CompilerSwiftCompilerPluginMessageHandling"
    );
}

/// The type and the conformance's own type may differ: a type bound to arguments, and the same type with its parameters.
#[test]
fn the_type_and_the_conformance_may_differ() {
    assert_eq!(
        dm("_$s10ACPToolLib11AuthOptionsVyAA11ResetSchemaVGACyxG14ArgumentParser17ParsableArgumentsAAWL"),
        "lazy protocol witness table cache variable for type ACPToolLib.AuthOptions<ACPToolLib.ResetSchema> and conformance ACPToolLib.AuthOptions<A> : ArgumentParser.ParsableArguments in ACPToolLib"
    );
    assert_eq!(
        dm("_$s10DVTIconKit31GeneratedIconPrimitiveValueSpecOySay12CoreGraphics7CGFloatVGGACyxGSeAAWL"),
        "lazy protocol witness table cache variable for type DVTIconKit.GeneratedIconPrimitiveValueSpec<[CoreGraphics.CGFloat]> and conformance DVTIconKit.GeneratedIconPrimitiveValueSpec<A> : Swift.Decodable in DVTIconKit"
    );
    // Constructed with `swiftc -enable-library-evolution` and checked with `swift-demangle`.
    assert_eq!(dm("_$sSay1m2SQVGSayxGSTsWL"), "lazy protocol witness table cache variable for type [m.SQ] and conformance [A] : Swift.Sequence in Swift");
    assert_eq!(dm("_$s1m2SQVACAA1PAAWL"), "lazy protocol witness table cache variable for type m.SQ and conformance m.SQ : m.P in m");
}

/// Constructed with `swiftc -enable-library-evolution` (`-Onone` and `-O`) and checked with `swift-demangle`.
#[test]
fn constructed_cache_variables_decode() {
    assert_eq!(dm("_$s1m2SQVACSHAAWL"), "lazy protocol witness table cache variable for type m.SQ and conformance m.SQ : Swift.Hashable in m");
    assert_eq!(dm("_$s1m1CCACSQAAWL"), "lazy protocol witness table cache variable for type m.C and conformance m.C : Swift.Equatable in m");
    assert_eq!(dm("_$s1m1EOACSYAAWL"), "lazy protocol witness table cache variable for type m.E and conformance m.E : Swift.RawRepresentable in m");
    assert_eq!(
        dm("_$s1m1KV10CodingKeys030_4732107139B3A8FABFCC290024902C1ALLOAFs0A3KeyAAWL"),
        "lazy protocol witness table cache variable for type m.K.(CodingKeys in _4732107139B3A8FABFCC290024902B3A) and conformance m.K.(CodingKeys in _4732107139B3A8FABFCC290024902B3A) : Swift.CodingKey in m"
    );
}

/// `Tm` after the symbol adds `merged`, as it does after the other witness symbols.
#[test]
fn a_merged_cache_variable_adds_merged() {
    assert_eq!(dm("_$s1m2SQVACSHAAWLTm"), "merged lazy protocol witness table cache variable for type m.SQ and conformance m.SQ : Swift.Hashable in m");
}

/// `swift-demangle` prints `(extension in M):` before a type declared in an extension, which this demangler leaves out by design.
#[test]
fn a_type_declared_in_an_extension_leaves_out_the_extension_prefix() {
    assert_eq!(
        dm("_$s10Foundation15AttributeScopesO14XCUIAutomationE28SyntaxHighlightingAttributesVAfA0B5ScopeADWL"),
        "lazy protocol witness table cache variable for type Foundation.AttributeScopes.SyntaxHighlightingAttributes and conformance Foundation.AttributeScopes.SyntaxHighlightingAttributes : Foundation.AttributeScope in XCUIAutomation"
    );
}

/// `swift-demangle` decodes none of these: `WL` needs a type and a whole conformance, as `Wl` does, and
/// reports the same inputs invalid. A metadata symbol or a witness symbol is not a type, a type is not a
/// module, and a conformance cannot be doubled up.
#[test]
fn malformed_operands_are_invalid() {
    for symbol in [
        "$s4mainWL",
        "_$s1m2SQVAA1PAAWL",
        "_$s1m2SQVMaACSHAAWL",
        "_$s1m2SQVMaSHAAWL",
        "_$s1m2SQVSHAAMaWL",
        "_$s1m2SQVACSHAAMaWL",
        "_$s1m2SQVACSHAAWLWL",
        "_$s1m2SQVACSHAAWVWL",
        "_$s1m2SQVAcSHAAWL",
        "_$s1m2SQVACSHSiWL",
        "_$s1m2SQVSHAAWL",
        "_$s1m1fyyFACSHAAWL",
        "_$s1m1fyyFAA2SQVSHAAWL",
    ] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}

/// A conformance with generic requirements (`…Rzl` before the `W`) is not read, here as for `Wl`, and is reported
/// invalid with a plain reason rather than a wrong text.
#[test]
fn a_conformance_with_generic_requirements_is_invalid() {
    let d = demangle("_$s10Foundation3URLVSgxSgSEsSERzlWL");
    assert_eq!(d.status, Status::Invalid);
    assert_eq!(d.reason.as_deref(), Some("a conformance with generic requirements is not supported"));
}
