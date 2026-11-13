use craft_anything_engine::{CallOptions, Caller, Consent, Engine};
use serde_json::json;

fn romfs_entry(kind: u32, name: &str, payload: &[u8], declared_size: u32) -> Vec<u8> {
    let mut data = vec![0; (96 + payload.len()).div_ceil(16) * 16];
    data[..8].copy_from_slice(b"-rom1fs-");
    let size = data.len() as u32;
    data[8..12].copy_from_slice(&size.to_be_bytes());
    data[16..22].copy_from_slice(b"craft\0");
    data[32..36].copy_from_slice(&1u32.to_be_bytes());
    data[36..40].copy_from_slice(&64u32.to_be_bytes());
    data[48] = b'.';
    data[64..68].copy_from_slice(&kind.to_be_bytes());
    data[72..76].copy_from_slice(&declared_size.to_be_bytes());
    data[80..80 + name.len()].copy_from_slice(name.as_bytes());
    data[96..96 + payload.len()].copy_from_slice(payload);
    let checksum = data.as_chunks::<4>().0.iter().fold(0u32, |sum, word| sum.wrapping_add(u32::from_be_bytes(*word)));
    data[12..16].copy_from_slice(&checksum.wrapping_neg().to_be_bytes());
    data
}

#[test]
fn filesystem_extraction_keeps_skips_and_empty_classification() {
    for (kind, name, payload, size, recovered) in [
        (1, "empty", b"".as_slice(), 0, false),
        (3, "link", b"target".as_slice(), 6, false),
        (2, "../refused", b"abc".as_slice(), 3, false),
        (2, "missing", b"abc".as_slice(), 4096, false),
        (2, "kept", b"abc".as_slice(), 3, true),
    ] {
        let directory = craft_anything_engine::testing::temp_dir();
        let input = directory.join("firmware.romfs");
        let output = directory.join("output");
        std::fs::write(&input, romfs_entry(kind, name, payload, size)).unwrap();
        let engine = Engine::new(craft_anything_engine::testing::session());
        let evidence = engine
            .call("firmware_image_extract", json!({"path": input, "output_directory": output}), CallOptions::new(Caller::Cli).with_consent(Consent::Granted))
            .unwrap()
            .to_json();
        let result = &evidence["result"];
        assert_eq!(result["files_written"], if recovered { 2 } else { 1 }, "{name}: {result}");
        assert_eq!(result["not_unpacked"].get("romfs").is_some(), kind != 2, "{name}: {result}");
        assert_eq!(!result["skipped"].as_array().unwrap().is_empty(), kind == 3 || (kind == 2 && !recovered), "{name}: {result}");
        if recovered {
            assert_eq!(std::fs::read(output.join("0-romfs.extracted/kept")).unwrap(), payload);
        }
        assert!(!output.join("0-romfs.extracted/link").exists());
        assert!(!output.join("refused").exists());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
