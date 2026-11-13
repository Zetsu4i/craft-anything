//! A minimal DER (ASN.1) reader: enough to walk PKCS#7 SignedData and pull X.509 certificate
//! facts (subject, issuer, serial, validity, algorithms). No signature checking. Bounded depth,
//! checked lengths, never panics.

use serde::Serialize;

/// Deepest nesting walked.
const MAX_DEPTH: usize = 32;
/// Most certificates taken from one PKCS#7 blob.
const MAX_CERTS: usize = 64;
/// A readable encoding limitation, distinct from the result of a signature check.
pub const NON_MINIMAL_LENGTH_NOTE: &str = "An element uses a non-minimal ASN.1 length; not strict DER.";

/// One TLV: tag byte, and the content range within the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tlv {
    pub tag: u8,
    /// Offset of the first byte of the TLV (the tag).
    pub start: usize,
    /// Offset of the content.
    pub content: usize,
    /// Content length.
    pub len: usize,
    /// The length uses a redundant long form or leading zero octet. Contents remain readable.
    pub non_minimal_length: bool,
}

impl Tlv {
    /// Offset just past this TLV.
    pub fn end(&self) -> usize {
        self.content.saturating_add(self.len)
    }

    pub fn bytes<'a>(&self, data: &'a [u8]) -> Option<&'a [u8]> {
        data.get(self.content..self.end())
    }

    /// The whole TLV, header included.
    pub fn whole<'a>(&self, data: &'a [u8]) -> Option<&'a [u8]> {
        data.get(self.start..self.end())
    }

    pub fn constructed(&self) -> bool {
        self.tag & 0x20 != 0
    }
}

/// Read one TLV at `off`. Only single-byte tags and definite lengths of up to 4 bytes.
/// Redundant length encodings are retained and flagged, rather than hiding their contents.
pub fn read(data: &[u8], off: usize) -> Option<Tlv> {
    let tag = *data.get(off)?;
    if tag & 0x1f == 0x1f {
        return None;
    }
    let l0 = *data.get(off.checked_add(1)?)?;
    let (len, hdr, non_minimal_length) = if l0 & 0x80 == 0 {
        (usize::from(l0), 2usize, false)
    } else {
        let n = usize::from(l0 & 0x7f);
        if n == 0 || n > 4 {
            return None;
        }
        let leading_zero = *data.get(off.checked_add(2)?)? == 0;
        let mut v = 0usize;
        for i in 0..n {
            v = v.checked_mul(256)?.checked_add(usize::from(*data.get(off.checked_add(2 + i)?)?))?;
        }
        (v, 2 + n, leading_zero || v < 128)
    };
    let content = off.checked_add(hdr)?;
    let end = content.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    Some(Tlv { tag, start: off, content, len, non_minimal_length })
}

/// Whether a non-minimal length is found while walking constructed elements within the
/// reader's depth/child limits and a 100,000-node budget. False is not a full DER validation.
pub fn has_non_minimal_length(data: &[u8]) -> bool {
    fn walk(data: &[u8], value: &Tlv, depth: usize, remaining: &mut usize) -> bool {
        if *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        if value.non_minimal_length {
            return true;
        }
        if depth >= MAX_DEPTH || !value.constructed() {
            return false;
        }
        for child in children(data, value) {
            if *remaining == 0 {
                break;
            }
            if walk(data, &child, depth + 1, remaining) {
                return true;
            }
        }
        false
    }
    let mut remaining = 100_000;
    read(data, 0).is_some_and(|value| walk(data, &value, 0, &mut remaining))
}

/// The children of a constructed TLV (stops at the first malformed child).
pub fn children(data: &[u8], parent: &Tlv) -> Vec<Tlv> {
    let mut out = Vec::new();
    let mut pos = parent.content;
    let end = parent.end();
    while pos < end && out.len() < 4096 {
        match read(data, pos) {
            Some(t) if t.end() <= end => {
                pos = t.end();
                out.push(t);
            }
            _ => break,
        }
    }
    out
}

/// Dotted form of an OBJECT IDENTIFIER's content.
pub fn oid(content: &[u8]) -> String {
    if content.is_empty() || content.len() > 64 {
        return "invalid".into();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut v: u64 = 0;
    let mut first = true;
    let mut arc_start = true;
    for &b in content {
        // DER subidentifiers use the shortest base-128 encoding.
        if arc_start && b == 0x80 {
            return "invalid".into();
        }
        arc_start = b & 0x80 == 0;
        v = match v.checked_mul(128) {
            Some(x) => x | u64::from(b & 0x7f),
            None => return "invalid".into(),
        };
        if b & 0x80 == 0 {
            if first {
                let (a, b2) = if v < 40 {
                    (0, v)
                } else if v < 80 {
                    (1, v - 40)
                } else {
                    (2, v - 80)
                };
                parts.push(a.to_string());
                parts.push(b2.to_string());
                first = false;
            } else {
                parts.push(v.to_string());
            }
            v = 0;
        }
    }
    if !arc_start {
        return "invalid".into();
    }
    parts.join(".")
}

/// Friendly names for the OIDs certificates usually carry.
pub fn oid_name(o: &str) -> Option<&'static str> {
    Some(match o {
        "2.5.4.3" => "CN",
        "2.5.4.6" => "C",
        "2.5.4.7" => "L",
        "2.5.4.8" => "ST",
        "2.5.4.10" => "O",
        "2.5.4.11" => "OU",
        "2.5.4.5" => "serialNumber",
        "1.2.840.113549.1.9.1" => "emailAddress",
        "1.2.840.113549.1.1.1" => "rsaEncryption",
        "1.2.840.113549.1.1.5" => "sha1WithRSAEncryption",
        "1.2.840.113549.1.1.11" => "sha256WithRSAEncryption",
        "1.2.840.113549.1.1.12" => "sha384WithRSAEncryption",
        "1.2.840.113549.1.1.13" => "sha512WithRSAEncryption",
        "1.2.840.10045.2.1" => "ecPublicKey",
        "1.2.840.10045.4.3.2" => "ecdsa-with-SHA256",
        "1.2.840.10045.4.3.3" => "ecdsa-with-SHA384",
        "1.2.840.10040.4.1" => "dsa",
        "1.2.840.10040.4.3" => "dsa-with-sha1",
        "2.16.840.1.101.3.4.3.2" => "dsa-with-sha256",
        "1.2.840.113549.1.7.2" => "signedData",
        "1.2.840.113549.1.7.1" => "data",
        _ => return None,
    })
}

fn text(data: &[u8], t: &Tlv) -> String {
    let b = t.bytes(data).unwrap_or(&[]);
    match t.tag {
        // BMPString: UTF-16BE.
        0x1e => {
            let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(b).into_owned(),
    }
}

/// Escape values without changing the display order of the certificate's name.
fn escaped_name_value(value: &str) -> String {
    let mut out = String::new();
    for (i, c) in value.char_indices() {
        if c == '\0' {
            out.push_str(r"\00");
            continue;
        }
        if matches!(c, '"' | '+' | ',' | ';' | '<' | '>' | '\\') || (i == 0 && matches!(c, ' ' | '#')) || (c == ' ' && i + 1 == value.len()) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Keep unsupported or malformed name values distinct instead of replacing their bytes.
fn decoded_name_value(data: &[u8], value: &Tlv) -> String {
    use std::fmt::Write;
    let bytes = value.bytes(data).unwrap_or_default();
    let decoded: Option<String> = match value.tag {
        0x0c => std::str::from_utf8(bytes).ok().map(str::to_owned),
        0x12 | 0x13 | 0x16 | 0x1a if bytes.is_ascii() => std::str::from_utf8(bytes).ok().map(str::to_owned),
        0x14 => Some(bytes.iter().copied().map(char::from).collect()),
        0x1e if bytes.len().is_multiple_of(2) => bytes.as_chunks::<2>().0.iter().map(|unit| char::from_u32(u32::from(u16::from_be_bytes(*unit)))).collect(),
        0x1c if bytes.len().is_multiple_of(4) => bytes.as_chunks::<4>().0.iter().map(|unit| char::from_u32(u32::from_be_bytes(*unit))).collect(),
        _ => None,
    };
    if let Some(decoded) = decoded {
        return escaped_name_value(&decoded);
    }
    let mut encoded = String::from("#");
    for byte in value.whole(data).unwrap_or_default() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

/// A distinguished name as `CN=…, O=…`.
pub fn name(data: &[u8], seq: &Tlv) -> String {
    let mut parts = Vec::new();
    for set in children(data, seq) {
        let mut attributes = Vec::new();
        for atv in children(data, &set) {
            let kids = children(data, &atv);
            if let (Some(o), Some(v)) = (kids.first(), kids.get(1)) {
                let id = oid(o.bytes(data).unwrap_or(&[]));
                let key = oid_name(&id).map(str::to_string).unwrap_or(id);
                attributes.push(format!("{key}={}", decoded_name_value(data, v)));
            }
        }
        if !attributes.is_empty() {
            parts.push(attributes.join("+"));
        }
    }
    parts.join(", ")
}

/// Facts read from one X.509 certificate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CertInfo {
    pub subject: String,
    pub issuer: String,
    /// Serial number, hex.
    pub serial: String,
    pub not_before: String,
    pub not_after: String,
    pub signature_algorithm: String,
    pub public_key_algorithm: String,
    pub version: u8,
    /// Encoding limitations that do not discard the readable certificate facts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

fn alg(data: &[u8], seq: &Tlv) -> String {
    children(data, seq)
        .first()
        .filter(|t| t.tag == 0x06)
        .map(|t| {
            let o = oid(t.bytes(data).unwrap_or(&[]));
            oid_name(&o).map(str::to_string).unwrap_or(o)
        })
        .unwrap_or_default()
}

/// Read a DER certificate.
pub fn certificate(data: &[u8]) -> Option<CertInfo> {
    let cert = read(data, 0)?;
    if cert.tag != 0x30 {
        return None;
    }
    let parts = children(data, &cert);
    let tbs = parts.first().filter(|t| t.tag == 0x30)?;
    let mut info = CertInfo { version: 1, ..CertInfo::default() };
    if has_non_minimal_length(data) {
        info.notes.push(NON_MINIMAL_LENGTH_NOTE.into());
    }
    if let Some(sa) = parts.get(1) {
        info.signature_algorithm = alg(data, sa);
    }
    let f = children(data, tbs);
    let mut i = 0usize;
    if let Some(v) = f.first().filter(|t| t.tag == 0xa0) {
        i = 1;
        if let Some(int) = children(data, v).first() {
            info.version = int.bytes(data).and_then(|b| b.last()).copied().unwrap_or(0).saturating_add(1);
        }
    }
    let serial = f.get(i).filter(|t| t.tag == 0x02)?;
    info.serial = serial.bytes(data).unwrap_or(&[]).iter().take(64).map(|b| format!("{b:02x}")).collect();
    if let Some(issuer) = f.get(i + 2) {
        info.issuer = name(data, issuer);
    }
    if let Some(validity) = f.get(i + 3) {
        let v = children(data, validity);
        info.not_before = v.first().map(|t| text(data, t)).unwrap_or_default();
        info.not_after = v.get(1).map(|t| text(data, t)).unwrap_or_default();
    }
    if let Some(subject) = f.get(i + 4) {
        info.subject = name(data, subject);
    }
    if let Some(spki) = f.get(i + 5)
        && let Some(a) = children(data, spki).first()
    {
        info.public_key_algorithm = alg(data, a);
    }
    Some(info)
}

/// The certificates (whole DER TLVs) inside a PKCS#7 SignedData blob, as in `META-INF/*.RSA`.
pub fn pkcs7_certificates(data: &[u8]) -> Vec<&[u8]> {
    let Some(ci) = read(data, 0).filter(|t| t.tag == 0x30) else { return Vec::new() };
    let kids = children(data, &ci);
    let is_signed = kids.first().is_some_and(|t| t.tag == 0x06 && oid(t.bytes(data).unwrap_or(&[])) == "1.2.840.113549.1.7.2");
    if !is_signed {
        return Vec::new();
    }
    let Some(explicit) = kids.get(1).filter(|t| t.tag == 0xa0) else { return Vec::new() };
    let Some(sd) = children(data, explicit).into_iter().next().filter(|t| t.tag == 0x30) else { return Vec::new() };
    let fields = children(data, &sd);
    let [version, digests, content, certificates, ..] = fields.as_slice() else { return Vec::new() };
    if (version.tag, digests.tag, content.tag, certificates.tag) != (0x02, 0x31, 0x30, 0xa0) {
        return Vec::new();
    }
    // Only the untagged SEQUENCE choice is an X.509 certificate.
    children(data, certificates).into_iter().filter(|t| t.tag == 0x30).take(MAX_CERTS).filter_map(|t| t.whole(data)).collect()
}

/// The dotted OID of an AlgorithmIdentifier SEQUENCE's first child (the algorithm OID).
pub fn seq_alg_oid(data: &[u8], seq: &Tlv) -> Option<String> {
    if seq.tag != 0x30 {
        return None;
    }
    let first = read(data.get(..seq.end())?, seq.content)?;
    if first.tag != 0x06 {
        return None;
    }
    let id = oid(first.bytes(data)?);
    (id != "invalid").then_some(id)
}

/// The byte ranges a verifier needs from one DER X.509 certificate.
#[derive(Debug, Clone)]
pub struct CertParts<'a> {
    /// The raw DER of the `tbsCertificate` SEQUENCE (header included): this is the signed message.
    pub tbs: &'a [u8],
    /// The `signatureAlgorithm` OID (dotted).
    pub signature_algorithm: String,
    /// The outer and signed AlgorithmIdentifier values, including their parameters.
    pub signature_algorithm_der: &'a [u8],
    pub signed_signature_algorithm_der: &'a [u8],
    /// The `signatureValue` BIT STRING contents (leading unused-bits byte stripped).
    pub signature: &'a [u8],
    /// The `subjectPublicKeyInfo` SEQUENCE (header included).
    pub spki: &'a [u8],
    /// The `issuer` Name SEQUENCE (header included).
    pub issuer: &'a [u8],
    /// The `subject` Name SEQUENCE (header included).
    pub subject: &'a [u8],
    /// The `serialNumber` INTEGER contents.
    pub serial: &'a [u8],
}

/// The supported key and signature schemes store whole octets in a BIT STRING.
fn bit_string_octets<'a>(data: &'a [u8], bits: &Tlv) -> Option<&'a [u8]> {
    let (unused, bytes) = bits.bytes(data)?.split_first()?;
    (bits.tag == 0x03 && *unused == 0 && !bytes.is_empty()).then_some(bytes)
}

/// Split a certificate into the ranges a verifier needs. `None` on any structural problem.
pub fn certificate_parts(data: &[u8]) -> Option<CertParts<'_>> {
    let cert = read(data, 0)?;
    if cert.tag != 0x30 {
        return None;
    }
    let parts = children(data, &cert);
    let tbs_tlv = parts.first().filter(|t| t.tag == 0x30)?;
    let tbs = tbs_tlv.whole(data)?;
    let sig_alg_seq = parts.get(1)?;
    let signature_algorithm = seq_alg_oid(data, sig_alg_seq)?;
    let signature = bit_string_octets(data, parts.get(2)?)?;
    let f = children(data, tbs_tlv);
    let mut i = 0usize;
    if f.first().is_some_and(|t| t.tag == 0xa0) {
        i = 1;
    }
    let serial_tlv = f.get(i).filter(|t| t.tag == 0x02)?;
    let serial = serial_tlv.bytes(data)?;
    let signed_algorithm = seq_alg_oid(data, f.get(i + 1)?)?;
    if signed_algorithm != signature_algorithm {
        return None;
    }
    let issuer = f.get(i + 2).filter(|t| t.tag == 0x30).and_then(|t| t.whole(data))?;
    let subject = f.get(i + 4).filter(|t| t.tag == 0x30).and_then(|t| t.whole(data))?;
    let spki = f.get(i + 5).filter(|t| t.tag == 0x30).and_then(|t| t.whole(data))?;
    let signature_algorithm_der = sig_alg_seq.whole(data)?;
    let signed_signature_algorithm_der = f.get(i + 1)?.whole(data)?;
    Some(CertParts { tbs, signature_algorithm, signature_algorithm_der, signed_signature_algorithm_der, signature, spki, issuer, subject, serial })
}

/// The `subjectPublicKeyInfo` SEQUENCE of a certificate (header included).
pub fn certificate_spki(data: &[u8]) -> Option<&[u8]> {
    certificate_parts(data).map(|p| p.spki)
}

/// The subject key identifier extension's OCTET STRING, when one complete extension supplies it.
pub fn certificate_subject_key_identifier(data: &[u8]) -> Option<&[u8]> {
    let tbs = certificate_parts(data)?.tbs;
    let fields = children(tbs, &read(tbs, 0)?);
    if fields.last().is_none_or(|last| last.end() != tbs.len()) {
        return None;
    }
    let mut wrappers = fields.iter().filter(|field| field.tag == 0xa3);
    let wrapper = wrappers.next()?;
    if wrappers.next().is_some() {
        return None;
    }
    let sequence = read(tbs, wrapper.content)?;
    if sequence.tag != 0x30 || sequence.end() != wrapper.end() {
        return None;
    }
    let extensions = children(tbs, &sequence);
    if extensions.last().is_none_or(|last| last.end() != sequence.end()) {
        return None;
    }
    let mut key_id = None;
    for extension in extensions {
        if extension.tag != 0x30 {
            return None;
        }
        let fields = children(tbs, &extension);
        let Some(id) = fields.first().filter(|field| field.tag == 0x06).and_then(|field| field.bytes(tbs)) else { continue };
        if oid(id) != "2.5.29.14" {
            continue;
        }
        if key_id.is_some() || fields.last().is_none_or(|last| last.end() != extension.end()) {
            return None;
        }
        let value_index = if let Some(critical) = fields.get(1).filter(|field| field.tag == 0x01) {
            if !matches!(critical.bytes(tbs)?, [0] | [0xff]) {
                return None;
            }
            2
        } else {
            1
        };
        if fields.len() != value_index + 1 {
            return None;
        }
        let encoded = fields.get(value_index).filter(|field| field.tag == 0x04)?.bytes(tbs)?;
        let value = read(encoded, 0)?;
        if value.tag != 0x04 || value.end() != encoded.len() {
            return None;
        }
        key_id = Some(value.bytes(encoded)?);
    }
    key_id
}

/// Split a `subjectPublicKeyInfo` into `(algorithm OID, public-key BIT STRING contents)`.
/// The BIT STRING's leading unused-bits byte is stripped.
pub fn spki_parts(data: &[u8]) -> Option<(String, &[u8])> {
    let seq = read(data, 0)?;
    if seq.tag != 0x30 {
        return None;
    }
    let kids = children(data, &seq);
    let alg = kids.first().filter(|t| t.tag == 0x30)?;
    let algorithm = seq_alg_oid(data, alg)?;
    let key = bit_string_octets(data, kids.get(1)?)?;
    Some((algorithm, key))
}

/// Walk every TLV to `MAX_DEPTH`, counting them (a structural sanity check).
pub fn count_nodes(data: &[u8]) -> usize {
    fn walk(data: &[u8], t: &Tlv, depth: usize, n: &mut usize) {
        *n += 1;
        if depth >= MAX_DEPTH || *n > 100_000 || !t.constructed() {
            return;
        }
        for c in children(data, t) {
            walk(data, &c, depth + 1, n);
        }
    }
    let mut n = 0;
    if let Some(t) = read(data, 0) {
        walk(data, &t, 0, &mut n);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature_identifier(id: &str, null_parameter: bool) -> Vec<u8> {
        use craft_anything_testkit::android::{der, der_oid};
        let mut fields = der_oid(id);
        if null_parameter {
            fields.extend_from_slice(&der(0x05, &[]));
        }
        der(0x30, &fields)
    }

    fn signed_certificate_with_algorithms(inner: &[u8], outer: &[u8], versioned: bool) -> Vec<u8> {
        use craft_anything_testkit::{android::der, crypto};
        let original = crypto::self_signed_rsa("Algorithm Fixture", "Fixture Org");
        let certificate = children(&original, &read(&original, 0).unwrap());
        let fields = children(&original, &certificate[0]);
        let mut tbs = Vec::new();
        for (i, field) in fields.iter().enumerate() {
            if i == 0 && !versioned {
                continue;
            }
            tbs.extend_from_slice(if i == 2 { inner } else { field.whole(&original).unwrap() });
        }
        let tbs = der(0x30, &tbs);
        let mut signature = vec![0];
        signature.extend_from_slice(&crypto::rsa_sign_sha256(&tbs));
        der(0x30, &[tbs.as_slice(), outer, &der(0x03, &signature)].concat())
    }

    fn assert_real_sha256_signature(cert: &[u8]) {
        use crate::verify::{Digest, SigScheme, Verified, verify_signature};
        let parts = children(cert, &read(cert, 0).unwrap());
        let tbs = parts[0].whole(cert).unwrap();
        let signature = &parts[2].bytes(cert).unwrap()[1..];
        assert_eq!(verify_signature(&craft_anything_testkit::crypto::rsa_spki(), SigScheme::RsaPkcs1v15(Digest::Sha256), tbs, signature), Verified::Valid);
    }

    #[test]
    fn conflicting_certificate_signature_oids_do_not_verify() {
        let outer = signature_identifier("1.2.840.113549.1.1.11", true);
        for versioned in [false, true] {
            for id in ["1.2.840.113549.1.1.5", "1.2.840.113549.1.1.12"] {
                let inner = signature_identifier(id, true);
                let cert = signed_certificate_with_algorithms(&inner, &outer, versioned);
                assert_real_sha256_signature(&cert);
                let chain = crate::verify::verify_chain(&[&cert]);
                assert!(!chain.fully_linked, "versioned={versioned}, inner={id}: {chain:?}");
                assert!(certificate_parts(&cert).is_none(), "versioned={versioned}, inner={id}");
            }
        }
    }

    #[test]
    fn malformed_signed_algorithm_fields_do_not_verify() {
        use craft_anything_testkit::android::der;
        let outer = signature_identifier("1.2.840.113549.1.1.11", true);
        for inner in [der(0x02, &[1]), der(0x30, &der(0x02, &[1])), der(0x30, &der(0x06, &[0x80, 0x2a]))] {
            let cert = signed_certificate_with_algorithms(&inner, &outer, true);
            assert_real_sha256_signature(&cert);
            let chain = crate::verify::verify_chain(&[&cert]);
            assert!(!chain.fully_linked, "{chain:?}");
            assert!(certificate_parts(&cert).is_none());
        }
    }

    #[test]
    fn matching_signature_oids_keep_rsa_null_and_absent_parameters() {
        for versioned in [false, true] {
            for inner_null in [false, true] {
                for outer_null in [false, true] {
                    let inner = signature_identifier("1.2.840.113549.1.1.11", inner_null);
                    let outer = signature_identifier("1.2.840.113549.1.1.11", outer_null);
                    let cert = signed_certificate_with_algorithms(&inner, &outer, versioned);
                    assert!(certificate_parts(&cert).is_some());
                    let chain = crate::verify::verify_chain(&[&cert]);
                    assert!(chain.fully_linked);
                    assert!(chain.links[0].signature_verified.is_valid());
                    assert_eq!(chain.trusted_root, None);
                }
            }
        }
    }

    #[test]
    fn verifier_checks_the_fields_before_exposing_their_bytes() {
        let cert = craft_anything_testkit::android::certificate_der("Subject", "Org");
        let outer = children(&cert, &read(&cert, 0).unwrap());
        let fields = children(&cert, &outer[0]);
        assert!(certificate_parts(&cert).is_some());
        for (off, value) in [(outer[1].start, 0x31), (outer[2].content, 1), (fields[3].start, 0x31), (fields[5].start, 0x31)] {
            let mut damaged = cert.clone();
            damaged[off] = value;
            assert!(certificate_parts(&damaged).is_none());
        }
        let spki = certificate_spki(&cert).unwrap();
        let fields = children(spki, &read(spki, 0).unwrap());
        let mut damaged = spki.to_vec();
        damaged[fields[1].content] = 1;
        assert!(spki_parts(spki).is_some());
        assert!(spki_parts(&damaged).is_none());
        // A NULL followed by an OID is not an AlgorithmIdentifier.
        let misplaced = [0x30, 5, 0x05, 0, 0x06, 1, 0x2a];
        assert!(seq_alg_oid(&misplaced, &read(&misplaced, 0).unwrap()).is_none());
    }

    #[test]
    fn name_marks_attribute_boundaries_and_literal_separators() {
        let cert = craft_anything_testkit::android::certificate_der("Alpha, O=Beta", "One+Two");
        let info = certificate(&cert).unwrap();
        assert_eq!(info.subject, r"CN=Alpha\, O=Beta, O=One\+Two");
        assert_eq!(info.issuer, info.subject);

        // One RDN containing a common name and an organization.
        let data = b"\x30\x16\x31\x14\x30\x08\x06\x03\x55\x04\x03\x0c\x01a\x30\x08\x06\x03\x55\x04\x0a\x0c\x01b";
        assert_eq!(name(data, &read(data, 0).unwrap()), "CN=a+O=b");
    }

    #[test]
    fn reads_synthetic_certificate() {
        let cert = craft_anything_testkit::android::certificate_der("Craft Test", "Craft Org");
        let info = certificate(&cert).unwrap();
        assert_eq!(info.subject, "CN=Craft Test, O=Craft Org");
        assert_eq!(info.issuer, "CN=Craft Test, O=Craft Org");
        assert_eq!(info.serial, "0102030405");
        assert_eq!(info.signature_algorithm, "sha256WithRSAEncryption");
        assert_eq!(info.public_key_algorithm, "rsaEncryption");
        assert_eq!(info.version, 3);
        assert_eq!(info.not_before, "250101000000Z");
        let p7 = craft_anything_testkit::android::pkcs7_der(&cert);
        let certs = pkcs7_certificates(&p7);
        assert_eq!(certs, vec![cert.as_slice()]);
        assert!(count_nodes(&p7) > 10);
    }

    #[test]
    fn only_x509_choices_in_the_certificate_field_are_returned() {
        let mut choices = vec![0xa2, 0x00];
        for i in 0..70 {
            choices.extend_from_slice(&[0x30, 3, 2, 1, i]);
        }
        let cms = craft_anything_testkit::android::pkcs7_der(&choices);
        let certs = pkcs7_certificates(&cms);
        assert_eq!(certs.len(), MAX_CERTS);
        assert_eq!(certs[0], [0x30, 3, 2, 1, 0]);
        assert_eq!(certs[MAX_CERTS - 1], [0x30, 3, 2, 1, 63]);

        let ci = children(&cms, &read(&cms, 0).unwrap());
        let sd = children(&cms, &ci[1])[0];
        let fields = children(&cms, &sd);
        for (offset, tag) in [(0, 0x31), (sd.start, 0x31), (fields[0].start, 0x04), (fields[3].start, 0xa1)] {
            let mut damaged = cms.clone();
            damaged[offset] = tag;
            assert!(pkcs7_certificates(&damaged).is_empty());
        }
    }

    #[test]
    fn der_lengths_retain_contents_and_mark_non_minimal_encodings() {
        for (header, len, non_minimal) in [
            (&[0x04, 0x00][..], 0, false),
            (&[0x04, 0x81, 0x00], 0, true),
            (&[0x04, 0x7f], 127, false),
            (&[0x04, 0x81, 0x7f], 127, true),
            (&[0x04, 0x81, 0x80], 128, false),
            (&[0x04, 0x82, 0x00, 0x80], 128, true),
            (&[0x04, 0x82, 0x01, 0x00], 256, false),
            (&[0x04, 0x83, 0x00, 0x01, 0x00], 256, true),
        ] {
            let mut data = header.to_vec();
            data.resize(header.len() + len, 0);
            let value = read(&data, 0).unwrap();
            assert_eq!(value.len, len);
            assert_eq!(value.non_minimal_length, non_minimal);
            assert_eq!(value.bytes(&data), Some(&data[header.len()..]));
            assert_eq!(has_non_minimal_length(&data), non_minimal);
        }
    }

    #[test]
    fn length_inspection_is_bounded_and_does_not_decode_primitive_payloads() {
        use craft_anything_testkit::android::der;
        let redundant = [0x04, 0x81, 0];
        assert!(has_non_minimal_length(&der(0x30, &redundant)));
        assert!(!has_non_minimal_length(&der(0x04, &redundant)));
        let mut nested = redundant.to_vec();
        for _ in 0..MAX_DEPTH {
            nested = der(0x30, &nested);
        }
        assert!(has_non_minimal_length(&nested));
        nested = der(0x30, &nested);
        assert!(!has_non_minimal_length(&nested));
        let branch = der(0x30, &[0x05, 0].repeat(4095));
        let over_budget = der(0x30, &[branch.repeat(25), redundant.to_vec()].concat());
        assert!(!has_non_minimal_length(&over_budget));
        for invalid in [&[0x04, 0x80][..], &[0x04, 0x85, 0, 0, 0, 0, 0], &[0x04, 0x82, 0], &[0x04, 0x81, 0x7f]] {
            assert!(read(invalid, 0).is_none());
        }
    }

    #[test]
    fn oids_and_garbage() {
        assert_eq!(oid(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b]), "1.2.840.113549.1.1.11");
        let cert = craft_anything_testkit::android::pkcs7_der(&craft_anything_testkit::android::certificate_der("A", "B"));
        for m in craft_anything_testkit::mutations(&cert) {
            let _ = pkcs7_certificates(&m);
            let _ = certificate(&m);
            let _ = count_nodes(&m);
        }
        for len in 0..cert.len() {
            let _ = pkcs7_certificates(&cert[..len]);
            let _ = certificate(&cert[..len]);
        }
        for seed in 0..64 {
            let n = craft_anything_testkit::noise(seed, 300);
            let _ = certificate(&n);
            let _ = pkcs7_certificates(&n);
        }
        assert!(read(&[0x30, 0x85, 1, 1, 1, 1, 1], 0).is_none());
        assert!(read(&[0x30, 0x84, 0xff, 0xff, 0xff, 0xff], 0).is_none());
    }
}
