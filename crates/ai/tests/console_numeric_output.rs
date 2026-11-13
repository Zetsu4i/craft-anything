use craft_anything_ai::rebuild::kinds::runs::run_claims;
use serde_json::json;

fn run(text: &str) -> serde_json::Value {
    json!({"name":"no arguments", "reason":"exited", "exit_code":0, "lines":text.lines().collect::<Vec<_>>()})
}

fn output_matches(original: &str, rebuilt: &str, names: &[String]) -> bool {
    let mut observed = run(original);
    observed["repeat"] = run(original);
    let claims = run_claims(&observed, &run(rebuilt), names);
    assert!(claims[0].passed, "these fixtures have matching exits");
    claims[1].passed
}

#[test]
fn different_bare_decimal_results_do_not_both_count_as_empty_output() {
    assert!(!output_matches("4.5", "9.0", &[]));
    assert!(!output_matches(".5", "5", &[]));
}

#[test]
fn numeric_signs_decimal_points_and_exponent_signs_are_observable() {
    for (original, rebuilt) in [("12", "-12"), ("12.34", "12 34"), ("1.2e-3", "1.2e+3")] {
        assert!(!output_matches(original, rebuilt, &[]), "{original} versus {rebuilt}");
    }
}

#[test]
fn numeric_output_order_and_repetition_are_observable() {
    assert!(!output_matches("4.5\n6.7", "6.7\n4.5", &[]));
    assert!(!output_matches("4.5\n4.5", "4.5", &[]));
}

#[test]
fn missing_or_added_numeric_results_do_not_match_silent_runs() {
    assert!(!output_matches("4.5", "", &[]));
    assert!(!output_matches("", "4.5", &[]));
}

#[test]
fn similar_explanatory_text_cannot_hide_a_different_integer_result() {
    let prose = "The requested calculation has completed successfully and its final result is";
    assert!(!output_matches(&format!("{prose} 42"), &format!("{prose} 43"), &[]));
    // These consecutive integers would alias if the oracle converted them to f64.
    assert!(!output_matches(&format!("{prose} 9007199254740992"), &format!("{prose} 9007199254740993"), &[]));
}

#[test]
fn matching_numeric_text_keeps_whitespace_and_case_tolerance() {
    assert!(output_matches("Result: -12.34\n+.5\n1.2e-3", "result: -12.34  +.5  1.2E-3", &[]));
    assert!(output_matches("4.5", "4.5", &[]));
    assert!(output_matches("", "", &[]));
}

#[test]
fn program_names_are_normalized_before_collecting_their_digits() {
    let names = vec!["calculator2".into(), "rebuilt9".into()];
    assert!(output_matches("calculator2 result: 4.5", "rebuilt9 result: 4.5", &names));
    assert!(!output_matches("calculator2 result: 4.5", "rebuilt9 result: 9.0", &names));
}

#[test]
fn program_name_substrings_cannot_hide_numeric_differences() {
    assert!(!output_matches("1e2", "1e3", &["e2".into(), "e3".into()]));
    assert!(!output_matches("1.e2", "1.e3", &["e2".into(), "e3".into()]));
    assert!(!output_matches("1.e-2", "1.e-3", &["e-2".into(), "e-3".into()]));
    assert!(!output_matches("4", "9", &["4".into(), "9".into()]));
    assert!(!output_matches("结果 4.5", "结果 9.0", &["tool".into()]));
    assert!(output_matches("路径/calculator2: 4.5", "路径/rebuilt9: 4.5", &["calculator2".into(), "rebuilt9".into()]));
}

#[test]
fn non_decimal_integer_notation_keeps_its_value() {
    for (original, rebuilt) in [("0x2a", "0x2b"), ("0b101", "0b110"), ("0o64", "0o65")] {
        let prose = "The requested calculation has completed successfully and its final result is";
        assert!(!output_matches(&format!("{prose} {original}"), &format!("{prose} {rebuilt}"), &[]));
    }
    assert!(output_matches("Result: 0x2A", "result: 0X2a", &[]));
}

#[test]
fn non_numeric_help_keeps_the_existing_word_comparison() {
    assert!(output_matches("Usage: original [options] <file>", "usage: rebuilt [options] <file>", &["original".into(), "rebuilt".into()]));
    assert!(!output_matches("Usage: original [options] <file>", "error: this input is unsupported", &[]));
}

#[test]
fn timing_and_process_numbers_that_vary_in_the_original_are_ignored() {
    let mut original = run("Result 42 completed in 123.4 ms by pid 12345");
    original["repeat"] = run("Result 42 completed in 456.7 ms by pid 98765");
    let built = run("Result 42 completed in 890.1 ms by pid 24680");
    let claims = run_claims(&original, &built, &[]);
    assert!(claims[1].passed, "{claims:?}");
}

#[test]
fn varying_timing_does_not_hide_a_wrong_stable_result() {
    let mut original = run("Result 12 completed in 123.4 ms");
    original["repeat"] = run("Result 12 completed in 456.7 ms");
    let claims = run_claims(&original, &run("Result -12 completed in 890.1 ms"), &[]);
    assert!(!claims[1].passed, "{claims:?}");
    assert!(claims[1].detail.contains("stable numeric output differs"), "{claims:?}");
}

#[test]
fn a_stable_number_count_still_rejects_missing_or_extra_results() {
    let mut original = run("Result 12 completed in 123.4 ms");
    original["repeat"] = run("Result 12 completed in 456.7 ms");
    for text in ["Result 12 completed in ms", "Result 12 13 completed in 890.1 ms"] {
        assert!(!run_claims(&original, &run(text), &[])[1].passed, "{text}");
    }
}

#[test]
fn varying_number_counts_fall_back_to_the_output_words_with_an_explanation() {
    let mut original = run("Sampling results: 12 34");
    original["repeat"] = run("Sampling results: 56");
    let claims = run_claims(&original, &run("Sampling results: 78 90 123"), &[]);
    assert!(claims[1].passed, "{claims:?}");
    assert!(!run_claims(&original, &run("Unrelated error message: 78 90 123"), &[])[1].passed);
}

#[test]
fn old_captures_without_a_repeat_keep_word_matching_and_report_the_limit() {
    let original = run("The requested operation completed successfully with total elapsed time 123");
    let claims = run_claims(&original, &run("The requested operation completed successfully with total elapsed time 456"), &[]);
    assert!(claims[1].passed, "{claims:?}");
}

#[test]
fn a_failed_or_incomplete_repeat_does_not_establish_numeric_stability() {
    for repeat in [json!({}), json!({"reason":"exited"}), json!({"reason":"timeout", "lines":["12"]})] {
        let mut original = run("The operation completed successfully with result 12");
        original["repeat"] = repeat;
        let claims = run_claims(&original, &run("The operation completed successfully with result 13"), &[]);
        assert!(claims[1].passed, "{claims:?}");
    }
}
