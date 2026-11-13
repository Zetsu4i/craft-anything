use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

// Symbols from macOS 15.7 and Xcode 26.0 beta system binaries, expected text from `swift-demangle`.

/// `Tv` with no index is outlined variable #0 of the symbol before it.
#[test]
fn an_outlined_variable_of_an_initializer_decodes() {
    assert_eq!(
        dm("_$s013CompilerSwiftA21PluginMessageHandling0C7FeatureO8rawValueACSgSS_tcfCTv_"),
        "outlined variable #0 of CompilerSwiftCompilerPluginMessageHandling.PluginFeature.init(rawValue: Swift.String) -> CompilerSwiftCompilerPluginMessageHandling.PluginFeature?"
    );
    assert_eq!(
        dm("_$s10DVTIconKit27DVTGeneratedIconEmojiVendorC18iconSetIdentifiersSaySSGvgTv_"),
        "outlined variable #0 of DVTIconKit.DVTGeneratedIconEmojiVendor.iconSetIdentifiers.getter : [Swift.String]"
    );
}

/// The operand can be a protocol witness, which carries its conformance.
#[test]
fn an_outlined_variable_of_a_protocol_witness_decodes() {
    assert_eq!(
        dm("_$s013CompilerSwiftA21PluginMessageHandling0C7FeatureOSYAASY8rawValuexSg03RawH0Qz_tcfCTWTv_"),
        "outlined variable #0 of protocol witness for Swift.RawRepresentable.init(rawValue: A.RawValue) -> A? in conformance CompilerSwiftCompilerPluginMessageHandling.PluginFeature : Swift.RawRepresentable in CompilerSwiftCompilerPluginMessageHandling"
    );
}

/// `Tv0_` is outlined variable #1: the index is one more than the number before the underscore.
#[test]
fn an_index_is_counted_from_one() {
    assert_eq!(
        dm("_$s10ACPToolLib13ConfirmPromptO13confirmAction_8canForceSbSS_SbtKFZTv0_"),
        "outlined variable #1 of static ACPToolLib.ConfirmPrompt.confirmAction(_: Swift.String, canForce: Swift.Bool) throws -> Swift.Bool"
    );
    assert_eq!(dm("_$s4main3fooyyFTv_"), "outlined variable #0 of main.foo() -> ()");
    assert_eq!(dm("_$s4main3fooyyFTv0_"), "outlined variable #1 of main.foo() -> ()");
    assert_eq!(dm("_$s4main3fooyyFTv00_"), "outlined variable #1 of main.foo() -> ()");
    assert_eq!(dm("_$s4main3fooyyFTv10_"), "outlined variable #11 of main.foo() -> ()");
}

/// The largest index `swift-demangle` prints is 2^31 - 1.
#[test]
fn the_largest_index_decodes() {
    assert_eq!(dm("_$s4main3fooyyFTv2147483646_"), "outlined variable #2147483647 of main.foo() -> ()");
}

/// Operands that are a getter, a thunk or a resume function of a thunk.
#[test]
fn the_operand_can_be_an_accessor_or_a_thunk() {
    assert_eq!(dm("_$s4main1AV1xSivgTv_"), "outlined variable #0 of main.A.x.getter : Swift.Int");
    assert_eq!(dm("_$s4main1AV1xSivgTv0_"), "outlined variable #1 of main.A.x.getter : Swift.Int");
    assert_eq!(dm("_$s4main3fooyyFTuTv_"), "outlined variable #0 of async function pointer to main.foo() -> ()");
    assert_eq!(dm("_$s4main3fooyyFTjTv_"), "outlined variable #0 of dispatch thunk of main.foo() -> ()");
    assert_eq!(dm("_$s4main3fooyyFTY0_Tv_"), "outlined variable #0 of (1) suspend resume partial function for main.foo() -> ()");
}

/// A type, or a metadata global of a type, is an operand too.
#[test]
fn the_operand_can_be_a_type_or_a_type_global() {
    assert_eq!(dm("_$s4main1AVTv_"), "outlined variable #0 of main.A");
    assert_eq!(dm("_$s4main1AVNTv_"), "outlined variable #0 of type metadata for main.A");
    assert_eq!(dm("_$s4main1AVWVTv_"), "outlined variable #0 of value witness table for main.A");
}

/// An index that is missing, unterminated, not a number, or 2^31 or more is invalid, as in `swift-demangle`.
#[test]
fn a_missing_or_bad_index_is_invalid() {
    for symbol in [
        "_$s4main3fooyyFTv",
        "_$s4main3fooyyFTv1",
        "_$s4main3fooyyFTvx_",
        "_$s4main3fooyyFTv2147483647_",
        "_$s4main3fooyyFTv2147483648_",
        "_$s4main3fooyyFTv4294967296_",
    ] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}

/// An outlined variable cannot be the operand of another one, and an operand that is not a whole
/// declaration or witness (a bare getter, or a witness with no conformance) is invalid.
#[test]
fn an_outlined_variable_of_an_outlined_variable_is_invalid() {
    for symbol in ["_$s4main3fooyyFTv_Tv_", "_$s4mainSivgTv_", "_$s4main3fooyyFTWTv_"] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}
