use craft_anything_formats::{der, pkcs7, verify};
use craft_anything_testkit::{
    android::{der as tlv, der_oid},
    crypto,
};
use rsa::{
    RsaPrivateKey, RsaPublicKey,
    pkcs8::EncodePublicKey,
    signature::{RandomizedSigner, SignatureEncoding},
};
use std::sync::OnceLock;

const PSS: &str = "1.2.840.113549.1.1.10";
const SHA256: &str = "2.16.840.1.101.3.4.2.1";
const SHA384: &str = "2.16.840.1.101.3.4.2.2";
const SHA512: &str = "2.16.840.1.101.3.4.2.3";

fn key() -> &'static RsaPrivateKey {
    static KEY: OnceLock<RsaPrivateKey> = OnceLock::new();
    KEY.get_or_init(|| RsaPrivateKey::new(&mut p256::elliptic_curve::rand_core::OsRng, 2048).unwrap())
}

fn seq(fields: &[Vec<u8>]) -> Vec<u8> {
    tlv(0x30, &fields.concat())
}

fn fields(data: &[u8]) -> Vec<Vec<u8>> {
    der::children(data, &der::read(data, 0).unwrap()).iter().map(|f| f.whole(data).unwrap().to_vec()).collect()
}

fn algorithm(hash: &str, salt: u8) -> Vec<u8> {
    let digest = seq(&[der_oid(hash), tlv(0x05, &[])]);
    seq(&[der_oid(PSS), seq(&[tlv(0xa0, &digest), tlv(0xa1, &seq(&[der_oid("1.2.840.113549.1.1.8"), digest])), tlv(0xa2, &tlv(0x02, &[salt]))])])
}

fn spki(algorithm: Option<&[u8]>) -> Vec<u8> {
    let ordinary = RsaPublicKey::from(key()).to_public_key_der().unwrap().as_bytes().to_vec();
    let Some(algorithm) = algorithm else { return ordinary };
    let root = der::read(&ordinary, 0).unwrap();
    let fields = der::children(&ordinary, &root);
    seq(&[algorithm.to_vec(), fields[1].whole(&ordinary).unwrap().to_vec()])
}

fn cert(inner: &[u8], outer: &[u8], public_key: &[u8], digest: verify::Digest, salt: usize) -> Vec<u8> {
    let template = crypto::make_cert("PSS Fixture", "Craft Test", "PSS Fixture", public_key, PSS, &|_| vec![1]);
    let root = der::read(&template, 0).unwrap();
    let fields = der::children(&template, &root);
    let mut tbs: Vec<_> = der::children(&template, &fields[0]).iter().map(|f| f.whole(&template).unwrap().to_vec()).collect();
    tbs[2] = inner.to_vec();
    let tbs = seq(&tbs);
    let signature = sign(&tbs, digest, salt);
    seq(&[tbs, outer.to_vec(), tlv(0x03, &[&[0][..], &signature].concat())])
}

fn sign(message: &[u8], digest: verify::Digest, salt: usize) -> Vec<u8> {
    let mut rng = p256::elliptic_curve::rand_core::OsRng;
    match digest {
        verify::Digest::Sha256 => rsa::pss::SigningKey::<sha2::Sha256>::new_with_salt_len(key().clone(), salt).sign_with_rng(&mut rng, message).to_vec(),
        verify::Digest::Sha384 => rsa::pss::SigningKey::<sha2::Sha384>::new_with_salt_len(key().clone(), salt).sign_with_rng(&mut rng, message).to_vec(),
        verify::Digest::Sha512 => rsa::pss::SigningKey::<sha2::Sha512>::new_with_salt_len(key().clone(), salt).sign_with_rng(&mut rng, message).to_vec(),
        _ => panic!("fixture signs SHA-256, SHA-384 and SHA-512"),
    }
}

fn result(cert: &[u8]) -> verify::Verified {
    let chain = verify::verify_chain(&[cert]);
    assert_eq!(chain.links.len(), 1);
    chain.links[0].signature_verified.clone()
}

#[test]
fn ordinary_pss_sha256_control() {
    let alg = algorithm(SHA256, 32);
    assert_eq!(result(&cert(&alg, &alg, &spki(None), verify::Digest::Sha256, 32)), verify::Verified::Valid);
}

#[test]
fn respects_declared_pss_hash() {
    let alg = algorithm(SHA384, 48);
    assert_ne!(result(&cert(&alg, &alg, &spki(None), verify::Digest::Sha256, 32)), verify::Verified::Valid);
}

#[test]
fn respects_declared_pss_salt_length() {
    let alg = algorithm(SHA256, 0);
    assert_ne!(result(&cert(&alg, &alg, &spki(None), verify::Digest::Sha256, 32)), verify::Verified::Valid);
}

#[test]
fn respects_pss_public_key_constraints() {
    let alg = algorithm(SHA256, 32);
    let key_alg = algorithm(SHA384, 64);
    assert_ne!(result(&cert(&alg, &alg, &spki(Some(&key_alg)), verify::Digest::Sha256, 32)), verify::Verified::Valid);
}

#[test]
fn requires_signed_and_outer_pss_parameters_to_agree() {
    let inner = algorithm(SHA384, 48);
    let outer = algorithm(SHA256, 32);
    assert_ne!(result(&cert(&inner, &outer, &spki(None), verify::Digest::Sha256, 32)), verify::Verified::Valid);
}

#[test]
fn verifies_genuine_sha384_pss() {
    let alg = algorithm(SHA384, 48);
    assert_eq!(result(&cert(&alg, &alg, &spki(None), verify::Digest::Sha384, 48)), verify::Verified::Valid);
}

#[test]
fn verifies_genuine_zero_salt_pss() {
    let alg = algorithm(SHA256, 0);
    assert_eq!(result(&cert(&alg, &alg, &spki(None), verify::Digest::Sha256, 0)), verify::Verified::Valid);
}

fn cms(content: &[u8], signature_algorithm: &[u8], digest: verify::Digest, salt: usize, with_attrs: bool) -> Vec<u8> {
    let cert_alg = algorithm(SHA256, 32);
    let certificate = cert(&cert_alg, &cert_alg, &spki(None), verify::Digest::Sha256, 32);
    let parts = der::certificate_parts(&certificate).unwrap();
    let digest_oid = match digest {
        verify::Digest::Sha256 => SHA256,
        verify::Digest::Sha384 => SHA384,
        verify::Digest::Sha512 => SHA512,
        _ => panic!("fixture uses SHA-2"),
    };
    let digest_alg = seq(&[der_oid(digest_oid), tlv(0x05, &[])]);
    let attributes = [
        seq(&[der_oid("1.2.840.113549.1.9.3"), tlv(0x31, &der_oid("1.2.840.113549.1.7.1"))]),
        seq(&[der_oid("1.2.840.113549.1.9.4"), tlv(0x31, &tlv(0x04, &digest.hash(content)))]),
    ]
    .concat();
    let signed_attrs = tlv(0x31, &attributes);
    let signature = sign(if with_attrs { &signed_attrs } else { content }, digest, salt);
    let mut signer = vec![tlv(0x02, &[1]), seq(&[parts.issuer.to_vec(), tlv(0x02, parts.serial)]), digest_alg.clone()];
    if with_attrs {
        signer.push(tlv(0xa0, &attributes));
    }
    signer.extend([signature_algorithm.to_vec(), tlv(0x04, &signature)]);
    let signed_data =
        seq(&[tlv(0x02, &[1]), tlv(0x31, &digest_alg), seq(&[der_oid("1.2.840.113549.1.7.1")]), tlv(0xa0, &certificate), tlv(0x31, &seq(&signer))]);
    seq(&[der_oid("1.2.840.113549.1.7.2"), tlv(0xa0, &signed_data)])
}

#[test]
fn cms_checks_parameters_with_and_without_signed_attributes() {
    let content = b"signed content";
    for attrs in [false, true] {
        for (oid, digest, salt) in [(SHA256, verify::Digest::Sha256, 0), (SHA384, verify::Digest::Sha384, 8), (SHA512, verify::Digest::Sha512, 64)] {
            let valid = cms(content, &algorithm(oid, salt), digest, usize::from(salt), attrs);
            let result = verify::verify_pkcs7_detached(content, &valid).unwrap();
            assert_eq!(result.signature_valid, verify::Verified::Valid);
            assert!(result.certificate_sha256.is_some());
            assert_ne!(verify::verify_pkcs7_detached(b"changed content", &valid).unwrap().signature_valid, verify::Verified::Valid);
            if attrs {
                assert!(verify::verify_macho_cms(&valid, content).unwrap().authenticated);
            }
        }
        let wrong_hash = cms(content, &algorithm(SHA384, 48), verify::Digest::Sha256, 32, attrs);
        assert_ne!(verify::verify_pkcs7_detached(content, &wrong_hash).unwrap().signature_valid, verify::Verified::Valid);
        let wrong_salt = cms(content, &algorithm(SHA256, 0), verify::Digest::Sha256, 32, attrs);
        assert_ne!(verify::verify_pkcs7_detached(content, &wrong_salt).unwrap().signature_valid, verify::Verified::Valid);
        if attrs {
            assert!(!verify::verify_macho_cms(&wrong_hash, content).unwrap().authenticated);
            assert!(!verify::verify_macho_cms(&wrong_salt, content).unwrap().authenticated);
        }
    }
}

#[test]
fn pss_key_constraints_allow_unrestricted_keys_and_longer_salts() {
    let alg = algorithm(SHA256, 32);
    for key_alg in [seq(&[der_oid(PSS)]), algorithm(SHA256, 8), algorithm(SHA256, 32)] {
        assert_eq!(result(&cert(&alg, &alg, &spki(Some(&key_alg)), verify::Digest::Sha256, 32)), verify::Verified::Valid);
    }
    let key_alg = algorithm(SHA256, 33);
    assert_ne!(result(&cert(&alg, &alg, &spki(Some(&key_alg)), verify::Digest::Sha256, 32)), verify::Verified::Valid);
}

#[test]
fn scheme_projection_does_not_discard_pss_parameters() {
    let content = b"explicit PSS parameters";
    let standard = cms(content, &algorithm(SHA256, 32), verify::Digest::Sha256, 32, true);
    assert_eq!(pkcs7::parse_signed_data(&standard).unwrap().scheme(), Some(verify::SigScheme::RsaPss(verify::Digest::Sha256)));
    for alg in [algorithm(SHA256, 0), algorithm(SHA384, 48)] {
        let changed = cms(content, &alg, verify::Digest::Sha256, 32, true);
        assert!(pkcs7::parse_signed_data(&changed).unwrap().scheme().is_none());
    }
    let mut parameters = fields(&fields(&algorithm(SHA256, 32))[1]);
    parameters[1] = tlv(0xa1, &seq(&[der_oid("1.2.840.113549.1.1.8"), seq(&[der_oid(SHA384)])]));
    let different_mask = cms(content, &seq(&[der_oid(PSS), seq(&parameters)]), verify::Digest::Sha256, 32, true);
    assert!(pkcs7::parse_signed_data(&different_mask).unwrap().scheme().is_none());
}

#[test]
fn defaults_and_semantically_equivalent_digest_identifiers_are_preserved() {
    let full = algorithm(SHA256, 20);
    let mut params = fields(&fields(&full)[1]);
    params.pop(); // omitted saltLength means 20 even with SHA-256
    let inner = seq(&[der_oid(PSS), seq(&params)]);
    let hash_without_null = seq(&[der_oid(SHA256)]);
    params[0] = tlv(0xa0, &hash_without_null);
    params[1] = tlv(0xa1, &seq(&[der_oid("1.2.840.113549.1.1.8"), hash_without_null]));
    params.push(tlv(0xa2, &tlv(0x02, &[20])));
    params.push(tlv(0xa3, &tlv(0x02, &[1])));
    let outer = seq(&[der_oid(PSS), seq(&params)]);
    assert_eq!(result(&cert(&inner, &outer, &spki(None), verify::Digest::Sha256, 20)), verify::Verified::Valid);
    let defaults = seq(&[der_oid(PSS), seq(&[])]);
    assert!(matches!(result(&cert(&defaults, &defaults, &spki(None), verify::Digest::Sha256, 32)), verify::Verified::Unsupported { .. }));
    assert_eq!(verify::scheme_from_oid(PSS), None);
}

#[test]
fn malformed_or_unsupported_parameters_keep_cms_metadata_without_verification() {
    let full = algorithm(SHA256, 32);
    let params = fields(&fields(&full)[1]);
    let mut malformed = vec![seq(&[der_oid(PSS)]), seq(&[der_oid(PSS), tlv(0x05, &[])])];
    for replacement in [
        tlv(0xa0, &seq(&[der_oid("1.2.3.4")])),
        tlv(0xa0, &seq(&[der_oid(SHA256), tlv(0x04, &[])])),
        tlv(0xa1, &seq(&[der_oid("1.2.3.4"), seq(&[der_oid(SHA256)])])),
        tlv(0xa1, &seq(&[der_oid("1.2.840.113549.1.1.8")])),
        tlv(0xa1, &seq(&[der_oid("1.2.840.113549.1.1.8"), seq(&[der_oid(SHA384)])])),
        tlv(0xa2, &tlv(0x02, &[])),
        tlv(0xa2, &tlv(0x02, &[0xff])),
        tlv(0xa2, &tlv(0x02, &[0, 32])),
        tlv(0xa2, &tlv(0x02, &[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff])),
    ] {
        let mut changed = params.clone();
        let tag = replacement[0];
        changed[usize::from(tag - 0xa0)] = replacement;
        malformed.push(seq(&[der_oid(PSS), seq(&changed)]));
    }
    for trailer in [0, 2] {
        let mut changed = params.clone();
        changed.push(tlv(0xa3, &tlv(0x02, &[trailer])));
        malformed.push(seq(&[der_oid(PSS), seq(&changed)]));
    }
    let mut duplicate = params.clone();
    duplicate.push(params[2].clone());
    malformed.push(seq(&[der_oid(PSS), seq(&duplicate)]));
    let mut out_of_order = params.clone();
    out_of_order.swap(0, 1);
    malformed.push(seq(&[der_oid(PSS), seq(&out_of_order)]));
    malformed.push(seq(&[der_oid(PSS), tlv(0x30, &[params.concat(), vec![0xa3, 2, 2]].concat())]));
    malformed.push(seq(&[der_oid(PSS), seq(&params), tlv(0x05, &[])]));
    let content = b"metadata survives unsupported parameters";
    for alg in malformed {
        let signed = cms(content, &alg, verify::Digest::Sha256, 32, true);
        let parsed = pkcs7::parse_signed_data(&signed).unwrap();
        assert_eq!(parsed.certificates.len(), 1);
        assert!(parsed.signer_cert().is_some());
        assert_eq!(parsed.digest, Some(verify::Digest::Sha256));
        let detached = verify::verify_pkcs7_detached(content, &signed).unwrap();
        assert_ne!(detached.signature_valid, verify::Verified::Valid);
        assert!(detached.signer_subject.is_some());
        assert!(detached.certificate_sha256.is_some());
        assert!(!verify::verify_macho_cms(&signed, content).unwrap().authenticated);
    }
}
