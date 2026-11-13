use craft_anything_formats::swift_demangle::{Status, demangle};

fn dm(symbol: &str) -> String {
    let d = demangle(symbol);
    assert_eq!(d.status, Status::Demangled, "{symbol}: {:?}", d.reason);
    d.demangled.unwrap_or_default()
}

// Symbols from macOS 15.7.4 and Xcode 26.0 beta system binaries, expected text from `swift-demangle`.

/// `Wl` names a type and then a conformance, which may be of another type or of a generic one.
#[test]
fn a_lazy_accessor_names_its_type_and_its_conformance() {
    assert_eq!(
        dm("_$s7SwiftUI4EdgeO3SetVAESQAAWl"),
        "lazy protocol witness table accessor for type SwiftUI.Edge.Set and conformance SwiftUI.Edge.Set : Swift.Equatable in SwiftUI"
    );
    assert_eq!(
        dm("_$s11SWBProtocol28BuildTaskStyleMessagePayloadOACSYAAWl"),
        "lazy protocol witness table accessor for type SWBProtocol.BuildTaskStyleMessagePayload and conformance SWBProtocol.BuildTaskStyleMessagePayload : Swift.RawRepresentable in SWBProtocol"
    );
    assert_eq!(
        dm("_$s9ACPAPIKit26RetryableRequestErrorAllOfV10CodingKeys33_352B664ED4F40B0BFFFCE9A65F6DAFCALLOAFs23CustomStringConvertibleAAWl"),
        "lazy protocol witness table accessor for type ACPAPIKit.RetryableRequestErrorAllOf.(CodingKeys in _352B664ED4F40B0BFFFCE9A65F6DAFCA) and conformance ACPAPIKit.RetryableRequestErrorAllOf.(CodingKeys in _352B664ED4F40B0BFFFCE9A65F6DAFCA) : Swift.CustomStringConvertible in ACPAPIKit"
    );
}

/// The type before the conformance need not be the conformance's own type, and a generic type prints its parameters.
#[test]
fn the_type_and_the_conformance_may_differ() {
    // Constructed with `swiftc` and checked with `swift-demangle`.
    assert_eq!(dm("_$s1m2SQVSiAA1PAAWl"), "lazy protocol witness table accessor for type m.SQ and conformance Swift.Int : m.P in m");
    assert_eq!(dm("_$s1m1HVySiGACyxGAA1RAAWl"), "lazy protocol witness table accessor for type m.H<Swift.Int> and conformance m.H<A> : m.R in m");
}

/// `Wb` names a base protocol and then the conformance that inherits it.
#[test]
fn a_base_accessor_names_a_protocol_and_a_conformance() {
    assert_eq!(
        dm("_$s16PreviewsPipeline24ExecutionPointPropertiesV10UpdateModeOSHAASQWb"),
        "base witness table accessor for Swift.Equatable in PreviewsPipeline.ExecutionPointProperties.UpdateMode : Swift.Hashable in PreviewsPipeline"
    );
    assert_eq!(
        dm("_$s13SwiftPackages5DequeVyxGSkAASKWb"),
        "base witness table accessor for Swift.BidirectionalCollection in SwiftPackages.Deque<A> : Swift.RandomAccessCollection in SwiftPackages"
    );
    assert_eq!(
        dm("_$s9ACPAPIKit25CKDBRecordChangesResponseV10CodingKeys33_E0431BA88FC7738BFAA921A9F433610FLLOSHAASQWb"),
        "base witness table accessor for Swift.Equatable in ACPAPIKit.CKDBRecordChangesResponse.(CodingKeys in _E0431BA88FC7738BFAA921A9F433610F) : Swift.Hashable in ACPAPIKit"
    );
    assert_eq!(dm("_$s1m1HVyxGSHAASQWb"), "base witness table accessor for Swift.Equatable in m.H<A> : Swift.Hashable in m");
}

/// `WT` names the path of an associated type, the protocol it must conform to, and a conformance. The path can have several elements, and an element does not have to name its protocol.
#[test]
fn an_associated_type_accessor_names_a_path_a_protocol_and_a_conformance() {
    assert_eq!(
        dm("_$sSu12SIMD4StorageVs11SIMDStorages6ScalarsACP_SEWT"),
        "associated type witness table accessor for Swift.SIMDStorage.Scalar : Swift.Encodable in Swift.UInt.SIMD4Storage : Swift.SIMDStorage in Swift"
    );
    assert_eq!(
        dm("_$s10Foundation10POSIXErrorVAA21_BridgedStoredNSErrorAA4CodeAaDP_AA06_ErrorF8ProtocolPWT"),
        "associated type witness table accessor for Foundation._BridgedStoredNSError.Code : Foundation._ErrorCodeProtocol in Foundation.POSIXError : Foundation._BridgedStoredNSError in Foundation"
    );
    assert_eq!(
        dm("_$s10Foundation10CocoaErrorVAA21_BridgedStoredNSErrorAA4CodeAaDP_8RawValueSYs17FixedWidthIntegerPWT"),
        "associated type witness table accessor for Foundation._BridgedStoredNSError.Code.Swift.RawRepresentable.RawValue : Swift.FixedWidthInteger in Foundation.CocoaError : Foundation._BridgedStoredNSError in Foundation"
    );
    assert_eq!(
        dm("_$s10Foundation27_JSONKeyedDecodingContainer33_12768CA107A31EF2DCE034FD75B541C9LLVyxGs05KeyedcD8ProtocolAA3Key_s06CodingN0PWT"),
        "associated type witness table accessor for Key : Swift.CodingKey in Foundation.(_JSONKeyedDecodingContainer in _12768CA107A31EF2DCE034FD75B541C9)<A> : Swift.KeyedDecodingContainerProtocol in Foundation"
    );
}

/// Constructed paths: one element with and without its protocol, and elements before and after the `_`.
#[test]
fn associated_type_paths_read_as_swift_demangle_reads_them() {
    // Constructed and checked with `swift-demangle`.
    assert_eq!(dm("_$s1m2SQVAA1PAA1AAaDP_AA1QPWT"), "associated type witness table accessor for m.P.A : m.Q in m.SQ : m.P in m");
    assert_eq!(dm("_$s1m2SQVAA1PAA1A_AA1QPWT"), "associated type witness table accessor for A : m.Q in m.SQ : m.P in m");
    assert_eq!(dm("_$s1m2SQVAA1PAA1A_1BSQWT"), "associated type witness table accessor for A.B : Swift.Equatable in m.SQ : m.P in m");
    assert_eq!(dm("_$s1m2SQVAA1PAA1AAaDP_1BAaDPAA1QPWT"), "associated type witness table accessor for m.P.A.m.P.B : m.Q in m.SQ : m.P in m");
    assert_eq!(dm("_$s1m2SQVAA1PAA1AAaDP_1B1CSQWT"), "associated type witness table accessor for m.P.A.B.C : Swift.Equatable in m.SQ : m.P in m");
}

/// `WI` is the function that builds a generic conformance's witness table. It takes the same conformance as `WP`.
#[test]
fn an_instantiation_function_names_a_conformance() {
    assert_eq!(
        dm("_$s19CompilerSwiftSyntax019SameTypeRequirementC0V04LeftE0OAA0C8ProtocolAAWI"),
        "instantiation function for generic protocol witness table for CompilerSwiftSyntax.SameTypeRequirementSyntax.LeftType : CompilerSwiftSyntax.SyntaxProtocol in CompilerSwiftSyntax"
    );
    assert_eq!(
        dm("_$s13XcodeCloudKit27TransformingObservableValue33_84B04509D109AA156A0BE9068F54C453LLCyxq_GAA0eF8ProtocolAAWI"),
        "instantiation function for generic protocol witness table for XcodeCloudKit.(TransformingObservableValue in _84B04509D109AA156A0BE9068F54C453)<A, B> : XcodeCloudKit.ObservableValueProtocol in XcodeCloudKit"
    );
    assert_eq!(dm("_$s1m1HVyxGAA1RAAWI"), "instantiation function for generic protocol witness table for m.H<A> : m.R in m");
}

/// `swift-demangle` prints `(extension in M):` before a type declared in an extension, which this demangler leaves out by design.
#[test]
fn a_type_declared_in_an_extension_leaves_out_the_extension_prefix() {
    assert_eq!(
        dm("_$s10Foundation15AttributeScopesO14XCUIAutomationE28SyntaxHighlightingAttributesVAfA0B5ScopeADWl"),
        "lazy protocol witness table accessor for type Foundation.AttributeScopes.SyntaxHighlightingAttributes and conformance Foundation.AttributeScopes.SyntaxHighlightingAttributes : Foundation.AttributeScope in XCUIAutomation"
    );
    assert_eq!(
        dm("_$s10Foundation3URLV21XCSchemaSerializationE0D5ErrorOAA09LocalizedE0ADs0E0PWb"),
        "base witness table accessor for Swift.Error in Foundation.URL.SerializationError : Foundation.LocalizedError in XCSchemaSerialization"
    );
    assert_eq!(
        dm("_$s10RealityKit12MeshResourceC0A10FoundationE4PartVs12IdentifiableAD2IDsAGP_SHWT"),
        "associated type witness table accessor for Swift.Identifiable.ID : Swift.Hashable in RealityKit.MeshResource.Part : Swift.Identifiable in RealityFoundation"
    );
}

/// `swift-demangle` decodes none of these. A conformance needs a type, a protocol and a module, and each
/// accessor needs its own further operands: a type for `Wl`, a base type for `Wb`, a path ending in `_` for
/// `WT`. A metadata symbol is not a type, and a type is not a module.
#[test]
fn malformed_operands_are_invalid() {
    for symbol in [
        "$s4mainWl",
        "$s4mainWb",
        "$s4mainWI",
        "$s4mainWT",
        "_$s1m2SQVAA1PAAWl",
        "_$s1m2SQVACAA1PAAWb",
        "_$s1m2SQVAA1PAAWT",
        "_$s1m2SQVMaACSHAAWl",
        "_$s1m2SQVMaSHAAWI",
        "_$s1m2SQVMaSHAAWb",
        "_$s1m2SQVSHAAMaWb",
        "_$s1m2SQVSHSQWI",
        "_$s1m2SQVSHSiWI",
        "_$s1m2SQVAA1PAA1ASQWT",
        "_$s1m2SQVAA1PAA_SQWT",
        "_$s1m2SQVAA1PAA1A_1B_SQWT",
        "_$s1m2SQVAA1PAA1ASi_SQWT",
        "_$s1m2SQVAA1PAA1A_AaDPSQWT",
    ] {
        assert_eq!(demangle(symbol).status, Status::Invalid, "{symbol}");
    }
}
