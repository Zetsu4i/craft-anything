use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

// Symbols that start `_$s1a`, `_$s1b` or `_$s1c` were built with `swiftc -enable-library-evolution`
// or edited by hand; the others come from macOS 15.7.4 and Xcode 26.0 beta system binaries. Every
// expected text is what `swift-demangle` prints, without its `(extension in …):` prefix, which
// this demangler leaves out by design.

/// A variable declared `some P` reads `: some` after its name, and its getter and property
/// descriptor keep their usual words.
#[test]
fn a_variable_with_an_opaque_type_reads_some() {
    assert_eq!(dm("_$s25IDEGameKitInspectorEditor15RootSidebarViewV4bodyQrvg"), "IDEGameKitInspectorEditor.RootSidebarView.body.getter : some");
    assert_eq!(
        dm("_$s22IDEGameKitCommonEditor18CircleProgressViewV4bodyQrvpMV"),
        "property descriptor for IDEGameKitCommonEditor.CircleProgressView.body : some"
    );
    assert_eq!(
        dm("_$s6Gluten29GraphViewPopupButton_PreviewsV8previewsQrvpZMV"),
        "property descriptor for static Gluten.GraphViewPopupButton_Previews.previews : some"
    );
    assert_eq!(
        dm("_$s8Xcode3UI27EditTargetMembershipOptionsV33buildFileAttributesEditorsContent33_0B0BBC9CC9EA70B116D3E56B52749C09LLQryF"),
        "Xcode3UI.EditTargetMembershipOptions.(buildFileAttributesEditorsContent in _0B0BBC9CC9EA70B116D3E56B52749C09)() -> some"
    );
    assert_eq!(dm("_$s1a2gvQrvg"), "a.gv.getter : some");
}

/// A function or subscript that returns `some P` prints `-> some`, with or without a generic
/// signature, and static or not.
#[test]
fn a_function_returning_an_opaque_type_prints_some_as_its_result() {
    assert_eq!(
        dm("_$s6Gluten30CompactPinLabelChangedModifierV4body7contentQr7SwiftUI05_ViewF8_ContentVyACG_tF"),
        "Gluten.CompactPinLabelChangedModifier.body(content: SwiftUI._ViewModifier_Content<Gluten.CompactPinLabelChangedModifier>) -> some"
    );
    assert_eq!(
        dm("_$s7SwiftUI4ViewP6GlutenE10edgeBorder_5edges5colorQr12CoreGraphics7CGFloatV_SayAA4EdgeOGAA5ColorVtF"),
        "SwiftUI.View.edgeBorder(_: CoreGraphics.CGFloat, edges: [SwiftUI.Edge], color: SwiftUI.Color) -> some"
    );
    assert_eq!(dm("_$s7SwiftUI4ViewPAAE9blendModeyQrAA05BlendE0OF"), "SwiftUI.View.blendMode(SwiftUI.BlendMode) -> some");
    assert_eq!(dm("_$s1a1HV1myQrSiF"), "a.H.m(Swift.Int) -> some");
    assert_eq!(dm("_$s1a1HV2smQryFZ"), "static a.H.sm() -> some");
    assert_eq!(dm("_$s1a1HVyQrSicig"), "a.H.subscript.getter : (Swift.Int) -> some");
}

/// A result with several opaque types lists them in a tuple. `Qr` is the first and `QR` with an
/// index is each later one; all print as `some`, and none takes a substitution slot.
#[test]
fn several_opaque_results_in_a_tuple() {
    assert_eq!(dm("_$s1a3mk3yQr_QR_tSiF"), "a.mk3(Swift.Int) -> (some, some)");
    assert_eq!(dm("_$s1b2m5Qr_QR_QR0_QR1_QR2_tyF"), "b.m5() -> (some, some, some, some, some)");
    assert_eq!(
        dm("_$s1b3m12Qr_QR_QR0_QR1_QR2_QR3_QR4_QR5_QR6_QR7_QR8_QR9_tyF"),
        "b.m12() -> (some, some, some, some, some, some, some, some, some, some, some, some)"
    );
    assert_eq!(dm("_$s1c2f1yQr_AA3FooVtADF"), "c.f1(c.Foo) -> (some, c.Foo)");
    assert_eq!(dm("_$s1c2f3yAA3FooV_QrQR_AdA3BarVyADGtADF"), "c.f3(c.Foo) -> (c.Foo, some, some, c.Foo, c.Bar<c.Foo>)");
    assert_eq!(dm("_$s1c2f8yQr_AA1WV_ADtAA3FooVtADF"), "c.f8(c.W) -> (some, (c.W, c.W), c.Foo)");
}

/// An opaque type inside another type prints `some` in place; an optional or a metatype of it is
/// put in parentheses.
#[test]
fn an_opaque_type_inside_another_type() {
    assert_eq!(dm("_$s1a3mk4SayQrGyF"), "a.mk4() -> [some]");
    assert_eq!(dm("_$s1a3optQrSgyF"), "a.opt() -> (some)?");
    assert_eq!(dm("_$s1a2mkQrmyF"), "a.mk() -> (some).Type");
    assert_eq!(dm("_$s1a2mkQrSgmyF"), "a.mk() -> (some)?.Type");
    assert_eq!(dm("_$s1a2mkQrSgSgyF"), "a.mk() -> (some)??");
    assert_eq!(dm("_$s1a2mkQrzyF"), "a.mk() -> inout some");
}

/// `QO` names the declaration and `Qo` with an index picks one of its opaque types: `<<opaque
/// return type of …>>.N`. Generic arguments between them are checked but not printed.
#[test]
fn a_use_of_an_opaque_type_names_its_declaration() {
    assert_eq!(
        dm("_$s7SwiftUI4ViewPAAE9focusableyQrSbFQOyAA6ButtonVyAA4TextVG_Qo_MD"),
        "demangling cache variable for type metadata for <<opaque return type of SwiftUI.View.focusable(Swift.Bool) -> some>>.0"
    );
    assert_eq!(
        dm("_$s17InstrumentsPlugIn12RevealButtonV11menuVersionQrvpQOy_Qo_SgMD"),
        "demangling cache variable for type metadata for (<<opaque return type of InstrumentsPlugIn.RevealButton.menuVersion : some>>.0)?"
    );
    assert_eq!(dm("_$s1a2mkQryFQO"), "<<opaque return type of a.mk() -> some>>");
    assert_eq!(dm("_$s1a2mkQryFQOyQo_"), "<<opaque return type of a.mk() -> some>>.0");
    assert_eq!(dm("_$s1a2mkQryFQOyQo0_"), "<<opaque return type of a.mk() -> some>>.1");
    assert_eq!(dm("_$s1a2mkQryFQOyQo1_"), "<<opaque return type of a.mk() -> some>>.2");
    assert_eq!(dm("_$s1a2mkQryFQOySi_SbQo_"), "<<opaque return type of a.mk() -> some>>.0");
    assert_eq!(dm("_$s1a2mkQryFQOySiSb_Qo_"), "<<opaque return type of a.mk() -> some>>.0");
    assert_eq!(dm("_$s1a2mkQryFQOyQo_Sg"), "(<<opaque return type of a.mk() -> some>>.0)?");
    assert_eq!(dm("_$s1a2mkQryFQOyQo_m"), "(<<opaque return type of a.mk() -> some>>.0).Type");
    assert_eq!(dm("_$s1a1HV4bodyQrvpQOyQo_"), "<<opaque return type of a.H.body : some>>.0");
    assert_eq!(dm("_$s1a1HVyQrSicipQOyQo_"), "<<opaque return type of a.H.subscript(Swift.Int) -> some>>.0");
}

/// The same declaration can appear several times in one symbol, and an opaque type takes a
/// substitution slot, so a later `A…` can point back at it.
#[test]
fn an_opaque_type_takes_a_substitution_slot() {
    assert_eq!(
        dm("_$s7SwiftUI19_ConditionalContentVyAA4ViewPAAE4helpyQrAA18LocalizedStringKeyVFQOyAA5ImageV_Qo_AKGMD"),
        "demangling cache variable for type metadata for SwiftUI._ConditionalContent<<<opaque return type of SwiftUI.View.help(SwiftUI.LocalizedStringKey) -> some>>.0, <<opaque return type of SwiftUI.View.help(SwiftUI.LocalizedStringKey) -> some>>.0>"
    );
    assert_eq!(
        dm(
            "_$s7SwiftUI9TupleViewVyAA0D0PAAE16keyboardShortcut_9modifiersQrAA13KeyEquivalentV_AA14EventModifiersVtFQOyAA6ButtonVyAA15ModifiedContentVyAA5ImageVAA12_FrameLayoutVGG_Qo__AeAEAF_AGQrAI_AKtFQOyAOyAuA012_EnvironmentH17TransformModifierVySbGG_Qo_tGMD"
        ),
        "demangling cache variable for type metadata for SwiftUI.TupleView<(<<opaque return type of SwiftUI.View.keyboardShortcut(_: SwiftUI.KeyEquivalent, modifiers: SwiftUI.EventModifiers) -> some>>.0, <<opaque return type of SwiftUI.View.keyboardShortcut(_: SwiftUI.KeyEquivalent, modifiers: SwiftUI.EventModifiers) -> some>>.0)>"
    );
}

/// Closures and thunks inside a declaration that returns an opaque type keep working.
#[test]
fn closures_in_a_declaration_with_an_opaque_type() {
    assert_eq!(
        dm("_$s17IDEStoreKitEditor0ab9CodeOfferC0V4bodyQrvgAC6editorQrvpQOy_Qo_ycfU2_"),
        "closure #4 () -> <<opaque return type of IDEStoreKitEditor.IDEStoreKitCodeOfferEditor.editor : some>>.0 in IDEStoreKitEditor.IDEStoreKitCodeOfferEditor.body.getter : some"
    );
    assert_eq!(
        dm("_$s6IDEKit20IDECodeModuleDetailsO8Overview33_467444FF5BF1EDB058CF4B52B82C78F8LLV4bodyQrvgyycfU0_TA"),
        "partial apply forwarder for closure #2 () -> () in IDEKit.IDECodeModuleDetails.(Overview in _467444FF5BF1EDB058CF4B52B82C78F8).body.getter : some"
    );
    assert_eq!(
        dm("_$s13PreviewsXcode16CanvasBannerViewV4bodyQrvgyycfU0_TATm"),
        "merged partial apply forwarder for closure #2 () -> () in PreviewsXcode.CanvasBannerView.body.getter : some"
    );
    assert_eq!(dm("_$s1a3twoQryFAA2mkQryFQOyQo_ycfU_"), "closure #1 () -> <<opaque return type of a.mk() -> some>>.0 in a.two() -> some");
}

/// Malformed uses that `swift-demangle` does not decode stay invalid: a `QR` with no index, a `Qo`
/// with no declaration or no argument list, an argument that is not a type, and two `Qo` in a row.
#[test]
fn malformed_opaque_types_are_invalid() {
    for s in [
        "_$s1a2mkQRyF",
        "_$s1a2mkQR0yF",
        "_$s1a2mkQryFQOQo_",
        "_$s1a2mkQryFQOy3fooQo_",
        "_$s1a2mkQryFQOysQo_",
        "_$s1a2mkQryFQOyKQo_",
        "_$s1a2mkQryFQOyQo_Qo_",
        "_$s1a2mkQryFQo_",
        "_$s1a2mkQo_",
        "_$s1a2mkQryFQOSi_Qo_",
    ] {
        let d = demangle(s);
        assert_eq!(d.status, Status::Invalid, "{s}: {:?}", d.demangled);
    }
}
