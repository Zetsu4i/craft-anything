use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

// Symbols from macOS 15.7 and Xcode 26.0 beta system binaries, or constructed with `swiftc`.
// Expected text from `swift-demangle`.

/// `Sc` and a letter spells a type of the concurrency library; `swift-demangle` prints each one in
/// module `Swift`.
#[test]
fn every_letter_names_its_type() {
    for (symbol, expected) in [
        ("_$sScAMp", "protocol descriptor for Swift.Actor"),
        ("_$sScFMp", "protocol descriptor for Swift.Executor"),
        ("_$sScfMp", "protocol descriptor for Swift.SerialExecutor"),
        ("_$sSchMp", "protocol descriptor for Swift.TaskExecutor"),
        ("_$sSciMp", "protocol descriptor for Swift.AsyncSequence"),
        ("_$sScIMp", "protocol descriptor for Swift.AsyncIteratorProtocol"),
        ("_$sScEN", "type metadata for Swift.CancellationError"),
        ("_$sScJN", "type metadata for Swift.UnownedJob"),
        ("_$sScMN", "type metadata for Swift.MainActor"),
        ("_$sScPN", "type metadata for Swift.TaskPriority"),
        ("_$sSceN", "type metadata for Swift.UnownedSerialExecutor"),
        ("_$sSctN", "type metadata for Swift.UnsafeCurrentTask"),
        ("_$sScCMF", "reflection metadata field descriptor Swift.CheckedContinuation"),
        ("_$sSccMF", "reflection metadata field descriptor Swift.UnsafeContinuation"),
        ("_$sScGMF", "reflection metadata field descriptor Swift.TaskGroup"),
        ("_$sScgMF", "reflection metadata field descriptor Swift.ThrowingTaskGroup"),
        ("_$sScSMF", "reflection metadata field descriptor Swift.AsyncStream"),
        ("_$sScsMF", "reflection metadata field descriptor Swift.AsyncThrowingStream"),
        ("_$sScTMF", "reflection metadata field descriptor Swift.Task"),
    ] {
        assert_eq!(dm(symbol), expected, "{symbol}");
    }
}

/// The generic ones take their arguments like any other standard type.
#[test]
fn generic_concurrency_types_take_arguments() {
    assert_eq!(
        dm("_$s10Foundation4UUIDV3key_ScTyyts5NeverOG5valuetMD"),
        "demangling cache variable for type metadata for (key: Foundation.UUID, value: Swift.Task<(), Swift.Never>)"
    );
    assert_eq!(
        dm("_$s11DVTProducts27MulticastAsyncStreamManagerC04makeD0ScSyxGyF"),
        "DVTProducts.MulticastAsyncStreamManager.makeStream() -> Swift.AsyncStream<A>"
    );
    assert_eq!(dm("_$sScG5groupScGyxGBp_tcfC"), "Swift.TaskGroup.init(group: Builtin.RawPointer) -> Swift.TaskGroup<A>");
    assert_eq!(
        dm("_$s1x5tasksyyScTySis5NeverOG_ScGySiGScgySis5Error_pGScPtF"),
        "x.tasks(Swift.Task<Swift.Int, Swift.Never>, Swift.TaskGroup<Swift.Int>, Swift.ThrowingTaskGroup<Swift.Int, Swift.Error>, Swift.TaskPriority) -> ()"
    );
    assert_eq!(
        dm("_$s1x7streamsyyScSySiG_ScsySis5Error_pGScCySisAD_pGSccySis5NeverOGtF"),
        "x.streams(Swift.AsyncStream<Swift.Int>, Swift.AsyncThrowingStream<Swift.Int, Swift.Error>, Swift.CheckedContinuation<Swift.Int, Swift.Error>, Swift.UnsafeContinuation<Swift.Int, Swift.Never>) -> ()"
    );
}

/// The protocols work where a protocol is needed: a conformance, a witness, an existential and a
/// generic requirement.
#[test]
fn concurrency_protocols_work_as_protocols() {
    assert_eq!(
        dm("_$s10Foundation17AsyncLineSequenceVyxGSciAAMc"),
        "protocol conformance descriptor for Foundation.AsyncLineSequence<A> : Swift.AsyncSequence in Foundation"
    );
    assert_eq!(dm("_$sSo17OS_dispatch_queueCSch8DispatchMc"), "protocol conformance descriptor for __C.OS_dispatch_queue : Swift.TaskExecutor in Dispatch");
    assert_eq!(
        dm("_$s10XCTHarness23InternalTelemetryLoggerCScAAAScA15unownedExecutorScevgTW"),
        "protocol witness for Swift.Actor.unownedExecutor.getter : Swift.UnownedSerialExecutor in conformance XCTHarness.InternalTelemetryLogger : Swift.Actor in XCTHarness"
    );
    assert_eq!(
        dm("_$s1x6protosyyScA_p_ScF_pScf_pSci_pScI_ptF"),
        "x.protos(Swift.Actor, Swift.Executor, Swift.SerialExecutor, Swift.AsyncSequence, Swift.AsyncIteratorProtocol) -> ()"
    );
    assert_eq!(dm("_$s1x3genyyxScARzlF"), "x.gen<A where A: Swift.Actor>(A) -> ()");
    assert_eq!(dm("_$s1x4execyySch_pF"), "x.exec(Swift.TaskExecutor) -> ()");
}

/// Types of the library that are neither generic nor protocols, mixed in one parameter list.
#[test]
fn plain_concurrency_types_in_a_signature() {
    assert_eq!(
        dm("_$s1x4miscyyScE_SceScJSctSgtF"),
        "x.misc(Swift.CancellationError, Swift.UnownedSerialExecutor, Swift.UnownedJob, Swift.UnsafeCurrentTask?) -> ()"
    );
    assert_eq!(dm("_$s1x1SV1tACyxGScTy7ElementQzs5Error_pGSg_tcfC"), "x.S.init(t: Swift.Task<A.Element, Swift.Error>?) -> x.S<A>");
}

/// `swift-demangle` has no type for these letters, so none decodes here either: a letter with no
/// concurrency type, a letter that is a standard type on its own (`Sa`), and `Sc` at the end.
#[test]
fn a_letter_with_no_type_is_invalid() {
    for symbol in [
        "$s4main1fyyScDF",
        "$s4main1fyyScZF",
        "$s4main1fyyScaF",
        "$s4main1fyyScoF",
        "$s4main1fyyScrF",
        "$s4main1fyyScyF",
        "$s4main1fyyScUF",
        "$s4main1fyyScNF",
        "$s4main1fyySc_F",
        "$s4main1fyySc",
        "$sSc",
    ] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}
