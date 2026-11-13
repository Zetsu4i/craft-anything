use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

/// Symbols from macOS 15.7 and Xcode 26.0 beta system binaries, expected text from `swift-demangle`.
#[test]
fn one_time_initialization_symbols_name_the_variable() {
    assert_eq!(dm("_$s10ACPToolLib11RemoveTokenV13configuration_WZ"), "one-time initialization function for configuration");
    assert_eq!(dm("_$s10ACPToolLib11RemoveTokenV13configuration_Wz"), "one-time initialization token for configuration");
    assert_eq!(dm("_$s10ACPToolLib13CKToolVersionO05buildD0_WZ"), "one-time initialization function for buildVersion");
    assert_eq!(dm("_$s10DVTIconKit0A9GeneratorC6shared_WZ"), "one-time initialization function for shared");
    assert_eq!(dm("_$s10RealityKit02__A15FileURLResolverC12schemePrefix_WZ"), "one-time initialization function for schemePrefix");
}

/// A private variable prints its name with the discriminator, as the private-name form does.
#[test]
fn a_private_variable_keeps_its_discriminator() {
    assert_eq!(
        dm("_$s10ACPToolLib15TokenManagementO11jsonDecoder33_CE97A694DE5FA682360A96716DFDBC11LL_WZ"),
        "one-time initialization function for (jsonDecoder in _CE97A694DE5FA682360A96716DFDBC11)"
    );
    assert_eq!(
        dm("_$s10IDETrayKit25TrayManagerImplementationC23openQuicklyBookmarkIcon33_F4479CD804F0270A452EBB23E0AA47B3LL_WZ"),
        "one-time initialization function for (openQuicklyBookmarkIcon in _F4479CD804F0270A452EBB23E0AA47B3)"
    );
}

/// A module-level variable has the module as its only context.
#[test]
fn a_module_level_variable_decodes() {
    assert_eq!(dm("_$s10DVTIconKit34DVTGeneratedIconSetVendorDidUpdate_WZ"), "one-time initialization function for DVTGeneratedIconSetVendorDidUpdate");
    assert_eq!(dm("_$s10XCTHarness10XCTHLogger_WZ"), "one-time initialization function for XCTHLogger");
}

/// The token form, on a module-level and a nested variable.
#[test]
fn the_token_form_decodes() {
    assert_eq!(dm("_$s10DVTIconKit34DVTGeneratedIconSetVendorDidUpdate_Wz"), "one-time initialization token for DVTGeneratedIconSetVendorDidUpdate");
    assert_eq!(dm("_$s3Foo5MyBarV3abc_Wz"), "one-time initialization token for abc");
    assert_eq!(dm("_$s3Foo3abc_Wz"), "one-time initialization token for abc");
}

/// Constructed symbols, expected text checked with `swift-demangle`.
#[test]
fn constructed_one_time_initialization_symbols_decode() {
    assert_eq!(dm("_$s3Foo5MyBarV3abc_WZ"), "one-time initialization function for abc");
    assert_eq!(dm("_$s3Foo3abc_WZ"), "one-time initialization function for abc");
    assert_eq!(dm("_$s3Foo5MyBarV3abc_WZTm"), "merged one-time initialization function for abc");
}

/// A variable inside an extension, and the merged form `WZTm` of a variable, from macOS and Xcode
/// system binaries.
#[test]
fn a_variable_in_an_extension_and_a_merged_variable_decode() {
    assert_eq!(
        dm("_$s10Foundation11JSONDecoderC015IDEIntelligenceA0E23forOutputSchemaDecoding_WZ"),
        "one-time initialization function for forOutputSchemaDecoding"
    );
    assert_eq!(
        dm("_$s10Foundation3URLV16IDEDocumentationE34minimizedDocumentationContentRoute_WZTm"),
        "merged one-time initialization function for minimizedDocumentationContentRoute"
    );
    assert_eq!(
        dm("_$s10ACPToolLib15TokenManagementO11jsonEncoder33_CE97A694DE5FA682360A96716DFDBC11LL_WZTm"),
        "merged one-time initialization function for (jsonEncoder in _CE97A694DE5FA682360A96716DFDBC11)"
    );
}

/// `swift-demangle` does not read these as a variable name. For some it prints the symbol back
/// unchanged, for others it prints `()` or text with the context in front, such as
/// `Fooone-time initialization function for abc`. All of them are invalid here.
#[test]
fn an_operand_that_is_not_a_variable_name_is_invalid() {
    for symbol in [
        "_$s13configuration_WZ",
        "_$s4main1xSivp_WZ",
        "_$s3Foo5MyBarV_WZ",
        "_$s4mainWZ",
        "_$s10ACPToolLib11RemoveTokenVWZ",
        "_$s3Foo5MyBarV3abcWZ",
        "_$s3Foo5MyBarV3abc_WZ_",
        "_$s3Foo5MyBarV3abc_Wz0_",
        "_$s3Foo1A3abc_WZ",
    ] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}
