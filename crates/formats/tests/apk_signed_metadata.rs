use craft_anything_formats::{apk_sign, verify};
use craft_anything_testkit::{android, crypto, electron::zip_stored};

const RSA_SHA256: u32 = 0x0103;
const UNKNOWN: u32 = 0x7fff_0001;

#[derive(Clone, Copy, Debug, Default)]
enum Damage {
    #[default]
    None,
    DigestListTail,
    SignatureListTail,
    DigestItemTail,
    SignatureItemTail,
    CertificateListTail,
    OversizedFirstCertificate,
    MissingAttributes,
    TruncatedAttributes,
    MalformedAttribute,
    SignerTail,
}

struct Fixture {
    scheme: u32,
    digest_ids: Vec<u32>,
    signature_ids: Vec<u32>,
    signed_sdk: (u32, u32),
    outer_sdk: (u32, u32),
    damage: Damage,
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            scheme: apk_sign::V3_ID,
            digest_ids: vec![RSA_SHA256],
            signature_ids: vec![RSA_SHA256],
            signed_sdk: (24, 0x7fff_ffff),
            outer_sdk: (24, 0x7fff_ffff),
            damage: Damage::None,
        }
    }
}

fn lp(bytes: &[u8]) -> Vec<u8> {
    [(bytes.len() as u32).to_le_bytes().as_slice(), bytes].concat()
}

fn fixture(config: &Fixture) -> (Vec<u8>, u64, apk_sign::Signer) {
    let zip = zip_stored(&[("fixture.bin", b"signed APK content")]);
    let cd = u32::from_le_bytes(zip[zip.len() - 6..zip.len() - 2].try_into().unwrap()) as usize;
    let digest = crypto::apk_content_digest_sha256(&zip, cd);
    let mut digests = Vec::new();
    for id in &config.digest_ids {
        let mut item = [id.to_le_bytes().as_slice(), lp(&digest).as_slice()].concat();
        if matches!(config.damage, Damage::DigestItemTail) {
            item.push(0);
        }
        digests.extend(lp(&item));
    }
    if matches!(config.damage, Damage::DigestListTail) {
        digests.push(0);
    }
    let mut signed = lp(&digests);
    let cert = crypto::self_signed_rsa("APK metadata", "Fixture Org");
    let mut certificates = Vec::new();
    if matches!(config.damage, Damage::OversizedFirstCertificate) {
        certificates.extend(lp(&vec![0; 64 * 1024 + 1]));
    }
    certificates.extend(lp(&cert));
    if matches!(config.damage, Damage::CertificateListTail) {
        certificates.push(0);
    }
    signed.extend(lp(&certificates));
    if config.scheme != apk_sign::V2_ID {
        signed.extend(config.signed_sdk.0.to_le_bytes());
        signed.extend(config.signed_sdk.1.to_le_bytes());
    }
    match config.damage {
        Damage::MissingAttributes => {}
        Damage::TruncatedAttributes => signed.push(0),
        Damage::MalformedAttribute => signed.extend(lp(&lp(&[1, 2, 3]))),
        _ => signed.extend(lp(&lp(&[0x5a, 0, 0, 0, 1, 2, 3]))),
    }
    let signature = crypto::rsa_sign_sha256(&signed);
    let mut signatures = Vec::new();
    for id in &config.signature_ids {
        let mut item = [id.to_le_bytes().as_slice(), lp(&signature).as_slice()].concat();
        if matches!(config.damage, Damage::SignatureItemTail) {
            item.push(0);
        }
        signatures.extend(lp(&item));
    }
    if matches!(config.damage, Damage::SignatureListTail) {
        signatures.push(0);
    }
    let mut signer = lp(&signed);
    if config.scheme != apk_sign::V2_ID {
        signer.extend(config.outer_sdk.0.to_le_bytes());
        signer.extend(config.outer_sdk.1.to_le_bytes());
    }
    signer.extend(lp(&signatures));
    signer.extend(lp(&crypto::rsa_spki()));
    if matches!(config.damage, Damage::SignerTail) {
        signer.push(0);
    }
    let value = lp(&lp(&signer));
    let pair = [((value.len() + 4) as u64).to_le_bytes().as_slice(), config.scheme.to_le_bytes().as_slice(), value.as_slice()].concat();
    let size = (pair.len() + 24) as u64;
    let block = [size.to_le_bytes().as_slice(), pair.as_slice(), size.to_le_bytes().as_slice(), b"APK Sig Block 42"].concat();
    let apk = android::with_signing_block(&zip, &block);
    let parsed = apk_sign::parse(&apk).unwrap();
    assert_eq!(parsed.signers.len(), 1);
    let signer = parsed.signers.into_iter().next().unwrap();
    assert!(verify::verify_signature(&signer.public_key, verify::SigScheme::RsaPkcs1v15(verify::Digest::Sha256), &signer.signed_data, &signature).is_valid());
    (apk, parsed.offset, signer)
}

fn report(config: &Fixture) -> verify::ApkSigner {
    let (apk, offset, signer) = fixture(config);
    let result = verify::verify_apk_signer(&apk, offset, &signer);
    assert!(result.signature_valid.is_valid(), "{result:?}");
    assert!(result.digest_matches, "{result:?}");
    assert!(result.public_key_matches_certificate, "{result:?}");
    result
}

#[test]
fn sdk_range_copies_must_agree_before_an_apk_is_verified() {
    for scheme in [apk_sign::V3_ID, apk_sign::V31_ID] {
        for (signed_sdk, outer_sdk) in [((24, 35), (25, 35)), ((24, 35), (24, 36)), ((25, 35), (24, 35))] {
            let result = report(&Fixture { scheme, signed_sdk, outer_sdk, ..Fixture::default() });
            assert!(!result.verified, "scheme {scheme:#x}, {signed_sdk:?}/{outer_sdk:?}: {result:?}");
        }
    }
}

#[test]
fn signature_algorithm_lists_must_match_in_order() {
    for scheme in [apk_sign::V2_ID, apk_sign::V3_ID, apk_sign::V31_ID] {
        for (digest_ids, signature_ids) in [
            (vec![RSA_SHA256, UNKNOWN], vec![RSA_SHA256]),
            (vec![RSA_SHA256], vec![RSA_SHA256, UNKNOWN]),
            (vec![RSA_SHA256, UNKNOWN], vec![UNKNOWN, RSA_SHA256]),
        ] {
            let result = report(&Fixture { scheme, digest_ids, signature_ids, ..Fixture::default() });
            assert!(!result.verified, "scheme {scheme:#x}: {result:?}");
        }
    }
}

#[test]
fn readable_prefixes_of_incomplete_signer_fields_do_not_verify() {
    for scheme in [apk_sign::V2_ID, apk_sign::V3_ID] {
        for damage in [
            Damage::DigestListTail,
            Damage::SignatureListTail,
            Damage::DigestItemTail,
            Damage::SignatureItemTail,
            Damage::CertificateListTail,
            Damage::OversizedFirstCertificate,
            Damage::MissingAttributes,
            Damage::TruncatedAttributes,
            Damage::MalformedAttribute,
            Damage::SignerTail,
        ] {
            let result = report(&Fixture { scheme, damage, ..Fixture::default() });
            assert!(!result.verified, "scheme {scheme:#x}, {damage:?}: {result:?}");
        }
    }
}

#[test]
fn complete_signers_keep_unknown_algorithms_and_matching_sdk_ranges() {
    for scheme in [apk_sign::V2_ID, apk_sign::V3_ID, apk_sign::V31_ID] {
        for ids in [vec![RSA_SHA256], vec![UNKNOWN, RSA_SHA256], vec![RSA_SHA256, UNKNOWN]] {
            let config = Fixture { scheme, digest_ids: ids.clone(), signature_ids: ids, signed_sdk: (28, 35), outer_sdk: (28, 35), ..Fixture::default() };
            assert!(report(&config).verified);
        }
    }
}

#[test]
fn capped_algorithm_lists_are_not_verified_as_complete_prefixes() {
    for count in [64, 65] {
        let ids: Vec<_> = std::iter::once(RSA_SHA256).chain(std::iter::repeat_n(UNKNOWN, count - 1)).collect();
        let result = report(&Fixture { digest_ids: ids.clone(), signature_ids: ids, ..Fixture::default() });
        assert_eq!(result.verified, count == 64, "{count} algorithms: {result:?}");
    }
}
