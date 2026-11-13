//! Text sources (a script, Markdown, settings, a log) read as text, a page of lines at a time
//! (#539). Before this, a text file outside a build could only be searched byte by byte: the
//! native tools refuse it ("Unknown file magic") and `build_file_read` works only in a build.
//! Nothing is run, rendered or formatted.

use super::native_common::{params_json, spec};
use super::static_input::{self, builder};
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_evidence::{Authority, Confidence, Coverage, CoverageStatus, ErrorCode, Limitation, Location, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The tool's id, for triage's next step.
pub const TEXT_READ_TOOL: &str = "core_text_read";

pub fn specs() -> Vec<ToolSpec> {
    vec![spec(
        TEXT_READ_TOOL,
        "Read a text file",
        "Read a text file (a script, Markdown, settings, a log) as text: an absolute path, or the active target. Paged by lines with offset/limit (200 lines by default, at most 2000, and at most 256 KiB of text a page; coverage gives the next offset). Reads UTF-8, and UTF-16 with a byte-order mark; bytes that aren't valid text show as U+FFFD and are counted. A file with a zero byte in its first 8 KiB is refused as binary (use core_target_triage or native_bytes_find). A line longer than 256 KiB comes cut to its start. Files up to 64 MiB. Nothing is run.",
        "Read a text file, such as a script or notes, a page at a time.",
        Toolset::Core,
        Effects::EVIDENCE | Effects::CARRIES_TARGET_CONTENT,
        Requires::Nothing,
        schema_of::<TextReadParams>,
        core_text_read,
    )]
}

/// Largest file read.
const MAX_FILE: u64 = 64 << 20;
/// Most text returned in one page, and the longest line shown whole.
const PAGE_BYTES: usize = 256 * 1024;
/// How much of the start is looked at for zero bytes and invalid text.
const SNIFF: usize = 8 * 1024;
const DEFAULT_LINES: usize = 200;
const MAX_LINES: usize = 2000;

fn default_limit() -> usize {
    DEFAULT_LINES
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextReadParams {
    /// Absolute path of the file; the active target when omitted.
    #[serde(default)]
    pub path: Option<String>,
    /// First line to show, counted from 0.
    #[serde(default)]
    pub offset: usize,
    /// Most lines to show (1 to 2000).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// How the file's bytes were read as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Encoding {
    Utf8,
    /// UTF-8 with a byte-order mark (dropped from the text).
    Utf8Bom,
    Utf16le,
    Utf16be,
}

impl Encoding {
    fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 | Encoding::Utf8Bom => "UTF-8",
            Encoding::Utf16le => "UTF-16LE",
            Encoding::Utf16be => "UTF-16BE",
        }
    }
}

/// The file's text, how it was read, and how many invalid sequences became U+FFFD.
#[derive(Debug)]
struct Decoded {
    text: String,
    encoding: Encoding,
    replaced: usize,
}

/// Read `bytes` as text, or `None` when it has a zero byte early on and no UTF-16 mark (binary).
fn decode(bytes: &[u8]) -> Option<Decoded> {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        let (text, replaced) = utf8(rest);
        return Some(Decoded { text, encoding: Encoding::Utf8Bom, replaced });
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        let (text, replaced) = utf16(rest, u16::from_le_bytes);
        return Some(Decoded { text, encoding: Encoding::Utf16le, replaced });
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        let (text, replaced) = utf16(rest, u16::from_be_bytes);
        return Some(Decoded { text, encoding: Encoding::Utf16be, replaced });
    }
    if bytes.iter().take(SNIFF).any(|b| *b == 0) {
        return None;
    }
    let (text, replaced) = utf8(bytes);
    Some(Decoded { text, encoding: Encoding::Utf8, replaced })
}

fn utf8(bytes: &[u8]) -> (String, usize) {
    let mut text = String::with_capacity(bytes.len());
    let mut replaced = 0usize;
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        if !chunk.invalid().is_empty() {
            text.push(char::REPLACEMENT_CHARACTER);
            replaced = replaced.saturating_add(1);
        }
    }
    (text, replaced)
}

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> (String, usize) {
    let (pairs, rest) = bytes.as_chunks::<2>();
    let odd = !rest.is_empty();
    let units = pairs.iter().map(|p| unit(*p));
    let mut text = String::with_capacity(bytes.len() / 2);
    let mut replaced = usize::from(odd);
    for c in char::decode_utf16(units) {
        text.push(c.unwrap_or_else(|_| {
            replaced = replaced.saturating_add(1);
            char::REPLACEMENT_CHARACTER
        }));
    }
    if odd {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    (text, replaced)
}

/// One page of lines: the text, how many lines it holds, the 1-based numbers of lines cut to
/// their start, and whether the page's byte limit (not `limit`) ended it.
struct Page {
    text: String,
    lines: usize,
    cut: Vec<usize>,
    full: bool,
}

/// The page of `lines` from `start`. Takes the lines as they come, so a file of millions of short
/// lines is never held as a list.
fn page<'a>(lines: impl Iterator<Item = &'a str>, start: usize, limit: usize) -> Page {
    let mut out = Page { text: String::new(), lines: 0, cut: Vec::new(), full: false };
    for (i, line) in lines.enumerate().skip(start) {
        if out.lines >= limit {
            break;
        }
        if out.lines > 0 && out.text.len().saturating_add(line.len()) > PAGE_BYTES {
            out.full = true;
            break;
        }
        // A line longer than a page comes cut to one, so reading always moves on.
        let part = line.get(..line.floor_char_boundary(PAGE_BYTES)).unwrap_or(line);
        if part.len() < line.len() {
            out.cut.push(i.saturating_add(1));
        }
        out.text.push_str(part);
        out.text.push('\n');
        out.lines = out.lines.saturating_add(1);
    }
    out
}

fn core_text_read(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: TextReadParams = parse_params(params)?;
    if !(1..=MAX_LINES).contains(&p.limit) {
        return Err(ToolError::invalid(format!("limit is {}; give 1 to {MAX_LINES} lines", p.limit)));
    }
    let input = static_input::load(ctx, p.path.as_deref(), MAX_FILE)?;
    ctx.report(None, format!("Reading {} as text", input.name));
    let Some(d) = decode(&input.bytes) else {
        return Err(ToolError::new(ErrorCode::NotSupported, format!("{} has zero bytes in its first 8 KiB, so it's binary, not text.", input.name))
            .with_remediation("Run core_target_triage to see which tools fit it, or search it with native_bytes_find."));
    };
    ctx.cancel.check()?;

    let total = d.text.lines().count();
    let start = p.offset.min(total);
    let pg = page(d.text.lines(), start, p.limit);
    let end = start.saturating_add(pg.lines);

    let mut lims = vec![Limitation::new(
        "text_as_stored",
        "The file is read as text as it is stored: nothing is run, rendered or formatted, and text a program builds or downloads while it runs isn't here.",
    )];
    if d.replaced > 0 {
        lims.push(Limitation::new("invalid_text", format!("{} sequence(s) of bytes aren't valid {} and show as U+FFFD.", d.replaced, d.encoding.name())));
    }
    if !pg.cut.is_empty() {
        let lines = pg.cut.iter().map(usize::to_string).collect::<Vec<_>>().join(", ");
        lims.push(Limitation::new("long_lines_cut", format!("Line(s) {lines} are longer than 256 KiB, so only their start is shown.")));
    }
    if pg.full {
        lims.push(Limitation::new("page_size", format!("This page stopped at 256 KiB of text; carry on from offset {end}.")));
    }

    let summary = if total == 0 {
        format!("{} has no text.", input.name)
    } else if pg.lines == 0 {
        format!("{} has {total} line(s); none from line {}.", input.name, p.offset.saturating_add(1))
    } else {
        format!("{}: lines {}–{end} of {total} ({}).", input.name, start.saturating_add(1), d.encoding.name())
    };
    let mut coverage = if total == 0 { Coverage::complete(0) } else { Coverage::page(pg.lines as u64, total as u64, start as u64) };
    // Reaching the last line does not restore the contents omitted by the line-size limit.
    if !pg.cut.is_empty() {
        coverage.status = CoverageStatus::Partial;
    }
    let ev = builder(ctx, TEXT_READ_TOOL, input.subject.clone(), params_json(&p))
        .result(json!({
            "path": input.path,
            "file_size": input.bytes.len(),
            "encoding": d.encoding,
            "replaced_sequences": d.replaced,
            "total_lines": total,
            "offset": start,
            "lines": pg.lines,
            "more": end < total,
            "cut_lines": pg.cut,
            "text": pg.text,
        }))
        .summary(summary)
        .confidence(Confidence::Observed)
        .authority(Authority::OriginalFiles)
        .limitations(lims)
        .coverage(coverage)
        .location(Location::new("path", input.path.display().to_string()))
        .build()?;
    Ok(ToolOutput::Evidence(Box::new(ev)))
}

#[cfg(test)]
mod tests {
    use super::super::native_common::test_support::{call, call_err, engine_with};
    use super::{PAGE_BYTES, SNIFF};
    use crate::Engine;
    use crate::target::looks_like_text;
    use craft_anything_evidence::ErrorCode;
    use craft_anything_testkit::temp_file;
    use serde_json::json;

    fn engine() -> Engine {
        Engine::new(crate::testing::session())
    }

    /// #539: a Markdown file and a PowerShell script read as text, from a path.
    #[test]
    fn reads_markdown_and_a_script_as_text() {
        let e = engine();
        let md = temp_file("notes.md", b"# Notes app\n\nSaves every 30 seconds.\n");
        let v = call(&e, "core_text_read", json!({"path": md}));
        let r = &v["result"];
        assert_eq!(r["text"], "# Notes app\n\nSaves every 30 seconds.\n");
        assert_eq!(r["total_lines"], 3);
        assert_eq!(r["encoding"], "utf8");
        assert_eq!(r["more"], false);
        assert_eq!(v["coverage"]["status"], "complete");
        assert_eq!(v["confidence"], "observed");

        let ps = temp_file("backup.ps1", b"param([string]$Path)\r\nWrite-Host \"Backing up $Path\"\r\n");
        let r = &call(&e, "core_text_read", json!({"path": ps}))["result"];
        assert_eq!(r["text"], "param([string]$Path)\nWrite-Host \"Backing up $Path\"\n");
        assert_eq!(r["total_lines"], 2);
    }

    #[test]
    fn reads_the_active_target() {
        let e = engine_with("notes.txt", b"one\ntwo\n");
        assert_eq!(call(&e, "core_text_read", json!({}))["result"]["text"], "one\ntwo\n");
    }

    #[test]
    fn pages_by_lines_and_says_where_to_carry_on() {
        let e = engine();
        let body: String = (1..=10).map(|n| format!("line {n}\n")).collect();
        let p = temp_file("ten.txt", body.as_bytes());
        let v = call(&e, "core_text_read", json!({"path": p, "offset": 3, "limit": 4}));
        assert_eq!(v["result"]["text"], "line 4\nline 5\nline 6\nline 7\n");
        assert_eq!(v["result"]["more"], true);
        assert_eq!(v["coverage"]["status"], "partial");
        assert_eq!(v["coverage"]["next_offset"], 7);
        assert!(v["summary"].as_str().unwrap().contains("lines 4–7 of 10"));
        // Past the end: nothing shown, no error, and the total still given.
        let v = call(&e, "core_text_read", json!({"path": p, "offset": 50}));
        assert_eq!(v["result"]["lines"], 0);
        assert_eq!(v["result"]["offset"], 10);
        assert_eq!(v["result"]["total_lines"], 10);
        call_err(&e, "core_text_read", json!({"path": p, "limit": 0}));
        call_err(&e, "core_text_read", json!({"path": p, "limit": 2001}));
    }

    #[test]
    fn reads_utf16_and_a_utf8_mark() {
        let e = engine();
        let mut le = vec![0xFF, 0xFE];
        le.extend("héllo\nwörld\n".encode_utf16().flat_map(u16::to_le_bytes));
        let r = &call(&e, "core_text_read", json!({"path": temp_file("le.txt", &le)}))["result"];
        assert_eq!(r["text"], "héllo\nwörld\n");
        assert_eq!(r["encoding"], "utf16le");
        let mut be = vec![0xFE, 0xFF];
        be.extend("한글\n".encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(call(&e, "core_text_read", json!({"path": temp_file("be.txt", &be)}))["result"]["text"], "한글\n");
        let r = &call(&e, "core_text_read", json!({"path": temp_file("bom.txt", b"\xEF\xBB\xBFkey = 1\n")}))["result"];
        assert_eq!(r["text"], "key = 1\n");
        assert_eq!(r["encoding"], "utf8-bom");
    }

    #[test]
    fn invalid_bytes_are_counted_not_dropped() {
        let e = engine();
        let v = call(&e, "core_text_read", json!({"path": temp_file("bad.txt", b"caf\xE9 au lait\n\xFF\n")}));
        assert_eq!(v["result"]["text"], "caf\u{FFFD} au lait\n\u{FFFD}\n");
        assert_eq!(v["result"]["replaced_sequences"], 2);
        assert!(v["limitations"].as_array().unwrap().iter().any(|l| l["code"] == "invalid_text"));
        // A lone surrogate and an odd last byte in UTF-16.
        let mut odd = vec![0xFF, 0xFE, b'a', 0, 0x00, 0xD8];
        odd.push(b'z');
        let v = call(&e, "core_text_read", json!({"path": temp_file("odd.txt", &odd)}));
        assert_eq!(v["result"]["text"], "a\u{FFFD}\u{FFFD}\n");
        assert_eq!(v["result"]["replaced_sequences"], 2);
    }

    #[test]
    fn refuses_a_binary_file() {
        let e = engine();
        let err = call_err(&e, "core_text_read", json!({"path": temp_file("prog.bin", b"\x7fELF\x02\x01\x01\x00\x00\x00")}));
        assert_eq!(err.code, ErrorCode::NotSupported);
        assert!(err.remediation.unwrap_or_default().contains("native_bytes_find"));
    }

    #[test]
    fn an_empty_file_has_no_text() {
        let v = call(&engine(), "core_text_read", json!({"path": temp_file("empty.txt", b"")}));
        assert_eq!(v["result"]["total_lines"], 0);
        assert_eq!(v["coverage"]["status"], "complete");
        assert!(v["summary"].as_str().unwrap().contains("no text"));
    }

    #[test]
    fn a_page_stops_at_its_size_and_a_long_line_is_cut() {
        let e = engine();
        let line = "x".repeat(PAGE_BYTES / 2);
        let body = format!("{line}\n{line}\n{line}\n");
        let v = call(&e, "core_text_read", json!({"path": temp_file("wide.txt", body.as_bytes())}));
        assert_eq!(v["result"]["lines"], 1);
        assert_eq!(v["coverage"]["next_offset"], 1);
        assert!(v["limitations"].as_array().unwrap().iter().any(|l| l["code"] == "page_size"));

        // One line longer than a page, ending in a character that a byte cut would split.
        let long = format!("{}é tail\nnext\n", "y".repeat(PAGE_BYTES - 1));
        let v = call(&e, "core_text_read", json!({"path": temp_file("long.txt", long.as_bytes())}));
        assert_eq!(v["result"]["cut_lines"], json!([1]));
        assert_eq!(v["result"]["lines"], 1);
        assert_eq!(v["result"]["text"].as_str().unwrap().len(), PAGE_BYTES);
        assert_eq!(v["coverage"]["next_offset"], 1);
    }

    #[test]
    fn refuses_a_relative_path() {
        call_err(&engine(), "core_text_read", json!({"path": "notes.md"}));
    }

    #[test]
    fn clean_room_refuses_it() {
        let mut s = crate::testing::session();
        s.settings.deep_mode = false;
        let e = Engine::new(s);
        let p = temp_file("notes.md", b"# private\n");
        let err = call_err(&e, "core_text_read", json!({"path": p}));
        assert_eq!(err.details.unwrap_or_default()["reason"], "clean_room");
        // Deep mode on the same file reads it.
        assert_eq!(call(&engine(), "core_text_read", json!({"path": p}))["result"]["text"], "# private\n");
        assert!(!crate::clean_room::TOOLS.contains(&"core_text_read"));
    }

    #[test]
    fn tells_text_from_binary() {
        assert!(looks_like_text(b"# Notes\n\n- one\n"));
        assert!(looks_like_text("param() # 한글\r\n".as_bytes()));
        assert!(looks_like_text(b"\xFF\xFEa\x00"));
        assert!(!looks_like_text(b""));
        assert!(!looks_like_text(b"\x7fELF\x02\x01\x01\x00"));
        assert!(!looks_like_text(b"caf\xE9"));
        assert!(!looks_like_text(&[0x01, 0x02, 0x03, b'a', 0x04, 0x05]));
        // A character split by the end of the window still counts as text.
        let mut big = "a".repeat(SNIFF - 1).into_bytes();
        big.extend("é and more".as_bytes());
        assert!(looks_like_text(&big));
    }
}
