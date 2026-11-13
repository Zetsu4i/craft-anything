use craft_anything_js::lex::{TokKind, tokenize};
use craft_anything_js::{Limits, ParseMode, admit, analyze};

fn facts_after_comment(separator: &str) {
    let source = format!("// banner{separator}import('./dep.js'); ipcRenderer.invoke('open'); fetch('/data');");
    let (tokens, truncated) = tokenize(&source, 100);
    assert!(!truncated);
    assert_eq!(tokens.first().unwrap().text, "// banner");
    assert_eq!(tokens.get(1).unwrap().text, "import");
    assert_eq!(tokens.get(1).unwrap().offset, "// banner".len() + separator.len());
    let analysis = analyze("app.js", &source, &Limits { max_parse_bytes: 0, ..Limits::default() });
    assert_eq!(analysis.facts.mode, ParseMode::Lexer);
    assert_eq!(analysis.facts.imports.first().unwrap().specifier, "./dep.js");
    assert!(analysis.facts.ipc.iter().any(|usage| usage.channel.as_deref() == Some("open")));
    assert!(analysis.facts.endpoints.iter().any(|endpoint| endpoint.url == "/data"));
}

#[test]
fn bare_carriage_return_preserves_following_facts() {
    facts_after_comment("\r");
}

#[test]
fn unicode_line_separator_preserves_following_facts() {
    facts_after_comment("\u{2028}");
}

#[test]
fn unicode_paragraph_separator_preserves_following_facts() {
    facts_after_comment("\u{2029}");
}

#[test]
fn comment_endings_cannot_hide_excessive_nesting_from_admission() {
    for separator in ["\r", "\u{2028}", "\u{2029}"] {
        let source = format!("// banner{separator}{}value{};", "(".repeat(32), ")".repeat(32));
        let limits = Limits { max_nesting: 8, ..Limits::default() };
        assert!(admit(&source, &limits).is_err(), "nesting after {separator:?} must be scanned");
        assert_eq!(analyze("app.js", &source, &limits).facts.mode, ParseMode::Lexer);
    }
}

#[test]
fn unicode_terminators_separate_identifiers_without_spending_tokens() {
    let (tokens, truncated) = tokenize("first\u{2028}second\u{2029}third", 3);
    assert!(!truncated);
    assert_eq!(tokens.iter().map(|token| token.text).collect::<Vec<_>>(), ["first", "second", "third"]);
    assert!(tokens.iter().all(|token| token.kind == TokKind::Ident));
}

#[test]
fn lf_and_crlf_comments_keep_following_tokens() {
    facts_after_comment("\n");
    facts_after_comment("\r\n");
}

#[test]
fn unicode_terminators_inside_strings_templates_and_block_comments_stay_inside() {
    let source = "'first\u{2028}second'; `third\u{2029}fourth`; /* ignored\u{2028}import('hidden')\u{2029} */ import('visible');";
    let (tokens, truncated) = tokenize(source, 100);
    assert!(!truncated);
    assert!(tokens.iter().any(|token| token.kind == TokKind::Str && token.text == "'first\u{2028}second'"));
    assert!(tokens.iter().any(|token| token.kind == TokKind::Template && token.text == "`third\u{2029}fourth`"));
    assert!(tokens.iter().any(|token| token.kind == TokKind::Comment && token.text.contains("import('hidden')")));
    let analysis = analyze("app.js", source, &Limits { max_parse_bytes: 0, ..Limits::default() });
    assert_eq!(analysis.facts.imports.iter().map(|import| import.specifier.as_str()).collect::<Vec<_>>(), ["visible"]);
}
