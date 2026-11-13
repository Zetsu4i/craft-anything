use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

// Symbols from macOS 15.7 and Xcode 26.0 beta system binaries, expected text from `swift-demangle`.

/// `TK` is the key path getter and `Tk` the key path setter. The operand under the base is the
/// property, read as a property descriptor, and it prints with its type after ` : `.
#[test]
fn a_key_path_getter_and_setter_decode() {
    assert_eq!(
        dm("_$s10DVTIconKit0A9GeneratorC6sharedACvpZACmTK"),
        "key path getter for static DVTIconKit.DVTIconGenerator.shared : DVTIconKit.DVTIconGenerator : DVTIconKit.DVTIconGenerator.Type"
    );
    assert_eq!(
        dm("_$s10DVTIconKit18GeneratedIconImageC14drawingContextAA0cd7DrawingG0CSgvpACTk"),
        "key path setter for DVTIconKit.GeneratedIconImage.drawingContext : DVTIconKit.GeneratedIconDrawingContext? : DVTIconKit.GeneratedIconImage"
    );
    assert_eq!(
        dm("_$s10DebuggerUI23DBGElidedStackFrameViewC9lineColorSo7NSColorCvpACTK"),
        "key path getter for DebuggerUI.DBGElidedStackFrameView.lineColor : __C.NSColor : DebuggerUI.DBGElidedStackFrameView"
    );
    assert_eq!(
        dm("_$s10DVTIconKit27DVTGeneratedIconEmojiVendorC6vendorAA09Generatedd3SetF0_pvpZACmTK"),
        "key path getter for static DVTIconKit.DVTGeneratedIconEmojiVendor.vendor : DVTIconKit.GeneratedIconSetVendor : DVTIconKit.DVTGeneratedIconEmojiVendor.Type"
    );
}

/// A subscript's descriptor (`ip`) prints its parameters and result type in place of a name.
#[test]
fn a_subscript_key_path_accessor_decodes() {
    assert_eq!(
        dm("_$s10Foundation4DataV06InlineB0Vys5UInt8VSicipAETK"),
        "key path getter for Foundation.Data.InlineData.subscript(Swift.Int) -> Swift.UInt8 : Foundation.Data.InlineData"
    );
}

/// Constructed with `swiftc -emit-library -enable-library-evolution` and checked with `swift-demangle`.
#[test]
fn constructed_key_path_accessors_decode() {
    assert_eq!(dm("_$s2kp3GenC1nSivpACTK"), "key path getter for kp.Gen.n : Swift.Int : kp.Gen");
    assert_eq!(dm("_$s2kp3GenC1nSivpACTk"), "key path setter for kp.Gen.n : Swift.Int : kp.Gen");
    assert_eq!(dm("_$s2kp3GenC6sharedACvpZACmTK"), "key path getter for static kp.Gen.shared : kp.Gen : kp.Gen.Type");
    assert_eq!(dm("_$s2kp3GenCySSSicipACTk"), "key path setter for kp.Gen.subscript(Swift.Int) -> Swift.String : kp.Gen");
    assert_eq!(dm("_$s2kp3GenC7doubledSivpACTK"), "key path getter for kp.Gen.doubled : Swift.Int : kp.Gen");
}

/// Not key path accessors: a missing base or descriptor, a module or a name as the base, a
/// global or a second thunk on the path. `swift-demangle` rejects each of these, so the demangler
/// reports them invalid.
#[test]
fn a_key_path_accessor_without_a_base_or_descriptor_is_invalid() {
    for symbol in [
        "_$s2kp3GenC1nSivpTk",
        "_$s2kp3GenC1nSivpTK",
        "_$s2kp3GenC1nSivpZTk",
        "_$s2kp3GenC1nSivp2kpTk",
        "_$s2kp3GenC1nSivpACvpTk",
        "_$s2kp3GenC1nSivpACTkTk",
        "_$s2kp3GenC1nSivpSiTKTk",
        "_$s2kp3GenC1nSivpSi_Tk",
    ] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}
