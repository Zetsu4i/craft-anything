//! A Swift symbol demangler for the common manglings (`$s`, `_$s`, `$S`, `$e`, `_T0`, and runtime
//! class names `_Tt…`). The mangling is postfix: operands are pushed on a stack and operators pop
//! them. We cover identifiers with word substitutions, substitutions, standard types, nominal
//! types, extensions, bound generics with sugar, tuples, function types (throws/async),
//! generic parameters and simple signatures, functions, initialisers, deinitialisers, variables
//! and subscripts with accessors, closures, static members, metadata and witness-table globals
//! and common thunks. Anything else is reported as invalid rather than guessed.

use serde::Serialize;

const MAX_LEN: usize = 4096;
const MAX_STACK: usize = 1024;
const MAX_SUBST: usize = 4096;
const MAX_RENDER_DEPTH: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Demangled,
    /// Not a Swift mangling: returned as is.
    Unchanged,
    /// Looks like a Swift mangling but could not be decoded.
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Demangled {
    pub symbol: String,
    pub status: Status,
    pub demangled: Option<String>,
    pub reason: Option<String>,
}

/// What a function type says about itself besides its parameters and result.
#[derive(Debug, Clone, PartialEq, Default)]
struct FnAttrs {
    is_async: bool,
    /// `@Sendable`.
    sendable: bool,
    /// `@isolated(any)`.
    isolated_any: bool,
    throws: Throws,
    /// The global actor of `@Swift.MainActor`.
    actor: Option<Box<N>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
enum Throws {
    #[default]
    No,
    /// `throws`.
    Any,
    /// `throws(E)`.
    Typed(Box<N>),
}

/// A marker on the stack for a function type to pick up, from the letters after `Y`.
#[derive(Debug, Clone, PartialEq)]
enum FnMarker {
    Sendable,
    IsolatedAny,
    /// `Yc`: the global actor type it popped.
    Actor(Box<N>),
    /// `YK`: the error type it popped.
    Thrown(Box<N>),
}

#[derive(Debug, Clone, PartialEq)]
enum N {
    Ident(String),
    /// A private declaration's name with its file discriminator, as printed: `(name in _ABC…)`.
    Private(String),
    /// An operator's name and fixity, as printed: `== infix`.
    Operator(String),
    Module(String),
    Nominal {
        kind: char,
        ctx: Box<N>,
        name: String,
    },
    Extension {
        module: Box<N>,
        ty: Box<N>,
    },
    Bound {
        base: Box<N>,
        args: Vec<N>,
    },
    Tuple(Vec<(Option<String>, N)>),
    Func {
        params: Box<N>,
        result: Box<N>,
        attrs: FnAttrs,
    },
    Metatype(Box<N>),
    /// The metatype of an existential type (`Xp`), printed `.Type` like any metatype.
    ExistentialMetatype(Box<N>),
    /// A metatype written with a representation (`XM`). It prints `.Protocol` when it is of an
    /// existential and `.Type` otherwise, and puts a function or an ownership type in parentheses.
    ReprMetatype(Box<N>),
    Existential(Vec<N>),
    /// A class (or any type) and the protocols a value must also conform to: `NSView & Sendable`.
    ClassBound {
        class: Box<N>,
        protos: Vec<N>,
    },
    /// The type of `Self` in a class method that returns it. The type it was built from is not printed.
    DynamicSelf,
    /// A type with a word before it: `weak `, `@thick `, `@autoclosure `, `@convention(c) `.
    /// `wrapped` says whether an optional of it needs parentheses, `(weak A)?`.
    Attributed {
        text: &'static str,
        inner: Box<N>,
        wrapped: bool,
    },
    Param(u64, u64),
    Member {
        base: Box<N>,
        name: String,
    },
    Inout(Box<N>),
    /// The opaque result type of the declaration being mangled (`some P`), printed as `some`.
    Opaque,
    /// The declaration that returns opaque types (`QO`), printed `<<opaque return type of …>>`.
    OpaqueDecl(Box<N>),
    /// One of that declaration's opaque types, by position (`Qo`), printed with `.N` after it.
    OpaqueRef {
        decl: Box<N>,
        index: u64,
    },
    Builtin(String),
    EmptyList,
    FirstMarker,
    Throws,
    Async,
    FnMarker(FnMarker),
    Requirement(String),
    Sig {
        reqs: Vec<String>,
    },
    Generic {
        sig: Box<N>,
        ty: Box<N>,
    },
    /// Functions, initialisers, variables, subscripts, closures.
    Entity {
        kind: EKind,
        ctx: Box<N>,
        name: Option<String>,
        labels: Vec<Option<String>>,
        ty: Option<Box<N>>,
        accessor: Option<&'static str>,
        is_static: bool,
    },
    Conformance {
        ty: Box<N>,
        proto: Box<N>,
        module: Box<N>,
    },
    Global {
        prefix: String,
        inner: Box<N>,
        suffix: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EKind {
    Func,
    AllocInit,
    Init,
    Deinit,
    DeallocDeinit,
    Var,
    Subscript,
    Closure(u64, bool),
    DefaultArg(u64),
    VarInit,
    IvarDestroyer,
    IvarInitializer,
}

struct D<'a> {
    s: &'a [u8],
    pos: usize,
    stack: Vec<N>,
    subs: Vec<N>,
    words: Vec<String>,
    max_param: Option<u64>,
    /// Nodes we may still copy into or out of substitutions (guards against expansion).
    budget: usize,
}

type R<T> = Result<T, String>;

fn is_lower(c: u8) -> bool {
    c.is_ascii_lowercase()
}

impl<'a> D<'a> {
    fn new(s: &'a [u8]) -> Self {
        D { s, pos: 0, stack: Vec::new(), subs: Vec::new(), words: Vec::new(), max_param: None, budget: 200_000 }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn next(&mut self) -> R<u8> {
        let c = self.peek().ok_or("unexpected end")?;
        self.pos += 1;
        Ok(c)
    }

    fn next_if(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn natural(&mut self) -> Option<u64> {
        let start = self.pos;
        let mut v: u64 = 0;
        while let Some(c) = self.peek().filter(u8::is_ascii_digit) {
            v = v.checked_mul(10)?.checked_add(u64::from(c - b'0'))?;
            self.pos += 1;
        }
        if self.pos == start { None } else { Some(v) }
    }

    /// INDEX ::= '_' | NATURAL '_'  →  0 | n+1
    fn index(&mut self) -> R<u64> {
        if self.next_if(b'_') {
            return Ok(0);
        }
        let n = self.natural().ok_or("expected an index")?;
        if !self.next_if(b'_') {
            return Err("index not terminated".into());
        }
        n.checked_add(1).ok_or_else(|| "index overflow".into())
    }

    fn push(&mut self, n: N) -> R<()> {
        if self.stack.len() >= MAX_STACK {
            return Err("too many nodes".into());
        }
        self.stack.push(n);
        Ok(())
    }

    /// Records a node that later `A…` substitutions can refer to by position. The mangler numbers
    /// identifiers, nominal types, bound generic types (an `Sg` optional too) and associated types
    /// (`Qz`, `Qy`) in the order they appear, but not extensions, so every index after a missed or
    /// extra entry would pick the wrong node.
    fn add_sub(&mut self, n: &N) -> R<()> {
        if self.subs.len() >= MAX_SUBST {
            return Err("too many substitutions".into());
        }
        self.budget = self.budget.checked_sub(weight(n, 0)).ok_or("symbol expands too much")?;
        self.subs.push(n.clone());
        Ok(())
    }

    fn pop(&mut self) -> R<N> {
        self.stack.pop().ok_or_else(|| "operator without operand".into())
    }

    fn pop_if(&mut self, f: impl Fn(&N) -> bool) -> Option<N> {
        if self.stack.last().is_some_and(f) { self.stack.pop() } else { None }
    }

    fn pop_type(&mut self) -> R<N> {
        match self.pop()? {
            N::Ident(_)
            | N::Private(_)
            | N::Operator(_)
            | N::EmptyList
            | N::FirstMarker
            | N::Throws
            | N::Async
            | N::FnMarker(_)
            | N::Requirement(_)
            | N::Sig { .. } => Err("expected a type".into()),
            N::Module(m) => Err(format!("expected a type, found module {m}")),
            other => Ok(other),
        }
    }

    fn pop_context(&mut self) -> R<N> {
        match self.pop()? {
            N::Ident(i) => Ok(N::Module(i)),
            n @ (N::Module(_) | N::Nominal { .. } | N::Extension { .. } | N::Entity { .. } | N::Bound { .. }) => Ok(n),
            _ => Err("expected a context".into()),
        }
    }

    fn pop_name(&mut self) -> R<String> {
        match self.pop()? {
            N::Ident(i) => Ok(i),
            _ => Err("expected a name".into()),
        }
    }

    /// One protocol of a protocol list (`…_p`): a context and a name, with no `P` kind letter and
    /// no substitution slot of its own, or a type such as a standard protocol (`SH`).
    fn pop_protocol(&mut self) -> R<N> {
        if matches!(self.stack.last(), Some(N::Ident(_) | N::Private(_) | N::Operator(_))) {
            let name = self.pop_decl_name()?;
            let ctx = self.pop_context()?;
            return Ok(N::Nominal { kind: 'P', ctx: Box::new(ctx), name });
        }
        self.pop_type()
    }

    fn identifier(&mut self) -> R<N> {
        let mut has_words = false;
        let mut puny = false;
        if self.next_if(b'0') {
            if self.next_if(b'0') {
                puny = true;
            } else {
                has_words = true;
            }
        }
        if puny {
            return Err("punycode identifiers are not supported".into());
        }
        let mut id = String::new();
        loop {
            while has_words && self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                let c = self.next()?;
                let idx = if is_lower(c) {
                    usize::from(c - b'a')
                } else {
                    has_words = false;
                    usize::from(c - b'A')
                };
                let w = self.words.get(idx).ok_or("word substitution out of range")?;
                id.push_str(w);
            }
            if self.next_if(b'0') {
                break;
            }
            let n = self.natural().ok_or("expected identifier length")?;
            let n = usize::try_from(n).map_err(|_| "identifier too long")?;
            if n == 0 {
                return Err("empty identifier".into());
            }
            let end = self.pos.checked_add(n).ok_or("identifier too long")?;
            let slice = self.s.get(self.pos..end).ok_or("identifier runs past the end")?;
            self.pos = end;
            let text = std::str::from_utf8(slice).map_err(|_| "identifier is not UTF-8")?;
            id.push_str(text);
            // Record words for later word substitutions.
            let bytes = text.as_bytes();
            let mut start: Option<usize> = None;
            for i in 0..=bytes.len() {
                let c = bytes.get(i).copied().unwrap_or(0);
                if let Some(st) = start {
                    let prev = bytes.get(i.wrapping_sub(1)).copied().unwrap_or(0);
                    let end_word = c == b'_' || c == 0 || (!prev.is_ascii_uppercase() && c.is_ascii_uppercase());
                    if end_word {
                        if i - st >= 2
                            && self.words.len() < 26
                            && let Some(w) = text.get(st..i)
                        {
                            self.words.push(w.to_string());
                        }
                        start = None;
                    }
                }
                if start.is_none() && c != 0 && c != b'_' && !c.is_ascii_digit() {
                    start = Some(i);
                }
            }
            if !has_words {
                break;
            }
        }
        if id.is_empty() {
            return Err("empty identifier".into());
        }
        let n = N::Ident(id);
        self.add_sub(&n)?;
        Ok(n)
    }

    fn substitution(&mut self) -> R<()> {
        let mut repeat: Option<u64> = None;
        loop {
            let c = self.next()?;
            if c.is_ascii_digit() {
                self.pos -= 1;
                repeat = self.natural();
                if self.peek() == Some(b'_') {
                    self.pos += 1;
                    let idx = repeat.ok_or("bad substitution")?.checked_add(27).ok_or("overflow")?;
                    return self.push_sub(idx, 1);
                }
                continue;
            }
            if c == b'_' {
                return self.push_sub(26, 1);
            }
            if c.is_ascii_lowercase() {
                self.push_sub(u64::from(c - b'a'), repeat.take().unwrap_or(1))?;
                continue;
            }
            if c.is_ascii_uppercase() {
                return self.push_sub(u64::from(c - b'A'), repeat.unwrap_or(1));
            }
            return Err("bad substitution".into());
        }
    }

    fn push_sub(&mut self, idx: u64, count: u64) -> R<()> {
        let n = usize::try_from(idx).ok().and_then(|i| self.subs.get(i)).ok_or("substitution out of range")?;
        if count > 64 {
            return Err("substitution repeat too large".into());
        }
        let cost = weight(n, 0).saturating_mul(usize::try_from(count).unwrap_or(usize::MAX));
        self.budget = self.budget.checked_sub(cost).ok_or("symbol expands too much")?;
        if count == 0 {
            return Ok(());
        }
        let n = n.clone();
        for _ in 1..count {
            self.push(n.clone())?;
        }
        self.push(n)
    }

    fn standard(&mut self) -> R<()> {
        let count = self.natural().unwrap_or(1);
        if count > 64 {
            return Err("standard type repeat too large".into());
        }
        let c = self.next()?;
        let (kind, name) = match c {
            b'g' => {
                let t = self.pop_type()?;
                // `Sg` spells `Optional<T>`; like any other bound generic type it takes a slot.
                let n = N::Bound { base: Box::new(Self::swift_enum("Optional")), args: vec![t] };
                self.add_sub(&n)?;
                return self.push(n);
            }
            b'o' => return self.push(N::Module("__C".into())),
            b'C' => return self.push(N::Module("__C_Synthesized".into())),
            b'a' => ('V', "Array"),
            b'b' => ('V', "Bool"),
            b'c' => Self::concurrency_type(self.next()?)?,
            b'D' => ('V', "Dictionary"),
            b'd' => ('V', "Double"),
            b'f' => ('V', "Float"),
            b'h' => ('V', "Set"),
            b'I' => ('V', "DefaultIndices"),
            b'i' => ('V', "Int"),
            b'J' => ('V', "Character"),
            b'N' => ('V', "ClosedRange"),
            b'n' => ('V', "Range"),
            b'O' => ('V', "ObjectIdentifier"),
            b'P' => ('V', "UnsafePointer"),
            b'p' => ('V', "UnsafeMutablePointer"),
            b'Q' => ('P', "Equatable"),
            b'q' => ('O', "Optional"),
            b'R' => ('V', "UnsafeBufferPointer"),
            b'r' => ('V', "UnsafeMutableBufferPointer"),
            b'S' => ('V', "String"),
            b's' => ('V', "Substring"),
            b'u' => ('V', "UInt"),
            b'V' => ('V', "UnsafeRawPointer"),
            b'v' => ('V', "UnsafeMutableRawPointer"),
            b'W' => ('V', "UnsafeRawBufferPointer"),
            b'w' => ('V', "UnsafeMutableRawBufferPointer"),
            b'A' => ('V', "AutoreleasingUnsafeMutablePointer"),
            b'B' => ('P', "BinaryFloatingPoint"),
            b'E' => ('P', "Encodable"),
            b'e' => ('P', "Decodable"),
            b'F' => ('P', "FloatingPoint"),
            b'G' => ('P', "RandomNumberGenerator"),
            b'H' => ('P', "Hashable"),
            b'j' => ('P', "Numeric"),
            b'K' => ('P', "BidirectionalCollection"),
            b'k' => ('P', "RandomAccessCollection"),
            b'L' => ('P', "Comparable"),
            b'l' => ('P', "Collection"),
            b'M' => ('P', "MutableCollection"),
            b'm' => ('P', "RangeReplaceableCollection"),
            b'T' => ('P', "Sequence"),
            b't' => ('P', "IteratorProtocol"),
            b'U' => ('P', "UnsignedInteger"),
            b'X' => ('P', "RangeExpression"),
            b'x' => ('P', "Strideable"),
            b'Y' => ('P', "RawRepresentable"),
            b'y' => ('P', "StringProtocol"),
            b'Z' => ('P', "SignedInteger"),
            b'z' => ('P', "BinaryInteger"),
            other => return Err(format!("unknown standard type S{}", char::from(other))),
        };
        let n = N::Nominal { kind, ctx: Box::new(N::Module("Swift".into())), name: name.into() };
        for _ in 0..count {
            self.push(n.clone())?;
        }
        Ok(())
    }

    /// The type behind `Sc` and one more letter, as `(kind, name)`. A letter with no type is an error.
    fn concurrency_type(c: u8) -> R<(char, &'static str)> {
        Ok(match c {
            b'A' => ('P', "Actor"),
            b'C' => ('V', "CheckedContinuation"),
            b'c' => ('V', "UnsafeContinuation"),
            b'E' => ('V', "CancellationError"),
            b'e' => ('V', "UnownedSerialExecutor"),
            b'F' => ('P', "Executor"),
            b'f' => ('P', "SerialExecutor"),
            b'G' => ('V', "TaskGroup"),
            b'g' => ('V', "ThrowingTaskGroup"),
            b'h' => ('P', "TaskExecutor"),
            b'I' => ('P', "AsyncIteratorProtocol"),
            b'i' => ('P', "AsyncSequence"),
            b'J' => ('V', "UnownedJob"),
            b'M' => ('C', "MainActor"),
            b'P' => ('V', "TaskPriority"),
            b'S' => ('V', "AsyncStream"),
            b's' => ('V', "AsyncThrowingStream"),
            b'T' => ('V', "Task"),
            b't' => ('V', "UnsafeCurrentTask"),
            other => return Err(format!("unknown concurrency standard type Sc{}", char::from(other))),
        })
    }

    fn swift_enum(name: &str) -> N {
        N::Nominal { kind: 'O', ctx: Box::new(N::Module("Swift".into())), name: name.into() }
    }

    fn pop_params(&mut self) -> R<N> {
        match self.pop()? {
            N::EmptyList => Ok(N::Tuple(Vec::new())),
            N::Ident(_)
            | N::Private(_)
            | N::Operator(_)
            | N::FirstMarker
            | N::Throws
            | N::Async
            | N::FnMarker(_)
            | N::Requirement(_)
            | N::Sig { .. }
            | N::Module(_) => Err("expected parameter types".into()),
            other => Ok(other),
        }
    }

    /// The markers a function type takes from the top of the stack, in the one order the mangling
    /// has them (from the top): a global actor or `@isolated(any)`, then `throws`, then
    /// `@Sendable`, then `async`. Each is optional and none repeats.
    fn pop_fn_attrs(&mut self) -> FnAttrs {
        let mut a = FnAttrs::default();
        if let Some(N::FnMarker(m)) = self.pop_if(|n| matches!(n, N::FnMarker(FnMarker::Actor(_) | FnMarker::IsolatedAny))) {
            match m {
                FnMarker::Actor(t) => a.actor = Some(t),
                _ => a.isolated_any = true,
            }
        }
        match self.pop_if(|n| matches!(n, N::Throws | N::FnMarker(FnMarker::Thrown(_)))) {
            Some(N::FnMarker(FnMarker::Thrown(t))) => a.throws = Throws::Typed(t),
            Some(_) => a.throws = Throws::Any,
            None => {}
        }
        a.sendable = self.pop_if(|n| matches!(n, N::FnMarker(FnMarker::Sendable))).is_some();
        a.is_async = self.pop_if(|n| matches!(n, N::Async)).is_some();
        a
    }

    fn pop_func_type(&mut self) -> R<N> {
        let attrs = self.pop_fn_attrs();
        let params = self.pop_params()?;
        let result = self.pop_params()?;
        Ok(N::Func { params: Box::new(params), result: Box::new(result), attrs })
    }

    /// The protocols of an existential, nothing for `Any`, or a list whose first entry
    /// is followed by a `_` marker, as for a tuple. Shared by `p` and the `X` types built on it.
    fn protocol_list(&mut self) -> R<Vec<N>> {
        let mut protos = Vec::new();
        if self.pop_if(|n| matches!(n, N::EmptyList)).is_none() {
            loop {
                if protos.len() > 32 {
                    return Err("too many protocols".into());
                }
                let first = self.pop_if(|n| matches!(n, N::FirstMarker)).is_some();
                protos.push(self.pop_protocol()?);
                if first {
                    break;
                }
            }
            protos.reverse();
        }
        Ok(protos)
    }

    /// The function type of `XE` and its relatives: the parameters and the result, then the markers
    /// of any function type. `attr` is the word printed before the type, if any.
    fn special_func(&mut self, attr: &'static str) -> R<N> {
        let f = self.pop_func_type()?;
        Ok(if attr.is_empty() { f } else { N::Attributed { text: attr, inner: Box::new(f), wrapped: true } })
    }

    /// `X` and a kind letter. The function kinds take parameters and a result like `c` does and
    /// print an attribute before them: `E` none, `K` and `A` `@autoclosure`, `C` `@convention(c)`,
    /// `B` `@convention(block)`, `f` `@convention(thin)`. `l` is `AnyObject` after a protocol list,
    /// `c` a class and a protocol list, `p` the metatype of a protocol, `D` the dynamic `Self`, `w`
    /// `o` `u` the ownership of a type, and `M` / `m` a metatype followed by its representation.
    fn special_type(&mut self) -> R<()> {
        let n = match self.next()? {
            b'E' => self.special_func("")?,
            b'K' | b'A' => self.special_func("@autoclosure ")?,
            b'C' => self.special_func("@convention(c) ")?,
            b'B' => self.special_func("@convention(block) ")?,
            b'f' => self.special_func("@convention(thin) ")?,
            b'l' => {
                let mut protos = self.protocol_list()?;
                protos.push(N::Nominal { kind: 'P', ctx: Box::new(N::Module("Swift".into())), name: "AnyObject".into() });
                N::Existential(protos)
            }
            b'c' => {
                let class = self.pop_type()?;
                let protos = self.protocol_list()?;
                N::ClassBound { class: Box::new(class), protos }
            }
            b'p' => N::ExistentialMetatype(Box::new(self.pop_type()?)),
            b'D' => {
                self.pop_type()?;
                N::DynamicSelf
            }
            b'w' => N::Attributed { text: "weak ", inner: Box::new(self.pop_type()?), wrapped: true },
            b'o' => N::Attributed { text: "unowned ", inner: Box::new(self.pop_type()?), wrapped: true },
            b'u' => N::Attributed { text: "unowned(unsafe) ", inner: Box::new(self.pop_type()?), wrapped: true },
            b'b' => N::Attributed { text: "@box ", inner: Box::new(self.pop_type()?), wrapped: false },
            k @ (b'M' | b'm') => {
                let text = match self.next()? {
                    b'T' => "@thick ",
                    b't' => "@thin ",
                    b'o' => "@objc_metatype ",
                    _ => return Err("unsupported metatype representation".into()),
                };
                let t = Box::new(self.pop_type()?);
                let meta = if k == b'M' { N::ReprMetatype(t) } else { N::ExistentialMetatype(t) };
                N::Attributed { text, inner: Box::new(meta), wrapped: false }
            }
            _ => return Err("unsupported special type".into()),
        };
        self.push(n)
    }

    fn param_count(f: &N) -> usize {
        match f {
            N::Func { params, .. } => match params.as_ref() {
                N::Tuple(t) => t.len(),
                _ => 1,
            },
            N::Generic { ty, .. } => Self::param_count(ty),
            _ => 0,
        }
    }

    fn pop_labels(&mut self, n: usize) -> R<Vec<Option<String>>> {
        if self.pop_if(|x| matches!(x, N::EmptyList)).is_some() {
            return Ok(vec![None; n]);
        }
        let mut out = Vec::new();
        for _ in 0..n {
            match self.stack.last() {
                Some(N::Ident(_)) => {
                    if let Some(N::Ident(l)) = self.stack.pop() {
                        out.push(Some(l));
                    }
                }
                Some(N::FirstMarker) => {
                    self.stack.pop();
                    out.push(None);
                }
                _ => return Err("expected argument labels".into()),
            }
        }
        out.reverse();
        Ok(out)
    }

    /// The function type of an entity, wrapped in its generic signature when one is present.
    /// Whether the stack holds an entity's (signature, labels, name, context) in one of the two
    /// layouts: a separate result and parameter list, or one function type built with `c`.
    fn shape_ok(&self, top_func: bool, need_name: bool) -> bool {
        let st = &self.stack;
        let at = |i: usize| i.checked_sub(1).and_then(|j| st.get(j));
        let typeish = |n: Option<&N>| matches!(n, Some(n) if !matches!(n, N::Ident(_) | N::Private(_) | N::Operator(_) | N::FirstMarker | N::Throws | N::Async | N::FnMarker(_) | N::Requirement(_) | N::Sig { .. } | N::Module(_)));
        let mut i = st.len();
        if matches!(at(i), Some(N::Sig { .. })) {
            i -= 1;
        }
        let n_params = if top_func {
            match at(i) {
                Some(f @ N::Func { .. }) => {
                    i -= 1;
                    Self::param_count(f)
                }
                _ => return false,
            }
        } else {
            while matches!(at(i), Some(N::Throws | N::Async | N::FnMarker(_))) {
                i -= 1;
            }
            let n = match at(i) {
                Some(N::EmptyList) => 0,
                Some(N::Tuple(t)) => t.len(),
                p if typeish(p) => 1,
                _ => return false,
            };
            i -= 1;
            if !typeish(at(i)) {
                return false;
            }
            i -= 1;
            n
        };
        if n_params > 0 {
            if matches!(at(i), Some(N::EmptyList)) {
                i -= 1;
            } else {
                for _ in 0..n_params {
                    if matches!(at(i), Some(N::Ident(_) | N::FirstMarker)) {
                        i -= 1;
                    } else {
                        return false;
                    }
                }
            }
        }
        if need_name {
            if !matches!(at(i), Some(N::Ident(_) | N::Private(_) | N::Operator(_))) {
                return false;
            }
            i -= 1;
        }
        matches!(at(i), Some(N::Ident(_) | N::Module(_) | N::Nominal { .. } | N::Extension { .. } | N::Entity { .. } | N::Bound { .. }))
    }

    fn pop_entity_type(&mut self, need_name: bool) -> R<N> {
        let top_func = !self.shape_ok(false, need_name) && self.shape_ok(true, need_name);
        let sig = self.pop_if(|n| matches!(n, N::Sig { .. }));
        let f = if top_func { self.pop()? } else { self.pop_func_type()? };
        Ok(match sig {
            Some(s) => N::Generic { sig: Box::new(s), ty: Box::new(f) },
            None => f,
        })
    }

    fn accessor(&mut self) -> R<Option<&'static str>> {
        let Some(c) = self.peek() else { return Ok(None) };
        let a = match c {
            b'g' => "getter",
            b's' => "setter",
            b'G' => "global getter",
            b'w' => "willset",
            b'W' => "didset",
            b'm' => "materializeForSet",
            b'M' => "modify",
            b'r' => "read",
            b'a' | b'l' => return self.addressor().map(Some),
            b'i' => "init",
            b'p' => "",
            _ => return Ok(None),
        };
        self.pos += 1;
        Ok(Some(a))
    }

    /// `w` and two letters: one value witness function of a type, the code the runtime calls to
    /// copy, move, destroy or read the enum tag of a value of that type. The operand must be a
    /// whole type; a module, a name, a member or another global before it is not one.
    fn value_witness(&mut self) -> R<()> {
        let name = match (self.next()?, self.next()?) {
            (b'a', b'l') => "allocateBuffer",
            (b'c', b'a') => "assignWithCopy",
            (b'c', b'p') => "initializeWithCopy",
            (b'd', b'e') => "deallocateBuffer",
            (b'e', b't') => "getEnumTagSinglePayload",
            (b'p', b'r') => "projectBuffer",
            (b's', b't') => "storeEnumTagSinglePayload",
            (b't', b'a') => "assignWithTake",
            (b't', b'k') => "initializeWithTake",
            (b't', b'T') => "initializeArrayWithTakeBackToFront",
            (b'u', b'g') => "getEnumTag",
            (b'u', b'i') => "destructiveInjectEnumTag",
            (b'u', b'p') => "destructiveProjectEnumData",
            (b'x', b'g') => "getExtraInhabitantIndex",
            (b'x', b's') => "storeExtraInhabitant",
            (b'x', b'x') => "destroy",
            (b'C', b'c') => "initializeArrayWithCopy",
            (b'C', b'p') => "initializeBufferWithCopy",
            (b'C', b'P') => "initializeBufferWithCopyOfBuffer",
            (b'T', b'k') => "initializeBufferWithTake",
            (b'T', b't') => "initializeArrayWithTakeFrontToBack",
            (b'T', b'K') => "initializeBufferWithTakeOfBuffer",
            (b'X', b'x') => "destroyArray",
            (b'X', b'X') => "destroyBuffer",
            _ => return Err("unsupported value witness kind".into()),
        };
        if matches!(self.stack.last(), Some(N::Global { .. } | N::Entity { .. } | N::Conformance { .. } | N::Extension { .. })) {
            return Err("value witness of a non-type".into());
        }
        let ty = self.pop_type()?;
        self.push(N::Global { prefix: format!("{name} value witness for"), inner: Box::new(ty), suffix: String::new() })
    }

    /// An addressor: `a` (mutable) or `l`, then a letter for its kind.
    fn addressor(&mut self) -> R<&'static str> {
        let mutable = self.next()? == b'a';
        Ok(match (mutable, self.next()?) {
            (true, b'u') => "unsafeMutableAddressor",
            (true, b'O') => "owningMutableAddressor",
            (true, b'o') => "nativeOwningMutableAddressor",
            (true, b'P') => "nativePinningMutableAddressor",
            (false, b'u') => "unsafeAddressor",
            (false, b'O') => "owningAddressor",
            (false, b'o') => "nativeOwningAddressor",
            (false, b'p') => "nativePinningAddressor",
            _ => return Err("unsupported addressor kind".into()),
        })
    }

    fn conformance(&mut self) -> R<N> {
        let module = self.pop_context()?;
        let proto = match self.pop()? {
            n @ N::Nominal { kind: 'P', .. } => n,
            N::Ident(name) | N::Private(name) | N::Operator(name) => {
                let ctx = self.pop_context()?;
                N::Nominal { kind: 'P', ctx: Box::new(ctx), name }
            }
            _ => return Err("expected a protocol".into()),
        };
        let ty = self.pop_type()?;
        Ok(N::Conformance { ty: Box::new(ty), proto: Box::new(proto), module: Box::new(module) })
    }

    fn global(&mut self, prefix: &str) -> R<()> {
        let inner = self.pop()?;
        self.push(N::Global { prefix: prefix.into(), inner: Box::new(inner), suffix: String::new() })
    }

    /// `TK` and `Tk`: the getter or the setter of a key path. The operand under the base is the
    /// property or subscript it reads, such as a property descriptor `vp` or `vpZ` or a subscript
    /// `ip`, and the base is the type the path starts from. A global, a module or a name in either
    /// place is not one, and `swift-demangle` rejects those too.
    fn key_path_accessor(&mut self, getter: bool) -> R<()> {
        if matches!(self.stack.last(), Some(N::Global { .. } | N::Entity { .. } | N::Conformance { .. } | N::Extension { .. })) {
            return Err("key path base is not a type".into());
        }
        let base = self.pop_type()?;
        let entity = match self.pop()? {
            e @ N::Entity { .. } => e,
            _ => return Err("key path of a non-entity".into()),
        };
        let phrase = if getter { "key path getter for" } else { "key path setter for" };
        let text = format!("{phrase} {} : {}", render(&entity, 0), render(&base, 0));
        self.push(N::Global { prefix: text, inner: Box::new(N::EmptyList), suffix: String::new() })
    }

    /// `Tu`, `TY` and `TQ`: the async function pointer, or the suspend or await resume function,
    /// of the function before it. The operand is a function, a type or another global (a thunk of
    /// a thunk is allowed); a bare name, a marker or a module is not.
    fn async_thunk(&mut self, prefix: String) -> R<()> {
        let inner = self.pop_type()?;
        self.push(N::Global { prefix, inner: Box::new(inner), suffix: String::new() })
    }

    /// `Tv` and an optional index: the outlined variable of the symbol before it, `#0` for `Tv_`
    /// and `#1` for `Tv0_`. The operand is a function, a type or a global, but not a bare name, a
    /// module or another outlined variable. `swift-demangle` stops at an index of 2^31 or more.
    fn outlined_variable(&mut self) -> R<()> {
        let i = self.index()?;
        if i >= 1 << 31 {
            return Err("outlined variable index too large".into());
        }
        if matches!(self.stack.last(), Some(N::Global { prefix, .. }) if prefix.starts_with("outlined variable")) {
            return Err("outlined variable of an outlined variable".into());
        }
        let inner = self.pop_type()?;
        self.push(N::Global { prefix: format!("outlined variable #{i} of"), inner: Box::new(inner), suffix: String::new() })
    }

    /// `WC`: the case's entity, such as `main.E.one(main.E.Type) -> main.E`, or a type. A module, a
    /// bare name or another global (a metadata or witness symbol) before it is not a case.
    fn enum_case(&mut self) -> R<()> {
        if matches!(self.stack.last(), Some(N::Global { .. })) {
            return Err("enum case of a global".into());
        }
        let inner = self.pop_type()?;
        self.push(N::Global { prefix: "enum case for".into(), inner: Box::new(inner), suffix: String::new() })
    }

    /// A whole type as the operand of a witness accessor. A module, a name, a member or another
    /// global (a metadata or witness symbol) before it is not one.
    fn witness_type(&mut self) -> R<N> {
        if matches!(self.stack.last(), Some(N::Global { .. } | N::Entity { .. } | N::Conformance { .. } | N::Extension { .. })) {
            return Err("witness operand is not a type".into());
        }
        self.pop_type()
    }

    /// A conformance as the new witness accessors read it: the module that declares it, then the
    /// protocol (a protocol type, or a name inside a module or a type), then a whole type. Unlike
    /// `conformance`, a type, a member or a global is not accepted as the module.
    fn witness_conformance(&mut self) -> R<N> {
        if matches!(self.stack.last(), Some(N::Sig { .. })) {
            return Err("a conformance with generic requirements is not supported".into());
        }
        let module = self.witness_module()?;
        let proto = match self.pop()? {
            n @ N::Nominal { kind: 'P', .. } => n,
            N::Ident(name) | N::Private(name) => {
                let ctx = match self.pop()? {
                    N::Ident(parent) => N::Module(parent),
                    parent @ (N::Module(_) | N::Nominal { .. }) => parent,
                    _ => return Err("expected a context".into()),
                };
                N::Nominal { kind: 'P', ctx: Box::new(ctx), name }
            }
            _ => return Err("expected a protocol".into()),
        };
        let ty = self.witness_type()?;
        Ok(N::Conformance { ty: Box::new(ty), proto: Box::new(proto), module: Box::new(module) })
    }

    /// `Wl` and `WL`: a type and the conformance it is used with. `what` is the thing the symbol
    /// is, `accessor for` or `cache variable for`.
    fn lazy_witness_table(&mut self, what: &str) -> R<()> {
        let conf = self.witness_conformance()?;
        let ty = self.witness_type()?;
        let prefix = format!("lazy protocol witness table {what} type {} and conformance", render(&ty, 0));
        self.push(N::Global { prefix, inner: Box::new(conf), suffix: String::new() })
    }

    fn witness_module(&mut self) -> R<N> {
        match self.pop()? {
            N::Ident(name) => Ok(N::Module(name)),
            module @ N::Module(_) => Ok(module),
            _ => Err("expected a module".into()),
        }
    }

    /// `WZ` and `Wz`: the one-time initialization function or token of a stored variable. The
    /// operand is a context (a module, a nominal type or an extension), then the variable's name or
    /// private name, then the `_` that ends it. Only the name is printed; the context is read and
    /// dropped, as `swift-demangle` prints it. A bare name, a type or module in place of the name,
    /// or any other context is rejected.
    fn one_time_init(&mut self, what: &str) -> R<()> {
        if !matches!(self.pop()?, N::FirstMarker) {
            return Err("one-time initialization without a name marker".into());
        }
        let name = match self.pop()? {
            n @ (N::Ident(_) | N::Private(_)) => n,
            _ => return Err("expected a variable name".into()),
        };
        match self.pop()? {
            N::Ident(_) | N::Module(_) | N::Nominal { .. } | N::Extension { .. } => {}
            _ => return Err("expected a context".into()),
        }
        self.push(N::Global { prefix: what.into(), inner: Box::new(name), suffix: String::new() })
    }

    /// `WT`: a conformance, the path of an associated type, and the protocol that type conforms
    /// to. The path is a first element, then `_`, then any further elements; an element is a name,
    /// optionally followed by the protocol that declares it.
    fn associated_type_witness_accessor(&mut self) -> R<()> {
        let proto = self.witness_type()?;
        let mut parts = Vec::new();
        let mut closed = false;
        while !closed {
            let mut node = self.pop()?;
            if matches!(node, N::FirstMarker) {
                closed = true;
                node = self.pop()?;
            }
            let part = match node {
                N::Ident(name) => name,
                owner @ N::Nominal { kind: 'P', .. } => format!("{}.{}", path(&owner, 0), self.pop_name()?),
                _ => return Err("expected an associated type name".into()),
            };
            parts.push(part);
        }
        parts.reverse();
        let conf = self.witness_conformance()?;
        let prefix = format!("associated type witness table accessor for {} : {} in", parts.join("."), render(&proto, 0));
        self.push(N::Global { prefix, inner: Box::new(conf), suffix: String::new() })
    }

    /// `WO` and a letter: an outlined value operation, a copy, move or destroy the compiler
    /// spelled out inline for one type, such as `outlined destroy of main.S`. The operand is the
    /// whole type before it. `i` and `j` take an index after the letter, which is not printed.
    fn outlined_op(&mut self) -> R<()> {
        let (phrase, indexed) = match self.next()? {
            b'h' => ("destroy", false),
            b'c' => ("init with copy", false),
            b'e' => ("consume", false),
            b'y' => ("copy", false),
            b'b' => ("init with take", false),
            b'd' => ("assign with take", false),
            b'f' => ("assign with copy", false),
            b'g' => ("enum get tag", false),
            b's' => ("release", false),
            b'r' => ("retain", false),
            b'i' => ("enum tag store", true),
            b'j' => ("enum project data for load", true),
            _ => return Err("unsupported outlined operation kind".into()),
        };
        if indexed {
            self.index()?;
        }
        let ty = self.witness_type()?;
        self.push(N::Global { prefix: format!("outlined {phrase} of"), inner: Box::new(ty), suffix: String::new() })
    }

    fn note_param(&mut self, idx: u64) {
        self.max_param = Some(self.max_param.map_or(idx, |m| m.max(idx)));
    }

    /// The name of a declaration: an identifier, an operator, or a private name built by `LL`.
    fn pop_decl_name(&mut self) -> R<String> {
        match self.pop()? {
            N::Ident(i) | N::Private(i) | N::Operator(i) => Ok(i),
            _ => Err("expected a name".into()),
        }
    }

    /// `LL`: a private declaration's name, then the discriminator of the file that declares it.
    /// Both are identifiers (the name may be an operator: `(< infix in _ABC…)`) and keep their
    /// substitution slots; the pair takes none of its own.
    fn private_name(&mut self) -> R<()> {
        let discriminator = self.pop_name()?;
        let name = match self.pop()? {
            N::Ident(i) | N::Operator(i) => i,
            _ => return Err("expected a name".into()),
        };
        self.push(N::Private(format!("({name} in {discriminator})")))
    }

    /// `o` and a fixity letter (`i` infix, `p` prefix, `P` postfix): the identifier before it
    /// spells an operator, with each ASCII operator character written as a letter, and it prints
    /// as the operator and its fixity: `2eeoi` is `== infix`. Bytes outside ASCII stand for
    /// themselves. Any other ASCII byte, such as a digit, a capital or `_`, is not part of an
    /// operator.
    fn operator_name(&mut self) -> R<()> {
        let fixity = match self.next()? {
            b'i' => "infix",
            b'p' => "prefix",
            b'P' => "postfix",
            _ => return Err("unknown operator fixity".into()),
        };
        let N::Ident(spelled) = self.pop()? else {
            return Err("operator without a name".into());
        };
        let mut name = String::with_capacity(spelled.len() + 8);
        for c in spelled.chars() {
            name.push(match c {
                'a' => '&',
                'c' => '@',
                'd' => '/',
                'e' => '=',
                'g' => '>',
                'l' => '<',
                'm' => '*',
                'n' => '!',
                'o' => '|',
                'p' => '+',
                'q' => '?',
                'r' => '%',
                's' => '-',
                't' => '~',
                'x' => '^',
                'z' => '.',
                c if !c.is_ascii() => c,
                _ => return Err("not an operator character".into()),
            });
        }
        self.push(N::Operator(format!("{name} {fixity}")))
    }

    fn operator(&mut self) -> R<()> {
        let c = self.next()?;
        match c {
            b'0'..=b'9' => {
                self.pos -= 1;
                let n = self.identifier()?;
                self.push(n)
            }
            b'A' => self.substitution(),
            b'B' => {
                let t = match self.next()? {
                    b'b' => "Builtin.BridgeObject".to_string(),
                    b'o' => "Builtin.NativeObject".into(),
                    b'O' => "Builtin.UnknownObject".into(),
                    b'p' => "Builtin.RawPointer".into(),
                    b't' => "Builtin.SILToken".into(),
                    b'w' => "Builtin.Word".into(),
                    b'B' => "Builtin.UnsafeValueBuffer".into(),
                    b'i' => {
                        let n = self.natural().ok_or("builtin width")?;
                        self.next_if(b'_');
                        format!("Builtin.Int{n}")
                    }
                    b'f' => {
                        let n = self.natural().ok_or("builtin width")?;
                        self.next_if(b'_');
                        format!("Builtin.FPIEEE{n}")
                    }
                    _ => return Err("unsupported builtin type".into()),
                };
                self.push(N::Builtin(t))
            }
            b'C' | b'V' | b'O' | b'P' | b'a' => {
                let name = self.pop_decl_name()?;
                let ctx = self.pop_context()?;
                let n = N::Nominal { kind: char::from(c), ctx: Box::new(ctx), name };
                self.add_sub(&n)?;
                self.push(n)
            }
            b'D' => {
                let t = self.pop_type()?;
                self.push(t)
            }
            b'E' => {
                let _sig = self.pop_if(|n| matches!(n, N::Sig { .. }));
                let module = self.pop_context()?;
                let ty = self.pop_type()?;
                // Extensions take no substitution slot.
                self.push(N::Extension { module: Box::new(module), ty: Box::new(ty) })
            }
            b'F' => {
                let ty = self.pop_entity_type(true)?;
                let labels = self.pop_labels(Self::param_count(&ty))?;
                let name = self.pop_decl_name()?;
                let ctx = self.pop_context()?;
                self.push(N::Entity {
                    kind: EKind::Func,
                    ctx: Box::new(ctx),
                    name: Some(name),
                    labels,
                    ty: Some(Box::new(ty)),
                    accessor: None,
                    is_static: false,
                })
            }
            b'G' => {
                let mut args = Vec::new();
                loop {
                    if self.pop_if(|n| matches!(n, N::EmptyList)).is_some() {
                        break;
                    }
                    if args.len() > 64 {
                        return Err("too many generic arguments".into());
                    }
                    args.push(self.pop_type()?);
                }
                args.reverse();
                let base = self.pop_type()?;
                let n = N::Bound { base: Box::new(base), args };
                self.add_sub(&n)?;
                self.push(n)
            }
            b'K' => self.push(N::Throws),
            b'L' => match self.next()? {
                b'L' => self.private_name(),
                k => Err(format!("unsupported operator 'L{}'", char::from(k))),
            },
            b'M' => {
                let kind = self.next()?;
                let p = match kind {
                    b'a' => "type metadata accessor for",
                    b'n' => "nominal type descriptor for",
                    b'N' => "noncanonical specialized generic type metadata for",
                    b'm' => "metaclass for",
                    b'f' => "full type metadata for",
                    b'p' => "protocol descriptor for",
                    b'o' => "class metadata base offset for",
                    b'l' => "type metadata singleton initialization cache for",
                    b'L' => "lazy cache variable for type metadata for",
                    b'r' => "type metadata completion function for",
                    b'i' => "type metadata instantiation function for",
                    b'I' => "type metadata instantiation cache for",
                    b'u' => "method lookup function for",
                    b'V' => "property descriptor for",
                    b'c' => "protocol conformance descriptor for",
                    b'j' => "opaque type descriptor accessor key for",
                    b'X' => "extension descriptor for",
                    b'z' => "flag for loading of canonical specialized generic type metadata for",
                    b'K' => "metadata instantiation cache for",
                    b'D' => "demangling cache variable for type metadata for",
                    b'F' => "reflection metadata field descriptor",
                    b'A' => "reflection metadata associated type descriptor",
                    b'P' => "generic type metadata pattern for",
                    b'B' => "reflection metadata builtin descriptor",
                    b'U' => "ObjC metadata update function for",
                    b't' => "full ObjC resilient class stub for",
                    b's' => "ObjC resilient class stub for",
                    _ => return Err("unsupported metadata kind".into()),
                };
                // These describe one type, so the operand must be a whole type, as for a value
                // witness: a module, a function, a conformance or another global is not one.
                if matches!(kind, b'D' | b'F' | b'P' | b'B' | b'U' | b't' | b's') {
                    if matches!(self.stack.last(), Some(N::Global { .. } | N::Entity { .. } | N::Conformance { .. } | N::Extension { .. })) {
                        return Err("metadata of a non-type".into());
                    }
                    let ty = self.pop_type()?;
                    return self.push(N::Global { prefix: p.into(), inner: Box::new(ty), suffix: String::new() });
                }
                if kind == b'K'
                    && matches!(
                        self.stack.last(),
                        Some(N::FirstMarker | N::EmptyList | N::Throws | N::Async | N::FnMarker(_) | N::Requirement(_) | N::Sig { .. })
                    )
                {
                    return Err("metadata cache of a marker".into());
                }
                if p.starts_with("protocol descriptor")
                    && let Some(N::Ident(_) | N::Private(_) | N::Operator(_)) = self.stack.last()
                {
                    let name = self.pop_decl_name()?;
                    let ctx = self.pop_context()?;
                    let proto = N::Nominal { kind: 'P', ctx: Box::new(ctx), name };
                    return self.push(N::Global { prefix: p.into(), inner: Box::new(proto), suffix: String::new() });
                }
                if p.starts_with("protocol conformance") || kind == b'A' {
                    let conf = self.conformance()?;
                    if kind == b'A'
                        && let N::Conformance { ty, module, .. } = &conf
                        && (!matches!(**module, N::Module(_))
                            || matches!(**ty, N::Global { .. } | N::Entity { .. } | N::Conformance { .. } | N::Extension { .. }))
                    {
                        return Err("associated type descriptor of a non-type or outside a module".into());
                    }
                    return self.push(N::Global { prefix: p.into(), inner: Box::new(conf), suffix: String::new() });
                }
                self.global(p)
            }
            b'N' => self.global("type metadata for"),
            b'Q' => match self.next()? {
                b'z' => {
                    let name = self.pop_name()?;
                    self.note_param(0);
                    let n = N::Member { base: Box::new(N::Param(0, 0)), name };
                    self.add_sub(&n)?;
                    self.push(n)
                }
                b'y' => {
                    // Numbered like `q`: `Qy_` is the second generic parameter, `Qy0_` the third.
                    let i = self.index()?.checked_add(1).ok_or("overflow")?;
                    let name = self.pop_name()?;
                    self.note_param(i);
                    let n = N::Member { base: Box::new(N::Param(0, i)), name };
                    self.add_sub(&n)?;
                    self.push(n)
                }
                b'r' => self.push(N::Opaque),
                b'R' => {
                    // The position among the declaration's opaque result types; every one prints
                    // as plain `some`.
                    self.index()?;
                    self.push(N::Opaque)
                }
                b'O' => match self.pop()? {
                    d @ N::Entity { .. } => self.push(N::OpaqueDecl(Box::new(d))),
                    _ => Err("opaque types of a non-declaration".into()),
                },
                b'o' => {
                    let index = self.index()?;
                    // The generic arguments the declaration was used with: types, after the `y`
                    // that opens the list. They are checked but not printed.
                    let mut count = 0;
                    loop {
                        if self.pop_if(|n| matches!(n, N::EmptyList)).is_some() {
                            break;
                        }
                        count += 1;
                        if count > 64 {
                            return Err("too many generic arguments".into());
                        }
                        if self.pop_if(|n| matches!(n, N::FirstMarker)).is_none() {
                            self.pop_type()?;
                        }
                    }
                    match self.pop()? {
                        N::OpaqueDecl(decl) => {
                            let n = N::OpaqueRef { decl, index };
                            // The slot keeps a copy of the whole declaration, so it is paid for like
                            // any other copy out of a substitution.
                            self.budget = self.budget.checked_sub(weight(&n, 0)).ok_or("symbol expands too much")?;
                            self.add_sub(&n)?;
                            self.push(n)
                        }
                        _ => Err("opaque type without its declaration".into()),
                    }
                }
                _ => Err("unsupported associated type".into()),
            },
            b'R' => {
                let (param, idx) = match self.next()? {
                    b'z' => ("A".to_string(), 0),
                    b'_' => ("B".into(), 1),
                    d if d.is_ascii_digit() => {
                        self.pos -= 1;
                        let n = self.natural().ok_or("requirement index")?;
                        if !self.next_if(b'_') {
                            return Err("requirement index".into());
                        }
                        let i = n.checked_add(2).ok_or("overflow")?;
                        (param_name(0, i), i)
                    }
                    _ => return Err("unsupported requirement kind".into()),
                };
                self.note_param(idx);
                let proto = self.pop_type()?;
                let text = format!("{param}: {}", render(&proto, 0));
                self.push(N::Requirement(text))
            }
            b'S' => self.standard(),
            b'T' => {
                let k = self.next()?;
                let prefix = match k {
                    b'K' | b'k' => return self.key_path_accessor(k == b'K'),
                    b'o' => "@objc",
                    b'O' => "@nonobjc",
                    b'q' => "method descriptor for",
                    b'j' => "dispatch thunk of",
                    b'D' => "dynamic",
                    b'd' => "direct method reference to",
                    b'A' => "partial apply forwarder for",
                    b'm' => "merged",
                    b'W' => {
                        let entity = self.pop()?;
                        let conf = self.conformance()?;
                        let text = format!("protocol witness for {} in conformance {}", render(&entity, 0), render(&conf, 0));
                        return self.push(N::Global { prefix: text, inner: Box::new(N::EmptyList), suffix: String::new() });
                    }
                    b'R' | b'r' => {
                        let to = self.pop_type()?;
                        let from = self.pop_type()?;
                        let text = format!("reabstraction thunk helper from {} to {}", render(&from, 0), render(&to, 0));
                        return self.push(N::Global { prefix: text, inner: Box::new(N::EmptyList), suffix: String::new() });
                    }
                    b'u' => return self.async_thunk("async function pointer to".into()),
                    b'Y' | b'Q' => {
                        // `swift-demangle` stops reading a number above 2^31 - 1.
                        let i = self.index()?;
                        if i > 1 << 31 {
                            return Err("resume index too large".into());
                        }
                        let phrase = if k == b'Y' { "suspend resume" } else { "await resume" };
                        return self.async_thunk(format!("({i}) {phrase} partial function for"));
                    }
                    b'v' => return self.outlined_variable(),
                    _ => return Err("unsupported thunk or specialization".into()),
                };
                self.global(prefix)
            }
            b'W' => {
                let k = self.next()?;
                match k {
                    b'Z' => self.one_time_init("one-time initialization function for"),
                    b'z' => self.one_time_init("one-time initialization token for"),
                    b'P' | b'p' | b'a' | b'G' => {
                        let conf = self.conformance()?;
                        let p = match k {
                            b'P' => "protocol witness table for",
                            b'p' => "protocol witness table pattern for",
                            b'a' => "protocol witness table accessor for",
                            _ => "generic protocol witness table for",
                        };
                        self.push(N::Global { prefix: p.into(), inner: Box::new(conf), suffix: String::new() })
                    }
                    b'V' => self.global("value witness table for"),
                    b'v' => {
                        let p = match self.next()? {
                            b'd' => "direct field offset for",
                            b'i' => "indirect field offset for",
                            _ => return Err("unsupported field offset".into()),
                        };
                        self.global(p)
                    }
                    b'L' => self.lazy_witness_table("cache variable for"),
                    b'C' => self.enum_case(),
                    b'I' => {
                        let conf = self.witness_conformance()?;
                        self.push(N::Global {
                            prefix: "instantiation function for generic protocol witness table for".into(),
                            inner: Box::new(conf),
                            suffix: String::new(),
                        })
                    }
                    b'l' => self.lazy_witness_table("accessor for"),
                    b'b' => {
                        let base = self.witness_type()?;
                        let conf = self.witness_conformance()?;
                        let prefix = format!("base witness table accessor for {} in", render(&base, 0));
                        self.push(N::Global { prefix, inner: Box::new(conf), suffix: String::new() })
                    }
                    b'T' => self.associated_type_witness_accessor(),
                    b'O' => self.outlined_op(),
                    _ => Err("unsupported witness kind".into()),
                }
            }
            b'X' => self.special_type(),
            b'Y' => match self.next()? {
                b'a' => self.push(N::Async),
                b'b' => self.push(N::FnMarker(FnMarker::Sendable)),
                b'A' => self.push(N::FnMarker(FnMarker::IsolatedAny)),
                b'c' => {
                    let t = self.pop_type()?;
                    self.push(N::FnMarker(FnMarker::Actor(Box::new(t))))
                }
                b'K' => {
                    let t = self.pop_type()?;
                    self.push(N::FnMarker(FnMarker::Thrown(Box::new(t))))
                }
                _ => Err("unsupported function attribute".into()),
            },
            b'Z' => {
                let mut e = self.pop()?;
                match &mut e {
                    N::Entity { is_static, .. } => *is_static = true,
                    _ => return Err("static applied to a non-member".into()),
                }
                self.push(e)
            }
            b'_' => self.push(N::FirstMarker),
            b'c' => {
                let f = self.pop_func_type()?;
                self.push(f)
            }
            b'f' => {
                let k = self.next()?;
                let kind = match k {
                    b'C' => EKind::AllocInit,
                    b'c' => EKind::Init,
                    b'D' => EKind::DeallocDeinit,
                    b'd' => EKind::Deinit,
                    b'E' => EKind::IvarDestroyer,
                    b'e' => EKind::IvarInitializer,
                    b'U' | b'u' => {
                        let i = self.index()?;
                        let ty = self.pop_type()?;
                        let ctx = self.pop_context()?;
                        return self.push(N::Entity {
                            kind: EKind::Closure(i, k == b'U'),
                            ctx: Box::new(ctx),
                            name: None,
                            labels: Vec::new(),
                            ty: Some(Box::new(ty)),
                            accessor: None,
                            is_static: false,
                        });
                    }
                    b'A' => {
                        let i = self.index()?;
                        let ctx = self.pop()?;
                        return self.push(N::Entity {
                            kind: EKind::DefaultArg(i),
                            ctx: Box::new(ctx),
                            name: None,
                            labels: Vec::new(),
                            ty: None,
                            accessor: None,
                            is_static: false,
                        });
                    }
                    b'i' => {
                        let ctx = self.pop()?;
                        return self.push(N::Entity {
                            kind: EKind::VarInit,
                            ctx: Box::new(ctx),
                            name: None,
                            labels: Vec::new(),
                            ty: None,
                            accessor: None,
                            is_static: false,
                        });
                    }
                    _ => return Err("unsupported function entity".into()),
                };
                match kind {
                    EKind::AllocInit | EKind::Init => {
                        let ty = self.pop_entity_type(false)?;
                        let labels = self.pop_labels(Self::param_count(&ty))?;
                        let ctx = self.pop_context()?;
                        self.push(N::Entity { kind, ctx: Box::new(ctx), name: None, labels, ty: Some(Box::new(ty)), accessor: None, is_static: false })
                    }
                    _ => {
                        let ctx = self.pop_context()?;
                        self.push(N::Entity { kind, ctx: Box::new(ctx), name: None, labels: Vec::new(), ty: None, accessor: None, is_static: false })
                    }
                }
            }
            b'i' => {
                let ty = self.pop_entity_type(false)?;
                let labels = self.pop_labels(Self::param_count(&ty))?;
                let ctx = self.pop_context()?;
                let accessor = self.accessor()?;
                self.push(N::Entity { kind: EKind::Subscript, ctx: Box::new(ctx), name: None, labels, ty: Some(Box::new(ty)), accessor, is_static: false })
            }
            b'l' => {
                let mut reqs = Vec::new();
                while let Some(N::Requirement(r)) = self.pop_if(|n| matches!(n, N::Requirement(_))) {
                    reqs.push(r);
                }
                reqs.reverse();
                self.note_param(0);
                self.push(N::Sig { reqs })
            }
            b'm' => {
                let t = self.pop_type()?;
                self.push(N::Metatype(Box::new(t)))
            }
            b'p' => {
                let protos = self.protocol_list()?;
                self.push(N::Existential(protos))
            }
            b'o' => self.operator_name(),
            b'q' => {
                let i = self.index()?.checked_add(1).ok_or("overflow")?;
                self.note_param(i);
                self.push(N::Param(0, i))
            }
            b's' => self.push(N::Module("Swift".into())),
            b't' => {
                let mut elems = Vec::new();
                if self.pop_if(|n| matches!(n, N::EmptyList)).is_none() {
                    loop {
                        if elems.len() > 256 {
                            return Err("tuple too large".into());
                        }
                        let first = self.pop_if(|n| matches!(n, N::FirstMarker)).is_some();
                        let label = match self.stack.last() {
                            Some(N::Ident(_)) => match self.stack.pop() {
                                Some(N::Ident(l)) => Some(l),
                                _ => None,
                            },
                            _ => None,
                        };
                        let ty = self.pop_type()?;
                        elems.push((label, ty));
                        if first {
                            break;
                        }
                    }
                    elems.reverse();
                }
                self.push(N::Tuple(elems))
            }
            b'u' => {
                let ty = self.pop_type()?;
                let sig = self.pop_if(|n| matches!(n, N::Sig { .. })).ok_or("generic type without signature")?;
                self.push(N::Generic { sig: Box::new(sig), ty: Box::new(ty) })
            }
            b'v' => {
                let ty = self.pop_type()?;
                let name = self.pop_decl_name()?;
                let ctx = self.pop_context()?;
                let accessor = self.accessor()?;
                self.push(N::Entity {
                    kind: EKind::Var,
                    ctx: Box::new(ctx),
                    name: Some(name),
                    labels: Vec::new(),
                    ty: Some(Box::new(ty)),
                    accessor,
                    is_static: false,
                })
            }
            b'w' => self.value_witness(),
            b'x' => {
                self.note_param(0);
                self.push(N::Param(0, 0))
            }
            b'y' => self.push(N::EmptyList),
            b'z' => {
                let t = self.pop_type()?;
                self.push(N::Inout(Box::new(t)))
            }
            other => Err(format!("unsupported operator '{}'", char::from(other))),
        }
    }
}

/// Approximate node count of a tree (bounded).
fn weight(n: &N, depth: usize) -> usize {
    if depth > MAX_RENDER_DEPTH {
        return usize::MAX / 4;
    }
    let d = depth + 1;
    let kids: usize = match n {
        N::Nominal { ctx, .. } => weight(ctx, d),
        N::Extension { module, ty } => weight(module, d).saturating_add(weight(ty, d)),
        N::Bound { base, args } => args.iter().fold(weight(base, d), |a, x| a.saturating_add(weight(x, d))),
        N::Tuple(t) => t.iter().fold(0usize, |a, (_, x)| a.saturating_add(weight(x, d))),
        N::Func { params, result, attrs } => {
            let w = weight(params, d).saturating_add(weight(result, d));
            w.saturating_add(attrs.actor.as_deref().into_iter().chain(typed_throws(&attrs.throws)).fold(0usize, |a, x| a.saturating_add(weight(x, d))))
        }
        N::FnMarker(FnMarker::Actor(x) | FnMarker::Thrown(x)) => weight(x, d),
        N::Metatype(x) | N::ExistentialMetatype(x) | N::ReprMetatype(x) | N::Inout(x) | N::OpaqueDecl(x) => weight(x, d),
        N::OpaqueRef { decl, .. } => weight(decl, d),
        N::Attributed { inner, .. } => weight(inner, d),
        N::ClassBound { class, protos } => protos.iter().fold(weight(class, d), |a, x| a.saturating_add(weight(x, d))),
        N::Member { base, .. } => weight(base, d),
        N::Existential(v) => v.iter().fold(0usize, |a, x| a.saturating_add(weight(x, d))),
        N::Generic { sig, ty } => weight(sig, d).saturating_add(weight(ty, d)),
        N::Entity { ctx, ty, .. } => weight(ctx, d).saturating_add(ty.as_ref().map_or(0, |t| weight(t, d))),
        N::Conformance { ty, proto, module } => weight(ty, d).saturating_add(weight(proto, d)).saturating_add(weight(module, d)),
        N::Global { inner, .. } => weight(inner, d),
        _ => 0,
    };
    kids.saturating_add(1)
}

fn param_name(depth: u64, index: u64) -> String {
    let letter = u8::try_from(index % 26).map(|i| char::from(b'A' + i)).unwrap_or('A');
    let round = index / 26;
    match (depth, round) {
        (0, 0) => letter.to_string(),
        (0, r) => format!("{letter}{r}"),
        (d, _) => format!("{letter}{d}"),
    }
}

fn path(n: &N, depth: usize) -> String {
    if depth > MAX_RENDER_DEPTH {
        return "…".into();
    }
    match n {
        N::Module(m) | N::Ident(m) => m.clone(),
        N::Nominal { ctx, name, .. } => format!("{}.{name}", path(ctx, depth + 1)),
        N::Extension { ty, .. } => path(ty, depth + 1),
        other => render(other, depth + 1),
    }
}

/// The argument list of a function type, with `labels`, an entity's argument labels, one per
/// argument. If at least one argument has a label, the others read `_:`, as in
/// `append(_: Swift.UnsafeRawPointer, length: Swift.Int)`; if none has, all print bare. A single
/// argument that isn't wrapped in a tuple prints bare whatever its label says.
fn params_text(p: &N, labels: &[Option<String>], depth: usize) -> String {
    let any_label = labels.iter().any(Option::is_some);
    let items: Vec<String> = match p {
        N::Tuple(t) => t
            .iter()
            .enumerate()
            .map(|(i, (own, ty))| {
                let ty = render(ty, depth + 1);
                match labels.get(i).cloned().flatten().or_else(|| own.clone()) {
                    Some(l) => format!("{l}: {ty}"),
                    None if any_label => format!("_: {ty}"),
                    None => ty,
                }
            })
            .collect(),
        other => vec![render(other, depth + 1)],
    };
    format!("({})", items.join(", "))
}

fn sig_text(sig: &N, max_param: u64) -> String {
    let N::Sig { reqs } = sig else { return String::new() };
    let params: Vec<String> = (0..=max_param.min(25)).map(|i| param_name(0, i)).collect();
    if reqs.is_empty() { format!("<{}>", params.join(", ")) } else { format!("<{} where {}>", params.join(", "), reqs.join(", ")) }
}

fn max_param(n: &N, depth: usize) -> u64 {
    if depth > MAX_RENDER_DEPTH {
        return 0;
    }
    match n {
        N::Param(_, i) => *i,
        N::Func { params, result, attrs } => attrs
            .actor
            .as_deref()
            .into_iter()
            .chain(typed_throws(&attrs.throws))
            .map(|x| max_param(x, depth + 1))
            .fold(max_param(params, depth + 1).max(max_param(result, depth + 1)), u64::max),
        N::FnMarker(FnMarker::Actor(x) | FnMarker::Thrown(x)) => max_param(x, depth + 1),
        N::Tuple(t) => t.iter().map(|(_, x)| max_param(x, depth + 1)).max().unwrap_or(0),
        N::Bound { base, args } => args.iter().map(|x| max_param(x, depth + 1)).max().unwrap_or(0).max(max_param(base, depth + 1)),
        N::Metatype(x) | N::ExistentialMetatype(x) | N::ReprMetatype(x) | N::Inout(x) => max_param(x, depth + 1),
        N::Attributed { inner, .. } => max_param(inner, depth + 1),
        N::ClassBound { class, protos } => protos.iter().map(|x| max_param(x, depth + 1)).max().unwrap_or(0).max(max_param(class, depth + 1)),
        N::Member { base, .. } => max_param(base, depth + 1),
        _ => 0,
    }
}

fn typed_throws(t: &Throws) -> Option<&N> {
    match t {
        Throws::Typed(e) => Some(e),
        _ => None,
    }
}

fn func_text(ty: &N, labels: &[Option<String>], depth: usize) -> String {
    match ty {
        N::Func { params, result, attrs } => {
            let mut s = String::new();
            if let Some(actor) = &attrs.actor {
                s.push_str(&format!("@{} ", render(actor, depth + 1)));
            }
            if attrs.isolated_any {
                s.push_str("@isolated(any) ");
            }
            if attrs.sendable {
                s.push_str("@Sendable ");
            }
            s.push_str(&params_text(params, labels, depth));
            if attrs.is_async {
                s.push_str(" async");
            }
            match &attrs.throws {
                Throws::No => {}
                Throws::Any => s.push_str(" throws"),
                Throws::Typed(e) => s.push_str(&format!(" throws({})", render(e, depth + 1))),
            }
            s.push_str(" -> ");
            s.push_str(&render(result, depth + 1));
            s
        }
        N::Generic { sig, ty } => format!("{}{}", sig_text(sig, max_param(ty, depth + 1)), func_text(ty, labels, depth + 1)),
        other => render(other, depth + 1),
    }
}

/// The wrapped type of a `?` or `!` optional. A function type or a protocol composition goes in
/// parentheses, so the `?` reads as applying to the whole type, not to the result or last protocol.
fn optional_operand(t: &N, depth: usize) -> String {
    let text = render(t, depth);
    match t {
        N::Func { .. } | N::ClassBound { .. } | N::Attributed { wrapped: true, .. } | N::Opaque | N::OpaqueRef { .. } => format!("({text})"),
        N::Existential(protos) if protos.len() > 1 => format!("({text})"),
        _ => text,
    }
}

/// An existential type or the metatype of one, which `XM` turns into a `.Protocol` metatype.
fn is_existential(n: &N) -> bool {
    match n {
        N::Existential(_) | N::ClassBound { .. } | N::ExistentialMetatype(_) => true,
        N::Attributed { inner, .. } => matches!(inner.as_ref(), N::ExistentialMetatype(_)),
        _ => false,
    }
}

fn render(n: &N, depth: usize) -> String {
    if depth > MAX_RENDER_DEPTH {
        return "…".into();
    }
    let d = depth + 1;
    match n {
        N::Ident(s) | N::Private(s) | N::Operator(s) | N::Module(s) | N::Builtin(s) => s.clone(),
        N::Nominal { .. } | N::Extension { .. } => path(n, d),
        N::Bound { base, args } => {
            let b = path(base, d);
            match (b.as_str(), args.as_slice()) {
                ("Swift.Array", [t]) => format!("[{}]", render(t, d)),
                ("Swift.Dictionary", [k, v]) => format!("[{} : {}]", render(k, d), render(v, d)),
                ("Swift.Optional", [t]) => format!("{}?", optional_operand(t, d)),
                ("Swift.ImplicitlyUnwrappedOptional", [t]) => format!("{}!", optional_operand(t, d)),
                _ => format!("{b}<{}>", args.iter().map(|a| render(a, d)).collect::<Vec<_>>().join(", ")),
            }
        }
        N::Tuple(t) => params_text(&N::Tuple(t.clone()), &[], d),
        N::Func { .. } | N::Generic { .. } => func_text(n, &[], d),
        N::Metatype(t) if matches!(**t, N::Opaque | N::OpaqueRef { .. }) => format!("({}).Type", render(t, d)),
        N::Metatype(t) => format!("{}.Type", render(t, d)),
        N::ExistentialMetatype(t) => format!("{}.Type", render(t, d)),
        N::ReprMetatype(t) => {
            let text = render(t, d);
            match t.as_ref() {
                N::Existential(ps) if ps.len() > 1 => format!("({text}).Protocol"),
                N::ClassBound { .. } => format!("({text}).Protocol"),
                other if is_existential(other) => format!("{text}.Protocol"),
                N::Func { .. } | N::Attributed { wrapped: true, .. } => format!("({text}).Type"),
                _ => format!("{text}.Type"),
            }
        }
        N::ClassBound { class, protos } => {
            let rest: Vec<String> = protos.iter().map(|p| render(p, d)).collect();
            format!("{} & {}", render(class, d), rest.join(" & "))
        }
        N::DynamicSelf => "Self".into(),
        N::Attributed { text, inner, .. } => format!("{text}{}", render(inner, d)),
        N::Existential(ps) => {
            if ps.is_empty() {
                "Any".into()
            } else {
                ps.iter().map(|p| render(p, d)).collect::<Vec<_>>().join(" & ")
            }
        }
        N::Param(dp, i) => param_name(*dp, *i),
        N::Member { base, name } => format!("{}.{name}", render(base, d)),
        N::Inout(t) => format!("inout {}", render(t, d)),
        N::Opaque => "some".into(),
        N::OpaqueDecl(decl) => format!("<<opaque return type of {}>>", render(decl, d)),
        N::OpaqueRef { decl, index } => format!("<<opaque return type of {}>>.{index}", render(decl, d)),
        N::EmptyList | N::FirstMarker | N::Throws | N::Async | N::FnMarker(_) => String::new(),
        N::Requirement(r) => r.clone(),
        N::Sig { .. } => sig_text(n, 0),
        N::Conformance { ty, proto, module } => format!("{} : {} in {}", render(ty, d), path(proto, d), path(module, d)),
        N::Global { prefix, inner, suffix } => {
            let inner = render(inner, d);
            let mut s = if inner.is_empty() { prefix.clone() } else { format!("{prefix} {inner}") };
            s.push_str(suffix);
            s
        }
        N::Entity { kind, ctx, name, labels, ty, accessor, is_static } => {
            let c = path(ctx, d);
            let st = if *is_static { "static " } else { "" };
            let ty_text = |t: &Option<Box<N>>| t.as_ref().map(|t| func_text(t, labels, d)).unwrap_or_default();
            let body = match kind {
                EKind::Func => format!("{c}.{}{}", name.clone().unwrap_or_default(), ty_text(ty)),
                // The allocating name is kept for a class's own initialisers only. In a struct, an
                // enum, a protocol or any extension, even of a class, it reads `init`.
                EKind::AllocInit if matches!(ctx.as_ref(), N::Nominal { kind: 'C', .. }) => format!("{c}.__allocating_init{}", ty_text(ty)),
                EKind::AllocInit | EKind::Init => format!("{c}.init{}", ty_text(ty)),
                EKind::Deinit => format!("{c}.deinit"),
                EKind::DeallocDeinit => format!("{c}.__deallocating_deinit"),
                EKind::IvarDestroyer => format!("{c}.__ivar_destroyer"),
                EKind::IvarInitializer => format!("{c}.__ivar_initializer"),
                EKind::Var => {
                    let acc = accessor.filter(|a| !a.is_empty()).map(|a| format!(".{a}")).unwrap_or_default();
                    format!("{c}.{}{acc} : {}", name.clone().unwrap_or_default(), ty.as_ref().map(|t| render(t, d)).unwrap_or_default())
                }
                // With an accessor the accessor comes right after `subscript` and the type follows
                // a colon, as for a variable: `X.subscript.getter : (Swift.Int) -> A`. Without one
                // (a property descriptor) the type is written straight after: `X.subscript(Swift.Int) -> A`.
                EKind::Subscript => match accessor.filter(|a| !a.is_empty()) {
                    Some(a) => format!("{c}.subscript.{a} : {}", ty_text(ty)),
                    None => format!("{c}.subscript{}", ty_text(ty)),
                },
                EKind::Closure(i, explicit) => {
                    format!(
                        "{} #{} {} in {c}",
                        if *explicit { "closure" } else { "implicit closure" },
                        i + 1,
                        ty.as_ref().map(|t| render(t, d)).unwrap_or_default()
                    )
                }
                EKind::DefaultArg(i) => format!("default argument {i} of {c}"),
                EKind::VarInit => format!("variable initialization expression of {c}"),
            };
            format!("{st}{body}")
        }
    }
}

/// True when the name uses a Swift mangling prefix.
pub fn looks_swift(sym: &str) -> bool {
    let s = sym.strip_prefix('_').unwrap_or(sym);
    s.starts_with("$s") || s.starts_with("$S") || s.starts_with("$e") || s.starts_with("T0") || s.starts_with("Tt")
}

fn runtime_name(body: &str) -> Result<String, String> {
    // _Tt<kinds><module><names…>
    let b = body.as_bytes();
    let mut i = 0;
    let mut kinds = 0usize;
    while let Some(&c) = b.get(i) {
        if matches!(c, b'C' | b'V' | b'O' | b'P') {
            kinds += 1;
            i += 1;
        } else {
            break;
        }
    }
    if kinds == 0 {
        return Err("unsupported runtime name".into());
    }
    let mut parts = Vec::new();
    if b.get(i) == Some(&b's') {
        parts.push("Swift".to_string());
        i += 1;
    } else if b.get(i..i + 2) == Some(b"So") {
        parts.push("__C".to_string());
        i += 2;
    }
    let mut d = D::new(b);
    d.pos = i;
    while d.peek().is_some_and(|c| c.is_ascii_digit()) {
        match d.identifier()? {
            N::Ident(s) => parts.push(s),
            _ => return Err("bad runtime name".into()),
        }
        if parts.len() > 64 {
            return Err("runtime name too long".into());
        }
    }
    let rest = body.get(d.pos..).unwrap_or("");
    if parts.len() != kinds + 1 || !(rest.is_empty() || rest == "_") {
        return Err("unsupported runtime name".into());
    }
    Ok(parts.join("."))
}

/// Demangle one symbol.
pub fn demangle(sym: &str) -> Demangled {
    let out = |status, demangled: Option<String>, reason: Option<String>| Demangled { symbol: sym.to_string(), status, demangled, reason };
    if !looks_swift(sym) {
        return out(Status::Unchanged, None, None);
    }
    if sym.len() > MAX_LEN {
        return out(Status::Invalid, None, Some(format!("longer than {MAX_LEN} characters")));
    }
    let s = sym.strip_prefix('_').unwrap_or(sym);
    if let Some(body) = s.strip_prefix("Tt") {
        return match runtime_name(body) {
            Ok(t) => out(Status::Demangled, Some(t), None),
            Err(e) => out(Status::Invalid, None, Some(e)),
        };
    }
    let body = s.get(2..).unwrap_or("");
    // A suffix after '.' (e.g. `.cold.1`) is not part of the mangling.
    let (body, suffix) = match body.find('.') {
        Some(p) => (body.get(..p).unwrap_or(""), body.get(p..).unwrap_or("")),
        None => (body, ""),
    };
    let mut d = D::new(body.as_bytes());
    let res = (|| -> R<String> {
        while d.pos < d.s.len() {
            d.operator()?;
        }
        if d.stack.len() != 1 {
            return Err(format!("{} nodes left after decoding", d.stack.len()));
        }
        let top = d.stack.first().ok_or("nothing decoded")?;
        let mut text = render(top, 0);
        if text.is_empty() {
            return Err("nothing decoded".into());
        }
        let _ = d.max_param;
        if !suffix.is_empty() {
            text.push_str(&format!(" with unmangled suffix \"{suffix}\""));
        }
        Ok(text)
    })();
    match res {
        Ok(t) => out(Status::Demangled, Some(t), None),
        Err(e) => out(Status::Invalid, None, Some(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm(s: &str) -> String {
        let d = demangle(s);
        assert_eq!(d.status, Status::Demangled, "{s}: {:?}", d.reason);
        d.demangled.unwrap()
    }

    #[test]
    fn common_symbols() {
        assert_eq!(dm("$s4main3fooyyF"), "main.foo() -> ()");
        assert_eq!(dm("_$s4main3fooyySiF"), "main.foo(Swift.Int) -> ()");
        assert_eq!(dm("$s4main3foo1xySi_tF"), "main.foo(x: Swift.Int) -> ()");
        assert_eq!(dm("$s4main3addyS2i_SitF"), "main.add(Swift.Int, Swift.Int) -> Swift.Int");
        assert_eq!(dm("$s4main6WidgetCMa"), "type metadata accessor for main.Widget");
        assert_eq!(dm("$s4main6WidgetCMn"), "nominal type descriptor for main.Widget");
        assert_eq!(dm("$s4main6WidgetCN"), "type metadata for main.Widget");
        assert_eq!(dm("$s4main8DelegateMp"), "protocol descriptor for main.Delegate");
        assert_eq!(dm("$s4main6WidgetC3tapyyF"), "main.Widget.tap() -> ()");
        assert_eq!(dm("$s4main6WidgetC3tapyyFTo"), "@objc main.Widget.tap() -> ()");
        assert_eq!(dm("$s4main6WidgetC5countSivg"), "main.Widget.count.getter : Swift.Int");
        assert_eq!(dm("$s4main6WidgetC5countSivs"), "main.Widget.count.setter : Swift.Int");
        assert_eq!(dm("$s4main6WidgetC5countSivpWvd"), "direct field offset for main.Widget.count : Swift.Int");
        assert_eq!(dm("$s4main6WidgetCACycfC"), "main.Widget.__allocating_init() -> main.Widget");
        assert_eq!(dm("$s4main6WidgetCfD"), "main.Widget.__deallocating_deinit");
        assert_eq!(dm("$s4main6WidgetC6sharedACvpZ"), "static main.Widget.shared : main.Widget");
        assert_eq!(dm("$s4main5namesSaySSGvp"), "main.names : [Swift.String]");
        assert_eq!(dm("$s4main3mapSDySSSiGvp"), "main.map : [Swift.String : Swift.Int]");
        assert_eq!(dm("$s4main4nameSSSgvp"), "main.name : Swift.String?");
        assert_eq!(dm("$s4main3fooyyxlF"), "main.foo<A>(A) -> ()");
        assert_eq!(dm("$s4main3fooyyxSHRzlF"), "main.foo<A where A: Swift.Hashable>(A) -> ()");
        assert_eq!(dm("$s4main4loadSSyKF"), "main.load() throws -> Swift.String");
        assert_eq!(dm("$s4main4loadSSyYaKF"), "main.load() async throws -> Swift.String");
        assert_eq!(dm("$sSi4mainE6doubleSiyF"), "Swift.Int.double() -> Swift.Int");
        assert_eq!(dm("$s4main6WidgetCAA8DelegateAAWP"), "protocol witness table for main.Widget : main.Delegate in main");
        assert_eq!(dm("$s4main5PointVMn"), "nominal type descriptor for main.Point");
        assert_eq!(dm("$s4main4ModeOMa"), "type metadata accessor for main.Mode");
        assert_eq!(dm("$sSo8NSObjectCMa"), "type metadata accessor for __C.NSObject");
        assert_eq!(dm("$s4main3fooyyF.cold.1"), "main.foo() -> () with unmangled suffix \".cold.1\"");
        assert_eq!(dm("_TtC4main6Widget"), "main.Widget");
        assert_eq!(dm("_TtCC4main5Outer5Inner"), "main.Outer.Inner");
        assert_eq!(dm("$s4main3runyyyycF"), "main.run(() -> ()) -> ()");
        assert_eq!(dm("$s4main6WidgetC4tick5timesySi_tF"), "main.Widget.tick(times: Swift.Int) -> ()");
        // Word substitution: "SettingsPanel" then "Settings" reused as word 0
        assert_eq!(dm("$s4main13SettingsPanelC0B0VMn"), "nominal type descriptor for main.SettingsPanel.Settings");
        assert_eq!(dm("$s4main3fooyyypF"), "main.foo(Any) -> ()");
        assert_eq!(dm("$s4main3fooyyAA8DelegateP_pF"), "main.foo(main.Delegate) -> ()");
        assert_eq!(dm("$s4main6WidgetCSgMa"), "type metadata accessor for main.Widget?");
    }

    /// Symbols from macOS system binaries. `Sg` optionals and `Qz`/`Qy` associated types take a
    /// substitution slot and extensions do not, so a later `A…` reference names the right node.
    #[test]
    fn substitutions_count_optionals_and_associated_types_but_not_extensions() {
        // `Sg`: `A2L` is `String?` twice, not the dictionary before it.
        assert_eq!(
            dm("_$s6NDOAPI22RequestPayloadProviderP07amsBodyC012serialNumber05extraC06ulCode0J6Params10Foundation4DataVSSSg_SDyS2SGA2LtKFTj"),
            "dispatch thunk of NDOAPI.RequestPayloadProvider.amsBodyPayload(serialNumber: Swift.String?, extraPayload: [Swift.String : Swift.String], ulCode: Swift.String?, ulParams: Swift.String?) throws -> Foundation.Data"
        );
        assert_eq!(dm("_$sxSg8ShelfKit13ObserverValueABMc"), "protocol conformance descriptor for A? : ShelfKit.ObserverValue in ShelfKit");
        // `Qz`: `AI` is `A.Subscriber`, not the bound generic after it.
        assert_eq!(
            dm("_$s8SPShared12SubscribableP9subscribe10subscriberAA12SubscriptionVy10SubscriberQzGAI_tFTq"),
            "method descriptor for SPShared.Subscribable.subscribe(subscriber: A.Subscriber) -> SPShared.Subscription<A.Subscriber>"
        );
        assert_eq!(dm("_$sSl5index5after5IndexQzAD_tFTj"), "dispatch thunk of Swift.Collection.index(after: A.Index) -> A.Index");
        assert_eq!(dm("_$ss10SetAlgebraP6removey7ElementQzSgAEFTq"), "method descriptor for Swift.SetAlgebra.remove(A.Element) -> A.Element?");
        // Extensions: `AH` is `Range<String.Index>`, `AE` is `UIButton.Configuration`.
        assert_eq!(
            dm("_$sSo11NLTokenizerC15NaturalLanguageE6tokens3forSaySnySS5IndexVGGAH_tF"),
            "__C.NLTokenizer.tokens(for: Swift.Range<Swift.String.Index>) -> [Swift.Range<Swift.String.Index>]"
        );
        assert_eq!(
            dm("_$sSo8UIButtonC5UIKitE13ConfigurationV13macIdiomStyleAE03MaceF0Ovs"),
            "__C.UIButton.Configuration.macIdiomStyle.setter : __C.UIButton.Configuration.MacIdiomStyle"
        );
        // An extension and an `Sg` before `AN`: right only when both are counted the mangler's way.
        assert_eq!(
            dm("_$sSo8NLTaggerC15NaturalLanguageE4tags2in4unit6scheme7optionsSaySo5NLTagaSg_SnySS5IndexVGtGAN_So11NLTokenUnitVSo0I6SchemeaSo0A7OptionsVtF"),
            "__C.NLTagger.tags(in: Swift.Range<Swift.String.Index>, unit: __C.NLTokenUnit, scheme: __C.NLTagScheme, options: __C.NLTaggerOptions) -> [(__C.NLTag?, Swift.Range<Swift.String.Index>)]"
        );
    }

    #[test]
    fn statuses() {
        assert_eq!(demangle("_main").status, Status::Unchanged);
        assert_eq!(demangle("objc_msgSend").status, Status::Unchanged);
        assert_eq!(demangle("$s").status, Status::Invalid);
        assert_eq!(demangle("$s4mai").status, Status::Invalid);
        assert_eq!(demangle("$s4mainXz").status, Status::Invalid);
        assert_eq!(demangle(&format!("$s{}", "A".repeat(5000))).status, Status::Invalid);
    }

    #[test]
    fn never_panics() {
        let seeds = ["$s4main3foo1xySi_tF", "$s4main6WidgetCACycfC", "$s4main3fooyyxSHRzlF", "$s4main6WidgetCAA8DelegateAAWP", "_TtCC4main5Outer5Inner"];
        for s in seeds {
            for i in 0..=s.len() {
                if let Some(p) = s.get(..i) {
                    let _ = demangle(p);
                }
            }
            let b = s.as_bytes();
            for i in 2..b.len() {
                for c in *b"09AZ_yGtlSqR" {
                    let mut m = b.to_vec();
                    m[i] = c;
                    if let Ok(t) = String::from_utf8(m) {
                        let _ = demangle(&t);
                    }
                }
            }
        }
        for seed in 0..2000u64 {
            let n = craft_anything_testkit::noise(seed, 40);
            let t: String = n.iter().map(|b| char::from(b"0123456789ABCDGFKMNSTWVyztlpqcfvx_aiuOPEQRZmsYo"[usize::from(*b) % 47])).collect();
            let _ = demangle(&format!("$s{t}"));
        }
        let deep = format!("$s{}", "Si".repeat(1000) + &"t".repeat(1000));
        let _ = demangle(&deep);
        let nested = format!("$s{}", "SaySi".repeat(600) + &"G".repeat(600));
        let _ = demangle(&nested);
    }
}
