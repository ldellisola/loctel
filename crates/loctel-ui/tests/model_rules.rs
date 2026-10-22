//! The Discard rules screen's models (M4 plan, Task 3): the cards' rows and
//! their exact count texts, the total-dropped line, the editor's draft and its
//! validation, and the rule "Discard lines like this…" suggests. Plain values;
//! the logs the suggestion is tried on are the fixed fake mesh's.

use std::sync::Arc;

use loctel_ingest::logs_from_request;
use loctel_meshgen::{Scenario, generate};
use loctel_model::{AttrValue, LogRecord, Resource, Severity};
use loctel_query::{logql, traceql};
use loctel_rules::{DropScope, Preview, RuleId, RuleLanguage, RuleSpec, RuleStatus};
use loctel_ui::model::rules::{
    Draft, DraftCheck, NO_RULES, PREVIEW_INVALID, PREVIEW_TITLE, RuleRow, SUBTITLE,
    draft_error_line, dropped_line, file_error_line, preview_text, rows, suggest_from_log,
};

const BASE: u64 = 1_800_000_000_000_000_000;

// --- the cards ------------------------------------------------------------------

fn spec(language: RuleLanguage, scope: DropScope, enabled: bool) -> RuleSpec {
    RuleSpec {
        name: "Health checks".to_owned(),
        enabled,
        language,
        query: match language {
            RuleLanguage::TraceQl => r#"{ name = "GET /healthz" }"#.to_owned(),
            RuleLanguage::LogQl => r#"{service_name="orders-api"} |= "heartbeat""#.to_owned(),
        },
        scope,
    }
}

fn status(spec: RuleSpec, spans: u64, logs: u64, traces: u64) -> RuleStatus {
    RuleStatus {
        id: RuleId(7),
        spec,
        error: None,
        dropped_spans: spans,
        dropped_logs: logs,
        dropped_traces: traces,
    }
}

fn counts_of(status: RuleStatus) -> String {
    rows(&[status]).remove(0).counts
}

#[test]
fn a_span_scope_trace_rule_counts_spans() {
    let rule = status(
        spec(RuleLanguage::TraceQl, DropScope::Span, true),
        48_210,
        0,
        0,
    );
    assert_eq!(counts_of(rule), "48,210 spans dropped");
}

#[test]
fn a_trace_scope_rule_adds_the_traces_it_dropped() {
    let rule = status(
        spec(RuleLanguage::TraceQl, DropScope::Trace, true),
        48_210,
        96,
        602,
    );
    assert_eq!(counts_of(rule), "48,210 spans dropped in 602 traces");
}

#[test]
fn a_trace_scope_rule_that_dropped_no_whole_trace_says_only_spans() {
    let rule = status(
        spec(RuleLanguage::TraceQl, DropScope::Trace, true),
        12,
        0,
        0,
    );
    assert_eq!(counts_of(rule), "12 spans dropped");
}

#[test]
fn a_span_scope_rule_never_names_traces() {
    // Even if a count were there, the scope says the rule drops spans.
    let rule = status(spec(RuleLanguage::TraceQl, DropScope::Span, true), 5, 0, 3);
    assert_eq!(counts_of(rule), "5 spans dropped");
}

#[test]
fn a_log_rule_counts_lines_not_the_spans_or_traces() {
    let rule = status(
        spec(RuleLanguage::LogQl, DropScope::Trace, true),
        999,
        96_433,
        5,
    );
    assert_eq!(counts_of(rule), "96,433 lines dropped");
}

#[test]
fn counts_are_singular_for_one_and_plural_for_none() {
    let trace_rule = |spans, traces| {
        counts_of(status(
            spec(RuleLanguage::TraceQl, DropScope::Trace, true),
            spans,
            0,
            traces,
        ))
    };
    assert_eq!(trace_rule(1, 1), "1 span dropped in 1 trace");
    assert_eq!(trace_rule(2, 1), "2 spans dropped in 1 trace");
    assert_eq!(trace_rule(0, 0), "0 spans dropped");
    let log_rule = |lines| {
        counts_of(status(
            spec(RuleLanguage::LogQl, DropScope::Trace, true),
            0,
            lines,
            0,
        ))
    };
    assert_eq!(log_rule(1), "1 line dropped");
    assert_eq!(log_rule(0), "0 lines dropped");
    assert_eq!(log_rule(1_000), "1,000 lines dropped");
    assert_eq!(log_rule(1_234_567_890), "1,234,567,890 lines dropped");
}

#[test]
fn a_rule_that_is_off_says_off_whatever_it_dropped() {
    for language in [RuleLanguage::TraceQl, RuleLanguage::LogQl] {
        let row = rows(&[status(
            spec(language, DropScope::Trace, false),
            48_210,
            9,
            602,
        )])
        .remove(0);
        assert_eq!(row.counts, "Off");
        assert!(!row.enabled);
        assert_eq!(row.error, None);
    }
}

#[test]
fn a_rule_that_does_not_parse_says_error_and_the_message() {
    let mut rule = status(spec(RuleLanguage::TraceQl, DropScope::Trace, true), 0, 0, 0);
    rule.error = Some("at column 3: expected `{`".to_owned());

    let row = rows(&[rule]).remove(0);

    assert_eq!(row.counts, "Error: at column 3: expected `{`");
    assert_eq!(row.error.as_deref(), Some("at column 3: expected `{`"));
}

#[test]
fn a_switched_off_rule_with_a_bad_query_is_off_not_an_error() {
    // As in the menu's rules line, an error only counts for a rule that is on:
    // a draft that is off does nothing wrong. The card says Off, and the error
    // field stays empty so the card is not painted red.
    let mut rule = status(spec(RuleLanguage::LogQl, DropScope::Trace, false), 0, 0, 0);
    rule.error = Some("at column 1: expected `{`".to_owned());

    let row = rows(&[rule]).remove(0);

    assert_eq!(row.counts, "Off");
    assert_eq!(row.error, None);
}

#[test]
fn a_row_carries_the_rules_identity_name_chip_and_query() {
    let trace = status(spec(RuleLanguage::TraceQl, DropScope::Trace, true), 1, 0, 1);
    let mut log = status(spec(RuleLanguage::LogQl, DropScope::Trace, true), 0, 2, 0);
    log.id = RuleId(9);
    log.spec.name = "Heartbeats".to_owned();

    let rows = rows(&[trace.clone(), log.clone()]);

    assert_eq!(
        rows,
        vec![
            RuleRow {
                id: RuleId(7),
                enabled: true,
                name: "Health checks".to_owned(),
                chip: "TRACEQL",
                query: r#"{ name = "GET /healthz" }"#.to_owned(),
                counts: "1 span dropped in 1 trace".to_owned(),
                error: None,
            },
            RuleRow {
                id: RuleId(9),
                enabled: true,
                name: "Heartbeats".to_owned(),
                chip: "LOGQL",
                query: r#"{service_name="orders-api"} |= "heartbeat""#.to_owned(),
                counts: "2 lines dropped".to_owned(),
                error: None,
            },
        ]
    );
}

#[test]
fn rows_keep_the_file_order_and_no_rules_are_no_rows() {
    assert!(rows(&[]).is_empty());
    let mut a = status(spec(RuleLanguage::TraceQl, DropScope::Trace, true), 0, 0, 0);
    a.id = RuleId(3);
    let mut b = a.clone();
    b.id = RuleId(1);
    let ids: Vec<u64> = rows(&[a, b]).iter().map(|row| row.id.0).collect();
    assert_eq!(ids, vec![3, 1]);
}

// --- the total ------------------------------------------------------------------

#[test]
fn the_total_line_says_the_share_dropped() {
    assert_eq!(dropped_line(0.31), "31% of incoming data dropped");
    assert_eq!(dropped_line(0.5), "50% of incoming data dropped");
    assert_eq!(dropped_line(1.0), "100% of incoming data dropped");
    assert_eq!(dropped_line(0.314), "31% of incoming data dropped");
    assert_eq!(dropped_line(0.316), "32% of incoming data dropped");
}

#[test]
fn nothing_dropped_is_said_in_words() {
    assert_eq!(dropped_line(0.0), "Nothing dropped yet");
}

#[test]
fn something_dropped_is_never_called_nothing() {
    // 0.4% of the data is dropped: not "nothing", and not a misleading `0%`.
    assert_eq!(dropped_line(0.004), "<1% of incoming data dropped");
    assert_eq!(
        dropped_line(f64::MIN_POSITIVE),
        "<1% of incoming data dropped"
    );
}

#[test]
fn a_fraction_that_is_not_a_share_is_cut_to_one_or_read_as_nothing() {
    // As in the menu's rules line: more than all is all, and what is not a
    // number (or not a finite one) or is negative is nothing.
    assert_eq!(dropped_line(7.0), "100% of incoming data dropped");
    assert_eq!(dropped_line(f64::NAN), "Nothing dropped yet");
    assert_eq!(dropped_line(-0.5), "Nothing dropped yet");
    assert_eq!(dropped_line(f64::NEG_INFINITY), "Nothing dropped yet");
    assert_eq!(dropped_line(f64::INFINITY), "Nothing dropped yet");
}

// --- the draft ------------------------------------------------------------------

fn draft(language: RuleLanguage, query: &str) -> Draft {
    Draft {
        name: "A rule".to_owned(),
        language,
        query: query.to_owned(),
        scope: DropScope::Trace,
        enabled: true,
    }
}

#[test]
fn a_draft_round_trips_through_its_spec() {
    for language in [RuleLanguage::TraceQl, RuleLanguage::LogQl] {
        for scope in [DropScope::Trace, DropScope::Span] {
            for enabled in [true, false] {
                // Exactly as typed, blanks and all: nothing is trimmed away,
                // so the rule keeps its identity (RuleSet keeps it by name,
                // language, query and scope).
                let spec = RuleSpec {
                    name: "  Name with blanks ".to_owned(),
                    enabled,
                    language,
                    query: " { name = \"x\" } ".to_owned(),
                    scope,
                };
                let draft = Draft::from_spec(&spec);
                assert_eq!(draft.name, spec.name);
                assert_eq!(draft.language, language);
                assert_eq!(draft.query, spec.query);
                assert_eq!(draft.scope, scope);
                assert_eq!(draft.enabled, enabled);
                assert_eq!(draft.to_spec(), spec);
            }
        }
    }
}

#[test]
fn a_good_trace_query_has_no_error() {
    let check = draft(RuleLanguage::TraceQl, "{ status = error && duration > 1s }").validate();
    assert_eq!(
        check,
        DraftCheck {
            name_ok: true,
            query: Ok(())
        }
    );
}

#[test]
fn a_good_log_query_has_no_error() {
    let check = draft(RuleLanguage::LogQl, r#"{service_name="a"} |= "x""#).validate();
    assert_eq!(check.query, Ok(()));
}

#[test]
fn a_trace_query_is_checked_with_the_trace_parser() {
    // A LogQL query is not a TraceQL query, and the other way round.
    let text = r#"{service_name="a"} |= "x""#;
    let check = draft(RuleLanguage::TraceQl, text).validate();
    let expected = traceql::parse(text).unwrap_err();
    assert_eq!(check.query, Err((expected.position + 1, expected.message)));

    let text = "{ status = error && duration > 1s }";
    let check = draft(RuleLanguage::LogQl, text).validate();
    let expected = logql::parse(text).unwrap_err();
    assert_eq!(check.query, Err((expected.position + 1, expected.message)));
}

#[test]
fn the_error_column_counts_from_one_like_the_query_bars() {
    // `{ status = ` is wrong at its end: 11 characters, so column 12.
    let check = draft(RuleLanguage::TraceQl, "{ status = ").validate();
    let (column, message) = check.query.unwrap_err();
    assert_eq!(column, 12);
    assert!(!message.is_empty());
    assert!(
        !message.contains("column"),
        "no column in the message: {message}"
    );
}

#[test]
fn an_empty_query_is_an_error_like_the_rules_engine_says() {
    for language in [RuleLanguage::TraceQl, RuleLanguage::LogQl] {
        for text in ["", "   "] {
            assert!(
                draft(language, text).validate().query.is_err(),
                "{language:?} {text:?}"
            );
        }
    }
}

#[test]
fn the_name_is_ok_when_it_has_something_in_it() {
    let mut d = draft(RuleLanguage::LogQl, r#"{service_name="a"}"#);
    for (name, ok) in [
        ("", false),
        ("   ", false),
        ("\t\n", false),
        ("x", true),
        (" x ", true),
    ] {
        d.name = name.to_owned();
        assert_eq!(d.validate().name_ok, ok, "{name:?}");
    }
}

#[test]
fn a_draft_can_be_saved_with_any_name_even_with_a_bad_query() {
    // Per M2, a query that does not parse is a draft: it is kept and shows as
    // that rule's error, so it can be saved.
    let mut d = draft(RuleLanguage::TraceQl, "{ not a query");
    assert!(d.validate().query.is_err());
    assert!(d.can_save());
    d.name = "  ".to_owned();
    assert!(!d.can_save());
    d.name = String::new();
    assert!(!d.can_save());
    d.name = "x".to_owned();
    d.query = String::new();
    assert!(d.can_save(), "an empty query is a draft too");
}

// --- "Discard lines like this…" ---------------------------------------------------

fn record(service: &str, body: &str) -> LogRecord {
    LogRecord {
        time_unix_nano: BASE,
        observed_unix_nano: BASE,
        severity: Severity::Error,
        severity_text: String::new(),
        body: body.to_owned(),
        attributes: Vec::new(),
        trace_id: None,
        span_id: None,
        resource: Arc::new(Resource {
            attributes: vec![(
                "service.name".to_owned(),
                AttrValue::Str(service.to_owned()),
            )],
        }),
        scope_name: String::new(),
    }
}

/// The suggestion for `body` from `payments-api`: its name and its query.
fn suggestion(body: &str) -> (String, String) {
    let draft = suggest_from_log(&record("payments-api", body));
    (draft.name, draft.query)
}

#[test]
fn a_short_body_is_the_whole_name_and_the_whole_filter() {
    let draft = suggest_from_log(&record("orders-api", "Created order 8812 for cart c-5521"));

    assert_eq!(
        draft.name,
        r#"Lines like "Created order 8812 for cart c-5521""#
    );
    assert_eq!(draft.language, RuleLanguage::LogQl);
    assert_eq!(
        draft.query,
        r#"{service_name="orders-api"} |= "Created order 8812 for cart c-5521""#
    );
    assert!(draft.enabled, "it is a rule to save, so it starts on");
}

#[test]
fn the_name_cuts_at_40_characters_with_an_ellipsis() {
    let body = "Charge failed for order 8812: CardGatewayTimeoutException: card-gateway did not respond after 2 attempts";
    let (name, _) = suggestion(body);
    assert_eq!(
        name,
        "Lines like \"Charge failed for order 8812: CardGatewa\u{2026}\""
    );
}

#[test]
fn the_name_has_no_ellipsis_when_the_body_fits_in_40() {
    let forty = "a".repeat(40);
    assert_eq!(suggestion(&forty).0, format!("Lines like \"{forty}\""));
    let forty_one = "a".repeat(41);
    assert_eq!(
        suggestion(&forty_one).0,
        format!("Lines like \"{}\u{2026}\"", "a".repeat(40))
    );
}

#[test]
fn the_filter_cuts_at_the_last_word_boundary_within_60_characters() {
    let body = "Charge failed for order 8812: CardGatewayTimeoutException: card-gateway did not respond after 2 attempts";
    let (_, query) = suggestion(body);
    // The first 60 characters end inside `card-gateway`, so it goes back to
    // the space before it.
    assert_eq!(
        query,
        r#"{service_name="payments-api"} |= "Charge failed for order 8812: CardGatewayTimeoutException:""#
    );
}

#[test]
fn a_cut_that_falls_on_a_word_boundary_keeps_the_whole_60() {
    // 59 characters, a space (the 60th), then more: the cut is at a boundary.
    let sixty = format!("{} ", "a".repeat(59));
    let body = format!("{sixty}tail of the line");
    assert_eq!(sixty.chars().count(), 60);
    let (_, query) = suggestion(&body);
    assert_eq!(
        query,
        format!(r#"{{service_name="payments-api"}} |= "{}""#, "a".repeat(59))
    );

    // Whereas 60 characters then a space is one whole word of 60.
    let body = format!("{} tail", "a".repeat(60));
    let (_, query) = suggestion(&body);
    assert_eq!(
        query,
        format!(r#"{{service_name="payments-api"}} |= "{}""#, "a".repeat(60))
    );
}

#[test]
fn a_body_of_exactly_60_characters_is_not_cut() {
    let body = format!("{} {}", "a".repeat(30), "b".repeat(29));
    assert_eq!(body.chars().count(), 60);
    let (_, query) = suggestion(&body);
    assert_eq!(
        query,
        format!(r#"{{service_name="payments-api"}} |= "{body}""#)
    );
}

#[test]
fn a_long_word_with_no_boundary_is_cut_hard_at_60() {
    let body = "x".repeat(200);
    let (_, query) = suggestion(&body);
    assert_eq!(
        query,
        format!(r#"{{service_name="payments-api"}} |= "{}""#, "x".repeat(60))
    );
}

#[test]
fn quotes_and_backslashes_are_escaped_in_the_filter_and_not_in_the_name() {
    let body = r#"Cannot parse "C:\temp\x" as JSON"#;
    let (name, query) = suggestion(body);
    assert_eq!(name, format!("Lines like \"{body}\""));
    assert_eq!(
        query,
        r#"{service_name="payments-api"} |= "Cannot parse \"C:\\temp\\x\" as JSON""#
    );
    // And it is a query that finds the line it came from.
    let parsed = logql::parse(&query).expect("the suggestion is valid LogQL");
    assert!(parsed.matches(&record("payments-api", body)));
}

#[test]
fn the_service_name_is_escaped_too() {
    let draft = suggest_from_log(&record(r#"we"ird\svc"#, "hello"));
    assert_eq!(draft.query, r#"{service_name="we\"ird\\svc"} |= "hello""#);
    let parsed = logql::parse(&draft.query).expect("valid LogQL");
    assert!(parsed.matches(&record(r#"we"ird\svc"#, "hello")));
}

#[test]
fn only_the_first_line_of_a_multi_line_body_is_used() {
    let body = "Unhandled exception\n  at Gateway.charge(Gateway.cs:41)\n  at Api.post";
    let (name, query) = suggestion(body);
    assert_eq!(name, "Lines like \"Unhandled exception\"");
    assert_eq!(
        query,
        r#"{service_name="payments-api"} |= "Unhandled exception""#
    );
}

#[test]
fn leading_blank_lines_are_skipped_so_the_filter_is_never_empty() {
    let (name, query) = suggestion("\n\r\n  \nreal message\nmore");
    assert_eq!(name, "Lines like \"real message\"");
    assert_eq!(query, r#"{service_name="payments-api"} |= "real message""#);
}

#[test]
fn a_body_with_nothing_in_it_filters_on_the_service_alone() {
    // There is no text to hold on to. The query is still valid, and the editor's
    // preview says what it would drop before anything is saved.
    for body in ["", "  \n \t"] {
        let draft = suggest_from_log(&record("payments-api", body));
        assert_eq!(draft.query, r#"{service_name="payments-api"}"#);
        assert!(draft.validate().query.is_ok());
    }
}

#[test]
fn a_cut_counts_characters_not_bytes() {
    let body = "\u{e9}".repeat(50);
    let (name, query) = suggestion(&body);
    assert_eq!(
        name,
        format!("Lines like \"{}\u{2026}\"", "\u{e9}".repeat(40))
    );
    assert_eq!(
        query,
        format!(
            r#"{{service_name="payments-api"}} |= "{}""#,
            "\u{e9}".repeat(50)
        )
    );
    let body = "\u{e9}".repeat(100);
    let (_, query) = suggestion(&body);
    assert_eq!(
        query,
        format!(
            r#"{{service_name="payments-api"}} |= "{}""#,
            "\u{e9}".repeat(60)
        )
    );
}

#[test]
fn the_suggestion_is_an_unsaved_logql_draft_that_is_valid_and_saveable() {
    let draft = suggest_from_log(&record("payments-api", "boom"));
    assert_eq!(draft.language, RuleLanguage::LogQl);
    assert_eq!(draft.scope, DropScope::Trace, "ignored for LogQL");
    let check = draft.validate();
    assert!(check.name_ok);
    assert_eq!(check.query, Ok(()));
    assert!(draft.can_save());
}

#[test]
fn every_log_of_the_failed_checkout_is_matched_by_its_own_suggestion() {
    let generated = generate(&Scenario::CheckoutFailure, 7, BASE);
    let mut seen = 0;
    for batch in &generated.batches {
        let Some(request) = &batch.logs else { continue };
        for log in logs_from_request(request.clone()).items {
            let draft = suggest_from_log(&log);
            assert_eq!(draft.language, RuleLanguage::LogQl);
            let query = logql::parse(&draft.query)
                .unwrap_or_else(|e| panic!("{:?} is not LogQL: {e}", draft.query));
            assert!(
                query.matches(&log),
                "{:?} does not match its own line {:?}",
                draft.query,
                log.body
            );
            assert!(draft.name.starts_with("Lines like \""));
            seen += 1;
        }
    }
    assert_eq!(seen, 7);
}

// --- the screen's own texts (Task 5) -----------------------------------------------

#[test]
fn the_fixed_texts_of_the_screen_are_the_plans() {
    assert_eq!(
        SUBTITLE,
        "Anything a rule matches is dropped as it arrives and never stored. \
         Rules are kept when loctel restarts."
    );
    assert_eq!(
        NO_RULES,
        "No discard rules. Nothing is dropped unless you add one."
    );
    assert_eq!(PREVIEW_TITLE, "Tried on the last 5 minutes");
    assert_eq!(PREVIEW_INVALID, "Fix the query to see what it would drop.");
}

#[test]
fn the_file_error_banner_names_the_error_and_says_the_old_rules_stay() {
    assert_eq!(
        file_error_line("expected a table, found a string at line 3"),
        "The rules file has an error: expected a table, found a string at line 3. \
         loctel keeps using the rules it had."
    );
    // An error that ends with a full stop does not end up with two.
    assert_eq!(
        file_error_line("unknown key `scpoe`."),
        "The rules file has an error: unknown key `scpoe`. \
         loctel keeps using the rules it had."
    );
    // The error of the TOML parser is several lines; the banner is one.
    assert_eq!(
        file_error_line("TOML parse error at line 2, column 1\n  |\n2 | [[rule]\n  | ^\n"),
        "The rules file has an error: TOML parse error at line 2, column 1 | 2 | [[rule] | ^. \
         loctel keeps using the rules it had."
    );
    // An error with no words still reads as a sentence.
    assert_eq!(
        file_error_line("  \n"),
        "The rules file has an error. loctel keeps using the rules it had."
    );
}

fn preview(traces: u64, spans: u64, logs: u64, error_traces: u64, error_logs: u64) -> Preview {
    Preview {
        traces,
        spans,
        logs,
        error_traces,
        error_logs,
    }
}

#[test]
fn a_trace_rule_preview_counts_spans_traces_and_the_traces_with_errors() {
    let trace_rule = RuleLanguage::TraceQl;
    assert_eq!(
        preview_text(trace_rule, &preview(602, 1_204, 0, 0, 0)),
        "Would have dropped 1,204 spans in 602 traces. None of those traces had errors."
    );
    assert_eq!(
        preview_text(trace_rule, &preview(602, 1_204, 0, 3, 0)),
        "Would have dropped 1,204 spans in 602 traces. 3 of those traces had errors."
    );
    // Logs of the dropped traces are not part of the sentence.
    assert_eq!(
        preview_text(trace_rule, &preview(602, 1_204, 9_999, 3, 5)),
        "Would have dropped 1,204 spans in 602 traces. 3 of those traces had errors."
    );
}

#[test]
fn a_trace_rule_preview_is_singular_for_one_and_says_nothing_for_none() {
    let trace_rule = RuleLanguage::TraceQl;
    assert_eq!(
        preview_text(trace_rule, &preview(1, 1, 0, 0, 0)),
        "Would have dropped 1 span in 1 trace. That trace had no errors."
    );
    assert_eq!(
        preview_text(trace_rule, &preview(1, 15, 0, 1, 0)),
        "Would have dropped 15 spans in 1 trace. That trace had errors."
    );
    assert_eq!(
        preview_text(trace_rule, &preview(0, 0, 0, 0, 0)),
        "Would have dropped nothing."
    );
}

#[test]
fn a_log_rule_preview_counts_lines_and_the_errors_among_them() {
    let log_rule = RuleLanguage::LogQl;
    assert_eq!(
        preview_text(log_rule, &preview(0, 0, 96, 0, 2)),
        "Would have dropped 96 lines. 2 were errors."
    );
    assert_eq!(
        preview_text(log_rule, &preview(0, 0, 96, 0, 0)),
        "Would have dropped 96 lines. None were errors."
    );
    assert_eq!(
        preview_text(log_rule, &preview(0, 0, 96, 0, 1)),
        "Would have dropped 96 lines. 1 was an error."
    );
    assert_eq!(
        preview_text(log_rule, &preview(0, 0, 12_345, 0, 1_200)),
        "Would have dropped 12,345 lines. 1,200 were errors."
    );
    assert_eq!(
        preview_text(log_rule, &preview(0, 0, 1, 0, 1)),
        "Would have dropped 1 line. It was an error."
    );
    assert_eq!(
        preview_text(log_rule, &preview(0, 0, 1, 0, 0)),
        "Would have dropped 1 line. It was not an error."
    );
    assert_eq!(
        preview_text(log_rule, &preview(0, 0, 0, 0, 0)),
        "Would have dropped nothing."
    );
}

#[test]
fn the_editor_says_where_the_query_is_wrong_but_not_before_anything_is_typed() {
    // Nothing typed: the query is an error to the parser, and the editor keeps
    // quiet about it (the preview says to fix it).
    assert_eq!(draft_error_line(&draft(RuleLanguage::TraceQl, "")), None);
    assert_eq!(draft_error_line(&draft(RuleLanguage::LogQl, "  \n ")), None);
    // A good query has no line.
    assert_eq!(
        draft_error_line(&draft(RuleLanguage::TraceQl, r#"{ name = "x" }"#)),
        None
    );
    // A bad one says `at column N: message`, with the column the draft's own
    // check gives (counted from 1).
    let bad = draft(RuleLanguage::TraceQl, "{ status = ");
    let (column, message) = bad.validate().query.expect_err("it is not a query");
    assert_eq!(
        draft_error_line(&bad),
        Some(format!("at column {column}: {message}"))
    );
    let bad_logs = draft(RuleLanguage::LogQl, "{service_name=\"x\"");
    let (column, message) = bad_logs.validate().query.expect_err("it is not a query");
    assert_eq!(
        draft_error_line(&bad_logs),
        Some(format!("at column {column}: {message}"))
    );
    // The language decides which parser reads it: a TraceQL query is not LogQL.
    assert!(draft_error_line(&draft(RuleLanguage::LogQl, "{ duration > 1s }")).is_some());
    assert_eq!(
        draft_error_line(&draft(RuleLanguage::TraceQl, "{ duration > 1s }")),
        None
    );
}
