//! CPython compiled files (`.pyc`) and the marshal format inside them: the header (which Python,
//! how the file ties to its source) and, for releases whose code-object layout is known, the
//! compiled functions with their names and text. Nothing is executed and no bytecode is decoded.
//!
//! Marshal is untrusted input here: depth, object count, text kept and memory are all bounded, and
//! a shared reference can't make a small file expand, because walking it spends the same budget.

use crate::common::FormatError;
use serde::Serialize;
use std::rc::Rc;

/// Deepest nesting read or walked. Real files stay far below it (two levels per nested function).
pub const MAX_DEPTH: u16 = 200;
/// Most marshal objects one read may create.
pub const MAX_OBJECTS: usize = 5_000_000;
/// Most code objects, names and constants visited while turning marshal into [`CodeObject`]s.
const MAX_WALK: usize = 4_000_000;
/// Most text kept from one file's code objects.
const MAX_TEXT: usize = 64 << 20;
/// Longest constant kept whole; a longer one is cut and marked.
const MAX_STRING: usize = 4096;

/// How a release lays out a marshalled code object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeLayout {
    /// 3.8 to 3.10: `co_nlocals` is stored, `co_qualname` isn't.
    Py38,
    /// 3.11 to 3.14: `localsplus` tables, `co_qualname` and the exception table.
    Py311,
}

/// One CPython release line and the magic numbers it used.
#[derive(Debug, Clone, Copy)]
pub struct Release {
    pub version: &'static str,
    /// Lowest magic number of its development.
    pub first: u16,
    /// Lowest magic number a final release wrote. Python 3.5.0 to 3.5.2 wrote 3350 and 3.5.3 moved
    /// to 3351; every other line kept one number through all its releases.
    pub released: u16,
    /// The magic number of the latest release.
    pub last: u16,
    /// Length of the header in front of the marshal data.
    pub header: usize,
    /// How code objects are stored, when we are sure of it.
    pub layout: Option<CodeLayout>,
}

/// Every release line we recognise. Magic numbers below `released` belong to that line's
/// pre-releases, whose code-object layout may differ, so only releases are read past the header.
pub const RELEASES: &[Release] = &[
    Release { version: "2.5", first: 62071, released: 62131, last: 62131, header: 8, layout: None },
    Release { version: "2.6", first: 62151, released: 62161, last: 62161, header: 8, layout: None },
    Release { version: "2.7", first: 62171, released: 62211, last: 62211, header: 8, layout: None },
    Release { version: "3.0", first: 3000, released: 3131, last: 3131, header: 8, layout: None },
    Release { version: "3.1", first: 3141, released: 3151, last: 3151, header: 8, layout: None },
    Release { version: "3.2", first: 3160, released: 3180, last: 3180, header: 8, layout: None },
    Release { version: "3.3", first: 3190, released: 3230, last: 3230, header: 12, layout: None },
    Release { version: "3.4", first: 3250, released: 3310, last: 3310, header: 12, layout: None },
    Release { version: "3.5", first: 3320, released: 3350, last: 3351, header: 12, layout: None },
    Release { version: "3.6", first: 3360, released: 3379, last: 3379, header: 12, layout: None },
    Release { version: "3.7", first: 3390, released: 3394, last: 3394, header: 16, layout: None },
    Release { version: "3.8", first: 3400, released: 3413, last: 3413, header: 16, layout: Some(CodeLayout::Py38) },
    Release { version: "3.9", first: 3420, released: 3425, last: 3425, header: 16, layout: Some(CodeLayout::Py38) },
    Release { version: "3.10", first: 3430, released: 3439, last: 3439, header: 16, layout: Some(CodeLayout::Py38) },
    Release { version: "3.11", first: 3450, released: 3495, last: 3495, header: 16, layout: Some(CodeLayout::Py311) },
    Release { version: "3.12", first: 3500, released: 3531, last: 3531, header: 16, layout: Some(CodeLayout::Py311) },
    Release { version: "3.13", first: 3550, released: 3571, last: 3571, header: 16, layout: Some(CodeLayout::Py311) },
    Release { version: "3.14", first: 3600, released: 3627, last: 3627, header: 16, layout: Some(CodeLayout::Py311) },
];

impl Release {
    /// True when a final release of this line wrote `magic`, not a pre-release.
    pub fn is_release(&self, magic: u16) -> bool {
        (self.released..=self.last).contains(&magic)
    }
}

/// The release line a magic number belongs to.
pub fn release_for_magic(magic: u16) -> Option<&'static Release> {
    RELEASES.iter().find(|r| (r.first..=r.last).contains(&magic))
}

/// Header length for a known magic number; unknown releases keep the modern 16-byte form.
pub(crate) fn header_size(magic: u16) -> usize {
    release_for_magic(magic).map_or(16, |release| release.header)
}

/// The release line called this ("3.12").
pub fn release_named(version: &str) -> Option<&'static Release> {
    RELEASES.iter().find(|r| r.version == version)
}

/// How a `.pyc` says whether it is still up to date with its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Invalidation {
    /// Source modification time and size.
    Timestamp,
    /// A hash of the source that Python checks when importing.
    CheckedHash,
    /// A hash of the source that Python doesn't check.
    UncheckedHash,
}

/// The front of a `.pyc`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PycHeader {
    pub magic: u16,
    /// The Python release line, when the magic number is one we know.
    pub version: Option<&'static str>,
    /// True when a final release wrote this magic number, not a pre-release.
    pub is_release: bool,
    /// Length of the header in front of the marshal data.
    pub size: usize,
    /// The flags word (3.7 and later).
    pub flags: Option<u32>,
    pub invalidation: Invalidation,
    pub source_mtime: Option<u32>,
    pub source_size: Option<u32>,
    /// The source hash in hex (hash-based files).
    pub source_hash: Option<String>,
    /// Header features we do not understand; known fields and readable code are retained.
    pub warnings: Vec<String>,
}

/// Read a `.pyc` header. Fails when the file isn't one.
pub fn parse_header(data: &[u8]) -> Result<PycHeader, FormatError> {
    let short = || FormatError::Truncated("the pyc header".into());
    let front = data.get(..4).ok_or_else(short)?;
    let (b0, b1, b2, b3) = match front {
        [a, b, c, d] => (*a, *b, *c, *d),
        _ => return Err(short()),
    };
    if (b2, b3) != (0x0d, 0x0a) {
        return Err(FormatError::Malformed("no CPython magic number (it ends in a carriage return and line feed)".into()));
    }
    let magic = u16::from_le_bytes([b0, b1]);
    let release = release_for_magic(magic);
    // An unknown number is newer than our table, and every recent release uses the 16-byte form.
    let size = header_size(magic);
    if data.len() < size {
        return Err(short());
    }
    let word = |at: usize| data.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes).ok_or_else(short);
    let mut h = PycHeader {
        magic,
        version: release.map(|r| r.version),
        is_release: release.is_some_and(|r| r.is_release(magic)),
        size,
        flags: None,
        invalidation: Invalidation::Timestamp,
        source_mtime: None,
        source_size: None,
        source_hash: None,
        warnings: Vec::new(),
    };
    match size {
        8 => h.source_mtime = Some(word(4)?),
        12 => {
            h.source_mtime = Some(word(4)?);
            h.source_size = Some(word(8)?);
        }
        _ => {
            let flags = word(4)?;
            let unknown_flags = flags & !3;
            if unknown_flags != 0 {
                h.warnings.push(format!("undefined Python bytecode header flags: {unknown_flags:#x}"));
            }
            h.flags = Some(flags);
            if flags & 1 == 0 {
                h.source_mtime = Some(word(8)?);
                h.source_size = Some(word(12)?);
            } else {
                h.invalidation = if flags & 2 != 0 { Invalidation::CheckedHash } else { Invalidation::UncheckedHash };
                h.source_hash = data.get(8..16).map(|b| b.iter().map(|x| format!("{x:02x}")).collect());
            }
        }
    }
    Ok(h)
}

/// A value read from marshal data. Only what callers look at is kept.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// None, booleans, floats and other values nothing here reads.
    Other,
    Int(i64),
    Str(String),
    /// A bytes object's length.
    Bytes(usize),
    /// Tuple, list, set or frozenset.
    Seq(Vec<Rc<Object>>),
    Dict(Vec<(Rc<Object>, Rc<Object>)>),
    Code(Box<RawCode>),
}

/// A value and how deep it nests, counting what shared references bring in.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub value: Value,
    height: u16,
}

/// A code object's fields as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct RawCode {
    arg_count: i32,
    posonly_arg_count: i32,
    kwonly_arg_count: i32,
    flags: i32,
    first_line: i32,
    bytecode: Rc<Object>,
    consts: Rc<Object>,
    names: Rc<Object>,
    filename: Rc<Object>,
    name: Rc<Object>,
    qualname: Option<Rc<Object>>,
}

struct Marshal<'a> {
    data: &'a [u8],
    pos: usize,
    refs: Vec<Rc<Object>>,
    objects: usize,
    layout: Option<CodeLayout>,
}

fn leaf(value: Value) -> Rc<Object> {
    Rc::new(Object { value, height: 0 })
}

fn truncated() -> FormatError {
    FormatError::Truncated("the marshal data".into())
}

fn bad(what: &str) -> FormatError {
    FormatError::Malformed(format!("marshal data: {what}"))
}

impl<'a> Marshal<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], FormatError> {
        let end = self.pos.checked_add(n).ok_or_else(truncated)?;
        let out = self.data.get(self.pos..end).ok_or_else(truncated)?;
        self.pos = end;
        Ok(out)
    }

    fn byte(&mut self) -> Result<u8, FormatError> {
        let b = self.data.get(self.pos).copied().ok_or_else(truncated)?;
        self.pos += 1;
        Ok(b)
    }

    fn int(&mut self) -> Result<i32, FormatError> {
        let b: [u8; 4] = self.take(4)?.try_into().map_err(|_| truncated())?;
        Ok(i32::from_le_bytes(b))
    }

    /// A stored count, which can't be negative and can't promise more items than bytes remain.
    fn count(&mut self) -> Result<usize, FormatError> {
        let n = usize::try_from(self.int()?).map_err(|_| bad("a negative length"))?;
        if n > self.data.len().saturating_sub(self.pos) {
            return Err(truncated());
        }
        Ok(n)
    }

    fn text(&mut self, n: usize) -> Result<Value, FormatError> {
        Ok(Value::Str(String::from_utf8_lossy(self.take(n)?).into_owned()))
    }

    /// Keep a slot in the reference table for a value about to be read, as CPython does for
    /// containers (their members refer to later slots).
    fn reserve(&mut self, flagged: bool) -> Option<usize> {
        flagged.then(|| {
            self.refs.push(leaf(Value::Other));
            self.refs.len() - 1
        })
    }

    fn finish(&mut self, slot: Option<usize>, object: Rc<Object>) -> Rc<Object> {
        if let Some(cell) = slot.and_then(|i| self.refs.get_mut(i)) {
            *cell = Rc::clone(&object);
        }
        object
    }

    /// A value with no children: registered after it is read.
    fn simple(&mut self, flagged: bool, value: Value) -> Rc<Object> {
        let object = leaf(value);
        if flagged {
            self.refs.push(Rc::clone(&object));
        }
        object
    }

    fn sequence(&mut self, n: usize, slot: Option<usize>, depth: u16) -> Result<Rc<Object>, FormatError> {
        let mut items = Vec::new();
        let mut height = 0u16;
        for _ in 0..n {
            let item = self.object(depth + 1)?;
            height = height.max(item.height);
            items.push(item);
        }
        let object = container(Value::Seq(items), height)?;
        Ok(self.finish(slot, object))
    }

    fn object(&mut self, depth: u16) -> Result<Rc<Object>, FormatError> {
        if depth > MAX_DEPTH {
            return Err(bad("nested too deeply"));
        }
        self.objects += 1;
        if self.objects > MAX_OBJECTS {
            return Err(FormatError::TooLarge(format!("more than {MAX_OBJECTS} marshal objects")));
        }
        let tag = self.byte()?;
        let flagged = tag & 0x80 != 0;
        match tag & 0x7f {
            b'N' | b'F' | b'T' | b'S' | b'.' => Ok(leaf(Value::Other)),
            b'0' => Err(bad("a null value in the wrong place")),
            b'i' => {
                let v = self.int()?;
                Ok(self.simple(flagged, Value::Int(i64::from(v))))
            }
            b'I' => {
                let b: [u8; 8] = self.take(8)?.try_into().map_err(|_| truncated())?;
                Ok(self.simple(flagged, Value::Int(i64::from_le_bytes(b))))
            }
            b'l' => {
                let v = self.long()?;
                Ok(self.simple(flagged, v))
            }
            kind @ (b'f' | b'x') => {
                let parts = if kind == b'x' { 2 } else { 1 };
                for _ in 0..parts {
                    let n = usize::from(self.byte()?);
                    self.take(n)?;
                }
                Ok(self.simple(flagged, Value::Other))
            }
            b'g' => {
                self.take(8)?;
                Ok(self.simple(flagged, Value::Other))
            }
            b'y' => {
                self.take(16)?;
                Ok(self.simple(flagged, Value::Other))
            }
            b's' => {
                let n = self.count()?;
                self.take(n)?;
                Ok(self.simple(flagged, Value::Bytes(n)))
            }
            b't' | b'u' | b'a' | b'A' => {
                let n = self.count()?;
                let v = self.text(n)?;
                Ok(self.simple(flagged, v))
            }
            b'z' | b'Z' => {
                let n = usize::from(self.byte()?);
                let v = self.text(n)?;
                Ok(self.simple(flagged, v))
            }
            b'r' => {
                let i = usize::try_from(self.int()?).map_err(|_| bad("a negative reference"))?;
                self.refs.get(i).cloned().ok_or_else(|| bad("a reference to nothing"))
            }
            b'(' | b'[' | b'<' | b'>' => {
                let n = self.count()?;
                let slot = self.reserve(flagged);
                self.sequence(n, slot, depth)
            }
            b')' => {
                let n = usize::from(self.byte()?);
                let slot = self.reserve(flagged);
                self.sequence(n, slot, depth)
            }
            b':' => {
                let slot = self.reserve(flagged);
                self.sequence(3, slot, depth)
            }
            b'{' => self.dict(flagged, depth),
            b'c' => self.code(flagged, depth),
            other => Err(bad(&format!("an unknown type code {other:#04x}"))),
        }
    }

    fn long(&mut self) -> Result<Value, FormatError> {
        let n = self.int()?;
        let digits = usize::try_from(n.unsigned_abs()).map_err(|_| truncated())?;
        let raw = self.take(digits.checked_mul(2).ok_or_else(truncated)?)?;
        let mut value = Some(0i64);
        let (digit_pairs, _) = raw.as_chunks::<2>();
        for (i, pair) in digit_pairs.iter().enumerate() {
            let digit = i64::from(u16::from_le_bytes(*pair));
            if digit > 0x7fff {
                return Err(bad("a long integer digit exceeds 15 bits"));
            }
            if i + 1 == digits && digit == 0 {
                return Err(bad("a long integer has a zero leading digit"));
            }
            let shift = u32::try_from(i).ok().and_then(|i| i.checked_mul(15)).filter(|s| *s < 62);
            value = value.and_then(|v| {
                let shift = shift?;
                let part = digit.checked_shl(shift)?;
                if part >> shift != digit { None } else { v.checked_add(part) }
            });
        }
        Ok(match value {
            Some(v) => Value::Int(if n < 0 { -v } else { v }),
            None => Value::Other,
        })
    }

    fn dict(&mut self, flagged: bool, depth: u16) -> Result<Rc<Object>, FormatError> {
        let slot = self.reserve(flagged);
        let mut pairs = Vec::new();
        let mut height = 0u16;
        loop {
            if self.data.get(self.pos) == Some(&b'0') {
                self.pos += 1;
                break;
            }
            let key = self.object(depth + 1)?;
            let value = self.object(depth + 1)?;
            height = height.max(key.height).max(value.height);
            pairs.push((key, value));
        }
        let object = container(Value::Dict(pairs), height)?;
        Ok(self.finish(slot, object))
    }

    fn code(&mut self, flagged: bool, depth: u16) -> Result<Rc<Object>, FormatError> {
        let layout = self.layout.ok_or_else(|| FormatError::Unsupported("code objects of this Python release".into()))?;
        let slot = self.reserve(flagged);
        let arg_count = self.int()?;
        let posonly_arg_count = self.int()?;
        let kwonly_arg_count = self.int()?;
        if layout == CodeLayout::Py38 {
            self.int()?; // co_nlocals
        }
        self.int()?; // co_stacksize
        let flags = self.int()?;
        let child = |m: &mut Self| m.object(depth + 1);
        let bytecode = child(self)?;
        let consts = child(self)?;
        let names = child(self)?;
        let filename;
        let name;
        let mut qualname = None;
        let first_line;
        let mut parts = vec![Rc::clone(&bytecode), Rc::clone(&consts), Rc::clone(&names)];
        match layout {
            CodeLayout::Py38 => {
                for _ in 0..3 {
                    parts.push(child(self)?); // varnames, freevars, cellvars
                }
                filename = child(self)?;
                name = child(self)?;
                first_line = self.int()?;
                parts.push(child(self)?); // lnotab or linetable
            }
            CodeLayout::Py311 => {
                for _ in 0..2 {
                    parts.push(child(self)?); // localsplusnames, localspluskinds
                }
                filename = child(self)?;
                name = child(self)?;
                let q = child(self)?;
                parts.push(Rc::clone(&q));
                qualname = Some(q);
                first_line = self.int()?;
                for _ in 0..2 {
                    parts.push(child(self)?); // linetable, exceptiontable
                }
            }
        }
        parts.push(Rc::clone(&filename));
        parts.push(Rc::clone(&name));
        let height = parts.iter().map(|p| p.height).max().unwrap_or(0);
        let raw = RawCode { arg_count, posonly_arg_count, kwonly_arg_count, flags, first_line, bytecode, consts, names, filename, name, qualname };
        let object = container(Value::Code(Box::new(raw)), height)?;
        Ok(self.finish(slot, object))
    }
}

/// Wrap children into a value, refusing nesting that shared references would make too deep to walk
/// (or to drop) safely.
fn container(value: Value, child_height: u16) -> Result<Rc<Object>, FormatError> {
    let height = child_height.saturating_add(1);
    if height > MAX_DEPTH {
        return Err(bad("nested too deeply"));
    }
    Ok(Rc::new(Object { value, height }))
}

/// Read one marshalled value. `layout` says how code objects are stored; `None` refuses them.
pub fn read_value(data: &[u8], layout: Option<CodeLayout>) -> Result<Rc<Object>, FormatError> {
    Marshal { data, pos: 0, refs: Vec::new(), objects: 0, layout }.object(0)
}

/// One compiled module, class or function.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct CodeObject {
    pub name: String,
    /// Its dotted name (3.11 and later).
    pub qualname: Option<String>,
    pub filename: String,
    pub first_line: i32,
    pub arg_count: i32,
    pub posonly_arg_count: i32,
    pub kwonly_arg_count: i32,
    pub flags: i32,
    pub bytecode_size: usize,
    /// Names it refers to: globals, attributes, imports.
    pub names: Vec<String>,
    /// Text among its constants, including inside constant tuples.
    pub strings: Vec<String>,
    /// Functions and classes defined in it.
    pub children: Vec<CodeObject>,
}

/// What turning marshal values into code objects may spend.
struct Walk {
    steps: usize,
    text: usize,
    cut: bool,
}

impl Walk {
    fn step(&mut self) -> bool {
        if self.steps == 0 {
            self.cut = true;
            return false;
        }
        self.steps -= 1;
        true
    }

    fn string(&mut self, s: &str) -> Option<String> {
        let keep = (0..=MAX_STRING.min(s.len())).rev().find(|i| s.is_char_boundary(*i)).unwrap_or(0);
        let piece = s.get(..keep).unwrap_or("");
        let shortened = keep < s.len();
        self.cut |= shortened;
        let bytes = piece.len() + if shortened { '…'.len_utf8() } else { 0 };
        if self.text.saturating_add(bytes) > MAX_TEXT {
            self.cut = true;
            return None;
        }
        self.text += bytes;
        Some(if shortened { format!("{piece}…") } else { piece.to_string() })
    }

    fn strings_in(&mut self, o: &Object, out: &mut Vec<String>, children: &mut Vec<CodeObject>) {
        if !self.step() {
            return;
        }
        match &o.value {
            Value::Str(s) => out.extend(self.string(s)),
            Value::Seq(items) => {
                for item in items {
                    self.strings_in(item, out, children);
                }
            }
            Value::Code(c) => match self.code(c) {
                Ok(code) => children.push(code),
                Err(_) => self.cut = true,
            },
            _ => {}
        }
    }

    /// A name field, spending the text budget: many code objects can share one huge string.
    fn field(&mut self, o: &Object, what: &str) -> Result<String, FormatError> {
        let Value::Str(s) = &o.value else {
            return Err(bad(&format!("a code object's {what} isn't text")));
        };
        self.string(s).ok_or_else(|| FormatError::TooLarge(format!("more than {MAX_TEXT} bytes of text")))
    }

    fn code(&mut self, raw: &RawCode) -> Result<CodeObject, FormatError> {
        let bytecode_size = match raw.bytecode.value {
            Value::Bytes(n) => n,
            _ => return Err(bad("a code object's bytecode isn't bytes")),
        };
        let mut code = CodeObject {
            name: self.field(&raw.name, "name")?,
            qualname: raw.qualname.as_deref().map(|q| self.field(q, "qualified name")).transpose()?,
            filename: self.field(&raw.filename, "file name")?,
            first_line: raw.first_line,
            arg_count: raw.arg_count,
            posonly_arg_count: raw.posonly_arg_count,
            kwonly_arg_count: raw.kwonly_arg_count,
            flags: raw.flags,
            bytecode_size,
            ..CodeObject::default()
        };
        let mut no_children = Vec::new();
        self.strings_in(&raw.names, &mut code.names, &mut no_children);
        self.strings_in(&raw.consts, &mut code.strings, &mut code.children);
        Ok(code)
    }
}

/// Code objects read from marshal data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeRead {
    pub root: CodeObject,
    /// True when a limit stopped the walk, so names, text or functions are missing.
    pub cut_short: bool,
}

/// Read marshal data holding one code object, stored the way `layout` says.
pub fn read_code(data: &[u8], layout: CodeLayout) -> Result<CodeRead, FormatError> {
    let object = read_value(data, Some(layout))?;
    let Value::Code(raw) = &object.value else {
        return Err(bad("the top-level value isn't a code object"));
    };
    let mut walk = Walk { steps: MAX_WALK, text: 0, cut: false };
    let root = walk.code(raw)?;
    Ok(CodeRead { root, cut_short: walk.cut })
}

/// Everything read from one `.pyc`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PycFile {
    pub header: PycHeader,
    /// The compiled code, when this release's layout is known and it read cleanly.
    pub code: Option<CodeRead>,
    /// Why there is no `code`.
    pub code_missing: Option<String>,
}

/// Read a `.pyc`. Its header must be sound; trouble after the header is reported in `code_missing`.
pub fn read_pyc(data: &[u8]) -> Result<PycFile, FormatError> {
    let header = parse_header(data)?;
    let release = release_for_magic(header.magic);
    let layout = release.filter(|r| r.is_release(header.magic)).and_then(|r| r.layout);
    let (code, code_missing) = match (layout, release) {
        (Some(layout), _) => match data.get(header.size..).ok_or_else(truncated).and_then(|body| read_code(body, layout)) {
            Ok(code) => (Some(code), None),
            Err(e) => (None, Some(format!("the compiled code couldn't be read: {e}"))),
        },
        (None, Some(r)) if !r.is_release(header.magic) => (
            None,
            Some(format!(
                "magic number {} is from a pre-release of Python {}, whose code layout may differ, so only the header was read",
                header.magic, r.version
            )),
        ),
        (None, Some(r)) => (None, Some(format!("the code layout of Python {} isn't one we read, so only the header was read", r.version))),
        (None, None) => (None, Some(format!("magic number {} isn't a Python release we know, so only the header was read", header.magic))),
    };
    Ok(PycFile { header, code, code_missing })
}

/// A `.pyc` around bare marshalled code: the magic number and a timestamp-style header of zeros.
pub fn wrap_marshal(magic: u16, marshal: &[u8]) -> Vec<u8> {
    let header = header_size(magic);
    let mut out = Vec::with_capacity(marshal.len().saturating_add(header));
    out.extend(magic.to_le_bytes());
    out.extend([0x0d, 0x0a]);
    out.resize(header, 0);
    out.extend(marshal);
    out
}

/// A code object in a flat list, for paging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlatCode<'a> {
    pub index: usize,
    /// The `index` of the code object it's defined in.
    pub parent: Option<usize>,
    pub depth: usize,
    pub name: &'a str,
    pub qualname: Option<&'a str>,
    pub filename: &'a str,
    pub first_line: i32,
    pub arg_count: i32,
    pub posonly_arg_count: i32,
    pub kwonly_arg_count: i32,
    pub flags: i32,
    pub bytecode_size: usize,
    pub names: &'a [String],
    pub strings: &'a [String],
}

/// The tree of code objects as a list, parents before children.
pub fn flatten(root: &CodeObject) -> Vec<FlatCode<'_>> {
    let mut out = Vec::new();
    let mut stack = vec![(root, None, 0usize)];
    while let Some((c, parent, depth)) = stack.pop() {
        let index = out.len();
        out.push(FlatCode {
            index,
            parent,
            depth,
            name: &c.name,
            qualname: c.qualname.as_deref(),
            filename: &c.filename,
            first_line: c.first_line,
            arg_count: c.arg_count,
            posonly_arg_count: c.posonly_arg_count,
            kwonly_arg_count: c.kwonly_arg_count,
            flags: c.flags,
            bytecode_size: c.bytecode_size,
            names: &c.names,
            strings: &c.strings,
        });
        for child in c.children.iter().rev() {
            stack.push((child, Some(index), depth + 1));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_testkit::packed::{code_38, code_311, marshal as m, pyc};

    fn sample_38() -> Vec<u8> {
        let inner = code_38("helper", "app.py", 7, &["os", "path"], &["inner text"], &[]);
        code_38("<module>", "app.py", 1, &["helper"], &["module doc"], &[inner])
    }

    fn sample_311() -> Vec<u8> {
        let inner = code_311("helper", "Thing.helper", "app.py", 7, &["os"], &["inner text"], &[]);
        code_311("<module>", "<module>", "app.py", 1, &["Thing"], &["module doc"], &[inner])
    }

    #[test]
    fn the_magic_table_is_ordered_and_names_each_release() {
        for r in RELEASES {
            assert!(r.first <= r.released && r.released <= r.last, "{}", r.version);
        }
        for w in RELEASES.windows(2) {
            if let [a, b] = w
                && a.version.starts_with('3')
            {
                assert!(a.last < b.first, "{} and {} overlap", a.version, b.version);
            }
        }
        for (magic, version) in [(3413, "3.8"), (3425, "3.9"), (3439, "3.10"), (3495, "3.11"), (3531, "3.12"), (3571, "3.13"), (3627, "3.14"), (62211, "2.7")] {
            assert_eq!(release_for_magic(magic).map(|r| r.version), Some(version));
            assert_eq!(release_named(version).map(|r| r.last), Some(magic));
        }
        assert!(release_for_magic(9999).is_none());
    }

    #[test]
    fn undefined_modern_header_flags_warn_without_losing_code() {
        for (magic, body) in [(3413, sample_38()), (3531, sample_311())] {
            for unknown in (2..32).map(|bit| 1u32 << bit).chain([!3u32]) {
                for defined in 0..4 {
                    let flags = unknown | defined;
                    let data = pyc(magic, flags, [0; 8], &body);
                    let header = parse_header(&data).unwrap();
                    let file = read_pyc(&data).unwrap();
                    let known = read_pyc(&pyc(magic, defined, [0; 8], &body)).unwrap();
                    assert_eq!(header.flags, Some(flags));
                    assert_eq!(header.invalidation, known.header.invalidation);
                    assert_eq!(header.source_mtime, known.header.source_mtime);
                    assert_eq!(header.source_size, known.header.source_size);
                    assert_eq!(header.source_hash, known.header.source_hash);
                    assert_eq!(file.code, known.code);
                    assert_eq!(file.code_missing, None);
                    let json = serde_json::to_value(&header).unwrap();
                    assert_eq!(json["warnings"], serde_json::json!([format!("undefined Python bytecode header flags: {unknown:#x}")]));
                }
            }
        }
    }

    #[test]
    fn unknown_header_flags_preserve_header_only_reports() {
        for magic in [3394, 4000] {
            let file = read_pyc(&pyc(magic, 0x8000_0003, [0xab; 8], b"")).unwrap();
            assert_eq!(file.header.flags, Some(0x8000_0003));
            assert_eq!(file.header.invalidation, Invalidation::CheckedHash);
            assert_eq!(file.header.source_hash.as_deref(), Some("abababababababab"));
            assert!(file.code.is_none());
            assert!(file.code_missing.is_some());
            let json = serde_json::to_value(file.header).unwrap();
            assert_eq!(json["warnings"], serde_json::json!(["undefined Python bytecode header flags: 0x80000000"]));
        }
    }

    #[test]
    fn defined_header_flags_keep_their_invalidation_mode() {
        for (flags, invalidation) in
            [(0, Invalidation::Timestamp), (1, Invalidation::UncheckedHash), (2, Invalidation::Timestamp), (3, Invalidation::CheckedHash)]
        {
            let data = pyc(3531, flags, [0; 8], &sample_311());
            let file = read_pyc(&data).unwrap();
            assert_eq!(file.header.invalidation, invalidation);
            assert!(file.header.warnings.is_empty());
            assert!(file.code.is_some());
        }
    }

    #[test]
    fn legacy_header_timestamps_are_not_flag_words() {
        for magic in [62211u16, 3379] {
            let mut data = magic.to_le_bytes().to_vec();
            data.extend([0x0d, 0x0a]);
            data.extend(u32::MAX.to_le_bytes());
            data.extend([0; 4]);
            let header = parse_header(&data).unwrap();
            assert_eq!(header.flags, None);
            assert_eq!(header.source_mtime, Some(u32::MAX));
            assert!(header.warnings.is_empty());
        }
    }
    #[test]
    fn every_header_form_reads() {
        let new = pyc(3531, 0, [1, 0, 0, 0, 2, 0, 0, 0], b"");
        let h = parse_header(&new).unwrap();
        assert_eq!((h.version, h.is_release, h.size), (Some("3.12"), true, 16));
        assert_eq!((h.invalidation, h.source_mtime, h.source_size), (Invalidation::Timestamp, Some(1), Some(2)));

        let checked = parse_header(&pyc(3531, 3, [0xab, 0, 0, 0, 0, 0, 0, 0xcd], b"")).unwrap();
        assert_eq!(checked.invalidation, Invalidation::CheckedHash);
        assert_eq!(checked.source_hash.as_deref(), Some("ab000000000000cd"));
        assert_eq!(checked.source_mtime, None);
        assert_eq!(parse_header(&pyc(3531, 1, [0; 8], b"")).unwrap().invalidation, Invalidation::UncheckedHash);

        let mut p36 = 3379u16.to_le_bytes().to_vec();
        p36.extend([0x0d, 0x0a, 5, 0, 0, 0, 6, 0, 0, 0]);
        let h = parse_header(&p36).unwrap();
        assert_eq!((h.size, h.source_mtime, h.source_size, h.flags), (12, Some(5), Some(6), None));

        let mut p27 = 62211u16.to_le_bytes().to_vec();
        p27.extend([0x0d, 0x0a, 9, 0, 0, 0]);
        let h = parse_header(&p27).unwrap();
        assert_eq!((h.version, h.size, h.source_mtime, h.source_size), (Some("2.7"), 8, Some(9), None));

        assert!(matches!(parse_header(b"not a pyc file at all"), Err(FormatError::Malformed(_))));
    }

    #[test]
    fn code_objects_read_in_both_layouts() {
        for (magic, body) in [
            (3413u16, sample_38()),
            (3425, sample_38()),
            (3439, sample_38()),
            (3495, sample_311()),
            (3531, sample_311()),
            (3571, sample_311()),
            (3627, sample_311()),
        ] {
            let file = read_pyc(&pyc(magic, 0, [0; 8], &body)).unwrap();
            let code = file.code.unwrap_or_else(|| panic!("{magic}: {:?}", file.code_missing));
            assert!(!code.cut_short);
            assert_eq!(code.root.name, "<module>");
            assert_eq!(code.root.strings, ["module doc"]);
            let helper = &code.root.children[0];
            assert_eq!(helper.name, "helper");
            assert_eq!(helper.first_line, 7);
            assert_eq!(helper.strings, ["inner text"]);
            assert_eq!(helper.bytecode_size, 4);
            assert_eq!(helper.qualname.as_deref(), (magic >= 3495).then_some("Thing.helper"));
            let flat = flatten(&code.root);
            assert_eq!(flat.len(), 2);
            assert_eq!((flat[1].parent, flat[1].depth, flat[1].name), (Some(0), 1, "helper"));
        }
    }

    #[test]
    fn other_releases_stop_at_the_header_and_say_why() {
        for (magic, why) in [(3394u16, "isn't one we read"), (3530, "pre-release"), (4000, "isn't a Python release")] {
            let file = read_pyc(&pyc(magic, 0, [0; 8], &sample_311())).unwrap();
            assert!(file.code.is_none());
            assert!(file.code_missing.as_deref().unwrap().contains(why), "{magic}: {:?}", file.code_missing);
        }
    }

    #[test]
    fn python_3_5_0_to_3_5_2_files_are_from_a_release() {
        // 3.5.0, 3.5.1 and 3.5.2 wrote 3350, 3.5.3 and later 3351; 3340 is from a 3.5 beta.
        let file = |magic: u16| [&magic.to_le_bytes()[..], &[0x0d, 0x0a, 5, 0, 0, 0, 6, 0, 0, 0]].concat();
        for (magic, released) in [(3350u16, true), (3351, true), (3340, false)] {
            let h = parse_header(&file(magic)).unwrap();
            assert_eq!((h.version, h.size, h.is_release), (Some("3.5"), 12, released), "{magic}");
            let why = read_pyc(&file(magic)).unwrap().code_missing.unwrap();
            let expected = if released { "the code layout of Python 3.5 isn't one we read" } else { "pre-release" };
            assert!(why.contains(expected), "{magic}: {why}");
        }
    }

    #[test]
    fn every_truncation_is_an_answer_not_a_panic() {
        for full in [pyc(3531, 0, [0; 8], &sample_311()), pyc(3425, 0, [0; 8], &sample_38())] {
            for len in 0..full.len() {
                if let Ok(f) = read_pyc(&full[..len]) {
                    assert!(f.code.is_none(), "cut at {len} of {}", full.len());
                }
            }
            assert!(read_pyc(&full).unwrap().code.is_some());
        }
    }

    #[test]
    fn single_flipped_bytes_never_panic() {
        let full = pyc(3531, 0, [0; 8], &sample_311());
        for at in 0..full.len() {
            for v in [0u8, 0x7f, 0x80, 0xff] {
                let mut d = full.clone();
                d[at] = v;
                let _ = read_pyc(&d);
            }
        }
    }

    #[test]
    fn deep_nesting_is_refused() {
        let deep = [[b'[', 1, 0, 0, 0].repeat(100_000), vec![b'N']].concat();
        assert!(read_value(&deep, None).is_err());
        assert!(read_value(&m::list(vec![m::none(); 3]), None).is_ok());
    }

    #[test]
    fn references_cant_build_depth_past_the_limit() {
        // Each round nests 150 tuples around a reference to the previous round's outermost tuple:
        // fine to read, but the second round would be too deep to walk.
        const LEVELS: u32 = 150;
        let mut d = vec![b'[', 3, 0, 0, 0];
        for round in 0..3u32 {
            for _ in 0..LEVELS {
                d.extend([b'(' | 0x80, 1, 0, 0, 0]);
            }
            match round.checked_sub(1) {
                Some(previous) => {
                    d.push(b'r');
                    d.extend((previous * LEVELS).to_le_bytes());
                }
                None => d.push(b'N'),
            }
        }
        assert!(matches!(read_value(&d, None), Err(FormatError::Malformed(why)) if why.contains("too deeply")));
    }

    #[test]
    fn shared_references_cant_expand_without_end() {
        // Ten rounds of a tuple holding the previous round many times: small to store, huge to walk.
        let mut consts = vec![b'(' | 0x80, 11, 0, 0, 0];
        consts.push(b't' | 0x80);
        consts.extend(1u32.to_le_bytes());
        consts.push(b'x');
        for slot in 1..=10u32 {
            consts.extend([b'(' | 0x80, 200, 0, 0, 0]);
            for _ in 0..200 {
                consts.push(b'r');
                consts.extend(slot.to_le_bytes());
            }
        }
        let code = m::code_38_raw(&consts);
        let read = read_code(&code, CodeLayout::Py38).unwrap();
        assert!(read.cut_short);
        assert!(read.root.strings.len() <= MAX_WALK);
    }

    #[test]
    fn references_resolve_to_the_right_slot() {
        // Tuples take their slot before their members, so "b" sits in slot 2 and "a" in slot 1.
        let mut d = vec![b'(' | 0x80, 3, 0, 0, 0];
        d.extend([b'z' | 0x80, 1, b'a', b'z' | 0x80, 1, b'b', b'r', 2, 0, 0, 0]);
        let o = read_value(&d, None).unwrap();
        let Value::Seq(items) = &o.value else { panic!("not a sequence") };
        assert_eq!(items[2].value, Value::Str("b".into()));
        assert!(read_value(&[b'r', 9, 0, 0, 0], None).is_err());
        assert!(read_value(&[b'r', 0xff, 0xff, 0xff, 0xff], None).is_err());
    }

    #[test]
    fn hostile_lengths_are_refused_before_any_allocation() {
        for tag in *b"([<>stual" {
            for n in [i32::MAX, i32::MIN, -1] {
                let mut d = vec![tag];
                d.extend(n.to_le_bytes());
                assert!(read_value(&d, None).is_err(), "{tag} with length {n}");
            }
        }
    }

    #[test]
    fn object_count_is_capped() {
        // One tuple holding more members than the cap, each a single byte.
        let n = MAX_OBJECTS + 2;
        let mut d = vec![b'('];
        d.extend((n as i32).to_le_bytes());
        d.resize(n + 5, b'N');
        assert!(matches!(read_value(&d, None), Err(FormatError::TooLarge(_))));
    }

    #[test]
    fn values_of_every_kind_read() {
        let mut d = vec![b'[', 12, 0, 0, 0];
        d.extend(b"NTF.S");
        d.extend([b'i', 5, 0, 0, 0]);
        d.push(b'g');
        d.extend(1.5f64.to_le_bytes());
        d.extend([b'f', 3, b'1', b'.', b'5']);
        d.extend([b'l', 2, 0, 0, 0, 1, 0, 1, 0]);
        d.extend([b'l', 0xfe, 0xff, 0xff, 0xff, 1, 0, 1, 0]);
        d.extend([b'{', b'a', 1, 0, 0, 0, b'k', b'i', 1, 0, 0, 0, b'0']);
        d.extend([b's', 2, 0, 0, 0, 1, 2]);
        let o = read_value(&d, None).unwrap();
        let Value::Seq(items) = &o.value else { panic!("not a list") };
        assert_eq!(items[5].value, Value::Int(5));
        assert_eq!(items[8].value, Value::Int(1 + (1 << 15)));
        assert_eq!(items[9].value, Value::Int(-(1 + (1 << 15))));
        assert_eq!(items[11].value, Value::Bytes(2));
        let Value::Dict(pairs) = &items[10].value else { panic!("not a dict") };
        assert_eq!(pairs[0].0.value, Value::Str("k".into()));
        // A long too wide for an i64 isn't a number we can use, and isn't an error either.
        let mut wide = vec![b'l', 8, 0, 0, 0];
        wide.extend([0xff, 0x7f].repeat(8));
        assert_eq!(read_value(&wide, None).unwrap().value, Value::Other);
        assert!(read_value(b"c", None).is_err(), "code objects need a layout");
        assert!(read_value(&[0x7e], None).is_err());
    }

    #[test]
    fn shortened_constants_report_a_partial_code_inventory() {
        for text in ["x".repeat(MAX_STRING + 1), "界".repeat(MAX_STRING / 3 + 1)] {
            let body = code_311("module", "module", "app.py", 1, &[], &[&text], &[]);
            let read = read_code(&body, CodeLayout::Py311).unwrap();
            assert!(read.cut_short);
            let kept = &read.root.strings[0];
            assert!(kept.ends_with('…'));
            assert!(!kept.contains('\u{fffd}'));
        }
    }

    #[test]
    fn shortened_name_fields_report_a_partial_code_inventory() {
        let long = "n".repeat(MAX_STRING + 1);
        for (name, file, qualname) in [(&long[..], "app.py", "module"), ("module", &long[..], "module"), ("module", "app.py", &long[..])] {
            let body = code_311(name, qualname, file, 1, &[], &[], &[]);
            assert!(read_code(&body, CodeLayout::Py311).unwrap().cut_short);
        }
        let short = read_code(&sample_311(), CodeLayout::Py311).unwrap();
        assert!(!short.cut_short);
    }

    #[test]
    fn retained_text_budget_includes_the_truncation_marker() {
        let long = "x".repeat(MAX_STRING + 1);
        let mut too_small = Walk { steps: MAX_WALK, text: MAX_TEXT - MAX_STRING, cut: false };
        assert!(too_small.string(&long).is_none());
        assert!(too_small.cut);
        let mut exact = Walk { steps: MAX_WALK, text: MAX_TEXT - MAX_STRING - '…'.len_utf8(), cut: false };
        assert!(exact.string(&long).unwrap().ends_with('…'));
        assert_eq!(exact.text, MAX_TEXT);
        assert!(exact.cut);
        let mut whole = Walk { steps: MAX_WALK, text: MAX_TEXT - 2, cut: false };
        assert_eq!(whole.string("ok").as_deref(), Some("ok"));
        assert_eq!(whole.text, MAX_TEXT);
        assert!(!whole.cut);
    }
    #[test]
    fn long_constants_are_cut_on_a_character_boundary() {
        let long = "é".repeat(MAX_STRING);
        let body = code_311("m", "m", "f", 1, &[], &[&long], &[]);
        let read = read_code(&body, CodeLayout::Py311).unwrap();
        let s = &read.root.strings[0];
        assert!(s.ends_with('…') && s.len() <= MAX_STRING + '…'.len_utf8());
    }

    #[test]
    fn a_shared_huge_name_spends_the_text_budget() {
        // Many code objects naming one long string through a reference.
        let long = "n".repeat(MAX_STRING + 10);
        let read = read_code(&code_38(&long, "f", 1, &[], &[], &[]), CodeLayout::Py38).unwrap();
        assert!(read.root.name.ends_with('…') && read.root.name.len() <= MAX_STRING + '…'.len_utf8());
        let mut walk = Walk { steps: MAX_WALK, text: MAX_TEXT, cut: false };
        assert!(matches!(walk.field(&leaf(Value::Str("name".into())), "name"), Err(FormatError::TooLarge(_))));
    }

    #[test]
    fn a_wrapped_marshal_reads_as_a_pyc() {
        let file = read_pyc(&wrap_marshal(3531, &sample_311())).unwrap();
        assert_eq!(file.code.unwrap().root.name, "<module>");
    }
}
