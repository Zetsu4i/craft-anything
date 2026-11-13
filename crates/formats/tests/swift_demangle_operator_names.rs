use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

// Symbols from macOS 15.7 and Xcode 26 system binaries, expected text from `swift-demangle`.

/// An infix operator is the operator's letters (`2ee` is `==`, `1p` is `+`) followed by `oi`.
#[test]
fn an_infix_operator_function_decodes() {
    assert_eq!(dm("_$s10Foundation4DateV2eeoiySbAC_ACtFZ"), "static Foundation.Date.== infix(Foundation.Date, Foundation.Date) -> Swift.Bool");
    assert_eq!(dm("_$s10Foundation4DateV1poiyA2C_SdtFZ"), "static Foundation.Date.+ infix(Foundation.Date, Swift.Double) -> Foundation.Date");
    assert_eq!(dm("_$s10Foundation4DateV2peoiyyACz_SdtFZ"), "static Foundation.Date.+= infix(inout Foundation.Date, Swift.Double) -> ()");
    assert_eq!(
        dm("_$s10Foundation9IndexPathV2geoiySbAC_ACtFZ"),
        "static Foundation.IndexPath.>= infix(Foundation.IndexPath, Foundation.IndexPath) -> Swift.Bool"
    );
    assert_eq!(dm("_$sSi1roiyS2i_SitFZ"), "static Swift.Int.% infix(Swift.Int, Swift.Int) -> Swift.Int");
    assert_eq!(dm("_$s9CoreAsset1xoiy10Foundation4UUIDVAE_AEtF"), "CoreAsset.^ infix(Foundation.UUID, Foundation.UUID) -> Foundation.UUID");
}

/// Every letter that stands for an operator character: `&@/=>< *!|+?%-~^.` in one place each.
#[test]
fn each_operator_letter_prints_its_character() {
    let letters = [
        ('a', '&'),
        ('c', '@'),
        ('d', '/'),
        ('e', '='),
        ('g', '>'),
        ('l', '<'),
        ('m', '*'),
        ('n', '!'),
        ('o', '|'),
        ('p', '+'),
        ('q', '?'),
        ('r', '%'),
        ('s', '-'),
        ('t', '~'),
        ('x', '^'),
        ('z', '.'),
    ];
    // Constructed and checked with `swift-demangle`: `$s4main1<letter>oiyyF`.
    for (letter, ch) in letters {
        assert_eq!(dm(&format!("$s4main1{letter}oiyyF")), format!("main.{ch} infix() -> ()"));
    }
    // Several characters in one name, from the standard library and Xcode.
    assert_eq!(
        dm("_$s17RealityFoundation2aaoiyAA14QueryPredicateVyxGAE_AEtlF"),
        "RealityFoundation.&& infix<A>(RealityFoundation.QueryPredicate<A>, RealityFoundation.QueryPredicate<A>) -> RealityFoundation.QueryPredicate<A>"
    );
    assert_eq!(dm("_$s10PreviewsUI2teoiySbAA14KnownAmberTypeV_SStF"), "PreviewsUI.~= infix(PreviewsUI.KnownAmberType, Swift.String) -> Swift.Bool");
    assert_eq!(
        dm("_$s13PreviewsModel14SourceLocationV5BoundV3zzloiyAcE_AEtFZ"),
        "static PreviewsModel.SourceLocation.Bound...< infix(PreviewsModel.SourceLocation.Bound, PreviewsModel.SourceLocation.Bound) -> PreviewsModel.SourceLocation"
    );
}

/// `op` is a prefix operator and `oP` a postfix one.
#[test]
fn prefix_and_postfix_operator_functions_decode() {
    assert_eq!(
        dm("_$s13SwiftProtobuf1sopyAA07Google_B9_DurationVADF"),
        "SwiftProtobuf.- prefix(SwiftProtobuf.Google_Protobuf_Duration) -> SwiftProtobuf.Google_Protobuf_Duration"
    );
    assert_eq!(
        dm("_$s18GPUToolsAdvancedUI8IVPoint3V1sopyA2CFZ"),
        "static GPUToolsAdvancedUI.IVPoint3.- prefix(GPUToolsAdvancedUI.IVPoint3) -> GPUToolsAdvancedUI.IVPoint3"
    );
    assert_eq!(dm("_$ss15UnboundedRange_O3zzzoPyyABFZ"), "static Swift.UnboundedRange_.... postfix(Swift.UnboundedRange_) -> ()");
    // Constructed with `swiftc -enable-library-evolution`: `prefix func ^^` and `postfix func ^^`
    // on a struct, and `static prefix func !`.
    assert_eq!(dm("_$s1x2xxopyAA1PVADF"), "x.^^ prefix(x.P) -> x.P");
    assert_eq!(dm("_$s1x2xxoPyAA1PVADF"), "x.^^ postfix(x.P) -> x.P");
    assert_eq!(dm("_$s1x1PV1nopySbACFZ"), "static x.P.! prefix(x.P) -> Swift.Bool");
}

/// The operator is a name like any other, so the symbols built around a function decode with it:
/// witnesses, dispatch thunks, method descriptors, merged functions and generic parameters.
#[test]
fn symbols_built_around_an_operator_function_decode() {
    assert_eq!(
        dm("_$s10Foundation11MeasurementVyxGSLAASL2geoiySbx_xtFZTW"),
        "protocol witness for static Swift.Comparable.>= infix(A, A) -> Swift.Bool in conformance Foundation.Measurement<A> : Swift.Comparable in Foundation"
    );
    assert_eq!(
        dm("_$s13UVTestSupport23RunDestinationPredicateV0E0O2aaoiyA2E_AEtFZTm"),
        "merged static UVTestSupport.RunDestinationPredicate.Predicate.&& infix(UVTestSupport.RunDestinationPredicate.Predicate, UVTestSupport.RunDestinationPredicate.Predicate) -> UVTestSupport.RunDestinationPredicate.Predicate"
    );
    // Constructed: a protocol requirement `static func <=>`.
    assert_eq!(dm("_$s1x1QP3legoiySbx_xtFZTj"), "dispatch thunk of static x.Q.<=> infix(A, A) -> Swift.Bool");
    assert_eq!(dm("_$s1x1QP3legoiySbx_xtFZTq"), "method descriptor for static x.Q.<=> infix(A, A) -> Swift.Bool");
    assert_eq!(dm("_$s1x1GV2qqoiySbACyxG_AEtFZ"), "static x.G.?? infix(x.G<A>, x.G<A>) -> Swift.Bool");
}

/// An operator declared in an extension prints without the `(extension in …)` prefix, as for any
/// other function in this demangler.
#[test]
fn an_operator_in_an_extension_leaves_out_the_extension_prefix() {
    // `swift-demangle` prints `static (extension in CoreDevice):CoreDevice._Error.~= infix …`.
    assert_eq!(dm("_$s10CoreDevice6_ErrorPAAE2teoiySbx_s0C0_ptFZ"), "static CoreDevice._Error.~= infix(A, Swift.Error) -> Swift.Bool");
}

/// A private operator function names the file after the operator: `(< infix in _<hash>)`.
#[test]
fn a_private_operator_function_decodes() {
    assert_eq!(
        dm("_$s7SWBCore1loi33_02E29C2CDABF15C01B3E4CEF0E561D16LLySbAA9SpecProxyC_AEtF"),
        "SWBCore.(< infix in _02E29C2CDABF15C01B3E4CEF0E561D16)(SWBCore.SpecProxy, SWBCore.SpecProxy) -> Swift.Bool"
    );
    // Constructed: `fileprivate func %%`.
    assert_eq!(dm("_$s1x2rroi33_613077CFEB536AFE339AC751B60607CDLLySiAA1PV_AEtF"), "x.(%% infix in _613077CFEB536AFE339AC751B60607CD)(x.P, x.P) -> Swift.Int");
}

/// Constructed with `swift-demangle` as the judge: the operator's name and fixity have to be
/// well formed. A letter that is not an operator character, a missing name, an unknown fixity
/// letter or a fixity with nothing before it are all rejected.
#[test]
fn a_malformed_operator_is_invalid() {
    for symbol in [
        // Letters that stand for no operator character: `b`, `f`, `h`, `i`, `k`, `u`, `y`.
        "$s4main1boiyyF",
        "$s4main1foiyyF",
        "$s4main1hoiyyF",
        "$s4main1ioiyyF",
        "$s4main1uoiyyF",
        "$s4main1yoiyyF",
        // A capital, a digit and an underscore are not operator characters either.
        "$s4main1AoiyyF",
        "$s4main1ABoiyyF",
        "$s4main10oiyyF",
        "$s4main1_oiyyF",
        "$s4main2e_oiyyF",
        // One bad character among good ones.
        "$s4main3eeboiyyF",
        // An unknown fixity letter, or none.
        "$s4main2eeoqyyF",
        "$s4main2eeoIyyF",
        "$s4main2eeoyyF",
        "$s4main2eeo",
        // No name before the fixity, or something that is not an identifier.
        "$s4mainoiyyF",
        "$s4mainSioiyyF",
        "$s4main1eoi1eoiyyF",
        // The private discriminator goes after the fixity, not before it.
        "$s4main2ee33_613077CFEB536AFE339AC751B60607CDLLoiyyF",
        "$s4main2ee33_613077CFEB536AFE339AC751B60607CDoiLLyyF",
        // An operator is a name, not a module or a type.
        "$s4main2eeoi1fyyF",
        "$s4main2eeoiSg",
    ] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}
