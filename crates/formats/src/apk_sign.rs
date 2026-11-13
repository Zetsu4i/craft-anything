//! The APK Signing Block (signature schemes v2, v3 and v3.1): the id-value pairs that sit
//! between the last ZIP entry and the central directory, and the signers inside them
//! (certificates, digests, signature algorithms, SDK ranges). Signatures are NOT verified.

use crate::bytes::Reader;
use serde::Serialize;
use std::collections::BTreeMap;

const MAGIC: &[u8; 16] = b"APK Sig Block 42";
const EOCD_SEARCH: usize = 22 + 0xffff;
const CD_HEADER_LEN: usize = 46;
// Enough boundaries for a full classic ZIP directory plus the search window's allowance.
// Keep cache allocation independent of the input payload size, including each directory's start.
const MAX_DIRECTORY_BOUNDARIES: usize = EOCD_SEARCH + u16::MAX as usize;
const MAX_PAIRS: usize = 256;
const MAX_ITEMS: usize = 64;
// One block has at most MAX_PAIRS * (1 + MAX_ITEMS) probe records. Bound both record
// maps together, independently of file size and the number of candidate block extents.
const MAX_PROBE_RECORDS: usize = EOCD_SEARCH;
/// Most certificate bytes kept per certificate.
const MAX_CERT: usize = 64 * 1024;

pub const V2_ID: u32 = 0x7109_871a;
pub const V3_ID: u32 = 0xf053_68c0;
pub const V31_ID: u32 = 0x1b93_ad61;

/// What a block id means, when we know.
pub fn block_name(id: u32) -> &'static str {
    match id {
        V2_ID => "signature scheme v2",
        V3_ID => "signature scheme v3",
        V31_ID => "signature scheme v3.1",
        0x4272_6577 => "verity padding",
        0x6dff_800d => "source stamp v2",
        0x2b09_189e => "source stamp v1",
        0x2146_444e => "store metadata",
        0x504b_4453 => "dependency info",
        _ => "unknown",
    }
}

/// Name of a signature algorithm id.
pub fn algorithm_name(id: u32) -> &'static str {
    match id {
        0x0101 => "RSASSA-PSS with SHA2-256",
        0x0102 => "RSASSA-PSS with SHA2-512",
        0x0103 => "RSASSA-PKCS1-v1_5 with SHA2-256",
        0x0104 => "RSASSA-PKCS1-v1_5 with SHA2-512",
        0x0201 => "ECDSA with SHA2-256",
        0x0202 => "ECDSA with SHA2-512",
        0x0301 => "DSA with SHA2-256",
        0x0421 => "RSASSA-PKCS1-v1_5 with SHA2-256 (verity)",
        0x0423 => "ECDSA with SHA2-256 (verity)",
        0x0425 => "DSA with SHA2-256 (verity)",
        _ => "unknown",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockEntry {
    pub id: String,
    pub name: &'static str,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignerDigest {
    pub algorithm: String,
    pub algorithm_name: &'static str,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Signer {
    /// v2, v3 or v3.1.
    pub scheme: &'static str,
    /// Certificates (DER), first is the signer's.
    #[serde(skip)]
    pub certificates: Vec<Vec<u8>>,
    pub digests: Vec<SignerDigest>,
    pub signature_algorithms: Vec<String>,
    pub min_sdk: Option<u32>,
    pub max_sdk: Option<u32>,
    pub public_key_bytes: usize,
    /// The signer's public key (SubjectPublicKeyInfo, DER).
    #[serde(skip)]
    pub public_key: Vec<u8>,
    /// The exact signed-data bytes the signatures are computed over.
    #[serde(skip)]
    pub signed_data: Vec<u8>,
    /// Signatures present, as (algorithm id, signature bytes).
    #[serde(skip)]
    pub signatures: Vec<(u32, Vec<u8>)>,
    /// The digests (algorithm id, raw bytes) carried inside the signed data.
    #[serde(skip)]
    pub signed_digests: Vec<(u32, Vec<u8>)>,
    /// Required fields were incomplete or unsigned algorithm/SDK values differed from
    /// their signed copies. Signature verification must not accept a recovered prefix.
    #[serde(skip)]
    pub verification_error: Option<&'static str>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SigningBlock {
    pub offset: u64,
    pub size: u64,
    pub entries: Vec<BlockEntry>,
    pub signers: Vec<Signer>,
    pub warnings: Vec<String>,
}

fn signing_block_extent(r: &Reader<'_>, cd: usize) -> Option<(usize, u64)> {
    if r.slice(cd.checked_sub(16)?, 16)? != MAGIC {
        return None;
    }
    let size = r.u64_le(cd.checked_sub(24)?)?;
    let total = usize::try_from(size).ok()?.checked_add(8)?;
    Some((cd.checked_sub(total)?, size))
}

#[derive(Default)]
struct DirectoryPrefix {
    boundaries: Vec<usize>,
    stopped: bool,
}

impl DirectoryPrefix {
    fn push(&mut self, boundary: usize, capacity_left: &mut usize) -> Option<()> {
        if self.boundaries.len() == self.boundaries.capacity() {
            let additional = self.boundaries.capacity().max(1).min(*capacity_left);
            if additional == 0 {
                return None;
            }
            let before = self.boundaries.capacity();
            self.boundaries.try_reserve_exact(additional).ok()?;
            *capacity_left = capacity_left.checked_sub(self.boundaries.capacity().checked_sub(before)?)?;
        }
        self.boundaries.push(boundary);
        Some(())
    }
}

struct DirectoryCache {
    prefixes: BTreeMap<usize, DirectoryPrefix>,
    capacity_left: usize,
}

impl DirectoryCache {
    fn new() -> Self {
        Self { prefixes: BTreeMap::new(), capacity_left: MAX_DIRECTORY_BOUNDARIES }
    }

    /// `None` means a scan-wide work or cache limit ran out; `Some(false)` means bad framing.
    fn framed(&mut self, r: &Reader<'_>, cd: usize, size: usize, count: u16, records_left: &mut usize) -> Option<bool> {
        if usize::from(count) > size / CD_HEADER_LEN {
            return Some(false);
        }
        let Some(end) = cd.checked_add(size) else { return Some(false) };
        if r.slice(cd, size).is_none() {
            return Some(false);
        }
        let prefix = self.prefixes.entry(cd).or_default();
        if prefix.boundaries.is_empty() {
            prefix.push(cd, &mut self.capacity_left)?;
        }
        // Only new header visits consume work. Inspect the immutable input, not this candidate's
        // declared size: a short candidate must not poison a later request for a longer prefix.
        while prefix.boundaries.len() <= usize::from(count) && !prefix.stopped {
            let pos = *prefix.boundaries.last()?;
            if pos >= end {
                return Some(false);
            }
            *records_left = records_left.checked_sub(1)?;
            if let Some(next) = directory_record_end(r, pos) {
                prefix.push(next, &mut self.capacity_left)?;
            } else {
                prefix.stopped = true;
            }
        }
        let Some(pos) = prefix.boundaries.get(usize::from(count)).copied().filter(|pos| *pos <= end) else { return Some(false) };
        if pos == end {
            return Some(true);
        }
        // The candidate's exact size still permits only one optional digital-signature record.
        let Some(tail) = r.slice(pos, end - pos) else { return Some(false) };
        let tail_reader = Reader::new(tail);
        Some(tail_reader.slice(0, 4) == Some(b"PK\x05\x05") && tail_reader.u16_le(4).is_some_and(|len| usize::from(len).checked_add(6) == Some(tail.len())))
    }
}

fn directory_record_end(r: &Reader<'_>, pos: usize) -> Option<usize> {
    let header = Reader::new(r.slice(pos, CD_HEADER_LEN)?);
    if header.slice(0, 4) != Some(b"PK\x01\x02") {
        return None;
    }
    let len = [28, 30, 32].into_iter().try_fold(CD_HEADER_LEN, |len, field| len.checked_add(usize::from(header.u16_le(field)?)))?;
    r.slice(pos, len)?;
    pos.checked_add(len)
}

struct EocdCandidate {
    offset: usize,
    directory: usize,
    has_signer: bool,
    ambiguous: bool,
}

fn find_eocd(data: &[u8]) -> Option<EocdCandidate> {
    // One input-sized pass plus the search window's allowance, shared by every candidate's
    // directory records, signing-block pairs and signer records.
    let records_left = (data.len() / CD_HEADER_LEN).checked_add(EOCD_SEARCH)?;
    find_eocd_with_budget(data, records_left)
}

fn find_eocd_with_budget(data: &[u8], mut records_left: usize) -> Option<EocdCandidate> {
    let r = Reader::new(data);
    let from = data.len().saturating_sub(EOCD_SEARCH);
    let mut i = data.len().checked_sub(22)?;
    // Empty or unreadable blocks must not hide recoverable signers. Among equally readable
    // candidates, retain the lowest offset and report distinct readable blocks as ambiguous.
    let mut found: Option<EocdCandidate> = None;
    let mut ambiguous = false;
    let mut directories = DirectoryCache::new();
    let mut probes = ProbeCache::new();
    loop {
        if let Some(end) = r.slice(i, 22).map(Reader::new)
            && end.slice(0, 4) == Some(b"PK\x05\x06")
            && end.u16_le(20).is_some_and(|n| usize::from(n) == data.len() - i - 22)
            && let Some(cd) = end.u32_le(16).and_then(|n| usize::try_from(n).ok())
            && let Some(size) = end.u32_le(12).and_then(|n| usize::try_from(n).ok())
            && cd.checked_add(size) == Some(i)
            && let Some((start, _)) = signing_block_extent(&r, cd)
            && let Some(count) = end.u16_le(10)
            && directories.framed(&r, cd, size, count, &mut records_left)?
        {
            let has_signer = probes.has_readable_signer(&r, start, cd, &mut records_left)?;
            if let Some(previous) = &found {
                ambiguous |= previous.has_signer && has_signer && previous.directory != cd;
            }
            if has_signer || found.as_ref().is_none_or(|previous| !previous.has_signer) {
                found = Some(EocdCandidate { offset: i, directory: cd, has_signer, ambiguous });
            }
        }
        if i == from {
            return found;
        }
        i -= 1;
    }
}

struct Lp<'a> {
    data: &'a [u8],
}

impl<'a> Lp<'a> {
    fn at(&self, pos: usize) -> Option<(&'a [u8], usize)> {
        let r = Reader::new(self.data);
        let n = r.u32_le(pos)? as usize;
        let start = pos.checked_add(4)?;
        let s = r.slice(start, n)?;
        Some((s, start.checked_add(n)?))
    }

    /// Every length-prefixed item in a sequence.
    fn items(data: &'a [u8]) -> Vec<&'a [u8]> {
        let l = Lp { data };
        let mut out = Vec::new();
        let mut pos = 0;
        while pos < data.len() && out.len() < MAX_ITEMS {
            match l.at(pos) {
                Some((s, next)) => {
                    out.push(s);
                    pos = next;
                }
                None => break,
            }
        }
        out
    }

    fn complete_items(data: &'a [u8]) -> Option<Vec<&'a [u8]>> {
        let items = Self::items(data);
        let consumed = items.iter().try_fold(0usize, |offset, item| offset.checked_add(4)?.checked_add(item.len()))?;
        (consumed == data.len()).then_some(items)
    }
}

#[derive(Clone, Copy)]
struct PairProbe {
    end: usize,
    // A recognized pair is unprobed until its complete value fits a candidate's extent.
    readable: Option<bool>,
}

impl PairProbe {
    fn read(r: &Reader<'_>, pos: usize) -> Option<Self> {
        let len = usize::try_from(r.u64_le(pos)?).ok()?;
        let body = pos.checked_add(8)?;
        if len < 4 || r.slice(body, len).is_none() {
            return None;
        }
        let readable = if matches!(r.u32_le(body)?, V2_ID | V3_ID | V31_ID) { None } else { Some(false) };
        Some(Self { end: body.checked_add(len)?, readable })
    }
}

#[derive(Clone, Copy)]
struct SignerProbe {
    end: usize,
    readable: bool,
}

impl SignerProbe {
    fn read(r: &Reader<'_>, pos: usize) -> Option<Self> {
        let len = usize::try_from(r.u32_le(pos)?).ok()?;
        let body = pos.checked_add(4)?;
        let item = Lp { data: r.slice(body, len)? };
        // parse_signer requires only the signed-data LP; all later fields are optional.
        Some(Self { end: body.checked_add(len)?, readable: item.at(0).is_some() })
    }
}

struct ProbeCache {
    pairs: BTreeMap<usize, Option<PairProbe>>,
    items: BTreeMap<usize, Option<SignerProbe>>,
    slots_left: usize,
}

impl ProbeCache {
    fn new() -> Self {
        Self { pairs: BTreeMap::new(), items: BTreeMap::new(), slots_left: MAX_PROBE_RECORDS }
    }

    fn spend(&mut self, records_left: &mut usize) -> Option<()> {
        let remaining = records_left.checked_sub(1)?;
        self.slots_left = self.slots_left.checked_sub(1)?;
        *records_left = remaining;
        Some(())
    }

    /// Cache physical records, not candidate decisions. Several block starts may reach the
    /// same pair, and different pairs' signer sequences may reach the same length prefix.
    fn sequence_has_signer(&mut self, r: &Reader<'_>, mut pos: usize, end: usize, records_left: &mut usize) -> Option<bool> {
        for _ in 0..MAX_ITEMS {
            if pos >= end {
                break;
            }
            let step = if let Some(step) = self.items.get(&pos) {
                *step
            } else {
                self.spend(records_left)?;
                let step = SignerProbe::read(r, pos);
                self.items.insert(pos, step);
                step
            };
            let Some(step) = step.filter(|step| step.end <= end) else { break };
            if step.readable {
                return Some(true);
            }
            pos = step.end;
        }
        Some(false)
    }

    /// None means the shared work or cache limit ran out, never that an unprobed signer is absent.
    fn has_readable_signer(&mut self, r: &Reader<'_>, start: usize, cd: usize, records_left: &mut usize) -> Option<bool> {
        let Some(pairs_end) = cd.checked_sub(24) else { return Some(false) };
        let Some(mut pos) = start.checked_add(8) else { return Some(false) };
        for _ in 0..MAX_PAIRS {
            if pos >= pairs_end {
                break;
            }
            let step = if let Some(step) = self.pairs.get(&pos) {
                *step
            } else {
                self.spend(records_left)?;
                let step = PairProbe::read(r, pos);
                self.pairs.insert(pos, step);
                step
            };
            let Some(step) = step.filter(|step| step.end <= pairs_end) else { break };
            let readable = if let Some(readable) = step.readable {
                readable
            } else {
                let value_at = pos.checked_add(12)?;
                let value = r.slice(value_at, step.end.checked_sub(value_at)?)?;
                let readable = if let Some((signers, _)) = (Lp { data: value }).at(0) {
                    let first = value_at.checked_add(4)?;
                    self.sequence_has_signer(r, first, first.checked_add(signers.len())?, records_left)?
                } else {
                    false
                };
                self.pairs.insert(pos, Some(PairProbe { end: step.end, readable: Some(readable) }));
                readable
            };
            if readable {
                return Some(true);
            }
            pos = step.end;
        }
        Some(false)
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().take(128).map(|x| format!("{x:02x}")).collect()
}

fn algorithm_ids(data: &[u8]) -> Option<Vec<u32>> {
    let mut ids = Vec::new();
    for item in Lp::complete_items(data)? {
        let id = Reader::new(item).u32_le(0)?;
        let (_, end) = (Lp { data: item }).at(4)?;
        if end != item.len() {
            return None;
        }
        ids.push(id);
    }
    (!ids.is_empty()).then_some(ids)
}

fn verification_fields(scheme: &str, data: &[u8]) -> Result<(), &'static str> {
    const INCOMPLETE: &str = "the APK signer has incomplete required fields";
    let l = Lp { data };
    let (signed, mut pos) = l.at(0).ok_or(INCOMPLETE)?;
    let outer_sdk = if scheme == "v2" {
        None
    } else {
        let r = Reader::new(data);
        let sdk = (r.u32_le(pos).ok_or(INCOMPLETE)?, r.u32_le(pos + 4).ok_or(INCOMPLETE)?);
        pos += 8;
        Some(sdk)
    };
    let (signatures, next) = l.at(pos).ok_or(INCOMPLETE)?;
    let (_, end) = l.at(next).ok_or(INCOMPLETE)?;
    if end != data.len() {
        return Err(INCOMPLETE);
    }
    let sl = Lp { data: signed };
    let (digests, next) = sl.at(0).ok_or(INCOMPLETE)?;
    let (certificates, mut pos) = sl.at(next).ok_or(INCOMPLETE)?;
    let certificates = Lp::complete_items(certificates).ok_or(INCOMPLETE)?;
    if certificates.is_empty() || certificates.iter().any(|cert| cert.is_empty() || cert.len() > MAX_CERT) {
        return Err("the APK signer's certificate list cannot be checked completely");
    }
    if let Some(outer) = outer_sdk {
        let r = Reader::new(signed);
        let inner = (r.u32_le(pos).ok_or(INCOMPLETE)?, r.u32_le(pos + 4).ok_or(INCOMPLETE)?);
        if inner != outer {
            return Err("the APK signer's SDK range differs from its signed copy");
        }
        pos += 8;
    }
    let (attributes, end) = sl.at(pos).ok_or(INCOMPLETE)?;
    if end != signed.len() || Lp::complete_items(attributes).ok_or(INCOMPLETE)?.iter().any(|item| item.len() < 4) {
        return Err(INCOMPLETE);
    }
    if algorithm_ids(signatures).ok_or(INCOMPLETE)? != algorithm_ids(digests).ok_or(INCOMPLETE)? {
        return Err("the APK signature algorithm list differs from the signed digest list");
    }
    Ok(())
}

fn parse_signer(scheme: &'static str, s: &[u8], warnings: &mut Vec<String>) -> Option<Signer> {
    let l = Lp { data: s };
    let r = Reader::new(s);
    let (signed, mut pos) = l.at(0)?;
    let mut min_sdk = None;
    let mut max_sdk = None;
    if scheme != "v2" {
        min_sdk = r.u32_le(pos);
        max_sdk = r.u32_le(pos + 4);
        pos += 8;
    }
    let mut signature_algorithms = Vec::new();
    let mut signatures = Vec::new();
    if let Some((sigs, next)) = l.at(pos) {
        for item in Lp::items(sigs) {
            // Each signature item: u32 algorithm id, then a length-prefixed signature.
            if let Some(id) = Reader::new(item).u32_le(0) {
                signature_algorithms.push(format!("{} ({id:#06x})", algorithm_name(id)));
                if let Some((sig, _)) = (Lp { data: item }).at(4) {
                    signatures.push((id, sig.to_vec()));
                }
            }
        }
        pos = next;
    }
    let public_key = l.at(pos).map(|(k, _)| k.to_vec()).unwrap_or_default();
    let public_key_bytes = public_key.len();
    // signed data: digests, certificates, [v3: min, max], attributes
    let sl = Lp { data: signed };
    let mut digests = Vec::new();
    let mut signed_digests = Vec::new();
    let mut certificates = Vec::new();
    if let Some((ds, next)) = sl.at(0) {
        for item in Lp::items(ds) {
            let ir = Reader::new(item);
            if let (Some(id), Some((d, _))) = (ir.u32_le(0), (Lp { data: item }).at(4)) {
                digests.push(SignerDigest { algorithm: format!("{id:#06x}"), algorithm_name: algorithm_name(id), digest: hex(d) });
                signed_digests.push((id, d.to_vec()));
            }
        }
        if let Some((cs, _)) = sl.at(next) {
            for c in Lp::items(cs) {
                if c.len() > MAX_CERT {
                    warnings.push(format!("a {scheme} certificate is {} bytes; skipped", c.len()));
                    continue;
                }
                certificates.push(c.to_vec());
            }
        }
    }
    Some(Signer {
        scheme,
        certificates,
        digests,
        signature_algorithms,
        min_sdk,
        max_sdk,
        public_key_bytes,
        public_key,
        signed_data: signed.to_vec(),
        signatures,
        signed_digests,
        verification_error: verification_fields(scheme, s).err(),
    })
}

/// Find and parse the APK Signing Block. `Ok(None)` when the APK has none (v1-only or unsigned).
pub fn parse(data: &[u8]) -> Option<SigningBlock> {
    let r = Reader::new(data);
    let eocd = find_eocd(data)?;
    let cd = r.u32_le(eocd.offset + 16)? as usize;
    let (start, size_footer) = signing_block_extent(&r, cd)?;
    let mut block = SigningBlock { offset: start as u64, size: size_footer, ..SigningBlock::default() };
    if eocd.ambiguous {
        block.warnings.push("multiple ZIP end records contain readable APK signers; signing metadata is ambiguous".into());
    }
    if r.u64_le(start) != Some(size_footer) {
        block.warnings.push("the block's two size fields disagree".into());
    }
    let pairs_end = cd.checked_sub(24)?;
    let mut pos = start.checked_add(8)?;
    while pos < pairs_end && block.entries.len() < MAX_PAIRS {
        let Some(len) = r.u64_le(pos).and_then(|l| usize::try_from(l).ok()) else { break };
        let Some(next) = pos.checked_add(8).and_then(|p| p.checked_add(len)) else { break };
        if len < 4 || next > pairs_end {
            block.warnings.push(format!("malformed id-value pair at offset {pos}"));
            break;
        }
        let Some(id) = r.u32_le(pos + 8) else { break };
        block.entries.push(BlockEntry { id: format!("{id:#010x}"), name: block_name(id), size: len as u64 - 4 });
        let scheme = match id {
            V2_ID => Some("v2"),
            V3_ID => Some("v3"),
            V31_ID => Some("v3.1"),
            _ => None,
        };
        if let (Some(scheme), Some(value)) = (scheme, r.slice(pos + 12, len - 4)) {
            // value: length-prefixed sequence of length-prefixed signers
            if let Some((signers, _)) = (Lp { data: value }).at(0) {
                for s in Lp::items(signers) {
                    match parse_signer(scheme, s, &mut block.warnings) {
                        Some(sg) => block.signers.push(sg),
                        None => block.warnings.push(format!("a {scheme} signer could not be read")),
                    }
                }
            }
        }
        pos = next;
    }
    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_testkit::android as fx;

    #[test]
    fn directory_candidates_reuse_scanned_headers() {
        let mut zip = craft_anything_testkit::electron::zip_stored(&[("a", b"a"), ("b", b"b")]);
        let eocd = zip.len() - 22;
        let cd = Reader::new(&zip).u32_le(eocd + 16).unwrap() as usize;
        zip.extend_from_slice(&[0; CD_HEADER_LEN]);
        let r = Reader::new(&zip);
        let mut cache = DirectoryCache::new();
        let mut records_left = 3;
        assert_eq!(cache.framed(&r, cd, eocd - cd, 1, &mut records_left), Some(false));
        assert_eq!(records_left, 2);
        assert_eq!(cache.framed(&r, cd, eocd - cd, 2, &mut records_left), Some(true));
        assert_eq!(records_left, 1);
        assert_eq!(cache.framed(&r, cd, zip.len() - cd, 3, &mut records_left), Some(false));
        assert_eq!(records_left, 0);
        assert_eq!(cache.framed(&r, cd, zip.len() - cd, 3, &mut records_left), Some(false));
        assert_eq!(cache.framed(&r, cd, eocd - cd, 2, &mut records_left), Some(true));
        assert_eq!(records_left, 0);
    }

    #[test]
    fn directory_cache_keeps_sizes_and_offsets_separate() {
        let zip = craft_anything_testkit::electron::zip_stored(&[("a", b"a"), ("b", b"b")]);
        let r = Reader::new(&zip);
        let cd = r.u32_le(zip.len() - 6).unwrap() as usize;
        let record_len = CD_HEADER_LEN + 1;
        let mut cache = DirectoryCache::new();
        let mut records_left = 1;
        assert_eq!(cache.framed(&r, cd, CD_HEADER_LEN, 1, &mut records_left), Some(false));
        assert_eq!(records_left, 0);
        assert_eq!(cache.framed(&r, cd, record_len, 1, &mut records_left), Some(true));
        assert_eq!(cache.framed(&r, cd + record_len, record_len, 1, &mut records_left), None);
        records_left = 1;
        assert_eq!(cache.framed(&r, cd + record_len, record_len, 1, &mut records_left), Some(true));
        assert_eq!(records_left, 0);
    }

    #[test]
    fn directory_cache_rechecks_optional_signature_tails() {
        let zip = craft_anything_testkit::electron::zip_stored(&[("a", b"a"), ("b", b"b")]);
        let eocd = zip.len() - 22;
        let cd = Reader::new(&zip).u32_le(eocd + 16).unwrap() as usize;
        let mut directory = zip[cd..eocd].to_vec();
        let headers_len = directory.len();
        directory.extend_from_slice(b"PK\x05\x05\x03\x00sig");
        let signed_len = directory.len();
        directory.push(0);
        let r = Reader::new(&directory);
        let mut cache = DirectoryCache::new();
        let mut records_left = 2;
        assert_eq!(cache.framed(&r, 0, signed_len, 2, &mut records_left), Some(true));
        assert_eq!(cache.framed(&r, 0, headers_len, 2, &mut records_left), Some(true));
        assert_eq!(cache.framed(&r, 0, signed_len - 1, 2, &mut records_left), Some(false));
        assert_eq!(cache.framed(&r, 0, directory.len(), 2, &mut records_left), Some(false));
        assert_eq!(records_left, 0);
    }

    #[test]
    fn directory_cache_capacity_includes_each_start() {
        let bytes = [0; 2];
        let r = Reader::new(&bytes);
        let mut cache = DirectoryCache::new();
        cache.capacity_left = 2;
        let mut records_left = 0;
        assert_eq!(cache.framed(&r, 0, 0, 0, &mut records_left), Some(true));
        assert_eq!(cache.framed(&r, 1, 0, 0, &mut records_left), Some(true));
        assert_eq!(cache.capacity_left, 0);
        assert_eq!(cache.framed(&r, 2, 0, 0, &mut records_left), None);
        assert_eq!(cache.framed(&r, 0, 0, 0, &mut records_left), Some(true));
        assert_eq!(cache.prefixes.values().map(|prefix| prefix.boundaries.capacity()).sum::<usize>(), 2);
        assert_eq!(records_left, 0);
    }

    #[test]
    fn eocd_lookup_rejects_an_exhausted_record_budget() {
        let apk = fx::notes_apk();
        let eocd = apk.len() - 22;
        let count = usize::from(Reader::new(&apk).u16_le(eocd + 10).unwrap());
        assert!(count > 0);
        assert_eq!(find_eocd_with_budget(&apk, count + 2).map(|candidate| candidate.offset), Some(eocd));
        assert!(find_eocd_with_budget(&apk, count + 1).is_none());
        assert!(find_eocd_with_budget(&apk, count - 1).is_none());
    }

    fn lp(value: &[u8]) -> Vec<u8> {
        let mut out = (value.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(value);
        out
    }

    fn probe_fixture(pairs: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (id, value) in pairs {
            body.extend_from_slice(&((value.len() + 4) as u64).to_le_bytes());
            body.extend_from_slice(&id.to_le_bytes());
            body.extend_from_slice(value);
        }
        let size = (body.len() + 24) as u64;
        let mut block = size.to_le_bytes().to_vec();
        block.extend_from_slice(&body);
        block.extend_from_slice(&size.to_le_bytes());
        block.extend_from_slice(MAGIC);
        fx::with_signing_block(&craft_anything_testkit::electron::zip_stored(&[]), &block)
    }

    fn assert_probe_matches_parser(pairs: &[(u32, Vec<u8>)], expected: bool) {
        let apk = probe_fixture(pairs);
        let mut records_left = EOCD_SEARCH;
        assert_eq!(ProbeCache::new().has_readable_signer(&Reader::new(&apk), 0, apk.len() - 22, &mut records_left), Some(expected));
        assert_eq!(!parse(&apk).unwrap().signers.is_empty(), expected);
    }

    #[test]
    fn signer_probe_reuses_records_across_candidate_extents() {
        let apk = fx::notes_apk();
        let r = Reader::new(&apk);
        let cd = r.u32_le(apk.len() - 6).unwrap() as usize;
        let (start, _) = signing_block_extent(&r, cd).unwrap();
        let first = start + 8;
        let full = first + 8 + usize::try_from(r.u64_le(first).unwrap()).unwrap() + 24;
        let mut cache = ProbeCache::new();
        let mut remaining = 2;
        assert_eq!(cache.has_readable_signer(&r, start, full - 1, &mut remaining), Some(false));
        assert_eq!(remaining, 1);
        assert_eq!(cache.has_readable_signer(&r, start, full, &mut remaining), Some(true));
        assert_eq!(remaining, 0);
        for _ in 0..300 {
            assert_eq!(cache.has_readable_signer(&r, start, full - 1, &mut remaining), Some(false));
            assert_eq!(cache.has_readable_signer(&r, start, cd, &mut remaining), Some(true));
        }
        assert_eq!((cache.pairs.len(), cache.items.len()), (1, 1));
    }

    #[test]
    fn cached_signer_items_respect_each_sequence_end() {
        let bytes = lp(&lp(&[]));
        let r = Reader::new(&bytes);
        let mut cache = ProbeCache::new();
        let mut remaining = 1;
        assert_eq!(cache.sequence_has_signer(&r, 0, bytes.len() - 1, &mut remaining), Some(false));
        assert_eq!(remaining, 0);
        assert_eq!(cache.sequence_has_signer(&r, 0, bytes.len(), &mut remaining), Some(true));
        assert_eq!(cache.sequence_has_signer(&r, 0, bytes.len() - 1, &mut remaining), Some(false));
        assert_eq!(cache.items.len(), 1);
    }

    #[test]
    fn an_unprobed_pair_is_not_marked_unreadable_when_resources_run_out() {
        let apk = fx::notes_apk();
        let r = Reader::new(&apk);
        let cd = r.u32_le(apk.len() - 6).unwrap() as usize;
        let (start, _) = signing_block_extent(&r, cd).unwrap();
        let mut cache = ProbeCache::new();
        cache.slots_left = 1;
        let mut remaining = 2;
        assert_eq!(cache.has_readable_signer(&r, start, cd, &mut remaining), None);
        assert_eq!(remaining, 1);
        assert_eq!((cache.pairs.len(), cache.items.len()), (1, 0));
        cache.slots_left = 1;
        assert_eq!(cache.has_readable_signer(&r, start, cd, &mut remaining), Some(true));
        assert_eq!(remaining, 0);
        assert_eq!(cache.slots_left, 0);
    }

    #[test]
    fn signer_probe_matches_the_parsers_recovery_boundaries() {
        let readable = lp(&lp(&lp(&[])));
        let bad_item = lp(&[u32::MAX.to_le_bytes().to_vec(), lp(&lp(&[]))].concat());
        for id in [V2_ID, V3_ID, V31_ID] {
            assert_probe_matches_parser(&[(id, lp(&lp(&[])))], false);
            assert_probe_matches_parser(&[(id, readable.clone())], true);
            assert_probe_matches_parser(&[(id, lp(&[lp(&[]), lp(&lp(&[]))].concat()))], true);
            assert_probe_matches_parser(&[(id, bad_item.clone())], false);
        }
        assert_probe_matches_parser(&[(V2_ID, vec![0xff; 4]), (V3_ID, readable.clone())], true);
        assert_probe_matches_parser(&[(V2_ID, bad_item), (V31_ID, readable)], true);
    }

    #[test]
    fn signer_probe_obeys_the_parsers_item_and_pair_limits() {
        let mut signers = lp(&[]).repeat(MAX_ITEMS);
        signers.extend_from_slice(&lp(&lp(&[])));
        assert_probe_matches_parser(&[(V2_ID, lp(&signers))], false);
        let mut pairs = vec![(0x1357_2468, Vec::new()); MAX_PAIRS];
        pairs.push((V2_ID, lp(&lp(&lp(&[])))));
        assert_probe_matches_parser(&pairs, false);

        // Reuse the ordinary limit fixture for many independent probe requests.
        let apk = probe_fixture(&pairs);
        let reader = Reader::new(&apk);
        let mut cache = ProbeCache::new();
        let mut remaining = MAX_PAIRS;
        for _ in 0..300 {
            assert_eq!(cache.has_readable_signer(&reader, 0, apk.len() - 22, &mut remaining), Some(false));
        }
        assert_eq!(remaining, 0);
        assert_eq!(cache.pairs.len(), MAX_PAIRS);
    }

    #[test]
    fn reads_v2_and_v3_signers() {
        let apk = fx::notes_apk();
        let b = parse(&apk).unwrap();
        assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        assert_eq!(b.entries.len(), 3);
        assert_eq!(b.entries[0].name, "signature scheme v2");
        assert_eq!(b.entries[1].name, "signature scheme v3");
        assert_eq!(b.signers.len(), 2);
        let v2 = &b.signers[0];
        assert_eq!(v2.scheme, "v2");
        assert_eq!(v2.certificates.len(), 1);
        assert_eq!(crate::der::certificate(&v2.certificates[0]).unwrap().subject, "CN=Craft Test, O=Craft Org");
        assert_eq!(v2.digests[0].algorithm_name, "RSASSA-PKCS1-v1_5 with SHA2-256");
        assert_eq!(v2.digests[0].digest, "aa".repeat(32));
        let v3 = &b.signers[1];
        assert_eq!((v3.min_sdk, v3.max_sdk), (Some(24), Some(0x7fff_ffff)));
        assert!(parse(&craft_anything_testkit::electron::zip_stored(&[("a", b"b")])).is_none());
    }

    #[test]
    fn hostile() {
        let apk = fx::notes_apk();
        for m in craft_anything_testkit::mutations(&apk) {
            let _ = parse(&m);
        }
        for len in 0..apk.len() {
            let _ = parse(&apk[..len]);
        }
        for seed in 0..32 {
            let mut n = craft_anything_testkit::noise(seed, 400);
            n.extend_from_slice(b"APK Sig Block 42PK\x05\x06");
            n.extend_from_slice(&[0; 18]);
            let _ = parse(&n);
        }
    }
}
