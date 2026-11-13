//! Public-key signature verification for code signatures, with pure-Rust RustCrypto back-ends
//! (RSA PKCS#1 v1.5 and PSS via `rsa`, ECDSA P-256/P-384 via `p256`/`p384`). Everything here is
//! honest about what it proves: a result is reported `valid` only when a cryptographic verify
//! returned success, `invalid` when it failed, and `unsupported` when we cannot check the
//! algorithm at all — never `valid` by default. Authenticity (a chain to a trusted root) is
//! reported separately from integrity/validity of a single signature.
//!
//! Wired into the Mach-O code-signature CMS (`verify_macho_cms`), the APK v2/v3 signing block
//! (`verify_apk_signer`) and the APK v1 (JAR) scheme (`verify_pkcs7_detached` + the JAR helpers).

use crate::der;
use crate::pkcs7;
use rsa::signature::Verifier as _;
use serde::Serialize;
use sha2::Digest as _;

// X.509 / PKCS algorithm OIDs.
const RSA_ENCRYPTION: &str = "1.2.840.113549.1.1.1";
const RSA_PSS: &str = "1.2.840.113549.1.1.10";
const EC_PUBLIC_KEY: &str = "1.2.840.10045.2.1";

/// A message-digest algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Digest {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Digest {
    pub fn hash(self, data: &[u8]) -> Vec<u8> {
        match self {
            Digest::Sha1 => sha1::Sha1::digest(data).to_vec(),
            Digest::Sha256 => sha2::Sha256::digest(data).to_vec(),
            Digest::Sha384 => sha2::Sha384::digest(data).to_vec(),
            Digest::Sha512 => sha2::Sha512::digest(data).to_vec(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Digest::Sha1 => "sha1",
            Digest::Sha256 => "sha256",
            Digest::Sha384 => "sha384",
            Digest::Sha512 => "sha512",
        }
    }
}

/// A public-key signature scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SigScheme {
    RsaPkcs1v15(Digest),
    /// Matching message/MGF1 digests and a digest-sized salt (the APK PSS schemes).
    RsaPss(Digest),
    Ecdsa(Digest),
}

/// Parameters needed to verify an RSASSA-PSS signature, including non-default salt lengths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PssParameters {
    digest: Digest,
    mask_digest: Digest,
    salt_len: usize,
}

impl PssParameters {
    fn for_digest(digest: Digest) -> Self {
        let salt_len = match digest {
            Digest::Sha1 => 20,
            Digest::Sha256 => 32,
            Digest::Sha384 => 48,
            Digest::Sha512 => 64,
        };
        Self { digest, mask_digest: digest, salt_len }
    }

    pub(crate) fn fixed_scheme(self, digest: Digest) -> Option<SigScheme> {
        (self == Self::for_digest(digest)).then_some(SigScheme::RsaPss(digest))
    }
}

// AlgorithmIdentifier has one OID and at most one complete parameter value.
fn algorithm_parts(data: &[u8]) -> Option<(String, Option<&[u8]>)> {
    let root = der::read(data, 0)?;
    if root.tag != 0x30 || root.end() != data.len() {
        return None;
    }
    let oid = der::read(data, root.content)?;
    if oid.tag != 0x06 {
        return None;
    }
    let params = if oid.end() == root.end() {
        None
    } else {
        let params = der::read(data, oid.end())?;
        if params.end() != root.end() {
            return None;
        }
        Some(params.whole(data)?)
    };
    Some((der::oid(oid.bytes(data)?), params))
}

fn pss_digest(data: &[u8]) -> Result<Digest, &'static str> {
    let (oid, params) = algorithm_parts(data).ok_or("malformed RSASSA-PSS digest algorithm")?;
    if params.is_some_and(|p| p != [0x05, 0x00]) {
        return Err("RSASSA-PSS digest parameters are neither absent nor NULL");
    }
    pkcs7::digest_from_oid(&oid).ok_or("RSASSA-PSS digest algorithm is not checked")
}

fn pss_integer(data: &[u8]) -> Option<usize> {
    let integer = der::read(data, 0)?;
    if integer.tag != 0x02 || integer.end() != data.len() {
        return None;
    }
    let bytes = integer.bytes(data)?;
    let first = *bytes.first()?;
    if first & 0x80 != 0 || (bytes.len() > 1 && first == 0 && bytes.get(1).is_some_and(|b| b & 0x80 == 0)) {
        return None;
    }
    bytes.iter().try_fold(0usize, |n, b| n.checked_mul(256)?.checked_add(usize::from(*b)))
}

fn pss_parameter_sequence(data: &[u8]) -> Result<PssParameters, &'static str> {
    let root = der::read(data, 0).ok_or("malformed RSASSA-PSS parameters")?;
    if root.tag != 0x30 || root.end() != data.len() {
        return Err("RSASSA-PSS parameters are not a complete sequence");
    }
    let mut params = PssParameters::for_digest(Digest::Sha1);
    let mut pos = root.content;
    let mut previous_tag = 0x9f;
    while pos < root.end() {
        let field = der::read(data, pos).ok_or("truncated RSASSA-PSS parameter")?;
        if !(0xa0..=0xa3).contains(&field.tag) || field.tag <= previous_tag {
            return Err("unknown, repeated or out-of-order RSASSA-PSS parameter");
        }
        let value = field.bytes(data).ok_or("truncated RSASSA-PSS parameter")?;
        match field.tag {
            0xa0 => params.digest = pss_digest(value)?,
            0xa1 => {
                let (oid, mask) = algorithm_parts(value).ok_or("malformed RSASSA-PSS mask algorithm")?;
                if oid != "1.2.840.113549.1.1.8" {
                    return Err("RSASSA-PSS mask algorithm is not MGF1");
                }
                params.mask_digest = pss_digest(mask.ok_or("RSASSA-PSS MGF1 has no digest parameters")?)?;
            }
            0xa2 => params.salt_len = pss_integer(value).ok_or("invalid RSASSA-PSS salt length")?,
            0xa3 => {
                if pss_integer(value) != Some(1) {
                    return Err("RSASSA-PSS trailer field is not 1");
                }
            }
            _ => return Err("unknown RSASSA-PSS parameter"),
        }
        previous_tag = field.tag;
        pos = field.end();
    }
    Ok(params)
}

/// Signature identifiers require a parameter sequence; defaults apply inside that sequence.
pub(crate) fn pss_parameters(algorithm: &[u8]) -> Result<PssParameters, &'static str> {
    let (oid, params) = algorithm_parts(algorithm).ok_or("malformed RSASSA-PSS signature algorithm")?;
    if oid != RSA_PSS {
        return Err("signature algorithm is not RSASSA-PSS");
    }
    pss_parameter_sequence(params.ok_or("RSASSA-PSS signature parameters are missing")?)
}

/// Hash several byte ranges as one message, without copying them together.
pub fn hash_parts(digest: Digest, parts: &[&[u8]]) -> Vec<u8> {
    fn run<D: sha2::Digest>(parts: &[&[u8]]) -> Vec<u8> {
        let mut h = D::new();
        for p in parts {
            h.update(p);
        }
        h.finalize().to_vec()
    }
    match digest {
        Digest::Sha1 => run::<sha1::Sha1>(parts),
        Digest::Sha256 => run::<sha2::Sha256>(parts),
        Digest::Sha384 => run::<sha2::Sha384>(parts),
        Digest::Sha512 => run::<sha2::Sha512>(parts),
    }
}

/// The signature algorithm OID of an X.509 `signatureAlgorithm` or CMS `signatureAlgorithm`.
pub fn scheme_from_oid(oid: &str) -> Option<SigScheme> {
    Some(match oid {
        "1.2.840.113549.1.1.5" => SigScheme::RsaPkcs1v15(Digest::Sha1),
        "1.2.840.113549.1.1.11" => SigScheme::RsaPkcs1v15(Digest::Sha256),
        "1.2.840.113549.1.1.12" => SigScheme::RsaPkcs1v15(Digest::Sha384),
        "1.2.840.113549.1.1.13" => SigScheme::RsaPkcs1v15(Digest::Sha512),
        "1.2.840.10045.4.3.2" => SigScheme::Ecdsa(Digest::Sha256),
        "1.2.840.10045.4.3.3" => SigScheme::Ecdsa(Digest::Sha384),
        "1.2.840.10045.4.3.4" => SigScheme::Ecdsa(Digest::Sha512),
        _ => return None,
    })
}

/// The outcome of one verification. `valid` only ever means a cryptographic verify succeeded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Verified {
    Valid,
    Invalid,
    Unsupported { reason: String },
}

impl Verified {
    pub fn is_valid(&self) -> bool {
        matches!(self, Verified::Valid)
    }
    fn unsupported(reason: impl Into<String>) -> Self {
        Verified::Unsupported { reason: reason.into() }
    }
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha256_hex(b: &[u8]) -> String {
    hexs(&sha2::Sha256::digest(b))
}

// ---------------------------------------------------------------------------------------------
// Primitive: verify one signature against a SubjectPublicKeyInfo.

/// Verify `sig` over `msg` with `scheme`, using the public key in `spki` (a DER
/// `SubjectPublicKeyInfo`). Returns `Valid` only when the crate's verify succeeded.
pub fn verify_signature(spki: &[u8], scheme: SigScheme, msg: &[u8], sig: &[u8]) -> Verified {
    let Some((alg_oid, key)) = der::spki_parts(spki) else {
        return Verified::unsupported("public key could not be parsed");
    };
    match scheme {
        SigScheme::RsaPkcs1v15(d) => {
            if alg_oid != RSA_ENCRYPTION {
                return Verified::unsupported(format!("PKCS#1 v1.5 signature but key is {alg_oid}"));
            }
            rsa_pkcs1v15(key, d, msg, sig)
        }
        SigScheme::RsaPss(d) => verify_pss_signature(spki, PssParameters::for_digest(d), msg, sig),
        SigScheme::Ecdsa(d) => {
            if alg_oid != EC_PUBLIC_KEY {
                return Verified::unsupported(format!("ECDSA signature but key is {alg_oid}"));
            }
            let Some(curve) = ec_named_curve(spki) else {
                return Verified::unsupported("EC public key has no complete named-curve parameter");
            };
            ecdsa(&curve, key, d, msg, sig)
        }
    }
}

fn ec_named_curve(spki: &[u8]) -> Option<String> {
    let root = der::read(spki, 0)?;
    let algorithm = der::read(spki.get(..root.end())?, root.content)?;
    let bounded = spki.get(..algorithm.end())?;
    let oid = der::read(bounded, algorithm.content)?;
    let curve = der::read(bounded, oid.end())?;
    if curve.tag != 0x06 || curve.end() != algorithm.end() {
        return None;
    }
    let name = der::oid(curve.bytes(spki)?);
    (name != "invalid").then_some(name)
}

fn rsa_key(pkcs1_der: &[u8]) -> Option<rsa::RsaPublicKey> {
    use rsa::pkcs1::DecodeRsaPublicKey;
    rsa::RsaPublicKey::from_pkcs1_der(pkcs1_der).ok()
}

fn rsa_pkcs1v15(pkcs1_der: &[u8], d: Digest, msg: &[u8], sig: &[u8]) -> Verified {
    let Some(key) = rsa_key(pkcs1_der) else { return Verified::Invalid };
    let Ok(signature) = rsa::pkcs1v15::Signature::try_from(sig) else { return Verified::Invalid };
    let ok = match d {
        Digest::Sha1 => rsa::pkcs1v15::VerifyingKey::<sha1::Sha1>::new(key).verify(msg, &signature).is_ok(),
        Digest::Sha256 => rsa::pkcs1v15::VerifyingKey::<sha2::Sha256>::new(key).verify(msg, &signature).is_ok(),
        Digest::Sha384 => rsa::pkcs1v15::VerifyingKey::<sha2::Sha384>::new(key).verify(msg, &signature).is_ok(),
        Digest::Sha512 => rsa::pkcs1v15::VerifyingKey::<sha2::Sha512>::new(key).verify(msg, &signature).is_ok(),
    };
    if ok { Verified::Valid } else { Verified::Invalid }
}

fn verify_pss_signature(spki: &[u8], params: PssParameters, msg: &[u8], sig: &[u8]) -> Verified {
    let Some((oid, key)) = der::spki_parts(spki) else {
        return Verified::unsupported("public key could not be parsed");
    };
    if oid != RSA_ENCRYPTION && oid != RSA_PSS {
        return Verified::unsupported(format!("RSASSA-PSS signature but key is {oid}"));
    }
    if oid == RSA_PSS {
        let algorithm =
            der::read(spki, 0).and_then(|root| der::read(spki.get(..root.end())?, root.content)).and_then(|alg| alg.whole(spki)).and_then(algorithm_parts);
        let Some((_, restriction)) = algorithm else {
            return Verified::unsupported("malformed RSASSA-PSS public-key algorithm");
        };
        if let Some(restriction) = restriction {
            let key_params = match pss_parameter_sequence(restriction) {
                Ok(p) => p,
                Err(reason) => return Verified::unsupported(reason),
            };
            if key_params.digest != params.digest || key_params.mask_digest != params.mask_digest || params.salt_len < key_params.salt_len {
                return Verified::Invalid;
            }
        }
    }
    if params.digest != params.mask_digest {
        return Verified::unsupported("RSASSA-PSS with different message and MGF1 digests is not checked");
    }
    rsa_pss(key, params, msg, sig)
}

fn rsa_pss(pkcs1_der: &[u8], params: PssParameters, msg: &[u8], sig: &[u8]) -> Verified {
    let Some(key) = rsa_key(pkcs1_der) else { return Verified::Invalid };
    // Bound attacker-controlled salt lengths before the verifier's padding-size arithmetic.
    if params.salt_len > rsa::traits::PublicKeyParts::size(&key) {
        return Verified::Invalid;
    }
    let Ok(signature) = rsa::pss::Signature::try_from(sig) else { return Verified::Invalid };
    let ok = match params.digest {
        Digest::Sha256 => rsa::pss::VerifyingKey::<sha2::Sha256>::new_with_salt_len(key, params.salt_len).verify(msg, &signature).is_ok(),
        Digest::Sha384 => rsa::pss::VerifyingKey::<sha2::Sha384>::new_with_salt_len(key, params.salt_len).verify(msg, &signature).is_ok(),
        Digest::Sha512 => rsa::pss::VerifyingKey::<sha2::Sha512>::new_with_salt_len(key, params.salt_len).verify(msg, &signature).is_ok(),
        Digest::Sha1 => return Verified::unsupported("RSASSA-PSS with SHA-1 is not checked"),
    };
    if ok { Verified::Valid } else { Verified::Invalid }
}

/// ECDSA over a SEC1 public-key point using the declared named curve. The
/// digest used is the curve's standard one, so non-standard curve/hash pairings are reported
/// unsupported rather than guessed.
fn ecdsa(curve: &str, sec1: &[u8], d: Digest, msg: &[u8], der_sig: &[u8]) -> Verified {
    match curve {
        "1.2.840.10045.3.1.7" => {
            if d != Digest::Sha256 {
                return Verified::unsupported(format!("ECDSA P-256 with {} is not checked", d.name()));
            }
            use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
            let Ok(vk) = VerifyingKey::from_sec1_bytes(sec1) else { return Verified::Invalid };
            let Ok(s) = Signature::from_der(der_sig) else { return Verified::Invalid };
            if vk.verify(msg, &s).is_ok() { Verified::Valid } else { Verified::Invalid }
        }
        "1.3.132.0.34" => {
            if d != Digest::Sha384 {
                return Verified::unsupported(format!("ECDSA P-384 with {} is not checked", d.name()));
            }
            use p384::ecdsa::{Signature, VerifyingKey, signature::Verifier};
            let Ok(vk) = VerifyingKey::from_sec1_bytes(sec1) else { return Verified::Invalid };
            let Ok(s) = Signature::from_der(der_sig) else { return Verified::Invalid };
            if vk.verify(msg, &s).is_ok() { Verified::Valid } else { Verified::Invalid }
        }
        _ => Verified::unsupported(format!("EC named curve {curve} is not checked (only P-256/P-384)")),
    }
}

// ---------------------------------------------------------------------------------------------
// Certificate chain.

/// Known Apple root CA certificate fingerprints: SHA-256 of the whole DER certificate.
/// Published by Apple at <https://www.apple.com/certificateauthority/>. A missing or wrong value
/// here can only ever suppress a positive "trusted root" claim; it can never create a false one,
/// because trust is only asserted on an exact match.
const APPLE_ROOTS: &[(&str, &str)] = &[
    ("Apple Root CA", "b0b1730ecbc7ff4505142c49f1295e6eda6bcaed7e2c68c5be91b5a11001f024"),
    ("Apple Root CA - G2", "c2b9b042dd57830e7d117dac55ac8ae19407d38e41d88f3215bc3a890444a050"),
    ("Apple Root CA - G3", "63343abfb89a6a03ebb57e9b3f5fa7be7c4f5c756f3017b3a8c488c3653e9179"),
];

/// One link in a certificate chain.
#[derive(Debug, Clone, Serialize)]
pub struct ChainLink {
    pub subject: String,
    pub issuer: String,
    pub sha256: String,
    /// Whether this certificate's signature was verified against its issuer in the set.
    pub signature_verified: Verified,
    /// The subject equals the issuer and the certificate's own key verified its signature.
    pub self_signed: bool,
}

/// The result of walking a certificate chain.
#[derive(Debug, Clone, Serialize)]
pub struct ChainResult {
    pub links: Vec<ChainLink>,
    /// Every link verified and the walk reached a verified self-signed top certificate.
    pub fully_linked: bool,
    pub root_subject: Option<String>,
    pub root_sha256: Option<String>,
    /// `Some(name)` when the top certificate's SHA-256 matches a bundled trusted Apple root.
    pub trusted_root: Option<&'static str>,
    pub notes: Vec<String>,
}

/// Verify `child`'s signature against `issuer`'s public key.
fn verify_cert_by(child: &[u8], issuer: &[u8]) -> Verified {
    let (Some(c), Some(i)) = (der::certificate_parts(child), der::certificate_parts(issuer)) else {
        return Verified::unsupported("certificate could not be parsed");
    };
    if c.signature_algorithm == RSA_PSS {
        let (outer, signed) = match (pss_parameters(c.signature_algorithm_der), pss_parameters(c.signed_signature_algorithm_der)) {
            (Ok(outer), Ok(signed)) => (outer, signed),
            (Err(reason), _) | (_, Err(reason)) => return Verified::unsupported(reason),
        };
        if outer != signed {
            return Verified::Invalid;
        }
        return verify_pss_signature(i.spki, outer, c.tbs, c.signature);
    }
    match scheme_from_oid(&c.signature_algorithm) {
        Some(scheme) => verify_signature(i.spki, scheme, c.tbs, c.signature),
        None => Verified::unsupported(format!("signature algorithm {} is not checked", c.signature_algorithm)),
    }
}

/// Walk and verify a certificate chain, starting from `certs[0]` (the leaf/signer) and following
/// issuer→subject links through the provided set. Bounded; tolerant of missing issuers.
pub fn verify_chain(certs: &[&[u8]]) -> ChainResult {
    let mut links: Vec<ChainLink> = Vec::new();
    let mut notes = Vec::new();
    let mut fully_linked = false;
    let mut seen: Vec<String> = Vec::new();
    let Some(&first) = certs.first() else {
        return ChainResult { links, fully_linked: false, root_subject: None, root_sha256: None, trusted_root: None, notes: vec!["no certificates".into()] };
    };
    let mut current = first;
    if certs.len() > 64 {
        notes.push("issuer search is limited to the first 64 certificates".into());
    }
    // Inspect the next identity at the boundary without adding or verifying a 17th link.
    for depth in 0..=16 {
        let Some(parts) = der::certificate_parts(current) else {
            notes.push("a certificate in the chain could not be parsed".into());
            break;
        };
        let sha = sha256_hex(current);
        if seen.contains(&sha) {
            notes.push("certificate chain loops; stopped".into());
            break;
        }
        if depth == 16 {
            notes.push("certificate chain walk limit reached; stopped".into());
            break;
        }
        seen.push(sha.clone());
        let info = der::certificate(current);
        if let Some(info) = &info {
            for note in &info.notes {
                if !notes.contains(note) {
                    notes.push(note.clone());
                }
            }
        }
        let subject = info.as_ref().map(|c| c.subject.clone()).unwrap_or_default();
        let issuer = info.as_ref().map(|c| c.issuer.clone()).unwrap_or_default();
        let mut issuer_cert = None;
        let mut signature_verified = Verified::unsupported("issuer certificate not present in the signature");
        if parts.subject == parts.issuer {
            signature_verified = verify_cert_by(current, current);
            if signature_verified.is_valid() {
                issuer_cert = Some(current);
            }
        }
        // Equal names do not establish which key signed the certificate. A self-issued
        // rollover can also need a different certificate with the same name.
        if issuer_cert.is_none() {
            for candidate in certs.iter().copied().take(64) {
                if candidate == current || !der::certificate_parts(candidate).is_some_and(|p| p.subject == parts.issuer) {
                    continue;
                }
                let verified = verify_cert_by(current, candidate);
                if verified.is_valid() {
                    issuer_cert = Some(candidate);
                    signature_verified = verified;
                    break;
                }
                if verified == Verified::Invalid || !matches!(signature_verified, Verified::Invalid) {
                    signature_verified = verified;
                }
            }
        }
        let self_signed = issuer_cert == Some(current);
        links.push(ChainLink { subject, issuer, sha256: sha, signature_verified, self_signed });
        if self_signed {
            fully_linked = true;
            break;
        }
        match issuer_cert {
            Some(ic) => {
                current = ic;
            }
            _ => break,
        }
    }
    let (root_subject, root_sha256, trusted_root) = match links.last() {
        Some(top) => {
            let trusted = top.self_signed.then(|| APPLE_ROOTS.iter().find(|(_, fp)| *fp == top.sha256).map(|(n, _)| *n)).flatten();
            if top.self_signed && trusted.is_none() {
                notes.push("chain not verified to a trusted root: the top certificate is self-signed but not a bundled Apple root".into());
            } else if !top.self_signed {
                notes.push("chain not verified to a trusted root: no self-signed root certificate was reached".into());
            }
            (Some(top.subject.clone()), Some(top.sha256.clone()), trusted)
        }
        None => (None, None, None),
    };
    ChainResult { links, fully_linked, root_subject, root_sha256, trusted_root, notes }
}

// ---------------------------------------------------------------------------------------------
// Mach-O CMS.

/// The result of verifying a Mach-O code-signature CMS blob against its CodeDirectory.
#[derive(Debug, Clone, Serialize)]
pub struct MachoCms {
    /// The signer's signature over the signed attributes verified cryptographically.
    pub signature_valid: Verified,
    /// The `messageDigest` signed attribute matched the hash of the CodeDirectory, so the
    /// signature covers this exact CodeDirectory.
    pub binds_code_directory: bool,
    /// Overall: `signature_valid` is Valid AND it binds the CodeDirectory.
    pub authenticated: bool,
    pub digest_algorithm: Option<&'static str>,
    pub signer_subject: Option<String>,
    pub signer_issuer: Option<String>,
    pub not_before: Option<String>,
    pub not_after: Option<String>,
    pub certificate_count: usize,
    pub chain: ChainResult,
    pub notes: Vec<String>,
}

fn verify_cms_signature(sd: &pkcs7::SignedData, spki: &[u8], message: &[u8]) -> Verified {
    if sd.signature_algorithm == RSA_PSS {
        let params = match sd.pss_parameters {
            Some(Ok(params)) => params,
            Some(Err(reason)) => return Verified::unsupported(reason),
            None => return Verified::unsupported("RSASSA-PSS signature parameters are missing"),
        };
        if sd.digest != Some(params.digest) {
            return Verified::Invalid;
        }
        return verify_pss_signature(spki, params, message, &sd.signature);
    }
    match sd.scheme() {
        Some(scheme) => verify_signature(spki, scheme, message, &sd.signature),
        None => Verified::unsupported(format!("signature algorithm {} is not checked", sd.signature_algorithm)),
    }
}

/// Verify a detached CMS `SignedData` (the Mach-O CMS blob) against the primary CodeDirectory.
pub fn verify_macho_cms(cms_der: &[u8], code_directory: &[u8]) -> Option<MachoCms> {
    let sd = pkcs7::parse_signed_data(cms_der)?;
    let mut notes = sd.signer_identity_notes.clone();
    if sd.non_minimal_length {
        notes.push(der::NON_MINIMAL_LENGTH_NOTE.into());
    }
    if let Some(reason) = sd.signed_attrs_error {
        notes.push(reason.into());
    }
    let signer = sd.signer_cert().map(<[u8]>::to_vec);
    let info = signer.as_deref().and_then(der::certificate);
    // Does the signed messageDigest attribute cover this CodeDirectory?
    let binds_code_directory = match (&sd.message_digest, sd.digest) {
        (Some(md), Some(d)) => hexs(&d.hash(code_directory)).eq_ignore_ascii_case(md),
        _ => {
            if sd.signed_attrs_error.is_none() {
                notes.push("no messageDigest signed attribute; cannot tie the signature to this CodeDirectory".into());
            }
            false
        }
    };
    // Verify the signer's signature over the (re-encoded) signed attributes.
    let signature_valid = match (sd.signed_attrs_reencoded.as_deref(), signer.as_deref()) {
        (Some(attrs), Some(cert)) => match der::certificate_spki(cert) {
            Some(spki) => verify_cms_signature(&sd, spki, attrs),
            None => Verified::unsupported("signer public key could not be read"),
        },
        (None, _) => Verified::unsupported("CMS carries no signed attributes; this detached form is not checked"),
        (_, None) => Verified::unsupported("no signer certificate"),
    };
    let certs: Vec<&[u8]> = ordered_certs(&sd);
    let chain = verify_chain(&certs);
    let authenticated = sd.signed_attrs_error.is_none() && signature_valid.is_valid() && binds_code_directory;
    Some(MachoCms {
        signature_valid,
        binds_code_directory,
        authenticated,
        digest_algorithm: sd.digest.map(Digest::name),
        signer_subject: info.as_ref().map(|c| c.subject.clone()),
        signer_issuer: info.as_ref().map(|c| c.issuer.clone()),
        not_before: info.as_ref().map(|c| c.not_before.clone()),
        not_after: info.as_ref().map(|c| c.not_after.clone()),
        certificate_count: sd.certificates.len(),
        chain,
        notes,
    })
}

/// Order a CMS certificate set so the signer's certificate is first (chain verification starts
/// from the leaf).
fn ordered_certs(sd: &pkcs7::SignedData) -> Vec<&[u8]> {
    let Some(signer) = sd.signer_cert() else { return Vec::new() };
    let mut out: Vec<&[u8]> = vec![signer];
    for c in &sd.certificates {
        if c.as_slice() != signer {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// APK signature scheme v2 / v3.

/// The result of verifying one APK v2/v3 signer.
#[derive(Debug, Clone, Serialize)]
pub struct ApkSigner {
    pub scheme: &'static str,
    /// A signature over the signed-data block verified cryptographically.
    pub signature_valid: Verified,
    /// The recomputed APK content digest matched the digest inside the signed data.
    pub digest_matches: bool,
    /// The signer's first certificate's public key matched the signing public key.
    pub public_key_matches_certificate: bool,
    /// Overall: required signer fields agree, signature valid, content digest matches and
    /// the signing key matches the certificate.
    pub verified: bool,
    pub algorithm: Option<String>,
    /// SHA-256 of the signer's first certificate: the fingerprint users compare.
    pub certificate_sha256: Option<String>,
    pub min_sdk: Option<u32>,
    pub max_sdk: Option<u32>,
    pub notes: Vec<String>,
}

/// Map an APK signature-algorithm id to `(scheme, content-digest, algorithm name)`.
fn apk_algorithm(id: u32) -> Option<(SigScheme, Digest, bool)> {
    // The bool is `true` for the "verity" (Merkle-tree) content digests we do not recompute.
    Some(match id {
        0x0101 => (SigScheme::RsaPss(Digest::Sha256), Digest::Sha256, false),
        0x0102 => (SigScheme::RsaPss(Digest::Sha512), Digest::Sha512, false),
        0x0103 => (SigScheme::RsaPkcs1v15(Digest::Sha256), Digest::Sha256, false),
        0x0104 => (SigScheme::RsaPkcs1v15(Digest::Sha512), Digest::Sha512, false),
        0x0201 => (SigScheme::Ecdsa(Digest::Sha256), Digest::Sha256, false),
        0x0202 => (SigScheme::Ecdsa(Digest::Sha512), Digest::Sha512, false),
        0x0421 => (SigScheme::RsaPkcs1v15(Digest::Sha256), Digest::Sha256, true),
        0x0423 => (SigScheme::Ecdsa(Digest::Sha256), Digest::Sha256, true),
        // 0x0301 (DSA) and 0x0425 (DSA verity) are not supported.
        _ => return None,
    })
}

/// Verify one APK v2/v3 signer against the full APK bytes.
pub fn verify_apk_signer(apk: &[u8], block_offset: u64, signer: &crate::apk_sign::Signer) -> ApkSigner {
    let mut notes = Vec::new();
    if let Some(reason) = signer.verification_error {
        notes.push(reason.into());
    }
    let certificate_sha256 = signer.certificates.first().map(|c| sha256_hex(c));
    let public_key_matches_certificate = match (signer.certificates.first(), der::spki_parts(&signer.public_key)) {
        (Some(c), Some((_, _))) => der::certificate_spki(c) == Some(signer.public_key.as_slice()),
        _ => false,
    };
    if !public_key_matches_certificate {
        notes.push("the signing public key does not match the first certificate".into());
    }
    // Pick the strongest supported signature algorithm present.
    let chosen = signer
        .signatures
        .iter()
        .filter_map(|(id, sig)| apk_algorithm(*id).map(|(scheme, digest, verity)| (*id, scheme, digest, verity, sig)))
        .max_by_key(|(_, _, digest, verity, _)| (!*verity, matches!(digest, Digest::Sha512)));
    let Some((id, scheme, digest, verity, sig)) = chosen else {
        notes.push("no supported signature algorithm present (DSA and unknown ids are not checked)".into());
        return ApkSigner {
            scheme: signer.scheme,
            signature_valid: Verified::unsupported("no supported signature algorithm"),
            digest_matches: false,
            public_key_matches_certificate,
            verified: false,
            algorithm: None,
            certificate_sha256,
            min_sdk: signer.min_sdk,
            max_sdk: signer.max_sdk,
            notes,
        };
    };
    let signature_valid = verify_signature(&signer.public_key, scheme, &signer.signed_data, sig);
    // Recompute the APK content digest for this algorithm and compare to the one in signed data.
    let digest_matches = if verity {
        notes.push("the verity (Merkle-tree) content digest is not recomputed; the signature over the signed data is still verified".into());
        false
    } else {
        match signer.signed_digests.iter().find(|(did, _)| *did == id) {
            Some((_, stored)) => match apk_content_digest(apk, block_offset, digest) {
                Some(computed) => computed == *stored,
                None => {
                    notes.push("the APK content digest could not be recomputed (unreadable ZIP layout)".into());
                    false
                }
            },
            None => {
                notes.push("the signed data carries no digest for the verified signature algorithm".into());
                false
            }
        }
    };
    let verified = signer.verification_error.is_none() && signature_valid.is_valid() && digest_matches && public_key_matches_certificate;
    ApkSigner {
        scheme: signer.scheme,
        signature_valid,
        digest_matches,
        public_key_matches_certificate,
        verified,
        algorithm: Some(crate::apk_sign::algorithm_name(id).to_string()),
        certificate_sha256,
        min_sdk: signer.min_sdk,
        max_sdk: signer.max_sdk,
        notes,
    }
}

/// The APK v2/v3 content digest: 1 MiB chunks of (ZIP entries | central directory | end-of-central
/// -directory with its CD offset patched to the signing block), each hashed with a `0xa5` prefix,
/// then a top-level hash with a `0x5a` prefix. The signing block itself is excluded.
pub fn apk_content_digest(apk: &[u8], block_offset: u64, d: Digest) -> Option<Vec<u8>> {
    let r = crate::bytes::Reader::new(apk);
    // Locate the end-of-central-directory record and the central directory.
    let from = apk.len().saturating_sub(22 + 0xffff);
    let mut eocd = None;
    let mut i = apk.len().checked_sub(22)?;
    while i >= from {
        if apk.get(i..i + 4) == Some(b"PK\x05\x06") {
            eocd = Some(i);
            break;
        }
        i = i.checked_sub(1)?;
    }
    let eocd = eocd?;
    let cd_off = r.u32_le(eocd + 16)? as usize;
    let bo = usize::try_from(block_offset).ok()?;
    if bo > apk.len() || cd_off > apk.len() || eocd > apk.len() || cd_off > eocd || bo > cd_off {
        return None;
    }
    let section1 = apk.get(0..bo)?;
    let section3 = apk.get(cd_off..eocd)?;
    // Section 4: the EOCD with its central-directory offset (bytes 16..20) set to the signing block.
    let mut section4 = apk.get(eocd..)?.to_vec();
    if section4.len() >= 20 {
        section4.get_mut(16..20)?.copy_from_slice(&(bo as u32).to_le_bytes());
    }
    const CHUNK: usize = 1024 * 1024;
    let mut chunk_count = 0u32;
    let mut all_chunk_digests: Vec<u8> = Vec::new();
    for section in [section1, section3, &section4] {
        for chunk in section.chunks(CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            chunk_count = chunk_count.checked_add(1)?;
            let mut buf = Vec::with_capacity(5 + chunk.len());
            buf.push(0xa5);
            buf.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
            buf.extend_from_slice(chunk);
            all_chunk_digests.extend_from_slice(&d.hash(&buf));
        }
    }
    let mut top = Vec::with_capacity(5 + all_chunk_digests.len());
    top.push(0x5a);
    top.extend_from_slice(&chunk_count.to_le_bytes());
    top.extend_from_slice(&all_chunk_digests);
    Some(d.hash(&top))
}

// ---------------------------------------------------------------------------------------------
// APK v1 (JAR) signatures.

/// The result of verifying one JAR (v1) signer's PKCS#7 over its signature file.
#[derive(Debug, Clone, Serialize)]
pub struct Pkcs7Signature {
    pub signature_valid: Verified,
    pub signer_subject: Option<String>,
    pub certificate_sha256: Option<String>,
    pub chain: ChainResult,
    pub notes: Vec<String>,
}

/// Verify a detached PKCS#7/CMS signature over `content` (for JAR signing, the `.SF` file bytes).
pub fn verify_pkcs7_detached(content: &[u8], pkcs7_der: &[u8]) -> Option<Pkcs7Signature> {
    let sd = pkcs7::parse_signed_data(pkcs7_der)?;
    let mut notes = sd.signer_identity_notes.clone();
    if sd.non_minimal_length {
        notes.push(der::NON_MINIMAL_LENGTH_NOTE.into());
    }
    if let Some(reason) = sd.signed_attrs_error {
        notes.push(reason.into());
    }
    let signer = sd.signer_cert().map(<[u8]>::to_vec);
    let cert_sha = signer.as_deref().map(sha256_hex);
    let subject = signer.as_deref().and_then(der::certificate).map(|c| c.subject);
    let signature_valid = match (sd.signed_attrs_error, signer.as_deref()) {
        (Some(_), _) => Verified::Invalid,
        (None, Some(cert)) => {
            let Some(spki) = der::certificate_spki(cert) else {
                return Some(Pkcs7Signature {
                    signature_valid: Verified::unsupported("signer key unreadable"),
                    signer_subject: subject,
                    certificate_sha256: cert_sha,
                    chain: verify_chain(&ordered_certs(&sd)),
                    notes,
                });
            };
            match sd.signed_attrs_reencoded.as_deref() {
                // Signed attributes present: the signature covers them; messageDigest must equal H(content).
                Some(attrs) => {
                    let binds = match (&sd.message_digest, sd.digest) {
                        (Some(md), Some(dd)) => hexs(&dd.hash(content)).eq_ignore_ascii_case(md),
                        _ => false,
                    };
                    if !binds {
                        notes.push("the signed messageDigest does not match the signature file".into());
                    }
                    let v = verify_cms_signature(&sd, spki, attrs);
                    if v.is_valid() && binds {
                        Verified::Valid
                    } else if v.is_valid() {
                        Verified::Invalid
                    } else {
                        v
                    }
                }
                // Classic JAR signing: the signature is over the raw .SF content.
                None => verify_cms_signature(&sd, spki, content),
            }
        }
        (None, None) => Verified::unsupported("no signer certificate"),
    };
    Some(Pkcs7Signature { signature_valid, signer_subject: subject, certificate_sha256: cert_sha, chain: verify_chain(&ordered_certs(&sd)), notes })
}

/// A parsed entry of a JAR manifest/signature file: its name and digest attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JarSection {
    pub name: Option<String>,
    /// Attribute key → value, with any continuation lines joined.
    pub attrs: Vec<(String, String)>,
}

/// Parse a JAR `MANIFEST.MF` or `.SF` file into its main section and per-entry sections.
/// Unfolds physical byte lines before decoding UTF-8 and accepts CR, LF or CRLF terminators.
pub fn parse_jar_sections(data: &[u8]) -> Vec<JarSection> {
    let mut logical: Vec<Vec<u8>> = Vec::new();
    let mut remaining = data;
    while !remaining.is_empty() {
        let end = remaining.iter().position(|byte| matches!(byte, b'\r' | b'\n')).unwrap_or(remaining.len());
        let line = remaining.get(..end).unwrap_or_default();
        let tail = remaining.get(end..).unwrap_or_default();
        remaining = tail.strip_prefix(b"\r\n").or_else(|| tail.get(1..)).unwrap_or_default();
        if let Some(rest) = line.strip_prefix(b" ") {
            if let Some(last) = logical.last_mut().filter(|line| !line.is_empty()) {
                last.extend_from_slice(rest);
            }
            continue;
        }
        logical.push(line.to_vec());
    }
    let mut sections = Vec::new();
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut name = None;
    let flush = |attrs: &mut Vec<(String, String)>, name: &mut Option<String>, sections: &mut Vec<JarSection>| {
        if !attrs.is_empty() || name.is_some() {
            sections.push(JarSection { name: name.take(), attrs: std::mem::take(attrs) });
        }
    };
    for line in logical {
        if line.is_empty() {
            flush(&mut attrs, &mut name, &mut sections);
            continue;
        }
        let line = String::from_utf8_lossy(&line);
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_string();
            let v = v.strip_prefix(' ').unwrap_or(v).to_string();
            if k.eq_ignore_ascii_case("Name") {
                name = Some(v);
            } else {
                attrs.push((k, v));
            }
        }
    }
    flush(&mut attrs, &mut name, &mut sections);
    sections
}

fn attr<'a>(section: &'a JarSection, key: &str) -> Option<&'a str> {
    section.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
}

/// Standard base64 (RFC 4648, padded), the encoding JAR digests use.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

const JAR_DIGESTS: [(Digest, &str); 3] = [(Digest::Sha256, "SHA-256-Digest"), (Digest::Sha512, "SHA-512-Digest"), (Digest::Sha1, "SHA1-Digest")];

/// The digest (and its algorithm) a JAR section records, in supported preference order.
fn section_digest(section: &JarSection, suffix: &str) -> Option<(Digest, String)> {
    for (d, key) in JAR_DIGESTS {
        if let Some(v) = attr(section, &format!("{key}{suffix}")) {
            return Some((d, v.to_string()));
        }
    }
    None
}

/// The outcome of checking a JAR (v1) MANIFEST.MF against the files it lists.
#[derive(Debug, Clone, Serialize)]
pub struct JarIntegrity {
    /// The `.SF` file's recorded digest of the whole manifest matched.
    pub manifest_digest_matches: Option<bool>,
    pub files_listed: usize,
    pub files_matched: usize,
    pub files_mismatched: usize,
    pub files_unchecked: usize,
    pub mismatched_names: Vec<String>,
    pub notes: Vec<String>,
}

/// Check a JAR signature file's manifest digest and every per-file digest in MANIFEST.MF.
/// `read_entry(name)` returns the uncompressed bytes of a named ZIP entry (or `None` if absent).
pub fn verify_jar_integrity(manifest_mf: &[u8], sf: &[u8], mut read_entry: impl FnMut(&str) -> Option<Vec<u8>>) -> JarIntegrity {
    let mut notes = Vec::new();
    let sf_sections = parse_jar_sections(sf);
    let manifest_digest_matches = sf_sections.first().and_then(|main| section_digest(main, "-Manifest").map(|(d, want)| base64(&d.hash(manifest_mf)) == want));
    if manifest_digest_matches.is_none() {
        notes.push("the signature file records no whole-manifest digest; per-file digests are still checked".into());
    }
    let sections = parse_jar_sections(manifest_mf);
    // Repeated sections describe one entry. Only attributes present in a later section replace
    // earlier values; preserve the other algorithms before selecting a preferred digest.
    let mut entries: std::collections::BTreeMap<&str, [Option<&str>; 3]> = std::collections::BTreeMap::new();
    for section in &sections {
        let Some(name) = section.name.as_deref() else { continue };
        let digests = entries.entry(name).or_insert([None; 3]);
        for (slot, (_, key)) in digests.iter_mut().zip(JAR_DIGESTS) {
            if let Some(value) = attr(section, key) {
                *slot = Some(value);
            }
        }
    }
    let mut files_listed = 0;
    let mut files_matched = 0;
    let mut files_mismatched = 0;
    let mut files_unchecked = 0;
    let mut mismatched_names = Vec::new();
    for (name, digests) in entries {
        files_listed += 1;
        let Some((d, want)) = digests.into_iter().zip(JAR_DIGESTS).find_map(|(want, (digest, _))| want.map(|want| (digest, want))) else {
            files_unchecked += 1;
            continue;
        };
        match read_entry(name) {
            Some(bytes) => {
                if base64(&d.hash(&bytes)) == want {
                    files_matched += 1;
                } else {
                    files_mismatched += 1;
                    if mismatched_names.len() < 32 {
                        mismatched_names.push(name.to_string());
                    }
                }
            }
            None => files_unchecked += 1,
        }
    }
    JarIntegrity { manifest_digest_matches, files_listed, files_matched, files_mismatched, files_unchecked, mismatched_names, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn der_fields(data: &[u8]) -> Vec<Vec<u8>> {
        der::children(data, &der::read(data, 0).unwrap()).iter().map(|field| field.whole(data).unwrap().to_vec()).collect()
    }

    fn cert_with_key_id(name: &str, key_id: &[u8], value_tag: u8, duplicate: bool, critical: Option<&[u8]>) -> Vec<u8> {
        use craft_anything_testkit::{android as a, crypto};
        let original = crypto::self_signed_rsa(name, "Org");
        let mut cert = der_fields(&original);
        let mut tbs = der_fields(&cert[0]);
        let mut fields = vec![a::der_oid("2.5.29.14")];
        if let Some(critical) = critical {
            fields.push(a::der(0x01, critical));
        }
        fields.push(a::der(0x04, &a::der(value_tag, key_id)));
        let extension = a::der(0x30, &fields.concat());
        let extensions = if duplicate { [extension.clone(), extension].concat() } else { extension };
        tbs.push(a::der(0xa3, &a::der(0x30, &extensions)));
        cert[0] = a::der(0x30, &tbs.concat());
        cert[2] = a::der(0x03, &[vec![0], crypto::rsa_sign_sha256(&cert[0])].concat());
        a::der(0x30, &cert.concat())
    }

    fn issuer_serial(cert: &[u8]) -> Vec<u8> {
        use craft_anything_testkit::android as a;
        let parts = der::certificate_parts(cert).unwrap();
        a::der(0x30, &[parts.issuer.to_vec(), a::der(0x02, parts.serial)].concat())
    }

    fn cms_with_identity(content: &[u8], certs: &[Vec<u8>], sid: Vec<u8>, version: u8) -> Vec<u8> {
        use craft_anything_testkit::{android as a, crypto};
        let original = crypto::cms_detached_rsa(content, "Signer", "Org");
        let mut ci = der_fields(&original);
        let mut sd = der_fields(&der_fields(&ci[1])[0]);
        let mut signer = der_fields(&der_fields(&sd[4])[0]);
        signer[0] = a::der(0x02, &[version]);
        signer[1] = sid;
        sd[0] = a::der(0x02, &[version]);
        sd[3] = a::der(0xa0, &certs.concat());
        sd[4] = a::der(0x31, &a::der(0x30, &signer.concat()));
        ci[1] = a::der(0xa0, &a::der(0x30, &sd.concat()));
        a::der(0x30, &ci.concat())
    }

    fn assert_unknown_cms_signer(cms: &[u8], content: &[u8]) {
        let parsed = pkcs7::parse_signed_data(cms).unwrap();
        assert!(parsed.signer_cert().is_none());
        assert!(ordered_certs(&parsed).is_empty());
        let macho = verify_macho_cms(cms, content).unwrap();
        assert!(!macho.authenticated);
        assert!(macho.signer_subject.is_none());
        assert!(macho.chain.links.is_empty());
        let jar = verify_pkcs7_detached(content, cms).unwrap();
        assert!(!jar.signature_valid.is_valid());
        assert!(jar.signer_subject.is_none());
        assert!(jar.certificate_sha256.is_none());
        assert!(jar.chain.links.is_empty());
    }

    #[test]
    fn cms_unmatched_issuer_or_serial_keeps_the_signer_unknown() {
        use craft_anything_testkit::{android as a, crypto};
        let content = b"signed identity fixture";
        let cert = crypto::self_signed_rsa("Signer", "Org");
        let fields = der_fields(&issuer_serial(&cert));
        for sid in [a::der(0x30, &[fields[0].clone(), a::der(0x02, &[9])].concat()), issuer_serial(&crypto::self_signed_rsa("Other", "Org"))] {
            let cms = cms_with_identity(content, std::slice::from_ref(&cert), sid, 1);
            assert_unknown_cms_signer(&cms, content);
        }
    }

    #[test]
    fn cms_key_identifier_selects_the_matching_certificate() {
        use craft_anything_testkit::android as a;
        let content = b"signed key identifier fixture";
        let key_id = [7; 20];
        let signer = cert_with_key_id("Signer", &key_id, 0x04, false, None);
        let unrelated = cert_with_key_id("Unrelated", &[8; 20], 0x04, false, None);
        let cms = cms_with_identity(content, &[unrelated, signer.clone()], a::der(0x80, &key_id), 3);
        let parsed = pkcs7::parse_signed_data(&cms).unwrap();
        assert_eq!(parsed.signer_cert(), Some(signer.as_slice()));
        let result = verify_macho_cms(&cms, content).unwrap();
        assert!(result.authenticated);
        assert_eq!(result.signer_subject.as_deref(), Some("CN=Signer, O=Org"));
        assert!(result.chain.fully_linked);
        assert!(verify_pkcs7_detached(content, &cms).unwrap().signature_valid.is_valid());
    }

    #[test]
    fn cms_missing_key_identifier_does_not_fall_back_to_a_certificate() {
        use craft_anything_testkit::{android as a, crypto};
        let content = b"missing key identifier fixture";
        for cert in [crypto::self_signed_rsa("Signer", "Org"), cert_with_key_id("Signer", &[8; 20], 0x04, false, None)] {
            let cms = cms_with_identity(content, &[cert], a::der(0x80, &[7; 20]), 3);
            assert_unknown_cms_signer(&cms, content);
        }
    }

    #[test]
    fn cms_key_identifier_requires_one_well_formed_extension() {
        use craft_anything_testkit::android as a;
        let content = b"malformed key identifier extension fixture";
        for (tag, duplicate) in [(0x02, false), (0x04, true)] {
            let cert = cert_with_key_id("Signer", &[7; 20], tag, duplicate, None);
            let cms = cms_with_identity(content, &[cert], a::der(0x80, &[7; 20]), 3);
            assert_unknown_cms_signer(&cms, content);
        }
    }

    #[test]
    fn cms_issuer_serial_finds_a_later_certificate() {
        use craft_anything_testkit::crypto;
        let content = b"later certificate fixture";
        let cert = crypto::self_signed_rsa("Signer", "Org");
        let cms = cms_with_identity(content, &[crypto::self_signed_rsa("Other", "Org"), cert.clone()], issuer_serial(&cert), 1);
        assert_eq!(pkcs7::parse_signed_data(&cms).unwrap().signer_cert(), Some(cert.as_slice()));
        assert!(verify_macho_cms(&cms, content).unwrap().authenticated);
    }

    #[test]
    fn cms_key_identifier_checks_critical_flag_encoding() {
        use craft_anything_testkit::android as a;
        let content = b"key identifier critical flag fixture";
        for flag in [&[0xff][..], &[0], &[], &[1], &[0xff, 0]] {
            let cert = cert_with_key_id("Signer", &[7; 20], 0x04, false, Some(flag));
            let cms = cms_with_identity(content, std::slice::from_ref(&cert), a::der(0x80, &[7; 20]), 3);
            if matches!(flag, [0] | [0xff]) {
                assert_eq!(pkcs7::parse_signed_data(&cms).unwrap().signer_cert(), Some(cert.as_slice()));
                assert!(verify_macho_cms(&cms, content).unwrap().authenticated);
            } else {
                assert_unknown_cms_signer(&cms, content);
            }
        }
    }

    #[test]
    fn cms_signer_identifier_rejects_unknown_structure() {
        use craft_anything_testkit::{android as a, crypto};
        let cert = crypto::self_signed_rsa("Signer", "Org");
        let fields = der_fields(&issuer_serial(&cert));
        for (sid, version) in [(a::der(0x31, &fields.concat()), 1), (a::der(0x30, &[fields[0].clone(), a::der(0x04, &[1, 2, 3, 4, 5])].concat()), 1)] {
            let cms = cms_with_identity(b"identifier structure", std::slice::from_ref(&cert), sid, version);
            assert!(pkcs7::parse_signed_data(&cms).is_none());
        }
    }

    /// A wrong SignerInfo version is a readable fact, not a reason to discard its identity.
    #[test]
    fn cms_identifier_version_mismatches_preserve_the_signature_and_report_a_note() {
        use craft_anything_testkit::android as a;
        let content = b"readable version mismatch";
        let key_id = [7; 20];
        let cert = cert_with_key_id("Signer", &key_id, 0x04, false, None);
        for (sid, version) in [(issuer_serial(&cert), 3), (issuer_serial(&cert), 0), (a::der(0x80, &key_id), 1), (a::der(0x80, &key_id), 9)] {
            let cms = cms_with_identity(content, std::slice::from_ref(&cert), sid, version);
            let parsed = pkcs7::parse_signed_data(&cms).expect("readable SignedData");
            assert_eq!(parsed.certificates.as_slice(), std::slice::from_ref(&cert));
            assert_eq!(parsed.digest, Some(Digest::Sha256));
            assert!(!parsed.signature_algorithm.is_empty());
            assert_eq!(parsed.signer_cert(), Some(cert.as_slice()));
            let macho = verify_macho_cms(&cms, content).unwrap();
            assert!(macho.authenticated);
            assert_eq!(macho.certificate_count, 1);
            assert!(macho.notes.iter().any(|note| note.contains("version")));
            let jar = verify_pkcs7_detached(content, &cms).unwrap();
            assert!(jar.signature_valid.is_valid());
            assert!(jar.notes.iter().any(|note| note.contains("version")));
        }
    }

    /// An empty serial cannot identify a signer, but its certificates and algorithms remain useful.
    #[test]
    fn cms_empty_signer_serial_preserves_metadata_and_leaves_the_signer_unknown() {
        use craft_anything_testkit::{android as a, crypto};
        let content = b"readable empty serial";
        let cert = crypto::self_signed_rsa("Signer", "Org");
        let fields = der_fields(&issuer_serial(&cert));
        for version in [1, 3] {
            let sid = a::der(0x30, &[fields[0].clone(), a::der(0x02, &[])].concat());
            let cms = cms_with_identity(content, std::slice::from_ref(&cert), sid, version);
            let parsed = pkcs7::parse_signed_data(&cms).expect("readable SignedData");
            assert_eq!(parsed.certificates.as_slice(), std::slice::from_ref(&cert));
            assert_eq!(parsed.digest, Some(Digest::Sha256));
            assert!(!parsed.signature_algorithm.is_empty());
            assert_unknown_cms_signer(&cms, content);
            let macho = verify_macho_cms(&cms, content).unwrap();
            assert_eq!(macho.certificate_count, 1);
            assert!(macho.notes.iter().any(|note| note.contains("serial") && note.contains("empty")));
            let jar = verify_pkcs7_detached(content, &cms).unwrap();
            assert!(jar.notes.iter().any(|note| note.contains("serial") && note.contains("empty")));
        }
    }

    #[test]
    fn base64_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn scheme_oids() {
        assert_eq!(scheme_from_oid("1.2.840.113549.1.1.11"), Some(SigScheme::RsaPkcs1v15(Digest::Sha256)));
        assert_eq!(scheme_from_oid("1.2.840.10045.4.3.2"), Some(SigScheme::Ecdsa(Digest::Sha256)));
        assert_eq!(scheme_from_oid("1.2.840.113549.1.1.4"), None);
    }

    #[test]
    fn jar_section_folding() {
        let mf = b"Manifest-Version: 1.0\r\nCreated-By: test\r\n\r\nName: a/b/really-long-\r\n name.txt\r\nSHA-256-Digest: AAA=\r\n\r\n";
        let s = parse_jar_sections(mf);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].name.as_deref(), Some("a/b/really-long-name.txt"));
        assert_eq!(attr(&s[1], "SHA-256-Digest"), Some("AAA="));
    }

    #[test]
    fn jar_unfolds_utf8_bytes_before_decoding_a_filename() {
        let name = format!("{}é.txt", "a".repeat(65));
        let header = format!("Name: {name}");
        assert_eq!(header.as_bytes()[71], 0xc3);
        let content = b"the named entry";
        let mut manifest = b"Manifest-Version: 1.0\r\n\r\n".to_vec();
        manifest.extend_from_slice(&header.as_bytes()[..72]);
        manifest.extend_from_slice(b"\r\n ");
        manifest.extend_from_slice(&header.as_bytes()[72..]);
        manifest.extend_from_slice(format!("\r\nSHA-256-Digest: {}\r\n\r\n", base64(&Digest::Sha256.hash(content))).as_bytes());
        let sections = parse_jar_sections(&manifest);
        assert_eq!(sections[1].name.as_deref(), Some(name.as_str()));
        let checked = verify_jar_integrity(&manifest, b"", |requested| (requested == name).then(|| content.to_vec()));
        assert_eq!((checked.files_listed, checked.files_matched, checked.files_mismatched, checked.files_unchecked), (1, 1, 0, 0));
    }

    #[test]
    fn jar_values_preserve_spaces_beyond_the_separator() {
        let name = " leading and trailing.txt  ";
        let content = b"entry with a whitespace-sensitive name";
        let manifest = format!("Manifest-Version: 1.0\nCreated-By:  original  \n\nName: {name}\nSHA-256-Digest: {}\n\n", base64(&Digest::Sha256.hash(content)));
        let sections = parse_jar_sections(manifest.as_bytes());
        assert_eq!(attr(&sections[0], "Created-By"), Some(" original  "));
        assert_eq!(sections[1].name.as_deref(), Some(name));
        let checked = verify_jar_integrity(manifest.as_bytes(), b"", |requested| (requested == name).then(|| content.to_vec()));
        assert_eq!((checked.files_matched, checked.files_mismatched, checked.files_unchecked), (1, 0, 0));
    }

    #[test]
    fn jar_sections_accept_cr_lf_and_crlf_newlines() {
        for newline in ["\r", "\n", "\r\n"] {
            let manifest =
                ["Manifest-Version: 1.0", "", "Name: file", " .txt", "SHA-256-Digest: AAA=", "", "Name: other.txt", "SHA1-Digest: BBB=", ""].join(newline);
            let sections = parse_jar_sections(manifest.as_bytes());
            assert_eq!(sections.len(), 3, "newline {newline:?}");
            assert_eq!(sections[1].name.as_deref(), Some("file.txt"));
            assert_eq!(attr(&sections[1], "SHA-256-Digest"), Some("AAA="));
            assert_eq!(sections[2].name.as_deref(), Some("other.txt"));
        }
        let mixed = parse_jar_sections(b"Manifest-Version: 1.0\r\n\rName: file\n .txt\rSHA-256-Digest: AAA=\n\r\n");
        assert_eq!(mixed.len(), 2);
        assert_eq!(mixed[1].name.as_deref(), Some("file.txt"));
    }

    #[test]
    fn jar_orphan_continuations_do_not_join_sections() {
        let sections = parse_jar_sections(b" orphan at start\nManifest-Version: 1.0\n\n Created-By: orphan\nName: file.txt\nSHA1-Digest: AAA=\n");
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, None);
        assert_eq!(attr(&sections[0], "Created-By"), None);
        assert_eq!(sections[1].name.as_deref(), Some("file.txt"));
    }

    #[test]
    fn unsupported_is_never_valid_on_garbage() {
        // Random bytes must never verify as Valid.
        for seed in 0..64 {
            let n = craft_anything_testkit::noise(seed, 200);
            let v = verify_signature(&n, SigScheme::RsaPkcs1v15(Digest::Sha256), &n, &n);
            assert!(!v.is_valid());
            let _ = verify_macho_cms(&n, &n);
            let _ = parse_jar_sections(&n);
        }
    }

    #[test]
    fn rsa_signature_round_trip_and_tamper() {
        use craft_anything_testkit::crypto;
        let (spki, sign) = crypto::rsa_signer();
        let msg = b"the exact bytes that were signed";
        let sig = sign(msg, 0);
        assert!(verify_signature(&spki, SigScheme::RsaPkcs1v15(Digest::Sha256), msg, &sig).is_valid());
        // A flipped message byte must not verify.
        let mut bad = msg.to_vec();
        bad[0] ^= 0xff;
        assert_eq!(verify_signature(&spki, SigScheme::RsaPkcs1v15(Digest::Sha256), &bad, &sig), Verified::Invalid);
        // A flipped signature byte must not verify.
        let mut bad_sig = sig.clone();
        bad_sig[0] ^= 0xff;
        assert_eq!(verify_signature(&spki, SigScheme::RsaPkcs1v15(Digest::Sha256), msg, &bad_sig), Verified::Invalid);
    }

    #[test]
    fn ecdsa_p256_round_trip() {
        use craft_anything_testkit::crypto;
        let (spki, sign) = crypto::p256_signer();
        let msg = b"ecdsa message";
        let sig = sign(msg);
        assert!(verify_signature(&spki, SigScheme::Ecdsa(Digest::Sha256), msg, &sig).is_valid());
        let mut bad = msg.to_vec();
        bad[0] ^= 1;
        assert_eq!(verify_signature(&spki, SigScheme::Ecdsa(Digest::Sha256), &bad, &sig), Verified::Invalid);
    }

    fn ec_spki_with_parameters(parameters: &[u8], point: &[u8]) -> Vec<u8> {
        use craft_anything_testkit::android::{der, der_oid};
        let algorithm = der(0x30, &[der_oid(EC_PUBLIC_KEY), parameters.to_vec()].concat());
        let key = der(0x03, &[&[0], point].concat());
        der(0x30, &[algorithm, key].concat())
    }

    #[test]
    fn ecdsa_never_guesses_a_curve_from_point_length() {
        use craft_anything_testkit::{android::der_oid, crypto};
        let original = crypto::p256_spki();
        let (_, point) = der::spki_parts(&original).unwrap();
        let message = b"the declared curve must select the verification group";
        let signature = crypto::p256_sign_sha256(message);
        assert!(verify_signature(&original, SigScheme::Ecdsa(Digest::Sha256), message, &signature).is_valid());
        for curve in ["1.3.132.0.10", "1.3.132.0.34", "1.3.132.0.35", "1.2.3.4"] {
            let spki = ec_spki_with_parameters(&der_oid(curve), point);
            let result = verify_signature(&spki, SigScheme::Ecdsa(Digest::Sha256), message, &signature);
            assert!(!result.is_valid(), "curve {curve}: {result:?}");
        }
    }

    #[test]
    fn ecdsa_requires_one_complete_named_curve_parameter() {
        use craft_anything_testkit::{
            android::{der, der_oid},
            crypto,
        };
        let original = crypto::p256_spki();
        let (_, point) = crate::der::spki_parts(&original).unwrap();
        let message = b"parameters are part of the public key";
        let signature = crypto::p256_sign_sha256(message);
        let curve = der_oid("1.2.840.10045.3.1.7");
        let parameters = [
            vec![],
            der(0x05, &[]),
            der(0x30, &curve),
            der(0x06, &[0x80, 0x2a]),
            [curve.clone(), curve.clone()].concat(),
            [curve.clone(), der(0x05, &[])].concat(),
            [curve, vec![0x06]].concat(),
        ];
        for parameter in parameters {
            let spki = ec_spki_with_parameters(&parameter, point);
            let result = verify_signature(&spki, SigScheme::Ecdsa(Digest::Sha256), message, &signature);
            assert!(!result.is_valid(), "parameter {parameter:?}: {result:?}");
        }
    }

    #[test]
    fn declared_curves_accept_compressed_and_uncompressed_points() {
        use craft_anything_testkit::{android::der_oid, crypto};
        use p384::{
            ecdsa::{Signature, SigningKey, signature::Signer},
            elliptic_curve::rand_core::OsRng,
        };
        let message = b"both supported named curves retain both SEC1 encodings";
        let original = crypto::p256_spki();
        let (_, point) = der::spki_parts(&original).unwrap();
        let p256_key = p256::ecdsa::VerifyingKey::from_sec1_bytes(point).unwrap();
        let p256_signature = crypto::p256_sign_sha256(message);
        let p384_key = SigningKey::random(&mut OsRng);
        let p384_signature: Signature = p384_key.sign(message);
        for compressed in [false, true] {
            let spki = ec_spki_with_parameters(&der_oid("1.2.840.10045.3.1.7"), p256_key.to_encoded_point(compressed).as_bytes());
            assert!(verify_signature(&spki, SigScheme::Ecdsa(Digest::Sha256), message, &p256_signature).is_valid());
            let spki = ec_spki_with_parameters(&der_oid("1.3.132.0.34"), p384_key.verifying_key().to_encoded_point(compressed).as_bytes());
            assert!(verify_signature(&spki, SigScheme::Ecdsa(Digest::Sha384), message, p384_signature.to_der().as_bytes()).is_valid());
            assert_eq!(verify_signature(&spki, SigScheme::Ecdsa(Digest::Sha384), b"changed", p384_signature.to_der().as_bytes()), Verified::Invalid);
        }
    }

    #[test]
    fn chain_links_and_self_signed() {
        use craft_anything_testkit::crypto;
        // Leaf signed by a self-signed CA: the leaf link verifies against the CA.
        let (leaf, ca) = crypto::cert_chain();
        let chain = verify_chain(&[&leaf, &ca]);
        assert!(chain.links[0].signature_verified.is_valid(), "{:?}", chain.links[0]);
        assert!(chain.links.last().unwrap().self_signed);
        // The top is self-signed but not a bundled Apple root, so it is not trusted.
        assert!(chain.trusted_root.is_none());
        assert!(chain.notes.iter().any(|n| n.contains("not verified to a trusted root")));
        // A single self-signed certificate verifies against itself.
        let ss = crypto::self_signed_rsa("Solo", "Org");
        assert!(verify_chain(&[&ss]).links[0].signature_verified.is_valid());
    }

    #[test]
    fn chain_tries_later_issuers_with_the_same_name() {
        use craft_anything_testkit::{
            android::{der, der_oid},
            crypto,
        };
        use p256::{
            ecdsa::{Signature, SigningKey, signature::Signer},
            elliptic_curve::rand_core::OsRng,
        };
        let decoy_key = SigningKey::random(&mut OsRng);
        let point = decoy_key.verifying_key().to_encoded_point(false);
        let algorithm = der(0x30, &[der_oid(EC_PUBLIC_KEY), der_oid("1.2.840.10045.3.1.7")].concat());
        let spki = der(0x30, &[algorithm, der(0x03, &[&[0], point.as_bytes()].concat())].concat());
        let decoy = crypto::make_cert("Shared CA", "Fixture Org", "Shared CA", &spki, "1.2.840.10045.4.3.2", &|message| {
            let signature: Signature = decoy_key.sign(message);
            signature.to_der().as_bytes().to_vec()
        });
        let issuer = crypto::make_cert("Shared CA", "Fixture Org", "Shared CA", &crypto::p256_spki(), "1.2.840.10045.4.3.2", &crypto::p256_sign_sha256);
        let leaf = crypto::make_cert("Leaf", "Fixture Org", "Shared CA", &crypto::rsa_spki(), "1.2.840.10045.4.3.2", &crypto::p256_sign_sha256);
        assert!(verify_cert_by(&decoy, &decoy).is_valid());
        assert_eq!(verify_cert_by(&leaf, &decoy), Verified::Invalid);
        assert!(verify_cert_by(&leaf, &issuer).is_valid());
        for certs in [[leaf.as_slice(), decoy.as_slice(), issuer.as_slice()], [leaf.as_slice(), issuer.as_slice(), decoy.as_slice()]] {
            let chain = verify_chain(&certs);
            assert!(chain.fully_linked, "{chain:?}");
            assert_eq!(chain.links.len(), 2);
            assert_eq!(chain.root_sha256.as_deref(), Some(sha256_hex(&issuer).as_str()));
            assert!(chain.links.iter().all(|link| link.signature_verified.is_valid()));
            assert_eq!(chain.trusted_root, None);
        }
        let absent = verify_chain(&[&leaf, &decoy]);
        assert!(!absent.fully_linked);
        assert_eq!(absent.links.len(), 1);
        assert_eq!(absent.links[0].signature_verified, Verified::Invalid);
        let mut bounded = vec![leaf.as_slice()];
        bounded.extend(std::iter::repeat_n(decoy.as_slice(), 62));
        bounded.push(issuer.as_slice());
        assert!(verify_chain(&bounded).fully_linked);
        bounded.insert(1, decoy.as_slice());
        let omitted = verify_chain(&bounded);
        assert!(!omitted.fully_linked);
        assert!(omitted.notes.iter().any(|note| note.contains("first 64 certificates")));
    }

    #[test]
    fn self_issued_rollover_follows_the_actual_issuer_key() {
        use craft_anything_testkit::crypto;
        let root = crypto::self_signed_rsa("Rollover CA", "Fixture Org");
        let rollover = crypto::make_cert("Rollover CA", "Fixture Org", "Rollover CA", &crypto::p256_spki(), "1.2.840.113549.1.1.11", &crypto::rsa_sign_sha256);
        assert!(!verify_cert_by(&rollover, &rollover).is_valid());
        assert!(verify_cert_by(&rollover, &root).is_valid());
        let chain = verify_chain(&[&rollover, &root]);
        assert!(chain.fully_linked, "{chain:?}");
        assert_eq!(chain.links.len(), 2);
        assert!(!chain.links[0].self_signed);
        assert!(chain.links[1].self_signed);
        assert_eq!(chain.root_sha256.as_deref(), Some(sha256_hex(&root).as_str()));
        let missing = verify_chain(&[&rollover]);
        assert!(!missing.fully_linked);
        assert!(!missing.links[0].self_signed);
    }

    #[test]
    fn verified_links_without_a_root_do_not_complete_a_chain() {
        use craft_anything_testkit::crypto;
        let a = crypto::make_cert("Cycle A", "Fixture Org", "Cycle B", &crypto::rsa_spki(), "1.2.840.10045.4.3.2", &crypto::p256_sign_sha256);
        let b = crypto::make_cert("Cycle B", "Fixture Org", "Cycle A", &crypto::p256_spki(), "1.2.840.113549.1.1.11", &crypto::rsa_sign_sha256);
        let cycle = verify_chain(&[&a, &b]);
        assert!(cycle.links.iter().all(|link| link.signature_verified.is_valid()));
        assert!(!cycle.fully_linked, "{cycle:?}");
        assert!(cycle.trusted_root.is_none());
        assert!(cycle.notes.iter().any(|note| note.contains("chain loops")), "{cycle:?}");
        assert!(!cycle.notes.iter().any(|note| note.contains("walk limit")), "{cycle:?}");

        let mut certificates = Vec::new();
        for i in 0..18 {
            certificates.push(crypto::make_cert(
                &format!("Depth {i}"),
                "Fixture Org",
                &format!("Depth {}", (i + 1).min(17)),
                &crypto::rsa_spki(),
                "1.2.840.113549.1.1.11",
                &crypto::rsa_sign_sha256,
            ));
        }
        let refs: Vec<_> = certificates.iter().map(Vec::as_slice).collect();
        let bounded = verify_chain(&refs);
        assert_eq!(bounded.links.len(), 16);
        assert!(bounded.links.iter().all(|link| link.signature_verified.is_valid()));
        assert!(!bounded.fully_linked, "{bounded:?}");
        assert!(bounded.notes.iter().any(|note| note.contains("walk limit")));
    }

    #[test]
    fn chain_cycles_at_the_walk_bound_are_distinguished_from_roots() {
        use craft_anything_testkit::crypto;
        for cycle in [true, false] {
            let certificates: Vec<_> = (0..16)
                .map(|i| {
                    let issuer = if i < 15 {
                        i + 1
                    } else if cycle {
                        0
                    } else {
                        i
                    };
                    crypto::make_cert(
                        &format!("Bound {i}"),
                        "Fixture Org",
                        &format!("Bound {issuer}"),
                        &crypto::p256_spki(),
                        "1.2.840.10045.4.3.2",
                        &crypto::p256_sign_sha256,
                    )
                })
                .collect();
            let refs: Vec<_> = certificates.iter().map(Vec::as_slice).collect();
            let chain = verify_chain(&refs);
            assert_eq!(chain.links.len(), 16);
            assert!(chain.links.iter().all(|link| link.signature_verified.is_valid()));
            assert_eq!(chain.fully_linked, !cycle, "{chain:?}");
            assert_eq!(chain.notes.iter().any(|note| note.contains("chain loops")), cycle, "{chain:?}");
            assert!(!chain.notes.iter().any(|note| note.contains("walk limit")), "{chain:?}");
        }
    }

    #[test]
    fn macho_cms_verifies_and_detects_tamper() {
        use craft_anything_testkit::{apple, crypto};
        let cd = apple::code_directory_blob();
        let cms = crypto::cms_detached_rsa(&cd, "Craft Signer", "Craft Org");
        let r = verify_macho_cms(&cms, &cd).expect("parse");
        assert!(r.signature_valid.is_valid(), "{:?}", r);
        assert!(r.binds_code_directory);
        assert!(r.authenticated);
        // A changed CodeDirectory no longer matches the signed messageDigest.
        let mut bad_cd = cd.clone();
        bad_cd[100] ^= 0xff;
        let r2 = verify_macho_cms(&cms, &bad_cd).expect("parse");
        assert!(!r2.binds_code_directory);
        assert!(!r2.authenticated);
    }

    #[test]
    fn apk_signer_verifies_and_detects_tamper() {
        use craft_anything_testkit::android;
        let apk = android::signed_apk(false);
        let block = crate::apk_sign::parse(&apk).expect("signing block");
        assert!(!block.signers.is_empty());
        for s in &block.signers {
            let r = verify_apk_signer(&apk, block.offset, s);
            assert!(r.signature_valid.is_valid(), "{}: {:?}", s.scheme, r);
            assert!(r.digest_matches, "{}: {:?}", s.scheme, r);
            assert!(r.public_key_matches_certificate);
            assert!(r.verified);
            assert_eq!(r.certificate_sha256.as_ref().unwrap().len(), 64);
        }
        // Tampering a content byte breaks the digest but not the (well-formed) signature.
        let bad = android::signed_apk(true);
        let bblock = crate::apk_sign::parse(&bad).expect("block");
        let r = verify_apk_signer(&bad, bblock.offset, &bblock.signers[0]);
        assert!(!r.digest_matches);
        assert!(!r.verified);
    }

    #[test]
    fn apk_content_digest_matches_reference() {
        use craft_anything_testkit::{android, crypto};
        let apk = android::signed_apk(false);
        let block = crate::apk_sign::parse(&apk).unwrap();
        let computed = apk_content_digest(&apk, block.offset, Digest::Sha256).unwrap();
        // The digest recorded in the signed data (algorithm 0x0103 = SHA-256) must match.
        let stored = &block.signers[0].signed_digests.iter().find(|(id, _)| *id == 0x0103).unwrap().1;
        assert_eq!(&computed, stored);
        // And it matches the independent testkit reference computation over the same APK.
        assert_eq!(computed, crypto::apk_content_digest_sha256(&apk, block.offset as usize).to_vec());
    }

    #[test]
    fn jar_v1_integrity_and_pkcs7() {
        use craft_anything_testkit::{android, crypto};
        // Build the same .SF/.RSA the signed APK carries and verify the PKCS#7 over the .SF.
        let sf = b"Signature-Version: 1.0\r\nSHA-256-Digest-Manifest: AAA=\r\n\r\n";
        let rsa = crypto::cms_detached_rsa(sf, "Craft Signer", "Craft Org");
        let v = verify_pkcs7_detached(sf, &rsa).expect("pkcs7");
        assert!(v.signature_valid.is_valid(), "{:?}", v);
        assert_eq!(v.certificate_sha256.as_ref().unwrap().len(), 64);
        // A changed signature file no longer matches the signed messageDigest.
        let v2 = verify_pkcs7_detached(b"tampered", &rsa).expect("pkcs7");
        assert!(!v2.signature_valid.is_valid());
        // Full JAR integrity against a built manifest.
        let apk = android::signed_apk(false);
        let z = crate::zip::parse(&apk, 10_000).unwrap();
        let read = |name: &str| -> Option<Vec<u8>> {
            let e = z.entries.iter().find(|e| e.name == name)?;
            crate::zip::read_entry(&apk, e, 4 << 20).ok()
        };
        let mf = read("META-INF/MANIFEST.MF").unwrap();
        let sf = read("META-INF/CERT.SF").unwrap();
        let integ = verify_jar_integrity(&mf, &sf, read);
        assert_eq!(integ.manifest_digest_matches, Some(true), "{integ:?}");
        assert!(integ.files_listed >= 3);
        assert_eq!(integ.files_mismatched, 0, "{integ:?}");
        assert!(integ.files_matched >= 3);
    }
}
