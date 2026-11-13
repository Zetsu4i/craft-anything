//! A small, forgiving JavaScript tokenizer.
//!
//! It never fails: unterminated strings, comments and templates just end at the end of input. It
//! serves two jobs: estimating how deeply a source nests before we hand it to the real parser
//! (whose recursion is unbounded), and extracting facts when parsing is refused or fails.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokKind {
    Ident,
    Str,
    /// One chunk of a template literal (between backticks / `${` / `}`).
    Template,
    Number,
    Punct,
    Regex,
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: TokKind,
    /// Raw source text of the token (quotes included for strings).
    pub text: &'a str,
    pub offset: usize,
}

const PUNCTS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=",
    "*=", "/=", "%=", "&=", "|=", "^=", "**", "<<", ">>",
];

const REGEX_AFTER_KEYWORDS: &[&str] = &["return", "typeof", "instanceof", "in", "of", "new", "delete", "void", "throw", "case", "do", "else", "yield", "await"];

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$' || b == b'\\' || b >= 0x80
}

fn is_ident_part(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

fn line_terminator_len(src: &str, offset: usize) -> usize {
    match src.as_bytes().get(offset) {
        Some(b'\n' | b'\r') => 1,
        Some(0xe2) if src.get(offset..).is_some_and(|rest| rest.starts_with('\u{2028}') || rest.starts_with('\u{2029}')) => 3,
        _ => 0,
    }
}

/// Tokenize `src`, stopping after `max_tokens` (the flag says whether it stopped early).
pub fn tokenize(src: &str, max_tokens: usize) -> (Vec<Token<'_>>, bool) {
    let mut out = Vec::new();
    let truncated = scan(src, max_tokens, |t| out.push(t));
    (out, truncated)
}

/// Stream tokens to `emit` without storing them. Returns true when `max_tokens` stopped it.
pub fn scan<'a>(src: &'a str, max_tokens: usize, mut emit: impl FnMut(Token<'a>)) -> bool {
    let b = src.as_bytes();
    let mut count = 0usize;
    let mut i = 0usize;
    // true = this brace opened a template substitution `${`.
    let mut braces: Vec<bool> = Vec::new();
    let slice = |a: usize, z: usize| src.get(a..z).unwrap_or("");
    let mut last_sig: Option<(TokKind, &str)> = None;
    while i < b.len() {
        let c = b.get(i).copied().unwrap_or(0);
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let line_end = line_terminator_len(src, i);
        if line_end != 0 {
            i += line_end;
            continue;
        }
        if count >= max_tokens {
            return true;
        }
        count += 1;
        let start = i;
        let next = b.get(i + 1).copied().unwrap_or(0);
        // Comments.
        if c == b'/' && next == b'/' {
            while i < b.len() && line_terminator_len(src, i) == 0 {
                i += 1;
            }
            emit(Token { kind: TokKind::Comment, text: slice(start, i), offset: start });
            continue;
        }
        if c == b'/' && next == b'*' {
            i += 2;
            while i < b.len() && !(b.get(i) == Some(&b'*') && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(b.len());
            emit(Token { kind: TokKind::Comment, text: slice(start, i), offset: start });
            continue;
        }
        let tok_kind;
        if c == b'"' || c == b'\'' {
            i += 1;
            while let Some(&d) = b.get(i) {
                i += 1;
                if d == b'\\' {
                    i += if b.get(i) == Some(&b'\r') && b.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                } else if d == c || d == b'\n' {
                    break;
                }
            }
            i = i.min(b.len());
            tok_kind = TokKind::Str;
        } else if c == b'`' || (c == b'}' && braces.last() == Some(&true)) {
            if c == b'}' {
                braces.pop();
            }
            i += 1;
            loop {
                match b.get(i) {
                    None => break,
                    Some(b'\\') => i += 2,
                    Some(b'`') => {
                        i += 1;
                        break;
                    }
                    Some(b'$') if b.get(i + 1) == Some(&b'{') => {
                        i += 2;
                        braces.push(true);
                        break;
                    }
                    Some(_) => i += 1,
                }
            }
            i = i.min(b.len());
            tok_kind = TokKind::Template;
        } else if is_ident_start(c) {
            while b.get(i).is_some_and(|&d| is_ident_part(d) && (d < 0x80 || line_terminator_len(src, i) == 0)) {
                i += 1;
            }
            tok_kind = TokKind::Ident;
        } else if c.is_ascii_digit() || (c == b'.' && next.is_ascii_digit()) {
            let radix = c == b'0' && matches!(next, b'x' | b'X' | b'b' | b'B' | b'o' | b'O');
            while let Some(&d) = b.get(i) {
                let exp_sign =
                    !radix && matches!(d, b'+' | b'-') && matches!(b.get(i.wrapping_sub(1)), Some(b'e' | b'E')) && b.get(i + 1).is_some_and(u8::is_ascii_digit);
                if d.is_ascii_alphanumeric() || d == b'.' || d == b'_' || exp_sign {
                    i += 1;
                } else {
                    break;
                }
            }
            tok_kind = TokKind::Number;
        } else if c == b'/' && regex_allowed(last_sig) {
            i += 1;
            let mut in_class = false;
            while let Some(&d) = b.get(i) {
                i += 1;
                match d {
                    b'\\' => i += 1,
                    b'[' => in_class = true,
                    b']' => in_class = false,
                    b'/' if !in_class => break,
                    b'\n' => break,
                    _ => {}
                }
            }
            while b.get(i).is_some_and(|&d| d.is_ascii_alphabetic()) {
                i += 1;
            }
            i = i.min(b.len());
            tok_kind = TokKind::Regex;
        } else {
            let rest = b.get(i..).unwrap_or_default();
            let len = PUNCTS.iter().find(|p| rest.starts_with(p.as_bytes())).map_or(1, |p| p.len());
            // Never split a multi-byte character.
            let mut z = i + len;
            while z < b.len() && !src.is_char_boundary(z) {
                z += 1;
            }
            if c == b'{' {
                braces.push(false);
            } else if c == b'}' {
                braces.pop();
            }
            i = z;
            tok_kind = TokKind::Punct;
        }
        let text = slice(start, i);
        last_sig = Some((tok_kind, text));
        emit(Token { kind: tok_kind, text, offset: start });
    }
    false
}

fn regex_allowed(prev: Option<(TokKind, &str)>) -> bool {
    match prev {
        None => true,
        Some((TokKind::Punct, p)) => !matches!(p, ")" | "]" | "}"),
        Some((TokKind::Ident, w)) => REGEX_AFTER_KEYWORDS.contains(&w),
        Some(_) => false,
    }
}

/// Decode the value of a string token (quotes stripped, common escapes resolved).
pub fn string_value(raw: &str) -> String {
    let inner = raw.get(1..).unwrap_or("");
    let inner = match raw.chars().next() {
        Some(q) if inner.ends_with(q) => inner.get(..inner.len().saturating_sub(1)).unwrap_or(inner),
        _ => inner,
    };
    unescape(inner)
}

/// Resolve JavaScript escape sequences (lenient: unknown escapes keep the character).
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('v') => out.push('\u{b}'),
            Some('0') => out.push('\0'),
            Some('x') => {
                let h: String = it.by_ref().take(2).collect();
                if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some('u') => {
                let hex: String = if it.peek() == Some(&'{') {
                    it.next();
                    it.by_ref().take_while(|c| *c != '}').take(8).collect()
                } else {
                    it.by_ref().take(4).collect()
                };
                let Ok(mut code) = u32::from_str_radix(&hex, 16) else { continue };
                if (0xD800..0xDC00).contains(&code) {
                    let mut ahead = it.clone();
                    if ahead.next() == Some('\\') && ahead.next() == Some('u') {
                        let lo: String = ahead.by_ref().take(4).collect();
                        if let Some(lo) = u32::from_str_radix(&lo, 16).ok().filter(|l| lo.len() == 4 && (0xDC00..0xE000).contains(l)) {
                            code = 0x10000 + ((code - 0xD800) << 10) + (lo - 0xDC00);
                            it = ahead;
                        }
                    }
                }
                out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
            }
            Some('\n') => {}
            Some('\r') => {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// An upper-bound style estimate of how deep the syntax tree for `tokens` will nest.
///
/// Brackets add a level; operators that nest expressions (member access, binary and assignment
/// operators, conditionals, arrows, unary prefixes) and nesting statement keywords add a level
/// until the next `,` or `;` at the same bracket depth. Over-estimates are fine: they only send a
/// file to the lexer fallback.
pub fn nesting_estimate(tokens: &[Token<'_>]) -> usize {
    let mut n = Nesting::default();
    for t in tokens {
        n.feed(t);
    }
    n.max
}

/// Streaming state for [`nesting_estimate`].
#[derive(Debug)]
pub struct Nesting {
    stack: Vec<usize>,
    sum: usize,
    pub max: usize,
}

impl Default for Nesting {
    fn default() -> Self {
        Nesting { stack: vec![0], sum: 0, max: 0 }
    }
}

impl Nesting {
    pub fn feed(&mut self, t: &Token<'_>) {
        let Nesting { stack, sum, max } = self;
        let closes = match t.kind {
            TokKind::Punct => matches!(t.text, ")" | "]" | "}"),
            TokKind::Template => t.text.starts_with('}'),
            _ => false,
        };
        let opens = match t.kind {
            TokKind::Punct => matches!(t.text, "(" | "[" | "{"),
            TokKind::Template => t.text.ends_with("${"),
            _ => false,
        };
        if closes
            && stack.len() > 1
            && let Some(n) = stack.pop()
        {
            *sum = sum.saturating_sub(n + 1);
        }
        if opens {
            stack.push(0);
            *sum += 1;
        }
        if !opens && !closes {
            let nests = match t.kind {
                TokKind::Punct => !matches!(t.text, "," | ";" | ":" | "++" | "--"),
                TokKind::Ident => matches!(
                    t.text,
                    "typeof" | "void" | "delete" | "await" | "new" | "if" | "else" | "for" | "while" | "do" | "with" | "in" | "instanceof" | "yield"
                ),
                _ => false,
            };
            if let Some(n) = stack.last_mut() {
                if matches!((t.kind, t.text), (TokKind::Punct, "," | ";")) {
                    *sum = sum.saturating_sub(*n);
                    *n = 0;
                } else if nests {
                    *n += 1;
                    *sum += 1;
                }
            }
        }
        *max = (*max).max(*sum);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(TokKind, String)> {
        tokenize(s, 1000).0.into_iter().map(|t| (t.kind, t.text.to_string())).collect()
    }

    #[test]
    fn operators_before_non_ascii_identifier() {
        for (src, op) in [("a===日本", "==="), ("a>>日本", ">>"), ("a=>é", "=>"), ("a!==😀", "!==")] {
            let t: Vec<&str> = tokenize(src, 100).0.into_iter().map(|t| t.text).collect();
            assert_eq!(t.len(), 3, "{src}: {t:?}");
            assert_eq!(t[1], op, "{src}");
        }
    }

    #[test]
    fn whitespace_does_not_count_against_limit() {
        let (t, truncated) = tokenize("a b c d", 4);
        assert_eq!(t.len(), 4);
        assert!(!truncated);
        let (t, truncated) = tokenize("a b c d", 3);
        assert_eq!(t.len(), 3);
        assert!(truncated);
    }

    #[test]
    fn exponent_sign() {
        let texts = |s: &str| tokenize(s, 100).0.into_iter().map(|t| t.text.to_string()).collect::<Vec<_>>();
        assert_eq!(texts("x = 1e+5 + 2.5E-3;"), ["x", "=", "1e+5", "+", "2.5E-3", ";"]);
        assert_eq!(texts("0x1e+5"), ["0x1e", "+", "5"]);
    }

    #[test]
    fn crlf_line_continuation_stays_in_string() {
        let k = kinds("x = 'a\\\r\nb';");
        assert!(k.contains(&(TokKind::Str, "'a\\\r\nb'".into())), "{k:?}");
        assert!(k.contains(&(TokKind::Punct, ";".into())), "{k:?}");
    }

    #[test]
    fn unescape_crlf_line_continuation() {
        assert_eq!(unescape("a\\\r\nb"), "ab");
        assert_eq!(unescape("a\\\rb"), "ab");
        assert_eq!(unescape("a\\\nb"), "ab");
    }

    #[test]
    fn unescape_backspace_formfeed_vtab() {
        assert_eq!(unescape(r"a\fb\vc\bd"), "a\u{c}b\u{b}c\u{8}d");
        assert_eq!(string_value(r#""\b\f\v""#), "\u{8}\u{c}\u{b}");
    }

    #[test]
    fn unescape_combines_surrogate_pairs() {
        assert_eq!(unescape("\\uD83D\\uDE00"), "😀");
        assert_eq!(unescape("a\\uD83D\\u0041"), "a\u{FFFD}A");
        assert_eq!(unescape("\\uDE00x"), "\u{FFFD}x");
        assert_eq!(unescape("\\u{1F600}"), "😀");
    }

    #[test]
    fn tokenizes_strings_templates_regex() {
        let k = kinds(r#"a = "x\"y" + `t${b}u` / 2; r = /a\/b[/]c/g"#);
        assert!(k.contains(&(TokKind::Str, r#""x\"y""#.into())));
        assert!(k.contains(&(TokKind::Template, "`t${".into())));
        assert!(k.contains(&(TokKind::Template, "}u`".into())));
        assert!(k.contains(&(TokKind::Regex, "/a\\/b[/]c/g".into())));
        assert!(k.contains(&(TokKind::Punct, "/".into())));
        assert_eq!(string_value(r#""aA\x42\n""#), "aAB\n");
    }

    #[test]
    fn nesting_grows_with_depth() {
        let shallow = nesting_estimate(&tokenize("a(b, c); d[e];", 100).0);
        let deep_src = format!("{}1{}", "(".repeat(300), ")".repeat(300));
        let deep = nesting_estimate(&tokenize(&deep_src, 10_000).0);
        let chain = nesting_estimate(&tokenize(&"a+".repeat(500), 10_000).0);
        let nots = nesting_estimate(&tokenize(&format!("{}x", "!".repeat(400)), 10_000).0);
        assert!(shallow < 5, "{shallow}");
        assert!(deep >= 300 && chain >= 500 && nots >= 400, "{deep} {chain} {nots}");
    }

    #[test]
    fn never_panics_on_junk() {
        for s in ["\"", "`${", "/*", "'\\", "}}}}", "`${`${`${", "/[", "é/é/", "\u{1F600}=`\u{1F600}${é}`", "a/**/b//"] {
            let (t, _) = tokenize(s, 100);
            let _ = nesting_estimate(&t);
            for tok in t {
                let _ = string_value(tok.text);
            }
        }
    }
}
