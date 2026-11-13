use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

// Symbols with a `_$s` prefix come from macOS 15.7 or Xcode 26.0 beta system binaries; the ones
// that start with `$s` are constructed (with `swiftc -enable-library-evolution` or by hand). The
// expected text is what `swift-demangle` prints, without its `(extension in M):` prefix.

/// `XE` is a function type that takes its parameters and result from the stack like `c`; it prints
/// as a plain function type.
#[test]
fn a_non_escaping_function_type_decodes() {
    assert_eq!(dm("_$s7SWBUtil5OSLogCfdyyXEfU_"), "closure #1 () -> () in SWBUtil.OSLog.deinit");
    assert_eq!(dm("_$s9RealityIO7OutputsCyypSgSScigADyXEfU_"), "closure #1 () -> Any? in RealityIO.Outputs.subscript.getter : (Swift.String) -> Any?");
    assert_eq!(dm("_$s9RealityIO4PrimC8traverse4withyyACXE_tF"), "RealityIO.Prim.traverse(with: (RealityIO.Prim) -> ()) -> ()");
}

/// `throws` and `async` markers come before `XE`, `async` first.
#[test]
fn a_function_type_keeps_its_throws_and_async() {
    assert_eq!(dm("_$sSr15_stableSortImpl2byySbx_xtKXE_tKF"), "Swift.UnsafeMutableBufferPointer._stableSortImpl(by: (A, A) throws -> Swift.Bool) throws -> ()");
    assert_eq!(dm("_$sSD7SWBUtilE11getOrInsertyq_x_q_yYaKXEtYaKF"), "Swift.Dictionary.getOrInsert(A, () async throws -> B) async throws -> B");
    assert_eq!(dm("$s1a1vSaySiSbYaXEGvp"), "a.v : [(Swift.Bool) async -> Swift.Int]");
}

/// `XK` and `XA` are autoclosures, `XC`, `XB` and `Xf` name a calling convention. All four print the
/// attribute before the function type, and an optional puts the whole type in parentheses.
#[test]
fn function_types_with_an_attribute_print_it() {
    assert_eq!(dm("_$sSD_7defaultq_x_q_yXKtcig"), "Swift.Dictionary.subscript.getter : (_: A, default: @autoclosure () -> B) -> B");
    assert_eq!(dm("_$s7SwiftUI9LazyStateV12wrappedValueACyxGxyXA_tcfC"), "SwiftUI.LazyState.init(wrappedValue: @autoclosure () -> A) -> SwiftUI.LazyState<A>");
    assert_eq!(dm("_$s6Darwin7SIG_DFLys5Int32VXCSgvg"), "Darwin.SIG_DFL.getter : (@convention(c) (Swift.Int32) -> ())?");
    assert_eq!(dm("_$s8Dispatch0A8WorkItemC6_blockyyXBvgTq"), "method descriptor for Dispatch.DispatchWorkItem._block.getter : @convention(block) () -> ()");
    assert_eq!(
        dm("_$ss28ComputedArgumentWitnessesPtrV7destroyySv_SitXfSgvg"),
        "Swift.ComputedArgumentWitnessesPtr.destroy.getter : (@convention(thin) (Swift.UnsafeMutableRawPointer, Swift.Int) -> ())?"
    );
    assert_eq!(
        dm("$s1m2f1__1cS2iyXK_yyKXEySizXEtKF"),
        "m.f1(_: @autoclosure () -> Swift.Int, _: () throws -> (), c: (inout Swift.Int) -> ()) throws -> Swift.Int"
    );
    assert_eq!(dm("$s1m2f4yys5Int32VADXC_ySiXBtF"), "m.f4(@convention(c) (Swift.Int32) -> Swift.Int32, @convention(block) (Swift.Int) -> ()) -> ()");
    assert_eq!(dm("$s1a1vSayyyXKSgGvp"), "a.v : [(@autoclosure () -> ())?]");
}

/// `Xl` after a protocol list adds `AnyObject`; after nothing it is `AnyObject` alone. `Xp` after a
/// type is its existential metatype.
#[test]
fn any_object_and_existential_metatypes_decode() {
    assert_eq!(dm("_$sSa6_owneryXlSgvg"), "Swift.Array._owner.getter : Swift.AnyObject?");
    assert_eq!(dm("_$sSOySOyXlcfC"), "Swift.ObjectIdentifier.init(Swift.AnyObject) -> Swift.ObjectIdentifier");
    assert_eq!(dm("_$syXlXpMa"), "type metadata accessor for Swift.AnyObject.Type");
    assert_eq!(dm("_$sSOySOypXpcfC"), "Swift.ObjectIdentifier.init(Any.Type) -> Swift.ObjectIdentifier");
    assert_eq!(
        dm("_$s10DebuggerUI022BreakpointEditorSourceD9TextFieldC23completionProviderClass0eD0014CodeCompletionI0_pXpvgZ"),
        "static DebuggerUI.BreakpointEditorSourceEditorTextField.completionProviderClass.getter : SourceEditor.CodeCompletionProvider.Type"
    );
    assert_eq!(dm("$s1m2f3yyAA1P_AA1QXl_AaC_AadA1KCXctF"), "m.f3(m.P & m.Q & Swift.AnyObject, m.K & m.P & m.Q) -> ()");
    assert_eq!(dm("$s1m1CC3pm2AA1P_AA1QpXpSgvg"), "m.C.pm2.getter : m.P & m.Q.Type?");
}

/// `Xc` takes a class and the protocols after it; an optional puts the whole type in parentheses.
#[test]
fn a_class_with_protocols_decodes() {
    assert_eq!(
        dm("_$s10XCTHarness29XCTHGenericRunDestinationPoolC010deallocatecD0yySo07XCTHRunD0_So8NSObjectCXcF"),
        "XCTHarness.XCTHGenericRunDestinationPool.deallocateRunDestination(__C.NSObject & __C.XCTHRunDestination) -> ()"
    );
    assert_eq!(dm("$s1m1CC3clsAA1P_AA1KCXcSgvg"), "m.C.cls.getter : (m.K & m.P)?");
    assert_eq!(dm("$s1m1CC4cls2AA1P_AA1QAA1KCXcSgvg"), "m.C.cls2.getter : (m.K & m.P & m.Q)?");
    assert_eq!(dm("$s1m2f5yySayyXlGSg_SayAA1P_AA1KCXcGtF"), "m.f5([Swift.AnyObject]?, [m.K & m.P]) -> ()");
}

/// `XD` is `Self` in a method that returns it, however the type it was built from reads.
#[test]
fn dynamic_self_decodes() {
    assert_eq!(dm("_$s7SWBUtil8SWBQueueC6globalACXDyFZ"), "static SWBUtil.SWBQueue.global() -> Self");
    assert_eq!(dm("_$s10RealityKit6EntityC5clone9recursiveACXDSb_tF"), "RealityKit.Entity.clone(recursive: Swift.Bool) -> Self");
    assert_eq!(dm("$s1m1CC2meACXDyF"), "m.C.me() -> Self");
}

/// Ownership (`Xw` weak, `Xo` unowned, `Xu` unowned(unsafe)) and `Xb` (a box) put a word before the
/// type. Only the ownership words ask for parentheses inside an optional.
#[test]
fn ownership_and_box_types_print_their_word() {
    assert_eq!(dm("$s1a1vSaySiXwGvp"), "a.v : [weak Swift.Int]");
    assert_eq!(dm("$s1a1vSaySiSgXwGvp"), "a.v : [weak Swift.Int?]");
    assert_eq!(dm("$s1a1vSaySiXwSgGvp"), "a.v : [(weak Swift.Int)?]");
    assert_eq!(dm("$s1a1vSaySiXoSgGvp"), "a.v : [(unowned Swift.Int)?]");
    assert_eq!(dm("$s1a1vSaySiXuGvp"), "a.v : [unowned(unsafe) Swift.Int]");
    assert_eq!(dm("$s1a1vSaySiXbSgGvp"), "a.v : [@box Swift.Int?]");
}

/// `XM` and `Xm` write a metatype with its representation. `XM` of an existential is a `.Protocol`
/// metatype, and of a function or an ownership type it parenthesises the operand; `Xm` is `.Type`.
#[test]
fn metatypes_with_a_representation_decode() {
    assert_eq!(dm("_$sypXmTML"), "lazy cache variable for type metadata for @thick Any.Type");
    assert_eq!(dm("$s1a1vSaySiXMTGvp"), "a.v : [@thick Swift.Int.Type]");
    assert_eq!(dm("$s1a1vSaySiSgXMtGvp"), "a.v : [@thin Swift.Int?.Type]");
    assert_eq!(dm("$s1a1vSaySiXMoGvp"), "a.v : [@objc_metatype Swift.Int.Type]");
    assert_eq!(dm("$s1a1vSayypXMTGvp"), "a.v : [@thick Any.Protocol]");
    assert_eq!(dm("$s1a1vSayypXmTGvp"), "a.v : [@thick Any.Type]");
    assert_eq!(dm("$s1a1vSayyXlXMTGvp"), "a.v : [@thick Swift.AnyObject.Protocol]");
    assert_eq!(dm("$s1a1vSaySiXpXMTGvp"), "a.v : [@thick Swift.Int.Type.Protocol]");
    assert_eq!(dm("$s1a1vSayyyXEXMTGvp"), "a.v : [@thick (() -> ()).Type]");
    assert_eq!(dm("$s1a1vSayyyXEXmTGvp"), "a.v : [@thick () -> ().Type]");
    assert_eq!(dm("$s1a1vSaySiXwXMTGvp"), "a.v : [@thick (weak Swift.Int).Type]");
    assert_eq!(dm("$s1a1vSaySiXbXMTGvp"), "a.v : [@thick @box Swift.Int.Type]");
}

/// `swift-demangle` decodes none of these: the operands are missing, are not types or are out of
/// order.
#[test]
fn a_special_type_with_a_wrong_operand_is_invalid() {
    for symbol in [
        // A function type needs two operands, and at most one `throws` then one `async`.
        "$s1a1vSayXEGvp",
        "$s1a1vSaySiXEGvp",
        "$s1a1vSayyKKXEGvp",
        "$s1a1vSayyYaYaXEGvp",
        "$s1a1vSaySiSbKYaXEGvp",
        "$s1a1vSaysyyXEGvp",
        "$s1a1vSayyyXKXAGvp",
        // `XD`, `Xp`, `Xw`, `Xo`, `Xu`, `Xb` and the metatypes need a type.
        "$s1a1vSayXDGvp",
        "$s1a1vSaysXDGvp",
        "$s1a1vSayyXDGvp",
        "$s1a1vSayyXpGvp",
        "$s1a1vSaysXpGvp",
        "$s1a1vSayyXwGvp",
        "$s1a1vSayyXoGvp",
        "$s1a1vSayyXuGvp",
        "$s1a1vSayyXbGvp",
        "$s1a1vSayyXMTGvp",
        "$s1a1vSaysXMTGvp",
        // A metatype's representation is `T`, `t` or `o`.
        "$s1a1vSaySiXMGvp",
        "$s1a1vSaySiXMmGvp",
        "$s1a1vSaySiXmXGvp",
        // `Xl` needs a protocol list, and so does `Xc` before its class.
        "$s1a1vSaySiXlGvp",
        "$s1a1vSayXlGvp",
        "$s1a1vSayyXcGvp",
        "$s1a1vSayXcGvp",
        "$s1a1vSaysXcGvp",
        // Kinds that are not special types.
        "$s1a1vSaySiXFGvp",
        "$s1a1vSaySiXXGvp",
        "$s1a1vSaySiXSGvp",
    ] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}
