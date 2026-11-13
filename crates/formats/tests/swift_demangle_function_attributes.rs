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
// The constructed ones are in module `x`, from `swiftc -enable-library-evolution`, or written by
// hand where noted; `swift-demangle` decides what is right.

/// `Yb` before the function's `c` or `XE` is `@Sendable`, printed before the parameters.
#[test]
fn a_sendable_function_type_decodes() {
    assert_eq!(
        dm("_$s10PreviewsUI18AutoResettingValueC10setupTimer33_0B5CD0D1EA5E13001AC7FF8C8A235CD9LLyyFySo7NSTimerCYbcfU_"),
        "closure #1 @Sendable (__C.NSTimer) -> () in PreviewsUI.AutoResettingValue.(setupTimer in _0B5CD0D1EA5E13001AC7FF8C8A235CD9)() -> ()"
    );
    assert_eq!(dm("_$s1x2f1yyyyYbXEF"), "x.f1(@Sendable () -> ()) -> ()");
    assert_eq!(dm("_$s1x2v1yyYbcvp"), "x.v1 : @Sendable () -> ()");
    assert_eq!(dm("_$s1x2f9yyyYbcS2iYbXEF"), "x.f9(@Sendable (Swift.Int) -> Swift.Int) -> @Sendable () -> ()");
    assert_eq!(dm("_$s1x3f24yyyyYbcyYbXEF"), "x.f24(@Sendable () -> @Sendable () -> ()) -> ()");
    assert_eq!(dm("_$s1x3f23yyyyYbcSgF"), "x.f23((@Sendable () -> ())?) -> ()");
}

/// `Yc` takes the actor type before it and prints `@` and the type, ahead of `@Sendable`.
#[test]
fn a_function_type_with_a_global_actor_decodes() {
    assert_eq!(
        dm("_$s10RealityKit23PhysicsMaterialResourceCfdyyScMYcXEfU_"),
        "closure #1 @Swift.MainActor () -> () in RealityKit.PhysicsMaterialResource.deinit"
    );
    assert_eq!(dm("_$s1x2f7yyyyYbAA2GACYcXEF"), "x.f7(@x.GA @Sendable () -> ()) -> ()");
    assert_eq!(dm("_$s1x2f8yySiyYbKAA2GACYcXEF"), "x.f8(@x.GA @Sendable () throws -> Swift.Int) -> ()");
    assert_eq!(dm("_$s1x2f4yyyyYaYbScMYcXEF"), "x.f4(@Swift.MainActor @Sendable () async -> ()) -> ()");
    assert_eq!(dm("_$s1x3f26yyyyyYbScMYcXEYbScMYcXEF"), "x.f26(@Swift.MainActor @Sendable (@Swift.MainActor @Sendable () -> ()) -> ()) -> ()");
}

/// `async`, `@Sendable` and `throws` come after the parameters in that order.
#[test]
fn async_sendable_and_throws_print_in_their_order() {
    assert_eq!(
        dm("_$s10SiriSSUKit12NLTeamLoggerC7measure4name7message6aroundxs12StaticStringV_AIxyYaYbKXEtYaKlF"),
        "SiriSSUKit.NLTeamLogger.measure<A>(name: Swift.StaticString, message: Swift.StaticString, around: @Sendable () async throws -> A) async throws -> A"
    );
    assert_eq!(dm("_$s1x2f3yySiyYaYbKXEF"), "x.f3(@Sendable () async throws -> Swift.Int) -> ()");
}

/// `YA` is `@isolated(any)`, printed before `@Sendable`.
#[test]
fn an_isolated_any_function_type_decodes() {
    assert_eq!(
        dm("_$s6XCTest10SerializerC3runyxxyYaYbKYAXEYaKlFyScCyyts5NeverOGXEfU_"),
        "closure #1 (Swift.CheckedContinuation<(), Swift.Never>) -> () in XCTest.Serializer.run<A>(@isolated(any) @Sendable () async throws -> A) async throws -> A"
    );
    assert_eq!(dm("_$s1x2f6yyyyYAXEF"), "x.f6(@isolated(any) () -> ()) -> ()");
    assert_eq!(dm("_$s1x3f17yyyyYaYbYAXEF"), "x.f17(@isolated(any) @Sendable () async -> ()) -> ()");
}

/// `YK` takes the error type before it and prints `throws(` and the type in place of `throws`.
#[test]
fn a_typed_throws_function_decodes() {
    assert_eq!(
        dm("_$s13XcodeCloudKit17ValidatorProtocolP8validateyy5InputQz1EQzYKFTq"),
        "method descriptor for XcodeCloudKit.ValidatorProtocol.validate(A.Input) throws(A.E) -> ()"
    );
    assert_eq!(
        dm(
            "_$s11DVTProducts28NotificationsSettingsProductV4name8bundleID04adamG08imageURL17productCategoriesACSS_S2S10Foundation0J0VSgSaySo18DVTProductCategoryCGtAC19InitializationErrorOYKcfC"
        ),
        "DVTProducts.NotificationsSettingsProduct.init(name: Swift.String, bundleID: Swift.String, adamID: Swift.String, imageURL: Foundation.URL?, productCategories: [__C.DVTProductCategory]) throws(DVTProducts.NotificationsSettingsProduct.InitializationError) -> DVTProducts.NotificationsSettingsProduct"
    );
    assert_eq!(dm("_$s1x3f20yyAA5MyErrVYKF"), "x.f20() throws(x.MyErr) -> ()");
    assert_eq!(dm("_$s1x2f5yyyyAA5MyErrVYKXEF"), "x.f5(() throws(x.MyErr) -> ()) -> ()");
    assert_eq!(dm("_$s1x3f10yyyyYaYbAA5MyErrVYKScMYcXEF"), "x.f10(@Swift.MainActor @Sendable () async throws(x.MyErr) -> ()) -> ()");
}

/// The other `X` function types take the same markers.
#[test]
fn special_function_types_take_the_markers() {
    assert_eq!(dm("_$s1x3f14yySiyYbXKF"), "x.f14(@autoclosure @Sendable () -> Swift.Int) -> ()");
    assert_eq!(dm("_$s1x3f15yyyyYbXBF"), "x.f15(@convention(block) @Sendable () -> ()) -> ()");
}

/// `swift-demangle` reads the markers in one order, once each, and rejects anything else.
#[test]
fn markers_in_the_wrong_order_or_repeated_are_invalid() {
    // These are written by hand around `$s1x2v1` and `$s1x3f20`.
    for s in [
        "_$s1x2v1yyYbYacvp",                  // async after @Sendable
        "_$s1x2v1yyYaYacvp",                  // async twice
        "_$s1x2v1yyYbYbcvp",                  // @Sendable twice
        "_$s1x2v1yyKYbcvp",                   // throws before @Sendable
        "_$s1x2v1yyYAYbcvp",                  // @isolated(any) before @Sendable
        "_$s1x2v1yyYbYAKcvp",                 // @isolated(any) before throws
        "_$s1x2v1yyScMYcYbcvp",               // global actor before @Sendable
        "_$s1x2v1yyYbYAScMYccvp",             // global actor and @isolated(any)
        "_$s1x2v1yyYbScMYcYAcvp",             // @isolated(any) after a global actor
        "_$s1x2v1yyScMYcScMYccvp",            // two global actors
        "_$s1x3f20yyAA5MyErrVYKKF",           // typed throws and throws
        "_$s1x3f20yyAA5MyErrVYKYKF",          // typed throws twice
        "_$s1x3f20yyKAA5MyErrVYKF",           // throws before typed throws
        "_$s1x3f20yyAA5MyErrVYcAA5MyErrVYKF", // global actor before typed throws
    ] {
        invalid(s);
    }
}

/// `Yc` and `YK` need a type before them.
#[test]
fn a_marker_without_its_type_is_invalid() {
    for s in [
        "_$s1x2v1yyYccvp",   // Yc with nothing before it but the result
        "_$s1x2v1yyKYccvp",  // Yc after a throws marker
        "_$s1x2v1yyYaYccvp", // Yc after an async marker
        "_$s1x3f20yyYKF",    // YK after the empty list
        "_$s1x2v1YcYcvp",
    ] {
        invalid(s);
    }
}
