//! LogQL lexer, parser, label set and evaluator: what parses, what does not and
//! where the error is, the label mapping, the parsers (`| json`, `| logfmt`),
//! label filters and their precedence. The grammar followed is Loki's
//! `pkg/logql/syntax/syntax.y` (see `docs/areas/query.md`).

mod fixtures;

use fixtures::{
    checkout_failure_logs, count_logs, log_in_service, log_with_attr, log_with_resource_attr,
    mk_log,
};
use loctel_model::{AttrValue, LogRecord, Severity, SpanId, TraceId, loki_label_name};
use loctel_query::logql::{
    self, LabelFilter, LabelMatcher, LineFilter, LogQuery, NumberOp, Stage, StringMatcher, labels,
    parse,
};
use loctel_query::{Pattern, QueryError};

// ---- helpers -----------------------------------------------------------------

/// A log from the service `svc` with `body`.
fn svc_log(body: &str) -> LogRecord {
    log_in_service(mk_log(body), "svc")
}

/// A log from `svc` with the given string attributes.
fn svc_log_with(body: &str, attrs: &[(&str, &str)]) -> LogRecord {
    attrs.iter().fold(svc_log(body), |log, (key, value)| {
        log_with_attr(log, key, AttrValue::Str((*value).to_owned()))
    })
}

fn matches(query: &str, log: &LogRecord) -> bool {
    parse(query)
        .unwrap_or_else(|err| panic!("{query}: {err}"))
        .matches(log)
}

/// Whether `{service_name="svc"}` followed by `stages` matches `log`.
fn stages_match(stages: &str, log: &LogRecord) -> bool {
    matches(&format!(r#"{{service_name="svc"}} {stages}"#), log)
}

fn err(query: &str) -> QueryError {
    match parse(query) {
        Ok(parsed) => panic!("{query} parsed, as {parsed:?}"),
        Err(err) => err,
    }
}

/// The character index of the first `needle` in `text`.
fn char_index(text: &str, needle: &str) -> usize {
    let byte = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is not in {text:?}"));
    text[..byte].chars().count()
}

// ---- the plan's table -----------------------------------------------------------

#[test]
fn plan_queries_count_the_checkout_failure_logs() {
    let logs = checkout_failure_logs();
    assert_eq!(logs.len(), 7);
    let cases: &[(&str, usize)] = &[
        (r#"{service_name="payments-api"}"#, 4),
        (r#"{service_name=~"orders-api|payments-api"} |= "8812""#, 5),
        (r#"{service_name="payments-api"} | level = "error""#, 2),
        (r#"{service_name="payments-api"} |~ "attempt [0-9]""#, 1),
        (r#"{service_name="payments-api"} != "card-gateway""#, 1),
        (
            r#"{service_name="orders-api", deployment_environment="local"}"#,
            3,
        ),
        (r#"{level="error"}"#, 3),
        (r#"{service_name=~"orders"}"#, 0), // anchored
    ];
    for (query, expected) in cases {
        assert_eq!(count_logs(query, &logs), *expected, "{query}");
    }
}

#[test]
fn json_parser_and_numeric_filter() {
    let log = svc_log(r#"{"user":{"id":42},"msg":"x"}"#);
    assert!(stages_match("| json | user_id = 42", &log));
    assert!(!stages_match("| json | user_id > 50", &log));
    // The nested field is flattened with `_`, the top-level one keeps its name.
    assert!(stages_match(
        r#"| json | user_id = "42" and msg = "x""#,
        &log
    ));
}

#[test]
fn logfmt_parser_and_duration_filter() {
    let log = svc_log("method=GET status=500 took=12ms");
    assert!(stages_match("| logfmt | status >= 500", &log));
    assert!(stages_match("| logfmt | took > 10ms", &log));
    assert!(!stages_match("| logfmt | took > 20ms", &log));
    assert!(stages_match(r#"| logfmt | method = "GET""#, &log));
}

#[test]
fn line_filter_regex_is_unanchored() {
    let log = svc_log("card-gateway did not answer, retrying (attempt 2 of 2)");
    assert!(stages_match(r#"|~ "attempt""#, &log));
    assert!(stages_match(r#"|~ "attempt [0-9]""#, &log));
    assert!(!stages_match(r#"|~ "^attempt""#, &log));
    assert!(!stages_match(r#"!~ "attempt""#, &log));
    assert!(stages_match(r#"!~ "^attempt""#, &log));
    // The compiled patterns say so too.
    let query = parse(r#"{a="b"} |~ "x" !~ "y""#).unwrap();
    for stage in &query.stages {
        match stage {
            Stage::LineFilter(LineFilter::Regex(pattern) | LineFilter::NotRegex(pattern)) => {
                assert!(!pattern.is_anchored());
            }
            other => panic!("not a regex line filter: {other:?}"),
        }
    }
}

#[test]
fn label_matcher_regex_is_anchored() {
    let logs = checkout_failure_logs();
    assert_eq!(count_logs(r#"{service_name=~"orders"}"#, &logs), 0);
    assert_eq!(count_logs(r#"{service_name=~"orders.*"}"#, &logs), 3);
    assert_eq!(count_logs(r#"{service_name=~"orders-api"}"#, &logs), 3);
    // `!~` is exactly the opposite: nothing is called just "orders".
    assert_eq!(count_logs(r#"{service_name!~"orders"}"#, &logs), 7);
    assert_eq!(count_logs(r#"{service_name!~"orders.*"}"#, &logs), 4);
    // And the same in a label filter.
    let log = svc_log("x");
    assert!(!stages_match(r#"| service_name =~ "sv""#, &log));
    assert!(stages_match(r#"| service_name =~ "sv.""#, &log));
    assert!(stages_match(r#"| service_name !~ "sv""#, &log));
    // The alternation is anchored as a whole: `a|b` is `^(?:a|b)$`.
    assert!(!matches(
        r#"{service_name=~"s|v"}"#,
        &svc_log("x") // "svc" is neither "s" nor "v"
    ));
    let query = parse(r#"{a=~"x", b!~"y"} | c =~ "z""#).unwrap();
    let mut patterns = Vec::new();
    for matcher in &query.matchers {
        if let StringMatcher::Regex(p) | StringMatcher::NotRegex(p) = &matcher.matcher {
            patterns.push(p.clone());
        }
    }
    if let Stage::LabelFilter(LabelFilter::String(LabelMatcher {
        matcher: StringMatcher::Regex(p),
        ..
    })) = &query.stages[0]
    {
        patterns.push(p.clone());
    }
    assert_eq!(patterns.len(), 3);
    assert!(patterns.iter().all(Pattern::is_anchored));
}

/// Every recognised-but-unsupported feature: the exact message, and the column
/// of the feature itself (not of the `|` before it).
const UNSUPPORTED: &[(&str, usize, &str)] = &[
    (
        r#"{service_name="x"} | line_format "{{.a}}""#,
        22,
        "line_format is not supported yet",
    ),
    (
        r#"rate({service_name="x"}[5m])"#,
        1,
        "metrics queries are not supported yet",
    ),
    (
        r#"{a="b"} | label_format x=y"#,
        11,
        "label_format is not supported yet",
    ),
    (
        r#"{a="b"} | unwrap latency"#,
        11,
        "unwrap is not supported yet",
    ),
    (
        r#"{a="b"} | decolorize"#,
        11,
        "decolorize is not supported yet",
    ),
    (r#"{a="b"} | drop x"#, 11, "drop is not supported yet"),
    (r#"{a="b"} | keep x"#, 11, "keep is not supported yet"),
    (
        r#"{a="b"} | pattern "<_> x""#,
        11,
        "pattern is not supported yet",
    ),
    (
        r#"{a="b"} | regexp "(?P<x>.)""#,
        11,
        "regexp is not supported yet",
    ),
    (r#"{a="b"} | unpack"#, 11, "unpack is not supported yet"),
    (
        r#"{a="b"} |= ip("10.0.0.1")"#,
        12,
        "ip is not supported yet",
    ),
    (
        r#"{a="b"} != ip("10.0.0.1")"#,
        12,
        "ip is not supported yet",
    ),
    (
        r#"{a="b"} | addr = ip("10.0.0.1")"#,
        18,
        "ip is not supported yet",
    ),
    (
        r#"{a="b"} | addr != ip("10.0.0.1")"#,
        19,
        "ip is not supported yet",
    ),
    (
        r#"{a="b"} |> "<_> x""#,
        9,
        "the pattern filter `|>` is not supported yet",
    ),
    (
        r#"{a="b"} !> "<_> x""#,
        9,
        "the pattern filter `!>` is not supported yet",
    ),
    (
        r#"{a="b"} |= "x" or "y""#,
        16,
        "or between line filters is not supported yet",
    ),
    (
        r#"{a="b"} | json x="y""#,
        16,
        "json label extraction is not supported yet",
    ),
    (
        r#"{a="b"} | logfmt --strict"#,
        18,
        "a logfmt flag is not supported yet",
    ),
    (
        r#"{a="b"} | logfmt x"#,
        18,
        "logfmt label extraction is not supported yet",
    ),
    (
        r#"{a="b"} | size > 10MB"#,
        18,
        "a byte size is not supported yet",
    ),
    (
        r#"sum by (service_name) (rate({a="b"}[1m]))"#,
        1,
        "metrics queries are not supported yet",
    ),
    (
        r#"  count_over_time({a="b"}[5m])"#,
        3,
        "metrics queries are not supported yet",
    ),
    (
        r#"topk(3, sum(rate({a="b"}[5m])))"#,
        1,
        "metrics queries are not supported yet",
    ),
    // Keywords are case-insensitive in Loki, and so are these.
    (
        r#"{a="b"} | LINE_FORMAT "x""#,
        11,
        "line_format is not supported yet",
    ),
    (
        r#"RATE({a="b"}[5m])"#,
        1,
        "metrics queries are not supported yet",
    ),
];

#[test]
fn unsupported_features_are_errors_with_position() {
    for (query, column, message) in UNSUPPORTED {
        let error = err(query);
        assert_eq!(error.message, *message, "{query}");
        assert_eq!(error.position + 1, *column, "{query}");
        assert_eq!(
            error.to_string(),
            format!("at column {column}: {message}"),
            "{query}"
        );
    }
}

#[test]
fn a_label_may_be_called_like_a_function() {
    // `ip`, `rate`, `count` and `sum` are functions only before `(`.
    let log = svc_log_with("x", &[("ip", "10.0.0.1"), ("rate", "5"), ("count", "2")]);
    assert!(matches(
        r#"{service_name="svc", ip="10.0.0.1"} | rate = 5 | count > 1"#,
        &log
    ));
}

#[test]
fn empty_selector_is_error() {
    for query in ["{}", "{ }", r#"{} |= "x""#, " {}"] {
        let error = err(query);
        assert!(
            error.message.contains("at least one matcher"),
            "{query}: {error}"
        );
        // At the opening brace.
        assert_eq!(error.position, char_index(query, "{"), "{query}");
    }
}

#[test]
fn trailing_comma_in_a_selector_is_an_error() {
    let error = err(r#"{a="b",}"#);
    assert_eq!(error.position, 7);
    assert_eq!(error.message, "expected a label name, found `}`");
}

// ---- parse errors -------------------------------------------------------------

/// `(query, where, message)`: `where` is the text the error points at (its
/// first occurrence), or `None` for the end of the query.
const SYNTAX_ERRORS: &[(&str, Option<&str>, &str)] = &[
    ("", None, "expected `{`"),
    ("   ", None, "expected `{`"),
    ("foo", Some("foo"), "expected `{`, found `foo`"),
    ("|= \"x\"", Some("|="), "expected `{`, found `|=`"),
    ("{", None, "expected a label name"),
    ("{a", None, "expected `=`, `!=`, `=~` or `!~`"),
    (
        "{a}",
        Some("}"),
        "expected `=`, `!=`, `=~` or `!~`, found `}`",
    ),
    ("{a=}", Some("}"), "expected a string, found `}`"),
    (
        "{a==\"b\"}",
        Some("=="),
        "expected `=`, `!=`, `=~` or `!~`, found `==`",
    ),
    (r#"{a="b""#, None, "expected `,` or `}`"),
    (
        r#"{a="b" c="d"}"#,
        Some("c"),
        "expected `,` or `}`, found `c`",
    ),
    (
        r#"{a="b"} |"#,
        None,
        "expected `json`, `logfmt` or a label filter",
    ),
    (
        r#"{a="b"} | )"#,
        Some(")"),
        "expected `json`, `logfmt` or a label filter, found `)`",
    ),
    (r#"{a="b"} |= "#, None, "expected a string"),
    (r#"{a="b"} |= 5"#, Some("5"), "expected a string, found `5`"),
    (
        r#"{a="b"} |= "x" "y""#,
        Some(r#""y""#),
        "expected a pipeline stage or the end of the query, found `\"y\"`",
    ),
    (r#"{a="b"} | x"#, None, "expected a comparison operator"),
    (r#"{a="b"} | x ! 5"#, Some("!"), "unexpected character `!`"),
    (
        r#"{a="b"} | x > "5""#,
        Some(r#""5""#),
        "expected a number or duration, found `\"5\"`",
    ),
    (
        r#"{a="b"} | x == "s""#,
        Some(r#""s""#),
        "expected a number or duration, found `\"s\"`",
    ),
    (
        r#"{a="b"} | x =~ 5"#,
        Some("5"),
        "expected a string, found `5`",
    ),
    (
        r#"{a="b"} | x = "#,
        None,
        "expected a string, number or duration",
    ),
    (r#"{a="b"} | (x="1""#, None, "expected `)`"),
    (r#"{a="b"} | x="1" and"#, None, "expected a label filter"),
    (
        r#"{a="b"} | x="1" or )"#,
        Some(")"),
        "expected a label filter, found `)`",
    ),
    (
        r#"{a="b"} foo"#,
        Some("foo"),
        "expected a pipeline stage or the end of the query, found `foo`",
    ),
    (r#"{a="b"} [5m]"#, Some("["), "unexpected character `[`"),
    ("{a=~\"(\"}", Some("\"(\""), "invalid regular expression"),
    ("{a!~\"[\"}", Some("\"[\""), "invalid regular expression"),
    (
        r#"{a="b"} |~ "(""#,
        Some("\"(\""),
        "invalid regular expression",
    ),
    (
        r#"{a="b"} | x =~ "(""#,
        Some("\"(\""),
        "invalid regular expression",
    ),
    (r#"{a="b} "#, Some("\"b"), "unterminated string"),
    ("{a=`b}", Some("`b"), "unterminated string"),
    (r#"{a="\d"}"#, Some("\\d"), "invalid escape sequence `\\d`"),
    (
        r#"{a="\xff"}"#,
        Some("\\xff"),
        "invalid escape sequence `\\xff`",
    ),
    (r#"{a="b"} @"#, Some("@"), "unexpected character `@`"),
    (
        r#"{a="b"} | x > 5d"#,
        Some("5d"),
        "invalid duration `5d` (units are ns, us, µs, ms, s, m, h)",
    ),
    (
        r#"{a="b"} | x > 1.2.3"#,
        Some("1.2.3"),
        "invalid number `1.2.3`",
    ),
    (
        r#"{a="b"} | x > 1e999"#,
        Some("1e999"),
        "invalid number `1e999`",
    ),
];

#[test]
fn parse_errors_have_positions() {
    for (query, at, message) in SYNTAX_ERRORS {
        let error = err(query);
        let position = match at {
            Some(needle) => char_index(query, needle),
            None => query.chars().count(),
        };
        assert!(
            error.message.starts_with(message),
            "{query:?}: {:?} does not start with {message:?}",
            error.message
        );
        assert_eq!(error.position, position, "{query:?}: {error}");
    }
}

#[test]
fn a_dotted_label_name_gets_a_hint() {
    let error = err(r#"{deployment.environment="local"}"#);
    assert_eq!(error.position, 1);
    assert!(
        error.message.contains("`deployment.environment`")
            && error.message.contains("`deployment_environment`"),
        "{error}"
    );
    let error = err(r#"{service_name="x"} | http.status > 500"#);
    assert!(error.message.contains("`http_status`"), "{error}");
    let error = err(r#"{http-method="x"}"#);
    assert!(error.message.contains("`http_method`"), "{error}");
}

#[test]
fn the_first_problem_is_the_one_reported() {
    // A bad character after a syntax error does not win.
    let error = err(r#"{a="b"} | x @"#);
    assert_eq!(error.position, char_index(r#"{a="b"} | x @"#, "@"));
    let query = r#"{a= } @"#;
    assert_eq!(err(query).position, char_index(query, "}"));
}

#[test]
fn positions_count_characters_not_bytes() {
    // `é` is two bytes and `日本` six: the column is still the character.
    let query = r#"{service_name="é日本"} | line_format "x""#;
    let error = err(query);
    assert_eq!(error.message, "line_format is not supported yet");
    assert_eq!(
        error.position,
        query.chars().take_while(|c| *c != 'l').count()
    );
    assert!(error.position < query.len());
    // The end of a non-ASCII query is its character count.
    let query = r#"{service_name="é""#;
    assert_eq!(err(query).position, query.chars().count());
}

// ---- what parses -----------------------------------------------------------------

const MUST_PARSE: &[&str] = &[
    r#"{app="foo"}"#,
    r#"{ app = "foo" }"#,
    r#"{app="foo", env=~"prod|staging", tier!="db", zone!~"eu-.*"}"#,
    r#"{app="foo"} |= "error""#,
    r#"{app="foo"} != "debug" |~ "err.*" !~ "timeout""#,
    r#"{app="foo"} |= "a" != "b" |~ "c" !~ "d""#,
    r#"{app="foo"} | json"#,
    r#"{app="foo"} | logfmt"#,
    r#"{app="foo"} | json | logfmt"#,
    r#"{app="foo"} | logfmt | level="error""#,
    r#"{app="foo"} | json | status >= 500 and method="GET""#,
    r#"{app="foo"} | json | (status >= 500 or status = 404) and method != "POST""#,
    r#"{app="foo"} | json | status == 500"#,
    r#"{app="foo"} | json | status = 500, method = "GET""#,
    r#"{app="foo"} | json | status = 500 method = "GET""#,
    r#"{app="foo"} | duration >= 20ms or duration < 1.5s"#,
    r#"{app="foo"} | x = 1h30m"#,
    r#"{app="foo"} | x > 1.5 | y < .5 | z != 5."#,
    r#"{app="foo"} | x > -5 | y < +5 | z > -1.5e3 | w = 2e-3"#,
    r#"{app="foo"} | x > -5s"#,
    r#"{app="foo"} | x = `a\b` | y =~ `\d+`"#,
    r#"{app="foo"} |~ `\d+ items`"#,
    r#"{app="foo"} | ((x="1"))"#,
    r#"{app="foo"} | __error__ = """#,
    r#"{app="foo"} | __error__ != "JSONParserErr""#,
    r#"{APP="foo"} | JSON | A="1" AND B="2" OR C="3""#,
    r#"{app="foo"} | LogFmt"#,
    "{app=\"foo\"} # a comment\n |= \"x\" # another\n | json",
    r#"{ app = "foo" , env = "x" } |= "a" | json | a = "b""#,
    r#"{app="a\"b\\c\n\té\x41\101"}"#,
    r#"{app="foo"} | label = "tab\there""#,
    r#"{app=""}"#,
    r#"{app="foo"} |= """#,
    r#"{app="foo"} | a > 1 | b > 2 | c > 3"#,
];

#[test]
fn must_pass_queries_parse() {
    for query in MUST_PARSE {
        if let Err(error) = parse(query) {
            panic!("{query} should parse, but: {error}");
        }
    }
}

#[test]
fn parsing_is_deterministic_and_trees_compare() {
    for query in MUST_PARSE {
        assert_eq!(parse(query).unwrap(), parse(query).unwrap(), "{query}");
    }
    assert_ne!(
        parse(r#"{a="b"}"#).unwrap(),
        parse(r#"{a="c"}"#).unwrap(),
        "different queries give different trees"
    );
    assert_ne!(
        parse(r#"{a="b"} |= "x""#).unwrap(),
        parse(r#"{a="b"} != "x""#).unwrap()
    );
    // The same query with different spacing and comments is the same tree.
    assert_eq!(
        parse(r#"{a="b",c=~"d"} |= "x""#).unwrap(),
        parse("{ a = \"b\" ,\n c =~ \"d\" } # note\n |= \"x\"").unwrap()
    );
}

#[test]
fn the_syntax_tree_of_a_query() {
    let query = parse(
        r#"{service_name="api", level!~"debug|trace"} |= "boom" | json | status >= 500 and took > 1.5s"#,
    )
    .unwrap();
    assert_eq!(
        query,
        LogQuery {
            matchers: vec![
                LabelMatcher {
                    label: "service_name".into(),
                    matcher: StringMatcher::Equal("api".into()),
                },
                LabelMatcher {
                    label: "level".into(),
                    matcher: StringMatcher::NotRegex(Pattern::anchored("debug|trace").unwrap()),
                },
            ],
            stages: vec![
                Stage::LineFilter(LineFilter::Contains("boom".into())),
                Stage::Json,
                Stage::LabelFilter(LabelFilter::And(
                    Box::new(LabelFilter::Number {
                        label: "status".into(),
                        op: NumberOp::GreaterOrEqual,
                        value: 500.0,
                    }),
                    Box::new(LabelFilter::Duration {
                        label: "took".into(),
                        op: NumberOp::Greater,
                        nanos: 1_500_000_000,
                    }),
                )),
            ],
        }
    );
    // `=` with a number is numeric, with a string it is a string match, and
    // `==` is the numeric one.
    let query = parse(r#"{a="b"} | n = 5 | s = "5" | m == 5"#).unwrap();
    assert!(matches!(
        &query.stages[0],
        Stage::LabelFilter(LabelFilter::Number {
            op: NumberOp::Equal,
            ..
        })
    ));
    assert!(matches!(
        &query.stages[1],
        Stage::LabelFilter(LabelFilter::String(_))
    ));
    assert!(matches!(
        &query.stages[2],
        Stage::LabelFilter(LabelFilter::Number {
            op: NumberOp::Equal,
            ..
        })
    ));
    // A negative literal keeps its sign.
    let query = parse(r#"{a="b"} | n > -5 | d > -5s"#).unwrap();
    assert!(matches!(
        &query.stages[0],
        Stage::LabelFilter(LabelFilter::Number { value, .. }) if *value == -5.0
    ));
    assert!(matches!(
        &query.stages[1],
        Stage::LabelFilter(LabelFilter::Duration { nanos, .. }) if *nanos == -5_000_000_000
    ));
}

#[test]
fn strings_and_escapes() {
    let log = svc_log("a\"b\\c\td é");
    assert!(stages_match(r#"|= "a\"b\\c\td é""#, &log));
    assert!(stages_match(r#"|= "é""#, &log));
    assert!(stages_match(r#"|= "\U000000e9""#, &log));
    assert!(stages_match(r#"|= "\x61\142""#, &svc_log("ab")));
    // Backticks are raw: no escapes, so regexes stay readable.
    assert!(stages_match(r#"|= `a"b\c`"#, &log));
    assert!(stages_match(
        r#"|~ `\d+ items`"#,
        &svc_log("found 12 items")
    ));
    assert!(!stages_match(
        r#"|~ `\d+ items`"#,
        &svc_log("found many items")
    ));
    assert!(stages_match(
        r#"|= "\a\b\f\v\r\n""#,
        &svc_log("\x07\x08\x0c\x0b\r\n")
    ));
    // A double-quoted `\d` is the same error as in Go.
    assert_eq!(
        err(r#"{a="b"} |~ "\d+""#).message,
        "invalid escape sequence `\\d`"
    );
}

#[test]
fn keywords_are_case_insensitive_and_comments_are_skipped() {
    let log = svc_log_with(r#"{"x":"1"}"#, &[("y", "1")]);
    assert!(stages_match(r#"| JSON | x = "1" AND y = "1""#, &log));
    assert!(stages_match(r#"| Json | x = "1" Or y = "2""#, &log));
    assert!(stages_match("# comment\n| json # more\n | x = \"1\"", &log));
    assert!(matches("{service_name=\"svc\"} # no stages yet", &log));
}

// ---- line filters ---------------------------------------------------------------

#[test]
fn line_filters_match_the_body() {
    let log = svc_log("Charge failed for order 8812");
    assert!(stages_match(r#"|= "failed""#, &log));
    assert!(!stages_match(r#"|= "Failed""#, &log)); // case-sensitive
    assert!(stages_match(r#"|~ "(?i)FAILED""#, &log));
    assert!(stages_match(r#"!= "succeeded""#, &log));
    assert!(!stages_match(r#"!= "failed""#, &log));
    assert!(stages_match(
        r#"|= "Charge" != "refund" |~ "order [0-9]+""#,
        &log
    ));
    assert!(!stages_match(r#"|= "Charge" != "order""#, &log));
    // The empty string is in every line.
    assert!(stages_match(r#"|= """#, &log));
    assert!(!stages_match(r#"!= """#, &log));
    assert!(stages_match(r#"|= """#, &svc_log("")));
}

#[test]
fn line_filters_see_the_original_body_not_a_parsed_label() {
    let log = svc_log(r#"{"msg":"hello"}"#);
    // Still the whole JSON line after `| json`.
    assert!(stages_match(r#"| json |= "msg""#, &log));
    assert!(stages_match(r#"| json |= "{\"msg\"""#, &log));
}

// ---- the label set --------------------------------------------------------------

fn label<'a>(set: &'a [(String, String)], name: &str) -> Option<&'a str> {
    set.iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn labels_include_resource_log_attrs_level_scope() {
    // The failing charge's last log: on span 10, with the exception attributes.
    let logs = checkout_failure_logs();
    let log = logs
        .iter()
        .find(|log| log.body.starts_with("Charge failed"))
        .unwrap();
    let set = labels(log);

    // Sorted by name, one entry per name.
    let names: Vec<&str> = set.iter().map(|(name, _)| name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(names, sorted);

    // Every resource attribute, under its mapped name.
    assert_eq!(label(&set, "service_name"), Some("payments-api"));
    assert_eq!(label(&set, "service_version"), Some("1.4.0"));
    assert_eq!(label(&set, "deployment_environment"), Some("local"));
    // Every log attribute.
    assert_eq!(
        label(&set, "exception_type"),
        log_attr(log, "exception.type").as_deref()
    );
    assert!(
        label(&set, "exception_message")
            .unwrap()
            .starts_with("card-gateway did not respond")
    );
    assert!(label(&set, "exception_stacktrace").is_some());
    // Severity, scope and trace context.
    assert_eq!(label(&set, "level"), Some("error"));
    assert_eq!(label(&set, "scope_name"), Some(log.scope_name.as_str()));
    assert!(!log.scope_name.is_empty());
    assert_eq!(
        label(&set, "trace_id"),
        Some(log.trace_id.unwrap().to_string().as_str())
    );
    assert_eq!(
        label(&set, "span_id"),
        Some(log.span_id.unwrap().to_string().as_str())
    );
    // Nothing else: 3 resource attributes + 3 exception attributes + the 5
    // computed ones, and `service_name` is both a resource attribute and
    // computed.
    assert_eq!(set.len(), 3 + 3 + 4, "{set:?}");
}

fn log_attr(log: &LogRecord, key: &str) -> Option<String> {
    log.attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.to_display_string())
}

#[test]
fn label_names_map_every_odd_character_to_an_underscore() {
    let log = log_with_attr(
        log_with_resource_attr(
            mk_log("x"),
            "deployment.environment",
            AttrValue::Str("local".into()),
        ),
        "http.request-method",
        AttrValue::Str("GET".into()),
    );
    let set = labels(&log);
    assert_eq!(label(&set, "deployment_environment"), Some("local"));
    assert_eq!(label(&set, "http_request_method"), Some("GET"));
    assert_eq!(label(&set, "deployment.environment"), None);
}

#[test]
fn label_values_are_the_display_strings() {
    let mut log = mk_log("x");
    log.attributes = vec![
        ("n".into(), AttrValue::Int(-42)),
        ("f".into(), AttrValue::Double(1.5)),
        ("b".into(), AttrValue::Bool(true)),
        ("bytes".into(), AttrValue::Bytes(vec![0xde, 0xad])),
        (
            "list".into(),
            AttrValue::Array(vec![AttrValue::Int(1), AttrValue::Str("a".into())]),
        ),
        (
            "map".into(),
            AttrValue::Map(vec![("k".into(), AttrValue::Int(1))]),
        ),
    ];
    let set = labels(&log);
    assert_eq!(label(&set, "n"), Some("-42"));
    assert_eq!(label(&set, "f"), Some("1.5"));
    assert_eq!(label(&set, "b"), Some("true"));
    assert_eq!(label(&set, "bytes"), Some("dead"));
    assert_eq!(label(&set, "list"), Some(r#"[1,"a"]"#));
    assert_eq!(label(&set, "map"), Some(r#"{"k":1}"#));
    // And a query reads them the same way.
    assert!(matches(
        r#"{n="-42", b="true", f="1.5", bytes="dead"}"#,
        &log
    ));
}

#[test]
fn computed_labels_win_and_resource_wins_over_log_attributes() {
    let mut log = log_in_service(mk_log("x"), "real");
    log.severity = Severity::Warn;
    log.scope_name = "my.scope".into();
    log = log_with_resource_attr(log, "env", AttrValue::Str("from-resource".into()));
    // Log attributes that collide with a computed label or a resource label.
    log.attributes = vec![
        ("env".into(), AttrValue::Str("from-log".into())),
        ("level".into(), AttrValue::Str("from-log".into())),
        ("service.name".into(), AttrValue::Str("from-log".into())),
        ("scope_name".into(), AttrValue::Str("from-log".into())),
        ("trace_id".into(), AttrValue::Str("app-trace".into())),
    ];
    let set = labels(&log);
    assert_eq!(label(&set, "env"), Some("from-resource"));
    assert_eq!(label(&set, "level"), Some("warn"));
    assert_eq!(label(&set, "service_name"), Some("real"));
    assert_eq!(label(&set, "scope_name"), Some("my.scope"));
    // Without trace context, an attribute called `trace_id` stays.
    assert_eq!(label(&set, "trace_id"), Some("app-trace"));
    log.trace_id = Some(TraceId([0xab; 16]));
    log.span_id = Some(SpanId([0xcd; 8]));
    let set = labels(&log);
    assert_eq!(
        label(&set, "trace_id"),
        Some("abababababababababababababababab")
    );
    assert_eq!(label(&set, "span_id"), Some("cdcdcdcdcdcdcdcd"));
}

#[test]
fn the_first_of_two_attributes_with_one_mapped_name_wins() {
    let mut log = mk_log("x");
    log.attributes = vec![
        ("a.b".into(), AttrValue::Str("first".into())),
        ("a_b".into(), AttrValue::Str("second".into())),
    ];
    assert_eq!(label(&labels(&log), "a_b"), Some("first"));
    assert!(matches(r#"{a_b="first"}"#, &log));
}

#[test]
fn defaults_for_a_bare_log() {
    let set = labels(&mk_log("x"));
    assert_eq!(
        set,
        vec![
            ("level".to_owned(), "info".to_owned()),
            ("service_name".to_owned(), "unknown_service".to_owned()),
        ],
        "no scope name, no trace context, no attributes"
    );
    let mut log = mk_log("x");
    log.severity = Severity::Unspecified;
    assert_eq!(label(&labels(&log), "level"), Some("unknown"));
    for (severity, name) in [
        (Severity::Trace, "trace"),
        (Severity::Debug, "debug"),
        (Severity::Info, "info"),
        (Severity::Warn, "warn"),
        (Severity::Error, "error"),
        (Severity::Fatal, "fatal"),
    ] {
        log.severity = severity;
        assert_eq!(label(&labels(&log), "level"), Some(name));
        assert!(matches(&format!(r#"{{level="{name}"}}"#), &log));
    }
}

#[test]
fn a_label_with_an_empty_value_does_not_exist() {
    let mut log = mk_log("x");
    log.attributes = vec![
        ("empty".into(), AttrValue::Str(String::new())),
        ("dup".into(), AttrValue::Str(String::new())),
        ("dup".into(), AttrValue::Str("second".into())),
    ];
    let set = labels(&log);
    assert_eq!(label(&set, "empty"), None);
    // The empty one is skipped before the first-wins rule.
    assert_eq!(label(&set, "dup"), Some("second"));
}

/// The selector reads labels straight from the record, without building the
/// set; the two must agree on every name.
#[test]
fn selector_lookup_agrees_with_the_label_set() {
    let mut weird = log_with_resource_attr(
        log_with_resource_attr(mk_log("x"), "a.b", AttrValue::Str("r".into())),
        "ünï-code",
        AttrValue::Str("u".into()),
    );
    weird.attributes = vec![
        ("a.b".into(), AttrValue::Str("l".into())),
        ("c d".into(), AttrValue::Int(7)),
        ("e".into(), AttrValue::Str(String::new())),
        ("trace_id".into(), AttrValue::Str("app".into())),
        ("level".into(), AttrValue::Str("shadowed".into())),
    ];
    weird.scope_name = "scope".into();
    let mut traced = weird.clone();
    traced.trace_id = Some(TraceId([1; 16]));
    traced.span_id = Some(SpanId([2; 8]));

    let mut all = checkout_failure_logs();
    all.extend([weird, traced, mk_log("bare")]);
    for log in &all {
        let set = labels(log);
        for (name, value) in &set {
            let query = format!("{{{name}=`{value}`}}");
            assert!(matches(&query, log), "{query} on {set:?}");
            let query = format!("{{{name}!=`{value}`}}");
            assert!(!matches(&query, log), "{query}");
        }
        // And a name the set lacks reads as empty.
        for name in ["nosuch", "e", "ünï", "a_b_"] {
            if label(&set, name).is_none() {
                let query = format!(r#"{{service_name!="", {name}=""}}"#);
                assert!(matches(&query, log), "{query}");
            }
        }
    }
}

#[test]
fn label_name_mapping_is_the_models() {
    // `loctel_model::loki_label_name` is the one definition; the selector's
    // non-allocating comparison has to give its answers. Seen through a query:
    // an attribute under any odd key is found by its mapped name.
    for key in [
        "plain",
        "a.b",
        "a-b",
        "a b/c:d",
        "naïve",
        "日本",
        "x__y",
        "_",
        "a.b.c.d",
        "ünï-code",
    ] {
        let mapped = loki_label_name(key);
        let log = log_with_attr(mk_log("x"), key, AttrValue::Str("v".into()));
        if mapped.is_empty() {
            continue;
        }
        let query = format!(r#"{{{mapped}="v"}}"#);
        assert!(matches(&query, &log), "{key} -> {query}");
    }
}

// ---- missing labels and the selector --------------------------------------------

#[test]
fn a_missing_label_is_an_empty_one_in_string_matchers() {
    let log = svc_log("x");
    for (matcher, expected) in [
        (r#"nosuch="""#, true),
        (r#"nosuch!="""#, false),
        (r#"nosuch!="x""#, true),
        (r#"nosuch="x""#, false),
        (r#"nosuch=~".*""#, true),
        (r#"nosuch=~".+""#, false),
        (r#"nosuch!~".+""#, true),
        (r#"nosuch!~".*""#, false),
    ] {
        let selector = format!(r#"{{service_name="svc", {matcher}}}"#);
        assert_eq!(matches(&selector, &log), expected, "{selector}");
        assert_eq!(
            stages_match(&format!("| {matcher}"), &log),
            expected,
            "| {matcher}"
        );
    }
}

#[test]
fn every_selector_matcher_must_hold() {
    let logs = checkout_failure_logs();
    assert_eq!(
        count_logs(r#"{service_name="payments-api", level="error"}"#, &logs),
        2
    );
    assert_eq!(
        count_logs(r#"{service_name="payments-api", level!="error"}"#, &logs),
        2
    );
    assert_eq!(
        count_logs(
            r#"{service_name="payments-api", service_name="orders-api"}"#,
            &logs
        ),
        0
    );
    assert_eq!(count_logs(r#"{scope_name="loctel-meshgen"}"#, &logs), 7);
    assert_eq!(count_logs(r#"{service_version="1.4.0"}"#, &logs), 7);
    assert_eq!(count_logs(r#"{exception_type=~".+"}"#, &logs), 1);
    assert_eq!(count_logs(r#"{trace_id!=""}"#, &logs), 7);
    assert_eq!(count_logs(r#"{span_id!=""}"#, &logs), 7);
}

#[test]
fn service_hint_exact_only() {
    let hint = |query: &str| parse(query).unwrap().service_hint().map(str::to_owned);
    assert_eq!(
        hint(r#"{service_name="payments-api"}"#).as_deref(),
        Some("payments-api")
    );
    assert_eq!(
        hint(r#"{level="error", service_name="web-bff"} |= "x" | json"#).as_deref(),
        Some("web-bff")
    );
    assert_eq!(hint(r#"{service_name=`raw`}"#).as_deref(), Some("raw"));
    // Anything but an exact match is no hint: a regex, a negation, or another
    // label.
    assert_eq!(hint(r#"{service_name=~"payments-api"}"#), None);
    assert_eq!(hint(r#"{service_name=~"payments.*"}"#), None);
    assert_eq!(hint(r#"{service_name!="payments-api"}"#), None);
    assert_eq!(hint(r#"{service_name!~"payments-api"}"#), None);
    assert_eq!(hint(r#"{level="error"}"#), None);
    assert_eq!(hint(r#"{service="payments-api"}"#), None);
    // A filter after the selector is not a selector matcher.
    assert_eq!(
        hint(r#"{level="error"} | service_name = "payments-api""#),
        None
    );
    // The hint is a prefilter: every log the query matches is from that service.
    let logs = checkout_failure_logs();
    let query = parse(r#"{service_name="payments-api"} |= "order""#).unwrap();
    let hinted = query.service_hint().unwrap();
    assert!(
        logs.iter()
            .filter(|log| query.matches(log))
            .all(|log| log.service() == hinted)
    );
}

// ---- label filters --------------------------------------------------------------

#[test]
fn numeric_comparisons() {
    let mut log = svc_log("x");
    log.attributes = vec![("n".into(), AttrValue::Int(5))];
    for (filter, expected) in [
        ("n = 5", true),
        ("n == 5", true),
        ("n = 5.0", true),
        ("n == 5e0", true),
        ("n = 4", false),
        ("n != 4", true),
        ("n != 5", false),
        ("n > 4", true),
        ("n > 5", false),
        ("n >= 5", true),
        ("n >= 6", false),
        ("n < 6", true),
        ("n < 5", false),
        ("n <= 5", true),
        ("n <= 4", false),
        ("n > -5", true),
        ("n > +4", true),
        ("n < -5", false),
    ] {
        assert_eq!(
            stages_match(&format!("| {filter}"), &log),
            expected,
            "{filter}"
        );
    }
    // Floats, from an attribute and from a parsed field.
    let mut log = svc_log("x");
    log.attributes = vec![("f".into(), AttrValue::Double(1.5))];
    assert!(stages_match("| f > 1 | f < 2 | f = 1.5 | f != 1", &log));
    assert!(stages_match("| json | n > 1e2", &svc_log(r#"{"n":150}"#)));
    assert!(stages_match("| json | n < 0", &svc_log(r#"{"n":-1}"#)));
}

#[test]
fn numeric_filters_on_missing_or_unparseable_labels_are_false() {
    let log = svc_log_with("x", &[("s", "abc"), ("took", "12ms"), ("n", "5")]);
    for filter in [
        // missing
        "nosuch = 5",
        "nosuch != 5",
        "nosuch > 5",
        "nosuch < 5",
        "nosuch == 0",
        "nosuch > 1s",
        "nosuch != 1s",
        // not a number (and not a duration)
        "s = 5",
        "s != 5",
        "s > 0",
        "s < 0",
        "s > 1s",
        "s != 1s",
        // a duration where a number is wanted, and the other way round
        "took > 1",
        "took != 1",
        "n > 1s",
        "n != 1s",
        "n < 1ms",
    ] {
        assert!(!stages_match(&format!("| {filter}"), &log), "| {filter}");
    }
}

#[test]
fn duration_comparisons() {
    let log = svc_log_with(
        "x",
        &[
            ("took", "1.5s"),
            ("fast", "250us"),
            ("neg", "-5ms"),
            ("zero", "0"),
            ("compound", "1h30m"),
        ],
    );
    for (filter, expected) in [
        ("took > 1s", true),
        ("took > 1500ms", false),
        ("took >= 1500ms", true),
        ("took == 1500ms", true),
        ("took = 1.5s", true),
        ("took != 1500ms", false),
        ("took < 2s", true),
        ("took <= 1s", false),
        ("fast < 1ms", true),
        ("fast = 250µs", true),
        ("fast = 250us", true),
        ("fast = 250000ns", true),
        ("neg < 0s", true),
        ("neg == -5ms", true),
        ("neg > -1s", true),
        ("zero == 0s", true),
        ("zero < 1ns", true),
        ("compound == 90m", true),
        ("compound > 1h", true),
        ("took > 1h30m", false),
    ] {
        assert_eq!(
            stages_match(&format!("| {filter}"), &log),
            expected,
            "{filter}"
        );
    }
}

/// `x`, `y`, `z`, `w` stand for `x="1"` and so on; each is true when its
/// attribute is "1". Checks `expr` against `expected` over all 16 settings.
fn check_truth_table(expr: &str, expected: impl Fn(bool, bool, bool, bool) -> bool) {
    let mut query = String::from(r#"{service_name="svc"} | "#);
    for c in expr.chars() {
        match c {
            'x' | 'y' | 'z' | 'w' => query.push_str(&format!(r#"{c}="1""#)),
            c => query.push(c),
        }
    }
    let parsed = parse(&query).unwrap_or_else(|e| panic!("{query}: {e}"));
    for bits in 0..16u8 {
        let [x, y, z, w] = [0, 1, 2, 3].map(|i| bits & (1 << i) != 0);
        let value = |b: bool| if b { "1" } else { "0" };
        let log = svc_log_with(
            "line",
            &[
                ("x", value(x)),
                ("y", value(y)),
                ("z", value(z)),
                ("w", value(w)),
            ],
        );
        assert_eq!(
            parsed.matches(&log),
            expected(x, y, z, w),
            "{expr}  with x={x} y={y} z={z} w={w}"
        );
    }
}

/// Loki's grammar (`syntax.y`) has `%left OR` below `%left AND`, and gives the
/// comma and the space no precedence at all, so yacc shifts: whatever follows a
/// comma or a space is grouped first. Each of these was checked against the
/// grammar run through goyacc (see docs/areas/query.md).
#[test]
fn label_filter_precedence_follows_loki() {
    // `and` binds tighter than `or`, both left to right.
    check_truth_table("x or y and z", |x, y, z, _| x || (y && z));
    check_truth_table("x and y or z", |x, y, z, _| (x && y) || z);
    check_truth_table("x or y or z", |x, y, z, _| x || y || z);
    check_truth_table("x and y and z", |x, y, z, _| x && y && z);
    check_truth_table("x or y and z or w", |x, y, z, w| x || (y && z) || w);
    check_truth_table("x and y or z and w", |x, y, z, w| (x && y) || (z && w));
    // A comma, or a space, is an `and` that reaches to the right.
    check_truth_table("x, y", |x, y, _, _| x && y);
    check_truth_table("x y", |x, y, _, _| x && y);
    check_truth_table("x, y or z", |x, y, z, _| x && (y || z));
    check_truth_table("x y or z", |x, y, z, _| x && (y || z));
    check_truth_table("x or y, z", |x, y, z, _| x || (y && z));
    check_truth_table("x or y z", |x, y, z, _| x || (y && z));
    check_truth_table("x and y, z or w", |x, y, z, w| x && y && (z || w));
    check_truth_table("x, y, z", |x, y, z, _| x && y && z);
    check_truth_table("x y z w", |x, y, z, w| x && y && z && w);
    // Parentheses override all of it.
    check_truth_table("(x or y) and z", |x, y, z, _| (x || y) && z);
    check_truth_table("(x or y), z", |x, y, z, _| (x || y) && z);
    check_truth_table("(x or y) z", |x, y, z, _| (x || y) && z);
    check_truth_table("x and (y or z)", |x, y, z, _| x && (y || z));
    check_truth_table("((x or y) and (z or w))", |x, y, z, w| (x || y) && (z || w));
    check_truth_table("(x, y) or (z, w)", |x, y, z, w| (x && y) || (z && w));
}

#[test]
fn separate_stages_are_an_and_too() {
    check_truth_table("x | y | z", |x, y, z, _| x && y && z);
    check_truth_table("x or y | z", |x, y, z, _| (x || y) && z);
}

#[test]
fn string_label_filters() {
    let log = svc_log_with("x", &[("method", "GET"), ("path", "/api/orders/7")]);
    assert!(stages_match(r#"| method = "GET""#, &log));
    assert!(stages_match(r#"| method != "POST""#, &log));
    assert!(stages_match(r#"| path =~ "/api/.*""#, &log));
    assert!(!stages_match(r#"| path =~ "/api""#, &log));
    assert!(stages_match(r#"| path !~ "/admin/.*""#, &log));
    assert!(stages_match(r#"| level = "info""#, &log));
    assert!(!stages_match(r#"| level = "error""#, &log));
    assert!(stages_match(
        r#"| service_name = "svc" and scope_name = """#,
        &log
    ));
}

// ---- | json ---------------------------------------------------------------------

#[test]
fn json_flattens_nested_objects_and_skips_arrays_and_nulls() {
    let log = svc_log(
        r#"{"user":{"id":42,"name":"bob","tags":["a","b"]},"msg":"x","ok":true,"n":null,"list":[1,2],"deep":{"a":{"b":{"c":"d"}}},"order":[{"id":1}]}"#,
    );
    let hit = |filter: &str| stages_match(&format!("| json | {filter}"), &log);
    assert!(hit(r#"user_id = "42""#));
    assert!(hit(r#"user_name = "bob""#));
    assert!(hit(r#"msg = "x""#));
    assert!(hit(r#"ok = "true""#));
    assert!(hit(r#"deep_a_b_c = "d""#));
    // Arrays (and what is inside them) and null are skipped.
    assert!(hit(r#"user_tags = """#));
    assert!(hit(r#"list = """#));
    assert!(hit(r#"order_id = """#));
    assert!(hit(r#"n = """#));
    assert!(!hit(r#"list != """#));
    // The top-level object itself is no label.
    assert!(hit(r#"user = """#));
}

#[test]
fn json_values_keep_their_text() {
    let log = svc_log(
        r#"{"price": 129.00, "big": 12345678901234567890123, "exp": 1e3, "neg": -0.50, "s": "a\"b\\c\ndé😀", "empty": ""}"#,
    );
    let hit = |filter: &str| stages_match(&format!("| json | {filter}"), &log);
    // Numbers are the text from the body, not a re-formatted float.
    assert!(hit(r#"price = "129.00""#));
    assert!(hit(r#"big = "12345678901234567890123""#));
    assert!(hit(r#"exp = "1e3""#));
    assert!(hit(r#"neg = "-0.50""#));
    // And they still compare as numbers.
    assert!(hit("price == 129 | exp = 1000 | neg < 0"));
    // Strings are unescaped, surrogate pairs included.
    assert!(hit("s = \"a\\\"b\\\\c\\nd\u{e9}\u{1f600}\""));
    // An empty value is no label.
    assert!(hit(r#"empty = """#));
}

#[test]
fn json_keys_are_sanitized() {
    let log = svc_log(r#"{"http.method":"GET","a-b":{"c d":1},"9lives":"x","日本":"y"}"#);
    let hit = |filter: &str| stages_match(&format!("| json | {filter}"), &log);
    assert!(hit(r#"http_method = "GET""#));
    assert!(hit("a_b_c_d = 1"));
    assert!(hit(r#"_9lives = "x""#));
    assert!(hit(r#"__ = "y""#));
}

#[test]
fn extracted_labels_that_collide_get_a_suffix() {
    // `level` and `service_name` are labels of the log already.
    let log = svc_log(r#"{"level":"debug","service_name":"other","msg":"x"}"#);
    let hit = |filter: &str| stages_match(&format!("| json | {filter}"), &log);
    assert!(hit(r#"level = "info""#), "the label of the log stays");
    assert!(hit(r#"level_extracted = "debug""#));
    assert!(hit(r#"service_name = "svc""#));
    assert!(hit(r#"service_name_extracted = "other""#));
    assert!(hit(r#"msg = "x""#));
    assert!(!hit(r#"msg_extracted != """#));
    // A nested one too, and logfmt does the same.
    let log = svc_log_with(r#"{"user":{"id":1}}"#, &[("user_id", "stream")]);
    assert!(stages_match(
        r#"| json | user_id = "stream" and user_id_extracted = "1""#,
        &log
    ));
    let log = svc_log("level=debug msg=x");
    assert!(stages_match(
        r#"| logfmt | level = "info" and level_extracted = "debug" and msg = "x""#,
        &log
    ));
}

#[test]
fn a_parser_run_twice_does_not_suffix_its_own_labels() {
    // The suffix is for a collision with the labels of the log, not with what
    // an earlier stage extracted.
    let log = svc_log("a=1");
    assert!(stages_match("| logfmt | logfmt | a = 1", &log));
    assert!(!stages_match(
        r#"| logfmt | logfmt | a_extracted != """#,
        &log
    ));
    let log = svc_log(r#"{"a":"1"}"#);
    assert!(stages_match(r#"| json | json | a = "1""#, &log));
}

#[test]
fn json_errors_set_the_error_label_and_drop_nothing() {
    for body in [
        "not json at all",
        "",
        "42",
        r#""just a string""#,
        "[1, 2, 3]",
        r#"{"a":"#,
        r#"{"a" 1}"#,
        r#"{"a":tru}"#,
        r#"{"a":1 "b":2}"#,
        r#"{"a":1,,"b":2}"#,
        r#"xx{"a":1}"#,
        r#"{a:1}"#,
        r#"{"a":"unterminated}"#,
        r#"{"a":[1,2 }"#,
        r#"{"a":+1}"#,
        "{",
    ] {
        let log = svc_log(body);
        assert!(stages_match("| json", &log), "{body:?} is not dropped");
        assert!(
            stages_match(r#"| json | __error__ = "JSONParserErr""#, &log),
            "{body:?} gets the error label"
        );
        assert!(
            !stages_match(r#"| json | __error__ = """#, &log),
            "{body:?}"
        );
    }
    // A good body has no error label, and the usual way to drop the bad ones
    // works.
    let good = svc_log(r#"{"a":"1"}"#);
    assert!(stages_match(r#"| json | __error__ = """#, &good));
    assert!(stages_match(
        r#"| json | __error__ != "JSONParserErr""#,
        &good
    ));
    assert!(!stages_match(
        r#"| json | __error__ = "JSONParserErr""#,
        &good
    ));
    // Numeric filters on a line that failed to parse have no label to read:
    // false, so a rule `| json | status >= 500` never drops a plain-text line.
    let text = svc_log("status 503 from upstream");
    assert!(stages_match("| json", &text));
    assert!(!stages_match("| json | status >= 500", &text));
    assert!(!stages_match("| json | status != 500", &text));
    // Labels read before the error stay.
    let partial = svc_log(r#"{"a":"1","b":"#);
    assert!(stages_match(
        r#"| json | a = "1" and __error__ = "JSONParserErr""#,
        &partial
    ));
}

#[test]
fn json_is_as_lenient_as_lokis_scanner() {
    let hit =
        |body: &str, filter: &str| stages_match(&format!("| json | {filter}"), &svc_log(body));
    // Whatever comes after the closing brace is not looked at.
    assert!(hit(r#"{"a":1} trailing"#, r#"a = 1 and __error__ = """#));
    assert!(hit(
        r#"{"a":1}{"b":2}"#,
        r#"a = 1 and b = "" and __error__ = """#
    ));
    // A trailing comma is fine.
    assert!(hit(r#"{"a":1,}"#, r#"a = 1 and __error__ = """#));
    assert!(hit(r#"{"a":1, }"#, r#"a = 1 and __error__ = """#));
    // A number is whatever runs up to the next delimiter, checked by nobody.
    assert!(hit(
        r#"{"a":01,"b":1.,"c":-,"d":1x}"#,
        r#"a = "01" and b = "1." and c = "-" and d = "1x""#
    ));
    // Arrays are skipped by matching their brackets, not by reading them.
    assert!(hit(
        r#"{"a":[1,2,],"b":"x"}"#,
        r#"b = "x" and a = "" and __error__ = """#
    ));
    assert!(hit(
        r#"{"a":[[1],["]"],{"k":"]"}],"b":"x"}"#,
        r#"b = "x" and __error__ = """#
    ));
    assert!(hit(
        r#"{"a":[1,"\"]"],"b":"x"}"#,
        r#"b = "x" and __error__ = """#
    ));
    // A string with a bad escape is a field with an empty value: no label, no
    // error. So is a surrogate with no partner.
    assert!(hit(
        r#"{"a":"x\q","b":"y"}"#,
        r#"a = "" and b = "y" and __error__ = """#
    ));
    assert!(hit(
        r#"{"a":"\u12","b":"y"}"#,
        r#"a = "" and b = "y" and __error__ = """#
    ));
    assert!(hit(
        r#"{"a":"\ud800","b":"y"}"#,
        r#"a = "" and b = "y" and __error__ = """#
    ));
    assert!(hit(r#"{"a":"\ud83d\ude00"}"#, "a = \"\u{1f600}\""));
    // A key is unescaped, and a bad escape in one is an error.
    assert!(hit(r#"{"a\u0062":1}"#, "ab = 1"));
    assert!(hit(r#"{"a\q":1}"#, r#"__error__ = "JSONParserErr""#));
    // `true`, `false` and `null` must be spelled out.
    assert!(hit(
        r#"{"a":true,"b":false,"c":null}"#,
        r#"a = "true" and b = "false" and c = """#
    ));
    assert!(hit(r#"{"a":nul}"#, r#"__error__ = "JSONParserErr""#));
}

#[test]
fn json_keeps_the_first_value_of_a_name_and_skips_blank_keys() {
    let hit =
        |body: &str, filter: &str| stages_match(&format!("| json | {filter}"), &svc_log(body));
    // The first one wins, in one object and across the flattening.
    assert!(hit(r#"{"a":1,"a":2}"#, "a = 1"));
    assert!(hit(r#"{"a":{"b":1,"b":2}}"#, "a_b = 1"));
    assert!(hit(r#"{"a":{"b":1},"a_b":2}"#, "a_b = 1"));
    assert!(hit(r#"{"a_b":2,"a":{"b":1}}"#, "a_b = 2"));
    // A blank key leaves no mark on the name.
    assert!(hit(
        r#"{"":1,"a":{"":2},"b":{" ":{"c":3}}}"#,
        "a = 2 and b_c = 3"
    ));
    // Escaped keys and unicode keys.
    assert!(hit(r#"{"é":"é"}"#, r#"_ = "é""#));
}

#[test]
fn json_nesting_has_a_limit_and_does_not_overflow_the_stack() {
    let run = || {
        let deep = format!("{}1{}", r#"{"a":"#.repeat(10_000), "}".repeat(10_000));
        let log = svc_log(&deep);
        assert!(stages_match(
            r#"| json | __error__ = "JSONParserErr""#,
            &log
        ));
        // Reasonable nesting is fine.
        let nested = format!("{}1{}", r#"{"a":"#.repeat(20), "}".repeat(20));
        let log = svc_log(&nested);
        assert!(stages_match(r#"| json | __error__ = """#, &log));
        let name = vec!["a"; 20].join("_");
        assert!(stages_match(&format!("| json | {name} = 1"), &log));
        let arrays = "[".repeat(50_000);
        assert!(stages_match(
            r#"| json | __error__ = "JSONParserErr""#,
            &svc_log(&arrays)
        ));
        let arrays_in_object = format!(r#"{{"a":{}}}"#, "[".repeat(50_000));
        assert!(stages_match(
            r#"| json | __error__ = "JSONParserErr""#,
            &svc_log(&arrays_in_object)
        ));
    };
    // A small stack, like a worker thread's.
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn json_whitespace_and_unicode_are_fine() {
    let log = svc_log("  \n\t{ \"a\" : \"é日本\" ,\r\n \"b\" : { \"c\" : 1 } }  \n");
    assert!(stages_match(r#"| json | a = "é日本" and b_c = 1"#, &log));
    assert!(stages_match(r#"| json | __error__ = """#, &log));
}

// ---- | logfmt -------------------------------------------------------------------

#[test]
fn logfmt_pairs_quotes_and_escapes() {
    let log = svc_log(
        r#"msg="hello world" code=7 quote="say \"hi\"" nl="a\nb" slash="a\/b\'c" 	 tab=ok uni="\u00e9\ud83d\ude00""#,
    );
    let hit = |filter: &str| stages_match(&format!("| logfmt | {filter}"), &log);
    assert!(hit(r#"msg = "hello world""#));
    assert!(hit("code = 7"));
    assert!(hit(r#"quote = "say \"hi\"""#));
    assert!(hit(r#"nl = "a\nb""#));
    assert!(hit(r#"slash = "a/b'c""#));
    assert!(hit("uni = \"\u{e9}\u{1f600}\""));
    assert!(
        hit(r#"tab = "ok""#),
        "tabs and runs of spaces separate pairs"
    );
}

#[test]
fn logfmt_skips_bare_keys_and_empty_values() {
    let log = svc_log("flag empty= b=1 =orphan");
    let hit = |filter: &str| stages_match(&format!("| logfmt | {filter}"), &log);
    assert!(hit(r#"flag = "" and empty = """#));
    assert!(hit(r#"b = "1""#));
    // Plain text has no pairs and is no error: lenient, like Loki's default.
    for body in ["hello world", "", "   ", "=", "\"", "a=\"unterminated"] {
        let log = svc_log(body);
        assert!(stages_match("| logfmt", &log), "{body:?}");
        assert!(
            stages_match(r#"| logfmt | __error__ = """#, &log),
            "{body:?}"
        );
    }
}

/// A pair that breaks the format is dropped, and so is the rest of its word;
/// what follows is read as usual. The expectations are Loki's decoder's.
#[test]
fn logfmt_drops_a_broken_pair_and_goes_on() {
    let labels_of = |body: &str, present: &[&str], absent: &[&str]| {
        let log = svc_log(body);
        for name in present {
            assert!(
                stages_match(&format!(r#"| logfmt | {name} != """#), &log),
                "{body:?} has {name}"
            );
        }
        for name in absent {
            assert!(
                stages_match(&format!(r#"| logfmt | {name} = """#), &log),
                "{body:?} lacks {name}"
            );
        }
    };
    // An `=` or a quote inside an unquoted value.
    labels_of("a=b=c d=e", &["d"], &["a"]);
    labels_of("path=/a/b?x=1 k=v", &["k"], &["path"]);
    labels_of(r#"a=x"y z=1"#, &["z"], &["a"]);
    // A quote in a key, an empty key.
    labels_of(r#"a"b"=1 c=2"#, &["c"], &["a", "b"]);
    labels_of("=orphan k=v", &["k"], &["orphan"]);
    // Text right after a closing quote is a bare key.
    labels_of(r#"a="x"y z=1"#, &["a", "z"], &["y"]);
    // An escape logfmt does not have, inside quotes.
    labels_of(r#"a="x\qy" b=1"#, &["b"], &["a"]);
    labels_of(r#"a="x\ay" b=1"#, &["b"], &["a"]);
    labels_of(r#"a="\u12" b=1"#, &["b"], &["a"]);
    // A quote that never closes takes the rest of the line with it.
    labels_of(r#"a=1 msg="never closed b=2"#, &["a"], &["msg", "b"]);
    labels_of(r#"a="x\" b=1"#, &[], &["a", "b"]);
    // A dangling `=` at the end, and a key at the end.
    labels_of("a=1 b=", &["a"], &["b"]);
    labels_of("a=1 b", &["a"], &["b"]);
    labels_of("k=é ünï=2", &["k", "_n_"], &[]);
}

#[test]
fn logfmt_keys_are_sanitized_and_the_first_value_wins() {
    let log = svc_log("http.method=GET a-b=1 9lives=x dup=1 dup=2");
    let hit = |filter: &str| stages_match(&format!("| logfmt | {filter}"), &log);
    assert!(hit(r#"http_method = "GET""#));
    assert!(hit("a_b = 1"));
    assert!(hit(r#"_9lives = "x""#));
    assert!(hit("dup = 1"));
    // Also when the two are on different sides of a collision with a label of
    // the log: the first `level` is `level_extracted`, the second is ignored.
    let log = svc_log("level=a level=b");
    assert!(stages_match(r#"| logfmt | level_extracted = "a""#, &log));
}

#[test]
fn logfmt_then_filters_then_json() {
    let log = svc_log("status=500 took=12ms level=error");
    assert!(stages_match(
        r#"| logfmt | status >= 500 and took > 10ms and level_extracted = "error""#,
        &log
    ));
    assert!(!stages_match("| logfmt | status >= 501", &log));
    // A label filter before the parser reads the labels of the log only.
    assert!(!stages_match("| status >= 500 | logfmt", &log));
    assert!(stages_match("| logfmt | status >= 500 | json", &log));
}

// ---- evaluation order and limits -------------------------------------------------

#[test]
fn stages_run_in_order() {
    let log = svc_log(r#"{"a":"1"}"#);
    // The filter is before the parser: `a` does not exist yet.
    assert!(!stages_match(r#"| a = "1" | json"#, &log));
    assert!(stages_match(r#"| json | a = "1""#, &log));
    // Line filters in between, on the original line.
    assert!(stages_match(r#"| json |= "a" | a = "1" != "zzz""#, &log));
}

#[test]
fn a_whole_query_may_be_parenthesised() {
    let log = svc_log("boom");
    assert!(matches(r#"({service_name="svc"})"#, &log));
    assert!(matches(
        r#"(({service_name="svc"} |= "boom" | level = "info"))"#,
        &log
    ));
    assert!(!matches(r#"( {service_name="svc"} |= "other" )"#, &log));
    assert_eq!(
        parse(r#"(({a="b"} |= "x"))"#).unwrap(),
        parse(r#"{a="b"} |= "x""#).unwrap()
    );
    // The parentheses must close, and nothing follows them.
    let error = err(r#"({service_name="svc"}"#);
    assert_eq!(error.message, "expected `)`");
    assert_eq!(error.position, 21);
    let error = err(r#"({service_name="svc"}) |= "x""#);
    assert_eq!(error.message, "expected the end of the query, found `|=`");
    assert_eq!(error.position, 23);
    let error = err(r#"{service_name="svc"})"#);
    assert_eq!(error.position, 20);
    // A metrics query inside them is still one, at its own position.
    let error = err(r#"(rate({a="b"}[5m]))"#);
    assert_eq!(error.message, "metrics queries are not supported yet");
    assert_eq!(error.position, 1);
    // Nesting is limited, like the parentheses of a label filter.
    let nested = |depth: usize| format!(r#"{}{{a="b"}}{}"#, "(".repeat(depth), ")".repeat(depth));
    assert!(parse(&nested(64)).is_ok());
    assert!(err(&nested(65)).message.contains("too deeply nested"));
}

#[test]
fn label_numbers_are_read_like_go_reads_them() {
    // Go's `strconv.ParseFloat`, which Loki uses: `_` between digits, `inf`
    // and `nan` in any case, a sign, an exponent. Not hex, not padding.
    let hit = |value: &str, filter: &str| {
        stages_match(&format!("| {filter}"), &svc_log_with("x", &[("n", value)]))
    };
    assert!(hit("1_000", "n = 1000"));
    assert!(hit("1_0_0", "n = 100"));
    assert!(!hit("1__0", "n >= 0"));
    assert!(!hit("_1", "n >= 0"));
    assert!(!hit("1_", "n >= 0"));
    assert!(!hit("1_.5", "n >= 0"));
    assert!(hit("+5", "n = 5"));
    assert!(hit("-.5", "n < 0"));
    assert!(hit("5.", "n = 5"));
    assert!(hit("1e3", "n = 1000"));
    assert!(hit("1E-3", "n < 1"));
    assert!(hit("Inf", "n > 1e300"));
    assert!(hit("-infinity", "n < -1e300"));
    assert!(hit("NaN", "n != 5"), "NaN is a number, and differs from 5");
    assert!(!hit("NaN", "n > 0"));
    assert!(!hit("0x10", "n >= 0"));
    assert!(!hit(" 5", "n = 5"));
    assert!(!hit("5 ", "n = 5"));
    assert!(!hit("", "n = 0"));
}

/// Loki also applies a label filter that stands *before* `| json` or
/// `| logfmt` to the label the parser extracts (an optimisation that pushes
/// single-label filters into the parsers). loctel runs the pipeline in order,
/// as the language is documented: the filter sees only what exists at its
/// stage. A pipeline written that way matches less here, never more.
#[test]
fn a_label_filter_before_a_parser_does_not_see_what_the_parser_extracts() {
    let log = svc_log("x=1 y=2");
    assert!(stages_match(r#"| x != "1" | logfmt"#, &log));
    assert!(!stages_match(r#"| logfmt | x != "1""#, &log));
    assert!(!stages_match(r#"| x = "1" | logfmt"#, &log));
    assert!(stages_match(r#"| logfmt | x = "1""#, &log));
}

/// Loki's line-filter simplification makes an empty regex in a label filter
/// match everything. Here the regex is anchored like every label regex, so
/// `=~ ""` matches the labels that are empty or missing, as in Prometheus.
#[test]
fn an_empty_regex_in_a_label_filter_matches_only_an_empty_label() {
    let log = svc_log_with("x", &[("a", "1")]);
    assert!(stages_match(r#"| nosuch =~ """#, &log));
    assert!(!stages_match(r#"| a =~ """#, &log));
    assert!(stages_match(r#"| a !~ """#, &log));
    assert!(!stages_match(r#"| nosuch !~ """#, &log));
    assert!(matches(r#"{service_name="svc", nosuch=~""}"#, &log));
    // And as a line filter an empty regex is in every line.
    assert!(stages_match(r#"|~ """#, &log));
}

#[test]
fn nesting_and_operators_are_limited() {
    let nested = |depth: usize| {
        format!(
            r#"{{a="b"}} | {}x="1"{}"#,
            "(".repeat(depth),
            ")".repeat(depth)
        )
    };
    assert!(parse(&nested(64)).is_ok());
    let error = err(&nested(65));
    assert!(error.message.contains("too deeply nested"), "{error}");
    assert!(error.message.contains("64"), "{error}");

    let chain = |join: &str, n: usize| {
        format!(r#"{{a="b"}} | x="1"{}"#, format!("{join}x=\"1\"").repeat(n))
    };
    for join in [" and ", " or ", ", ", " "] {
        assert!(parse(&chain(join, 1000)).is_ok(), "{join:?} x1000");
        let error = err(&chain(join, 1001));
        assert!(
            error.message.contains("too many operators"),
            "{join:?}: {error}"
        );
        assert!(error.message.contains("1000"), "{join:?}: {error}");
    }
    // Neither the parser, nor the evaluator, nor `Drop` may overflow a small
    // stack on the deepest tree that is allowed.
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let log = svc_log_with("x", &[("x", "1")]);
            for join in [" and ", " or ", ", ", " "] {
                let query = format!(
                    r#"{{service_name="svc"}} | x="1"{}"#,
                    format!("{join}x=\"1\"").repeat(1000)
                );
                let parsed = parse(&query).unwrap();
                assert!(parsed.matches(&log));
                let cloned = parsed.clone();
                assert_eq!(cloned, parsed);
                drop(parsed);
            }
            let alternating = format!(
                r#"{{service_name="svc"}} | x="1"{}"#,
                [" and ", ", ", " or ", " "]
                    .iter()
                    .cycle()
                    .take(1000)
                    .map(|join| format!("{join}x=\"1\""))
                    .collect::<String>()
            );
            assert!(parse(&alternating).unwrap().matches(&log));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn many_stages_and_matchers_are_fine() {
    let stages = " |= \"l\"".repeat(10_000);
    let query = format!(r#"{{service_name="svc"}}{stages}"#);
    assert!(parse(&query).unwrap().matches(&svc_log("l")));
    let matchers = vec![r#"service_name!="""#; 5_000].join(", ");
    assert!(
        parse(&format!("{{{matchers}}}"))
            .unwrap()
            .matches(&svc_log("l"))
    );
}

#[test]
fn a_huge_line_and_a_huge_regex_are_handled() {
    let body = "x".repeat(2_000_000);
    let log = svc_log(&body);
    assert!(stages_match(r#"|~ "x{1000}""#, &log));
    // The `regex` crate refuses patterns that compile to something enormous,
    // which comes back as an error and not a hang.
    let error = err(r#"{a="b"} |~ "((((a{1000}){1000}){1000}){1000})""#);
    assert!(
        error.message.starts_with("invalid regular expression"),
        "{error}"
    );
}

#[test]
fn every_truncation_of_every_query_parses_or_errors_without_panicking() {
    let mut queries: Vec<&str> = MUST_PARSE.to_vec();
    queries.extend(UNSUPPORTED.iter().map(|(query, _, _)| *query));
    queries.extend(SYNTAX_ERRORS.iter().map(|(query, _, _)| *query));
    for query in queries {
        let chars: Vec<char> = query.chars().collect();
        for end in 0..=chars.len() {
            let prefix: String = chars[..end].iter().collect();
            if let Err(error) = parse(&prefix) {
                assert!(error.position <= end, "{prefix:?}: {error}");
            }
        }
    }
}

#[test]
fn garbage_never_panics() {
    // Some deterministic noise, built from the query's own alphabet.
    let alphabet: Vec<char> =
        "{}()[],=~!<>|&#\"`\\ \n\t._-+*/@$%^:;?0123456789abcxyzorand json logfmt é日"
            .chars()
            .collect();
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let log = svc_log(r#"{"a":1} a=1"#);
    for _ in 0..20_000 {
        let len = (next() % 40) as usize;
        let text: String = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        if let Ok(query) = parse(&text) {
            let _ = query.matches(&log);
        }
        // And with a valid selector in front, to get past the first token.
        if let Ok(query) = parse(&format!(r#"{{service_name="svc"}} {text}"#)) {
            let _ = query.matches(&log);
        }
    }
}

#[test]
fn matches_does_not_depend_on_what_ran_before() {
    // `matches` takes `&self`: a parser's extracted labels do not leak from one
    // log to the next.
    let query = parse(r#"{service_name="svc"} | json | a = "1""#).unwrap();
    let with = svc_log(r#"{"a":"1"}"#);
    let without = svc_log(r#"{"b":"1"}"#);
    assert!(query.matches(&with));
    assert!(!query.matches(&without));
    assert!(query.matches(&with));
}

#[test]
fn logql_module_exports() {
    // The plan names these paths.
    let _: fn(&str) -> Result<logql::LogQuery, QueryError> = logql::parse;
    let _: fn(&LogRecord) -> Vec<(String, String)> = logql::labels;
    let _: fn(&LogRecord) -> Vec<(String, String)> = loctel_query::labels;
}
