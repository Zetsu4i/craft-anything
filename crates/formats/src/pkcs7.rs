//! CMS `SignedData` (PKCS#7) as used by Apple code signing: the certificate set, the signer's
//! digest and signature algorithms, the authenticated (signed) attributes, the `messageDigest`
//! attribute (the hash of the signed content — for code signing, the primary CodeDirectory), and
//! the signature. Parsing is bounded and never panics; verification is done in [`crate::verify`].

use crate::der::{self, Tlv};
use crate::verify::{Digest, PssParameters, SigScheme};

const SIGNED_DATA: &str = "1.2.840.113549.1.7.2";
const OID_MESSAGE_DIGEST: &str = "1.2.840.113549.1.9.4";
const OID_CONTENT_TYPE: &str = "1.2.840.113549.1.9.3";
const RSA_ENCRYPTION: &str = "1.2.840.113549.1.1.1";
const RSA_PSS: &str = "1.2.840.113549.1.1.10";
const EC_PUBLIC_KEY: &str = "1.2.840.10045.2.1";

/// What the verifier needs out of a CMS `SignedData` blob.
#[derive(Debug, Clone)]
pub struct SignedData {
    /// Certificates (DER), leaf first (as `der::pkcs7_certificates` returns them).
    pub certificates: Vec<Vec<u8>>,
    /// The signer's digest algorithm.
    pub digest: Option<Digest>,
    /// The signer's `signatureAlgorithm` OID (dotted).
    pub signature_algorithm: String,
    /// Parsed PSS parameters, or the reason they cannot be checked; other metadata is retained.
    pub pss_parameters: Option<Result<PssParameters, &'static str>>,
    /// The signature bytes.
    pub signature: Vec<u8>,
    /// The signed (authenticated) attributes re-encoded with a SET tag (`0x31`) — the exact bytes
    /// the signature is computed over. `None` when the signer carries no signed attributes.
    pub signed_attrs_reencoded: Option<Vec<u8>>,
    /// A readable signed-attribute defect that prevents authentication without hiding metadata.
    pub signed_attrs_error: Option<&'static str>,
    /// The `messageDigest` signed attribute (the hash of the signed content), hex.
    pub message_digest: Option<String>,
    /// The `contentType` signed attribute OID (dotted).
    pub content_type: Option<String>,
    /// The signer's issuer `Name` (DER) and serial (bytes), empty for a key identifier.
    pub signer_issuer: Vec<u8>,
    pub signer_serial: Vec<u8>,
    /// The signer's subject key identifier, matched against the certificate extension.
    pub signer_key_identifier: Option<Vec<u8>>,
    /// Readable identity irregularities; these do not discard the certificates or algorithms.
    pub signer_identity_notes: Vec<String>,
    /// At least one walked ASN.1 element uses a redundant length encoding.
    pub non_minimal_length: bool,
    /// The encapsulated eContent, when present (detached signatures have none).
    pub econtent: Option<Vec<u8>>,
}

fn oid_of(data: &[u8], t: &Tlv) -> String {
    der::oid(t.bytes(data).unwrap_or(&[]))
}

pub(crate) fn digest_from_oid(oid: &str) -> Option<Digest> {
    Some(match oid {
        "1.3.14.3.2.26" => Digest::Sha1,
        "2.16.840.1.101.3.4.2.1" => Digest::Sha256,
        "2.16.840.1.101.3.4.2.2" => Digest::Sha384,
        "2.16.840.1.101.3.4.2.3" => Digest::Sha512,
        _ => return None,
    })
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn complete_children(data: &[u8], parent: &Tlv) -> Option<Vec<Tlv>> {
    let fields = der::children(data, parent);
    (fields.last().map_or(parent.content, Tlv::end) == parent.end()).then_some(fields)
}

/// Validate required authenticated attributes while retaining a concrete reason for rejection.
fn signed_attributes(data: &[u8], attrs: &Tlv, encapsulated_type: &str) -> Result<(String, String), &'static str> {
    let mut message_digest = None;
    let mut content_type = None;
    for attribute in complete_children(data, attrs).ok_or("CMS signed attributes: the attribute list is incomplete.")? {
        if attribute.tag != 0x30 {
            return Err("CMS signed attributes: an attribute is not a SEQUENCE.");
        }
        let fields = complete_children(data, &attribute).ok_or("CMS signed attributes: an attribute is incomplete.")?;
        let [oid, values] = fields.as_slice() else { return Err("CMS signed attributes: an attribute must contain an OID and one value set.") };
        if oid.tag != 0x06 || values.tag != 0x31 {
            return Err("CMS signed attributes: an attribute must contain an OID and one value set.");
        }
        let name = oid_of(data, oid);
        if name == "invalid" {
            return Err("CMS signed attributes: an attribute OID is malformed.");
        }
        let values = complete_children(data, values).ok_or("CMS signed attributes: an attribute value set is incomplete.")?;
        if name == OID_MESSAGE_DIGEST || name == OID_CONTENT_TYPE {
            let [value] = values.as_slice() else { return Err("CMS signed attributes: messageDigest and contentType must each have one value.") };
            if name == OID_MESSAGE_DIGEST {
                if message_digest.is_some() {
                    return Err("CMS signed attributes: messageDigest occurs more than once.");
                }
                if value.tag != 0x04 {
                    return Err("CMS signed attributes: messageDigest is not an OCTET STRING.");
                }
                message_digest = Some(hexs(value.bytes(data).ok_or("CMS signed attributes: messageDigest bytes are incomplete.")?));
            } else {
                if content_type.is_some() {
                    return Err("CMS signed attributes: contentType occurs more than once.");
                }
                if value.tag != 0x06 {
                    return Err("CMS signed attributes: contentType is not an OID.");
                }
                if oid_of(data, value) != encapsulated_type {
                    return Err("CMS signed attributes: contentType does not match the encapsulated content type.");
                }
                content_type = Some(encapsulated_type.to_string());
            }
        }
    }
    Ok((message_digest.ok_or("CMS signed attributes: messageDigest is missing.")?, content_type.ok_or("CMS signed attributes: contentType is missing.")?))
}

/// Parse a CMS `SignedData` blob. `None` when it is not well-formed SignedData.
pub fn parse_signed_data(cms: &[u8]) -> Option<SignedData> {
    let ci = der::read(cms, 0)?;
    let kids = der::children(cms, &ci);
    if !kids.first().is_some_and(|t| t.tag == 0x06 && oid_of(cms, t) == SIGNED_DATA) {
        return None;
    }
    let explicit = kids.get(1).filter(|t| t.tag == 0xa0)?;
    let sd = der::children(cms, explicit).into_iter().next()?;
    let fields = der::children(cms, &sd);
    let encapsulated = fields.get(2).filter(|field| field.tag == 0x30)?;
    let ec = complete_children(cms, encapsulated)?;
    let encapsulated_type = oid_of(cms, ec.first().filter(|field| field.tag == 0x06)?);
    if encapsulated_type == "invalid" {
        return None;
    }
    let econtent = ec
        .get(1)
        .filter(|field| field.tag == 0xa0)
        .and_then(|inner| der::children(cms, inner).into_iter().next())
        .and_then(|value| value.bytes(cms))
        .map(<[u8]>::to_vec);
    let mut signer_infos = None;
    for f in &fields {
        if f.tag == 0x31 {
            signer_infos = Some(*f);
        }
    }
    let certificates: Vec<Vec<u8>> = der::pkcs7_certificates(cms).into_iter().map(<[u8]>::to_vec).collect();
    let signer_infos = signer_infos?;
    let signer = der::children(cms, &signer_infos).into_iter().next()?;
    let si = der::children(cms, &signer);
    let version = si.first().filter(|field| field.tag == 0x02)?.bytes(cms)?;
    let mut signer_identity_notes = Vec::new();
    let mut idx = 1usize;
    let sid = si.get(idx)?;
    let (signer_issuer, signer_serial, signer_key_identifier) = match sid.tag {
        0x30 => {
            if version != [1] {
                signer_identity_notes.push("SignerInfo version does not match its issuer-and-serial identifier (expected 1).".into());
            }
            let sid_kids = der::children(cms, sid);
            if sid_kids.len() != 2 || sid_kids.last().is_none_or(|last| last.end() != sid.end()) {
                return None;
            }
            let issuer = sid_kids.first().filter(|field| field.tag == 0x30)?.whole(cms)?.to_vec();
            let serial = sid_kids.get(1).filter(|field| field.tag == 0x02)?.bytes(cms)?;
            if serial.is_empty() {
                signer_identity_notes.push("The signer serial number is empty; its issuer-and-serial identity cannot be matched.".into());
            }
            (issuer, serial.to_vec(), None)
        }
        0x80 => {
            if version != [3] {
                signer_identity_notes.push("SignerInfo version does not match its subject key identifier (expected 3).".into());
            }
            (Vec::new(), Vec::new(), Some(sid.bytes(cms)?.to_vec()))
        }
        _ => return None,
    };
    idx += 1;
    let digest_alg = si.get(idx).filter(|t| t.tag == 0x30)?;
    let digest = der::seq_alg_oid(cms, digest_alg).and_then(|o| digest_from_oid(&o));
    idx += 1;
    let mut signed_attrs_error = None;
    let (signed_attrs_reencoded, message_digest, content_type) = if si.get(idx).is_some_and(|t| t.tag == 0xa0) {
        let attrs = si.get(idx).copied()?;
        idx += 1;
        let (md, ct) = match signed_attributes(cms, &attrs, &encapsulated_type) {
            Ok((md, ct)) => (Some(md), Some(ct)),
            Err(reason) => {
                signed_attrs_error = Some(reason);
                (None, None)
            }
        };
        let reenc = attrs.whole(cms).map(|w| {
            let mut v = w.to_vec();
            if let Some(first) = v.first_mut() {
                *first = 0x31;
            }
            v
        });
        (reenc, md, ct)
    } else {
        (None, None, None)
    };
    let sig_alg = si.get(idx).filter(|t| t.tag == 0x30)?;
    let signature_algorithm = der::seq_alg_oid(cms, sig_alg).unwrap_or_default();
    let pss_parameters = if signature_algorithm == RSA_PSS { Some(crate::verify::pss_parameters(sig_alg.whole(cms)?)) } else { None };
    idx += 1;
    let signature = si.get(idx).filter(|t| t.tag == 0x04).and_then(|t| t.bytes(cms)).unwrap_or(&[]).to_vec();
    Some(SignedData {
        certificates,
        digest,
        signature_algorithm,
        pss_parameters,
        signature,
        signed_attrs_reencoded,
        signed_attrs_error,
        message_digest,
        content_type,
        signer_issuer,
        signer_serial,
        signer_key_identifier,
        signer_identity_notes,
        non_minimal_length: der::has_non_minimal_length(cms),
        econtent,
    })
}

impl SignedData {
    /// The certificate whose issuer+serial or subject key identifier matches the SignerIdentifier.
    /// An absent match leaves the signer's identity unknown.
    pub fn signer_cert(&self) -> Option<&[u8]> {
        if let Some(key_id) = &self.signer_key_identifier {
            return self.certificates.iter().find(|c| der::certificate_subject_key_identifier(c) == Some(key_id.as_slice())).map(Vec::as_slice);
        }
        if self.signer_issuer.is_empty() || self.signer_serial.is_empty() {
            return None;
        }
        for c in &self.certificates {
            if let Some(p) = der::certificate_parts(c)
                && p.issuer == self.signer_issuer.as_slice()
                && p.serial == self.signer_serial.as_slice()
            {
                return Some(c);
            }
        }
        None
    }

    /// The signature scheme implied by the signer's signature and digest algorithms.
    /// PSS is returned only when its parameters fit `SigScheme` without losing information;
    /// the CMS verification functions also support parameter sets with other salt lengths.
    pub fn scheme(&self) -> Option<SigScheme> {
        let digest = self.digest?;
        match self.signature_algorithm.as_str() {
            RSA_ENCRYPTION => Some(SigScheme::RsaPkcs1v15(digest)),
            RSA_PSS => self.pss_parameters.as_ref()?.as_ref().ok().and_then(|params| params.fixed_scheme(digest)),
            EC_PUBLIC_KEY => Some(SigScheme::Ecdsa(digest)),
            other => crate::verify::scheme_from_oid(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTENT: &[u8] = b"the content bound by the CMS signed attributes";
    const DATA_TYPE: &str = "1.2.840.113549.1.7.1";

    fn attribute(oid: &str, values: &[Vec<u8>]) -> Vec<u8> {
        use craft_anything_testkit::android::{der, der_oid};
        der(0x30, &[der_oid(oid), der(0x31, &values.concat())].concat())
    }

    fn required_attributes() -> (Vec<u8>, Vec<u8>) {
        use craft_anything_testkit::{
            android::{der, der_oid},
            crypto,
        };
        (attribute(OID_CONTENT_TYPE, &[der_oid(DATA_TYPE)]), attribute(OID_MESSAGE_DIGEST, &[der(0x04, &crypto::sha256(CONTENT))]))
    }

    fn genuinely_signed_cms(attributes: Option<Vec<Vec<u8>>>, tail: &[u8]) -> Vec<u8> {
        use craft_anything_testkit::{android::der, crypto};
        let template = crypto::cms_detached_rsa(CONTENT, "Attribute Signer", "Fixture Org");
        let ci = super::der::children(&template, &super::der::read(&template, 0).unwrap());
        let sd = super::der::children(&template, &ci[1]);
        let fields = super::der::children(&template, &sd[0]);
        let signers = super::der::children(&template, fields.last().unwrap());
        let signer = super::der::children(&template, &signers[0]);
        let attribute_bytes = attributes.map(|mut attributes| {
            attributes.sort();
            [attributes.concat(), tail.to_vec()].concat()
        });
        let signed = attribute_bytes.as_ref().map(|bytes| der(0x31, bytes)).unwrap_or_else(|| CONTENT.to_vec());
        let signature = crypto::rsa_sign_sha256(&signed);
        assert!(crate::verify::verify_signature(&crypto::rsa_spki(), SigScheme::RsaPkcs1v15(Digest::Sha256), &signed, &signature).is_valid());
        let mut signer_fields = Vec::new();
        for (i, field) in signer.iter().enumerate() {
            match i {
                3 => {
                    if let Some(bytes) = &attribute_bytes {
                        signer_fields.extend_from_slice(&der(0xa0, bytes));
                    }
                }
                5 => signer_fields.extend_from_slice(&der(0x04, &signature)),
                _ => signer_fields.extend_from_slice(field.whole(&template).unwrap()),
            }
        }
        let mut signed_data = Vec::new();
        for field in fields.iter().take(fields.len() - 1) {
            signed_data.extend_from_slice(field.whole(&template).unwrap());
        }
        signed_data.extend_from_slice(&der(0x31, &der(0x30, &signer_fields)));
        der(0x30, &[ci[0].whole(&template).unwrap(), &der(0xa0, &der(0x30, &signed_data))].concat())
    }

    fn assert_attributes_rejected(cms: &[u8]) {
        let parsed = parse_signed_data(cms).expect("readable SignedData with malformed attributes");
        assert_eq!(parsed.certificates.len(), 1);
        assert_eq!(parsed.digest, Some(Digest::Sha256));
        assert!(!parsed.signature_algorithm.is_empty());
        assert!(parsed.signer_cert().is_some());
        assert!(!parsed.signature.is_empty());
        let macho = crate::verify::verify_macho_cms(cms, CONTENT).expect("verification result with limitations");
        assert!(!macho.authenticated);
        assert!(macho.signature_valid.is_valid(), "the exact attribute bytes really were signed");
        assert_eq!(macho.certificate_count, 1);
        assert!(macho.signer_subject.is_some());
        assert_eq!(macho.digest_algorithm, Some("sha256"));
        assert!(macho.notes.iter().any(|note| note.contains("signed attributes")), "{:?}", macho.notes);
        let jar = crate::verify::verify_pkcs7_detached(CONTENT, cms).expect("detached verification result with limitations");
        assert_eq!(jar.signature_valid, crate::verify::Verified::Invalid);
        assert!(jar.signer_subject.is_some());
        assert!(jar.certificate_sha256.is_some());
        assert!(jar.notes.iter().any(|note| note.contains("signed attributes")), "{:?}", jar.notes);
    }

    #[test]
    fn message_digest_must_be_an_octet_string() {
        use craft_anything_testkit::{android::der, crypto};
        let (content_type, _) = required_attributes();
        for tag in [0x02, 0x0c, 0x03] {
            let digest = attribute(OID_MESSAGE_DIGEST, &[der(tag, &crypto::sha256(CONTENT))]);
            assert_attributes_rejected(&genuinely_signed_cms(Some(vec![content_type.clone(), digest]), &[]));
        }
    }

    #[test]
    fn required_attributes_are_unique_and_single_valued() {
        use craft_anything_testkit::{
            android::{der, der_oid},
            crypto,
        };
        let (content_type, digest) = required_attributes();
        let multiple_digests = attribute(OID_MESSAGE_DIGEST, &[der(0x04, &crypto::sha256(CONTENT)), der(0x04, b"other")]);
        let multiple_types = attribute(OID_CONTENT_TYPE, &[der_oid(DATA_TYPE), der_oid(SIGNED_DATA)]);
        for attributes in [
            vec![content_type.clone(), digest.clone(), digest.clone()],
            vec![content_type.clone(), content_type.clone(), digest.clone()],
            vec![content_type.clone(), multiple_digests],
            vec![multiple_types, digest.clone()],
            vec![digest],
            vec![content_type],
        ] {
            assert_attributes_rejected(&genuinely_signed_cms(Some(attributes), &[]));
        }
    }

    #[test]
    fn signed_content_type_must_match_encapsulated_type() {
        use craft_anything_testkit::android::{der, der_oid};
        let (_, digest) = required_attributes();
        let oid = der_oid(DATA_TYPE);
        let encoded_oid = super::der::read(&oid, 0).unwrap();
        for value in [der_oid(SIGNED_DATA), der(0x04, encoded_oid.bytes(&oid).unwrap()), der(0x06, &[0x80, 0x2a])] {
            let content_type = attribute(OID_CONTENT_TYPE, &[value]);
            assert_attributes_rejected(&genuinely_signed_cms(Some(vec![content_type, digest.clone()]), &[]));
        }
    }

    #[test]
    fn malformed_attribute_containers_cannot_hide_behind_a_valid_prefix() {
        use craft_anything_testkit::android::der;
        let (content_type, digest) = required_attributes();
        assert_attributes_rejected(&genuinely_signed_cms(Some(vec![content_type.clone(), digest.clone()]), &[0x30]));
        let mut wrong_tag = digest.clone();
        wrong_tag[0] = 0x31;
        let digest_tlv = super::der::read(&digest, 0).unwrap();
        let fields = digest_tlv.bytes(&digest).unwrap();
        for malformed in [wrong_tag, der(0x30, &[fields, &[0x06]].concat()), der(0x30, &[fields, &der(0x05, &[])].concat())] {
            assert_attributes_rejected(&genuinely_signed_cms(Some(vec![content_type.clone(), malformed]), &[]));
        }
    }

    #[test]
    fn complete_attributes_keep_unknown_attributes_and_content_binding() {
        use craft_anything_testkit::android::der;
        let (content_type, digest) = required_attributes();
        let cms = genuinely_signed_cms(
            Some(vec![content_type, digest, attribute("1.2.3.4", &[]), attribute("1.2.3.5", &[der(0x04, b"one"), der(0x04, b"two")])]),
            &[],
        );
        let parsed = parse_signed_data(&cms).unwrap();
        assert_eq!(parsed.content_type.as_deref(), Some(DATA_TYPE));
        assert!(crate::verify::verify_macho_cms(&cms, CONTENT).unwrap().authenticated);
        assert!(crate::verify::verify_pkcs7_detached(CONTENT, &cms).unwrap().signature_valid.is_valid());
        assert!(!crate::verify::verify_macho_cms(&cms, b"changed").unwrap().authenticated);
        assert!(!crate::verify::verify_pkcs7_detached(b"changed", &cms).unwrap().signature_valid.is_valid());
    }

    #[test]
    fn detached_data_without_signed_attributes_remains_supported() {
        let cms = genuinely_signed_cms(None, &[]);
        assert!(parse_signed_data(&cms).unwrap().signed_attrs_reencoded.is_none());
        assert!(crate::verify::verify_pkcs7_detached(CONTENT, &cms).unwrap().signature_valid.is_valid());
        assert!(!crate::verify::verify_pkcs7_detached(b"changed", &cms).unwrap().signature_valid.is_valid());
    }

    #[test]
    fn malformed_attributes_keep_signer_identity_notes_in_both_reports() {
        let (_, digest) = required_attributes();
        let mut cms = genuinely_signed_cms(Some(vec![digest]), &[]);
        let ci = super::der::children(&cms, &super::der::read(&cms, 0).unwrap());
        let sd = super::der::children(&cms, &ci[1]);
        let fields = super::der::children(&cms, &sd[0]);
        let signers = super::der::children(&cms, fields.last().unwrap());
        let signer = super::der::children(&cms, &signers[0]);
        // The version is outside the signed bytes, so the genuine attribute signature remains valid.
        cms[signer[0].content] = 3;

        let parsed = parse_signed_data(&cms).unwrap();
        let identity_note = parsed.signer_identity_notes.first().expect("the identifier/version mismatch");
        let attribute_error = parsed.signed_attrs_error.expect("the missing contentType");
        assert!(identity_note.contains("version"));
        assert!(attribute_error.contains("contentType"));
        assert_attributes_rejected(&cms);

        let macho = crate::verify::verify_macho_cms(&cms, CONTENT).unwrap();
        assert!(macho.notes.contains(identity_note));
        assert!(macho.notes.iter().any(|note| note == attribute_error));
        let detached = crate::verify::verify_pkcs7_detached(CONTENT, &cms).unwrap();
        assert!(detached.notes.contains(identity_note));
        assert!(detached.notes.iter().any(|note| note == attribute_error));
    }

    #[test]
    fn empty_and_garbage() {
        assert!(parse_signed_data(&[]).is_none());
        for seed in 0..64 {
            let n = craft_anything_testkit::noise(seed, 300);
            let _ = parse_signed_data(&n);
        }
        let p7 = craft_anything_testkit::android::pkcs7_der(&craft_anything_testkit::android::certificate_der("A", "B"));
        let _ = parse_signed_data(&p7);
    }
}
