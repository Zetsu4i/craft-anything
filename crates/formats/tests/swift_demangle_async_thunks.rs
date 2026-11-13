use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

// Symbols from macOS 15.7 and Xcode 26.0 beta system binaries, expected text from `swift-demangle`.

/// `Tu` is the async function pointer of the function before it.
#[test]
fn an_async_function_pointer_decodes() {
    assert_eq!(dm("_$s10XCTestCore5QueueC11synchronizeyyYaFTu"), "async function pointer to XCTestCore.Queue.synchronize() async -> ()");
    assert_eq!(dm("_$s9SKLogging15NonDarwinLoggerV5flushyyYaFZTu"), "async function pointer to static SKLogging.NonDarwinLogger.flush() async -> ()");
    assert_eq!(dm("_$s7Testing16__requiringAwaitytvgTu"), "async function pointer to Testing.__requiringAwait.getter : ()");
}

/// The function can itself be a thunk: a dispatch thunk, a protocol witness or a partial apply forwarder.
#[test]
fn an_async_function_pointer_of_a_thunk_decodes() {
    assert_eq!(
        dm("_$s10CoreDevice06RemoteB0C7connectyyYaKFTjTu"),
        "async function pointer to dispatch thunk of CoreDevice.RemoteDevice.connect() async throws -> ()"
    );
    assert_eq!(
        dm("_$s7Testing3BugVAA5TraitA2aDP7prepare3foryAA4TestV_tYaKFTWTu"),
        "async function pointer to protocol witness for Testing.Trait.prepare(for: Testing.Test) async throws -> () in conformance Testing.Bug : Testing.Trait in Testing"
    );
    assert_eq!(
        dm("_$s14XCPreviewAgent7useTaskyyFyyYacfU_TATu"),
        "async function pointer to partial apply forwarder for closure #1 () async -> () in XCPreviewAgent.useTask() -> ()"
    );
}

/// `TY` and `TQ` take an index: `_` is 0, `0_` is 1, `10_` is 11. The text shows the index
/// as `swift-demangle` counts it.
#[test]
fn resume_functions_print_their_index() {
    assert_eq!(dm("_$s7XCBuild4mains5NeverOyYaFTY0_"), "(1) suspend resume partial function for XCBuild.main() async -> Swift.Never");
    assert_eq!(dm("_$s7XCBuild12swbuild_replSbyYaKFTQ3_"), "(4) await resume partial function for XCBuild.swbuild_repl() async throws -> Swift.Bool");
    assert_eq!(dm("_$s7XCBuild4main4argsSiSaySSG_tYaFTY10_"), "(11) suspend resume partial function for XCBuild.main(args: [Swift.String]) async -> Swift.Int");
    // Constructed with `swiftc -enable-library-evolution` and checked with `swift-demangle`.
    assert_eq!(dm("$s4main1fyyYaFTY_"), "(0) suspend resume partial function for main.f() async -> ()");
    assert_eq!(dm("$s4main1fyyYaFTQ_"), "(0) await resume partial function for main.f() async -> ()");
    assert_eq!(dm("$s4main1fyyYaFTY00_"), "(1) suspend resume partial function for main.f() async -> ()");
}

/// The operand can be a closure, a thunk or a type, and the operators can be stacked.
#[test]
fn the_operand_can_be_a_closure_a_thunk_or_another_async_thunk() {
    assert_eq!(
        dm("_$s16PreviewsPipeline14PreviewServiceCfdyyYacfU_TY1_"),
        "(2) suspend resume partial function for closure #1 () async -> () in PreviewsPipeline.PreviewService.deinit"
    );
    assert_eq!(
        dm("_$s14XCPreviewAgent7useTaskyyFyyYacfU_TATQ0_"),
        "(1) await resume partial function for partial apply forwarder for closure #1 () async -> () in XCPreviewAgent.useTask() -> ()"
    );
    // Constructed and checked with `swift-demangle`.
    assert_eq!(dm("$s1a1CC1myyYaKFTjTQ0_"), "(1) await resume partial function for dispatch thunk of a.C.m() async throws -> ()");
    assert_eq!(dm("$s1a1SV1hyxxYalFTY0_"), "(1) suspend resume partial function for a.S.h<A>(A) async -> A");
    assert_eq!(dm("$s4main1fyyYaFTuTu"), "async function pointer to async function pointer to main.f() async -> ()");
}

/// A type or a metadata accessor is accepted as the operand too, as `swift-demangle` does.
#[test]
fn a_type_or_a_metadata_accessor_is_an_operand() {
    assert_eq!(dm("$sSiTu"), "async function pointer to Swift.Int");
    assert_eq!(dm("$sSiTY0_"), "(1) suspend resume partial function for Swift.Int");
    assert_eq!(dm("$s4main1AVMaTQ_"), "(0) await resume partial function for type metadata accessor for main.A");
}

/// Symbols `swift-demangle` leaves as they are: no index, a bad index, an index on `Tu`, or an
/// index above 2^31 - 1.
#[test]
fn a_missing_or_bad_index_is_invalid() {
    for symbol in [
        "$s4main1fyyYaFTY",
        "$s4main1fyyYaFTQ",
        "$s4main1fyyYaFTYa_",
        "$s4main1fyyYaFTQx_",
        "$s4main1fyyYaFTY0",
        "$s4main1fyyYaFTYTu",
        "$s4main1fyyYaFTYTQ_",
        "$s4main1fyyYaFTu0_",
        "$s4main1fyyYaFTY99999999999999999999_",
        "$s4main1fyyYaFTY2147483648_",
        "$s4main1fyyYaFTQ18446744073709551615_",
    ] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}

/// The largest index `swift-demangle` accepts is 2147483647, printed as 2147483648.
#[test]
fn the_largest_index_decodes() {
    assert_eq!(dm("$s4main1fyyYaFTY2147483647_"), "(2147483648) suspend resume partial function for main.f() async -> ()");
}

/// With nothing, or only a name, a marker or a module before it, there is no function to take.
/// `swift-demangle` prints some of these with the missing part blank or run together
/// (`async function pointer to mainf`); this demangler does not decode them.
#[test]
fn nothing_to_take_is_invalid() {
    for symbol in ["$sTu", "$sTY_", "$s4mainTu", "$s4main1fTu", "$s4main_Tu", "$s4main1fyyYaF_TQ_", "$s4main1fyyYaFYaTY_"] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}
