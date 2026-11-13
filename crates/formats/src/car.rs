//! Compiled asset catalogs (`Assets.car`): the BOM store (block index, named variables, B-trees)
//! and the catalog records inside it — the CAR header, extended metadata, key format, facet
//! names, appearance names and rendition headers. Pixel data is not decoded.

use crate::FormatError;
use crate::bytes::Reader;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

const MAX_BLOCKS: usize = 4_000_000;
const MAX_VARS: usize = 4096;
const MAX_TREE_ENTRIES: usize = 1_000_000;
const MAX_TREE_NODES: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bom {
    pub version: u32,
    pub block_count: u32,
    /// (address, length) per block id.
    #[serde(skip)]
    pub blocks: Vec<(u32, u32)>,
    pub vars: Vec<(String, u32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CarHeader {
    pub coreui_version: u32,
    pub storage_version: u32,
    pub storage_timestamp: u32,
    pub rendition_count: u32,
    pub main_version: String,
    pub version: String,
    pub uuid: String,
    pub associated_checksum: u32,
    pub schema_version: u32,
    pub color_space_id: u32,
    pub key_semantics: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Facet {
    pub name: String,
    pub attributes: BTreeMap<String, u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rendition {
    /// Key attributes by name.
    pub key: BTreeMap<String, u16>,
    pub facet: Option<String>,
    pub name: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub scale: Option<f32>,
    pub pixel_format: Option<String>,
    pub color_space: Option<u32>,
    pub layout: Option<u16>,
    pub flags: Option<u32>,
    pub payload_bytes: Option<u32>,
    pub value_bytes: u32,
    pub csi: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssetCatalog {
    pub bom: Bom,
    pub header: Option<CarHeader>,
    pub extended_metadata: Option<BTreeMap<String, String>>,
    pub key_format: Vec<String>,
    pub facets: Vec<Facet>,
    pub appearances: Vec<(String, u32)>,
    pub renditions: Vec<Rendition>,
    /// A declared tree could not be read completely; retained entries are a partial inventory.
    pub tree_incomplete: bool,
    pub warnings: Vec<String>,
}

/// Name of a rendition key attribute (best-effort names; unknown ones are `attribute-<n>`).
pub fn attribute_name(id: u32) -> String {
    match id {
        0 => "look",
        1 => "element",
        2 => "part",
        3 => "size",
        4 => "direction",
        5 => "placeholder",
        6 => "value",
        7 => "appearance",
        8 => "dimension1",
        9 => "dimension2",
        10 => "state",
        11 => "layer",
        12 => "scale",
        13 => "localization",
        14 => "presentation-state",
        15 => "idiom",
        16 => "subtype",
        17 => "identifier",
        18 => "previous-value",
        19 => "previous-state",
        20 => "horizontal-size-class",
        21 => "vertical-size-class",
        22 => "memory-level-class",
        23 => "graphics-feature-set-class",
        24 => "display-gamut",
        25 => "deployment-target",
        26 => "glyph-weight",
        27 => "glyph-size",
        o => return format!("attribute-{o}"),
    }
    .into()
}

fn fourcc(v: u32) -> String {
    v.to_be_bytes().iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { char::from(b) } else { '?' }).collect::<String>().trim_end().to_string()
}

fn fixed_str(b: &[u8]) -> String {
    let end = b.iter().position(|&x| x == 0).unwrap_or(b.len());
    String::from_utf8_lossy(b.get(..end).unwrap_or(b)).into_owned()
}

/// True for a BOM store.
pub fn is_bom(data: &[u8]) -> bool {
    data.starts_with(b"BOMStore")
}

/// Read the BOM container: header, block index and variables.
pub fn parse_bom(data: &[u8]) -> Result<Bom, FormatError> {
    if !is_bom(data) {
        return Err(FormatError::Malformed("no BOMStore header".into()));
    }
    let r = Reader::new(data);
    let t = || FormatError::Truncated("BOM header".into());
    let version = r.u32_be(8).ok_or_else(t)?;
    let block_count = r.u32_be(12).ok_or_else(t)?;
    let directory = |field, label: &str| {
        let off = r.u32_be(field).ok_or_else(t)? as usize;
        let len = r.u32_be(field + 4).ok_or_else(t)? as usize;
        r.slice(off, len).ok_or_else(|| FormatError::Truncated(label.into()))
    };
    let index = Reader::new(directory(16, "block index")?);
    let vars_table = Reader::new(directory(24, "variables")?);
    let short_index = || FormatError::Truncated("block index".into());
    let n = index.u32_be(0).ok_or_else(short_index)? as usize;
    if n > MAX_BLOCKS {
        return Err(FormatError::Malformed(format!("{n} blocks exceeds the block limit")));
    }
    if n > index.len().saturating_sub(4) / 8 {
        return Err(short_index());
    }
    let mut blocks = Vec::with_capacity(n);
    for i in 0..n {
        let a = index.u32_be(4 + i * 8).ok_or_else(short_index)?;
        let l = index.u32_be(8 + i * 8).ok_or_else(short_index)?;
        blocks.push((a, l));
    }
    let short_vars = || FormatError::Truncated("variables".into());
    let nv = vars_table.u32_be(0).ok_or_else(short_vars)? as usize;
    let mut vars = Vec::new();
    let mut p = 4;
    for _ in 0..nv.min(MAX_VARS) {
        let idx = vars_table.u32_be(p).ok_or_else(short_vars)?;
        let len = usize::from(vars_table.u8(p + 4).ok_or_else(short_vars)?);
        let name = vars_table.slice(p + 5, len).ok_or_else(short_vars)?;
        vars.push((String::from_utf8_lossy(name).into_owned(), idx));
        p += 5 + len;
    }
    Ok(Bom { version, block_count, blocks, vars })
}

impl Bom {
    pub fn block<'a>(&self, data: &'a [u8], id: u32) -> Option<&'a [u8]> {
        let (a, l) = *self.blocks.get(id as usize)?;
        Reader::new(data).slice(a as usize, l as usize)
    }

    pub fn var<'a>(&self, data: &'a [u8], name: &str) -> Option<&'a [u8]> {
        let id = self.vars.iter().find(|(n, _)| n == name)?.1;
        self.block(data, id)
    }

    /// Entries (key bytes, value bytes) of a BOM B-tree, in order.
    pub fn tree<'a>(&self, data: &'a [u8], name: &str, warnings: &mut Vec<String>) -> Vec<(&'a [u8], &'a [u8])> {
        self.tree_entries(data, name, warnings, &mut false)
    }

    fn tree_entries<'a>(&self, data: &'a [u8], name: &str, warnings: &mut Vec<String>, incomplete: &mut bool) -> Vec<(&'a [u8], &'a [u8])> {
        let mut out = Vec::new();
        let Some((_, id)) = self.vars.iter().find(|(n, _)| n == name) else { return out };
        let Some(t) = self.block(data, *id) else {
            tree_problem(warnings, incomplete, format_args!("{name}: missing tree block {id}"));
            return out;
        };
        let r = Reader::new(t);
        if t.get(..4) != Some(b"tree") {
            warnings.push(format!("{name} is not a tree"));
            return out;
        }
        let Some(mut node_id) = r.u32_be(8) else {
            tree_problem(warnings, incomplete, format_args!("{name}: tree header cut short"));
            return out;
        };
        if node_id == 0 {
            return out; // A null root is an empty tree, not a missing child.
        }
        let mut seen = HashSet::new();
        // Descend to the leftmost leaf.
        loop {
            if !seen.insert(node_id) || seen.len() > MAX_TREE_NODES {
                tree_problem(warnings, incomplete, format_args!("{name}: tree loops or exceeds the node limit"));
                return out;
            }
            let Some(n) = self.block(data, node_id) else {
                tree_problem(warnings, incomplete, format_args!("{name}: missing tree node {node_id}"));
                return out;
            };
            let Some((leaf, count, _)) = node_header(n) else {
                tree_problem(warnings, incomplete, format_args!("{name}: node {node_id} header cut short"));
                return out;
            };
            if leaf {
                break;
            }
            if count == 0 {
                return out;
            }
            let Some(child) = Reader::new(n).u32_be(12).filter(|child| *child != 0) else {
                tree_problem(warnings, incomplete, format_args!("{name}: node {node_id} has no readable child"));
                return out;
            };
            node_id = child;
        }
        // Walk leaves through their forward links.
        let mut leaves = HashSet::new();
        while node_id != 0 {
            if !leaves.insert(node_id) || leaves.len() > MAX_TREE_NODES {
                tree_problem(warnings, incomplete, format_args!("{name}: leaf chain loops or exceeds the node limit"));
                break;
            }
            let Some(n) = self.block(data, node_id) else {
                tree_problem(warnings, incomplete, format_args!("{name}: missing leaf {node_id}"));
                break;
            };
            let nr = Reader::new(n);
            let Some((leaf, count, next)) = node_header(n) else {
                tree_problem(warnings, incomplete, format_args!("{name}: leaf {node_id} header cut short"));
                break;
            };
            if !leaf {
                tree_problem(warnings, incomplete, format_args!("{name}: forward link reaches a non-leaf node {node_id}"));
                break;
            }
            for i in 0..count {
                let (Some(v), Some(k)) = (nr.u32_be(12 + i * 8), nr.u32_be(16 + i * 8)) else {
                    tree_problem(warnings, incomplete, format_args!("{name}: leaf {node_id} cut short"));
                    break;
                };
                match (self.block(data, k), self.block(data, v)) {
                    (Some(kb), Some(vb)) => out.push((kb, vb)),
                    _ => {
                        tree_problem(warnings, incomplete, format_args!("{name}: entry points at a missing block"));
                    }
                }
                if out.len() >= MAX_TREE_ENTRIES {
                    if i + 1 < count || next != 0 {
                        tree_problem(warnings, incomplete, format_args!("{name}: stopped at {MAX_TREE_ENTRIES} entries"));
                    }
                    return out;
                }
            }
            node_id = next;
        }
        out
    }
}

/// A node's fixed header, including its backward link even when only forward traversal is used.
fn node_header(data: &[u8]) -> Option<(bool, usize, u32)> {
    let r = Reader::new(data);
    r.slice(0, 12)?;
    Some((r.u16_be(0)? != 0, usize::from(r.u16_be(2)?), r.u32_be(4)?))
}

fn tree_problem(warnings: &mut Vec<String>, incomplete: &mut bool, message: std::fmt::Arguments<'_>) {
    *incomplete = true;
    if warnings.len() < 100 {
        warnings.push(message.to_string());
    }
}

/// Parse a compiled asset catalog.
pub fn parse(data: &[u8]) -> Result<AssetCatalog, FormatError> {
    let bom = parse_bom(data)?;
    let mut warnings = Vec::new();
    let mut tree_incomplete = false;
    let header = bom.var(data, "CARHEADER").and_then(|b| {
        let r = Reader::new(b);
        Some(CarHeader {
            coreui_version: r.u32_le(4)?,
            storage_version: r.u32_le(8)?,
            storage_timestamp: r.u32_le(12)?,
            rendition_count: r.u32_le(16)?,
            main_version: fixed_str(r.slice(20, 128)?),
            version: fixed_str(r.slice(148, 256)?),
            uuid: r.slice(404, 16)?.iter().map(|x| format!("{x:02X}")).collect(),
            associated_checksum: r.u32_le(420)?,
            schema_version: r.u32_le(424)?,
            color_space_id: r.u32_le(428)?,
            key_semantics: r.u32_le(432)?,
        })
    });
    if header.is_none() {
        warnings.push("no readable CARHEADER (this BOM may not be an asset catalog)".into());
    }
    let extended_metadata = bom.var(data, "EXTENDED_METADATA").map(|b| {
        let r = Reader::new(b);
        let mut m = BTreeMap::new();
        for (i, k) in ["thinning_arguments", "deployment_platform_version", "deployment_platform", "authoring_tool"].iter().enumerate() {
            if let Some(s) = r.slice(4 + i * 256, 256) {
                m.insert((*k).to_string(), fixed_str(s));
            }
        }
        m
    });
    let mut key_ids: Vec<u32> = bom
        .var(data, "KEYFORMAT")
        .map(|b| {
            let r = Reader::new(b);
            let n = r.u32_le(8).unwrap_or(0) as usize;
            (0..n.min(64)).map_while(|i| r.u32_le(12 + i * 4)).collect()
        })
        .unwrap_or_default();
    // A catalog built for a system that predates an attribute (display gamut before macOS 10.12)
    // keeps a stand-in in KEYFORMAT and the real attribute in KEYFORMATWORKAROUND: one
    // little-endian u32 per key slot, where a non-zero value replaces the slot's attribute.
    if let Some(b) = bom.var(data, "KEYFORMATWORKAROUND") {
        let r = Reader::new(b);
        for (i, id) in key_ids.iter_mut().enumerate() {
            if let Some(w) = r.u32_le(i * 4)
                && w != 0
            {
                *id = w;
            }
        }
    }
    let key_format: Vec<String> = key_ids.iter().map(|&i| attribute_name(i)).collect();
    let mut facets = Vec::new();
    let mut ident_to_facet: BTreeMap<u16, String> = BTreeMap::new();
    for (k, v) in bom.tree_entries(data, "FACETKEYS", &mut warnings, &mut tree_incomplete) {
        let name = String::from_utf8_lossy(k).trim_end_matches('\0').to_string();
        let r = Reader::new(v);
        let n = usize::from(r.u16_le(4).unwrap_or(0));
        let mut attributes = BTreeMap::new();
        for i in 0..n.min(64) {
            let (Some(a), Some(val)) = (r.u16_le(6 + i * 4), r.u16_le(8 + i * 4)) else { break };
            if a == 17 {
                ident_to_facet.insert(val, name.clone());
            }
            attributes.insert(attribute_name(u32::from(a)), val);
        }
        facets.push(Facet { name, attributes });
    }
    let appearances = bom
        .tree_entries(data, "APPEARANCEKEYS", &mut warnings, &mut tree_incomplete)
        .into_iter()
        .map(|(k, v)| {
            let r = Reader::new(v);
            (String::from_utf8_lossy(k).trim_end_matches('\0').to_string(), r.u32_le(0).or_else(|| r.u16_le(0).map(u32::from)).unwrap_or(0))
        })
        .collect();
    let ident_pos = key_ids.iter().position(|&i| i == 17);
    let mut renditions = Vec::new();
    for (k, v) in bom.tree_entries(data, "RENDITIONS", &mut warnings, &mut tree_incomplete) {
        let kr = Reader::new(k);
        let mut key = BTreeMap::new();
        for (i, id) in key_ids.iter().enumerate() {
            if let Some(val) = kr.u16_le(i * 2)
                && val != 0
            {
                key.insert(attribute_name(*id), val);
            }
        }
        let facet = ident_pos.and_then(|p| kr.u16_le(p * 2)).and_then(|id| ident_to_facet.get(&id).cloned());
        let r = Reader::new(v);
        let csi = v.get(..4) == Some(b"ISTC");
        let rd = if csi {
            Rendition {
                key,
                facet,
                name: r.slice(40, 128).map(fixed_str),
                width: r.u32_le(12),
                height: r.u32_le(16),
                scale: r.u32_le(20).map(|s| s as f32 / 100.0),
                pixel_format: r.u32_le(24).map(fourcc),
                color_space: r.u32_le(28).map(|c| c & 0xf),
                layout: r.u16_le(36),
                flags: r.u32_le(8),
                payload_bytes: r.u32_le(180),
                value_bytes: v.len() as u32,
                csi,
            }
        } else {
            Rendition {
                key,
                facet,
                name: None,
                width: None,
                height: None,
                scale: None,
                pixel_format: None,
                color_space: None,
                layout: None,
                flags: None,
                payload_bytes: None,
                value_bytes: v.len() as u32,
                csi,
            }
        };
        renditions.push(rd);
    }
    Ok(AssetCatalog { bom, header, extended_metadata, key_format, facets, appearances, renditions, tree_incomplete, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_testkit::apple as fx;

    #[test]
    fn bom_directories_cannot_read_past_their_declared_sizes() {
        let mut writer = fx::BomWriter::new();
        writer.var("first", b"asset".to_vec());
        writer.var("second", b"rendition".to_vec());
        let file = writer.finish();
        let bom = parse_bom(&file).unwrap();
        assert_eq!(bom.var(&file, "second"), Some(&b"rendition"[..]));
        for length_field in [20, 28] {
            let old = u32::from_be_bytes(file[length_field..length_field + 4].try_into().unwrap());
            let mut shortened = file.clone();
            shortened[length_field..length_field + 4].copy_from_slice(&(old - 1).to_be_bytes());
            assert!(parse_bom(&shortened).is_err(), "table length at {length_field}");
        }
        let mut beyond = file.clone();
        beyond[28..32].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(parse_bom(&beyond).is_err());
    }

    #[test]
    fn reads_a_catalog() {
        let c = parse(&fx::assets_car()).unwrap();
        let h = c.header.as_ref().unwrap();
        assert_eq!(h.rendition_count, 2);
        assert_eq!(h.main_version, "@(#)PROGRAM:CoreUI  PROJECT:CoreUI-1");
        assert_eq!(c.key_format, vec!["scale", "idiom", "identifier"]);
        assert_eq!(c.facets.len(), 2);
        assert_eq!(c.facets[0].name, "AppIcon");
        assert_eq!(c.renditions.len(), 2);
        let logo = c.renditions.iter().find(|r| r.facet.as_deref() == Some("Logo")).unwrap();
        assert_eq!(logo.name.as_deref(), Some("Logo@2x.png"));
        assert_eq!(logo.width, Some(64));
        assert_eq!(logo.scale, Some(2.0));
        assert_eq!(logo.pixel_format.as_deref(), Some("ARGB"));
        assert_eq!(logo.key["scale"], 2);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    /// What actool writes for a macOS 10.11 target with sRGB and Display P3 variants of one image:
    /// KEYFORMAT holds the placeholder attribute (5) where display gamut (24) belongs, and
    /// KEYFORMATWORKAROUND holds 24 for that slot. `workaround_len` cuts the workaround block.
    fn catalog_with_key_workaround(workaround_len: usize) -> Vec<u8> {
        use craft_anything_testkit::Buf;
        let mut w = fx::BomWriter::new();
        let mut kf = Buf::with_len(24);
        kf.put(0, b"tmfk");
        kf.u32(8, 3);
        kf.u32(12, 12);
        kf.u32(16, 17);
        kf.u32(20, 5);
        w.var("KEYFORMAT", kf.into_vec());
        let mut wa = Buf::with_len(12);
        wa.u32(8, 24);
        let mut wa = wa.into_vec();
        wa.truncate(workaround_len);
        w.var("KEYFORMATWORKAROUND", wa);
        let mut facet = Buf::with_len(10);
        facet.u16(4, 1);
        facet.u16(6, 17);
        facet.u16(8, 7);
        w.tree("FACETKEYS", vec![(b"Logo".to_vec(), facet.into_vec())]);
        let key = |gamut: u16| {
            let mut b = Buf::with_len(6);
            b.u16(0, 1);
            b.u16(2, 7);
            b.u16(4, gamut);
            b.into_vec()
        };
        w.tree("RENDITIONS", vec![(key(0), b"sRGB".to_vec()), (key(1), b"P3".to_vec())]);
        w.finish()
    }

    /// A catalog keyed like the system apps' symbol images: identifier, deployment target, glyph
    /// weight and glyph size, as `assetutil --info` lists them (`kCRThemeGlyphWeightName`,
    /// `kCRThemeGlyphSizeName` after `kCRThemeDeploymentTargetName`).
    #[test]
    fn glyph_weight_and_size_attributes_are_named() {
        use craft_anything_testkit::Buf;
        let mut w = fx::BomWriter::new();
        let mut kf = Buf::with_len(28);
        kf.put(0, b"tmfk");
        kf.u32(8, 4);
        for (i, id) in [17u32, 25, 26, 27].into_iter().enumerate() {
            kf.u32(12 + 4 * i, id);
        }
        w.var("KEYFORMAT", kf.into_vec());
        let mut key = Buf::with_len(8);
        key.u16(0, 7);
        key.u16(2, 6);
        key.u16(4, 4);
        key.u16(6, 2);
        w.tree("RENDITIONS", vec![(key.into_vec(), b"symbol".to_vec())]);
        let c = parse(&w.finish()).unwrap();
        assert_eq!(c.key_format, vec!["identifier", "deployment-target", "glyph-weight", "glyph-size"]);
        assert_eq!(c.renditions[0].key.get("glyph-weight"), Some(&4));
        assert_eq!(c.renditions[0].key.get("glyph-size"), Some(&2));
        assert_eq!(attribute_name(28), "attribute-28");
    }

    #[test]
    fn key_format_workaround_names_the_real_attribute() {
        let c = parse(&catalog_with_key_workaround(12)).unwrap();
        assert_eq!(c.key_format, vec!["scale", "identifier", "display-gamut"]);
        let p3 = &c.renditions[1];
        assert_eq!(p3.key.get("display-gamut"), Some(&1));
        assert!(!p3.key.contains_key("placeholder"), "{:?}", p3.key);
        assert_eq!(p3.facet.as_deref(), Some("Logo"));
        // Zero slots keep their attribute, and slots past the end of the block are not replaced.
        let cut = parse(&catalog_with_key_workaround(8)).unwrap();
        assert_eq!(cut.key_format, vec!["scale", "identifier", "placeholder"]);
    }

    #[test]
    fn hostile_inputs() {
        for s in [fx::assets_car(), catalog_with_key_workaround(12)] {
            for m in craft_anything_testkit::mutations(&s) {
                let _ = parse(&m);
            }
            for i in (0..s.len()).step_by(7) {
                let _ = parse(&s[..i]);
            }
        }
        for seed in 0..100 {
            let mut n = b"BOMStore".to_vec();
            n.extend(craft_anything_testkit::noise(seed, 256));
            let _ = parse(&n);
        }
    }
}
