//! `rules.toml`: reading it, writing it, and what a fresh one looks like.

mod fixtures;

use fixtures::*;
use loctel_meshgen::Scenario;
use loctel_query::traceql::{self, TraceView};
use loctel_rules::{DropScope, RuleLanguage, RuleSpec, RulesFile};

const BASE: u64 = 2_000_000_000_000;

fn sample_rules() -> Vec<RuleSpec> {
    vec![
        RuleSpec {
            name: "Health checks".to_string(),
            enabled: true,
            language: RuleLanguage::TraceQl,
            // Quotes, a backslash and a regex: all have to survive the round trip.
            query: r#"{ span.url.path =~ "/(health|ready)(/.*)?" && name != "a\\b" }"#.to_string(),
            scope: DropScope::Trace,
        },
        RuleSpec {
            name: "Noisy \"quoted\" logs".to_string(),
            enabled: false,
            language: RuleLanguage::LogQl,
            query: r#"{service_name="orders-api"} |~ `SELECT .* FROM carts`"#.to_string(),
            scope: DropScope::Trace,
        },
        RuleSpec {
            name: "Just the cache span".to_string(),
            enabled: true,
            language: RuleLanguage::TraceQl,
            query: "{ name = \"MGET\" }".to_string(),
            scope: DropScope::Span,
        },
    ]
}

#[test]
fn missing_file_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");

    assert_eq!(RulesFile::load(&path).unwrap(), None);
    // Loading does not create it.
    assert!(!path.exists());
}

#[test]
fn roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    // The directory does not exist yet: save creates it.
    let path = dir.path().join("nested").join("loctel").join("rules.toml");
    let rules = sample_rules();

    RulesFile::save(&path, &rules).unwrap();

    assert_eq!(RulesFile::load(&path).unwrap(), Some(rules.clone()));
    // Saving again over an existing file gives the same.
    RulesFile::save(&path, &rules[..1]).unwrap();
    assert_eq!(RulesFile::load(&path).unwrap(), Some(rules[..1].to_vec()));
    // The temporary file the atomic write used is gone.
    let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["rules.toml"]);
}

#[test]
fn saved_file_is_human_readable_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");

    RulesFile::save(&path, &sample_rules()).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    // Keys, in a form a person can edit.
    assert!(text.contains("[[rule]]"), "{text}");
    assert!(text.contains("language = \"traceql\""), "{text}");
    assert!(text.contains("language = \"logql\""), "{text}");
    assert!(text.contains("scope = \"span\""), "{text}");
    assert!(text.contains("enabled = false"), "{text}");
    // The explanation stays at the top of a saved file too.
    assert!(text.starts_with('#'), "{text}");
    assert!(text.contains("docs/areas/rules.md"), "{text}");
}

#[test]
fn scope_defaults_to_trace() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    std::fs::write(
        &path,
        r#"
[[rule]]
name = "No scope given"
enabled = true
language = "traceql"
query = "{ status = error }"

[[rule]]
name = "Span scope"
enabled = true
language = "traceql"
query = "{ name = \"x\" }"
scope = "span"
"#,
    )
    .unwrap();

    let rules = RulesFile::load(&path).unwrap().unwrap();

    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].scope, DropScope::Trace);
    assert_eq!(rules[1].scope, DropScope::Span);
    assert_eq!(DropScope::default(), DropScope::Trace);
}

#[test]
fn empty_file_has_no_rules_and_shows_the_example() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("rules.toml");

    RulesFile::write_empty(&path).unwrap();

    // No default rules: loctel never decides what to discard.
    assert_eq!(RulesFile::load(&path).unwrap(), Some(vec![]));
    let text = std::fs::read_to_string(&path).unwrap();
    for line in RulesFile::EXAMPLE_RULE.lines() {
        assert!(
            text.contains(&format!("# {line}")),
            "the header lacks the example line `{line}`:\n{text}"
        );
    }
    // Every example line is commented out, so none of it is a live rule.
    assert!(
        text.lines()
            .all(|line| line.is_empty() || line.starts_with('#')),
        "an empty rules file holds only comments:\n{text}"
    );
    assert!(text.contains("docs/areas/rules.md"), "{text}");
}

#[test]
fn example_rule_matches_health_paths_only() {
    // The example, uncommented, is one rule in a file of its own.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    std::fs::write(&path, RulesFile::EXAMPLE_RULE).unwrap();
    let rules = RulesFile::load(&path).unwrap().unwrap();
    assert_eq!(rules.len(), 1);
    let example = &rules[0];
    assert_eq!(example.name, "Health checks");
    assert!(example.enabled);
    assert_eq!(example.language, RuleLanguage::TraceQl);
    assert_eq!(example.scope, DropScope::Trace);

    let query = traceql::parse(&example.query).unwrap_or_else(|err| panic!("{err}"));

    // Every span of the health checks matches; it is a per-span query.
    let health = scenario_spans(&Scenario::HealthChecks { count: 2 }, 1, BASE);
    assert_eq!(health.len(), 10);
    assert!(!query.needs_whole_trace());
    for span in &health {
        assert_eq!(query.matches_span(span), Some(true), "{}", span.name);
    }
    // And nothing in the failing checkout does.
    let checkout = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    assert!(query.evaluate(&TraceView::new(&checkout)).is_empty());
    assert!(
        checkout
            .iter()
            .all(|span| query.matches_span(span) == Some(false))
    );
}

#[test]
fn example_rule_is_a_toml_rule_table() {
    // The constant is the text of one `[[rule]]` table, ending in a newline
    // so it can be appended to a file.
    assert!(RulesFile::EXAMPLE_RULE.starts_with("[[rule]]\n"));
    assert!(RulesFile::EXAMPLE_RULE.ends_with('\n'));
}

#[test]
fn the_example_can_be_enabled_by_uncommenting_it() {
    // What the header tells a person to do: remove the `# ` from each line of
    // the example.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    RulesFile::write_empty(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let example_lines: Vec<&str> = RulesFile::EXAMPLE_RULE.lines().collect();

    let live: Vec<&str> = text
        .lines()
        .map(|line| {
            line.strip_prefix("# ")
                .filter(|rest| example_lines.contains(rest))
                .unwrap_or(line)
        })
        .collect();
    std::fs::write(&path, live.join("\n")).unwrap();

    let rules = RulesFile::load(&path).unwrap().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].name, "Health checks");
}

#[test]
fn a_hand_written_file_loads_in_file_order() {
    // The example of docs/areas/rules.md: a trace rule, a log rule that says
    // nothing about scope, and a span-scope rule that is switched off.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    std::fs::write(
        &path,
        format!(
            "# my rules\n\n{}\n[[rule]]\nname = \"Framework SQL logs\"\nenabled = true\nlanguage = \"logql\"\nquery = '{{service_name=\"orders-api\"}} |= \"Executed DbCommand\"'\n\n[[rule]]\nname = \"Cache calls\"\nenabled = false\nlanguage = \"traceql\"\nscope = \"span\"\nquery = '{{ name = \"MGET\" }}'\n",
            RulesFile::EXAMPLE_RULE
        ),
    )
    .unwrap();

    let rules = RulesFile::load(&path).unwrap().unwrap();

    let summary: Vec<_> = rules
        .iter()
        .map(|r| (r.name.as_str(), r.enabled, r.language, r.scope))
        .collect();
    assert_eq!(
        summary,
        [
            (
                "Health checks",
                true,
                RuleLanguage::TraceQl,
                DropScope::Trace
            ),
            (
                "Framework SQL logs",
                true,
                RuleLanguage::LogQl,
                DropScope::Trace
            ),
            ("Cache calls", false, RuleLanguage::TraceQl, DropScope::Span),
        ]
    );
    assert_eq!(
        rules[1].query,
        r#"{service_name="orders-api"} |= "Executed DbCommand""#
    );
}

#[test]
fn bad_toml_is_an_error_that_names_the_file_and_the_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    std::fs::write(
        &path,
        "[[rule]]\nname = \"ok\"\nenabled = true\nlanguage = \"traceql\"\nquery = \"{ }\"\n\n[[rule]\nname = \"oops\"\n",
    )
    .unwrap();

    let error = RulesFile::load(&path).unwrap_err().to_string();

    assert!(error.contains("rules.toml"), "{error}");
    assert!(error.contains("line 7"), "{error}");
    // One line: the engine shows it in a menu.
    assert!(!error.contains('\n'), "{error}");
}

#[test]
fn a_rule_with_a_wrong_value_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let cases = [
        ("language", "\"sql\""),
        ("scope", "\"everything\""),
        ("enabled", "\"yes\""),
        ("name", "7"),
    ];
    for (key, bad) in cases {
        let mut fields = vec![
            ("name", "\"r\""),
            ("enabled", "true"),
            ("language", "\"traceql\""),
            ("scope", "\"trace\""),
            ("query", "\"{ }\""),
        ];
        for field in &mut fields {
            if field.0 == key {
                field.1 = bad;
            }
        }
        let mut text = String::from("[[rule]]\n");
        for (name, value) in fields {
            text.push_str(&format!("{name} = {value}\n"));
        }
        std::fs::write(&path, &text).unwrap();
        assert!(RulesFile::load(&path).is_err(), "{text}");
    }
}

#[test]
fn a_rule_missing_a_key_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    // No `enabled`: the file says nothing about whether to apply the rule.
    std::fs::write(
        &path,
        "[[rule]]\nname = \"r\"\nlanguage = \"traceql\"\nquery = \"{ }\"\n",
    )
    .unwrap();

    let error = RulesFile::load(&path).unwrap_err().to_string();

    assert!(error.contains("enabled"), "{error}");
}

#[test]
fn a_misspelled_key_is_an_error_not_a_default() {
    // `scope` is the only optional key, so a typo in its name must not load as
    // a rule without one: that rule would drop whole traces and their logs
    // when the person asked for only the matching spans to go.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    for typo in ["scpoe", "Scope", "scopes", "quey", "Enabled"] {
        let text = format!(
            "[[rule]]\nname = \"a\"\nenabled = true\nlanguage = \"traceql\"\nquery = \"{{ }}\"\n{typo} = \"span\"\n"
        );
        std::fs::write(&path, &text).unwrap();

        let error = RulesFile::load(&path).unwrap_err().to_string();

        assert!(error.contains("rules.toml"), "{error}");
        assert!(error.contains(typo), "the key is named: {error}");
        assert!(error.contains("line 6"), "{error}");
        assert!(!error.contains('\n'), "{error}");
    }
}

#[test]
fn an_unknown_top_level_key_is_an_error() {
    // `[[rules]]` for `[[rule]]`: without this the file would load as a file
    // with no rules at all, and every rule in it would silently do nothing.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    std::fs::write(
        &path,
        "[[rules]]\nname = \"a\"\nenabled = true\nlanguage = \"traceql\"\nquery = \"{ }\"\n",
    )
    .unwrap();

    let error = RulesFile::load(&path).unwrap_err().to_string();

    assert!(error.contains("rules.toml"), "{error}");
    assert!(error.contains("rules"), "{error}");
    assert!(!error.contains('\n'), "{error}");
}

#[test]
fn every_key_the_header_documents_is_known() {
    // The other side of `deny_unknown_fields`: a saved file, with every key
    // the header lists, still loads.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    RulesFile::save(&path, &sample_rules()).unwrap();

    assert_eq!(RulesFile::load(&path).unwrap(), Some(sample_rules()));
}

#[test]
fn a_bad_query_is_not_a_file_error() {
    // Queries are checked when the rules are applied, one rule at a time
    // (RuleSet), so that one typo cannot switch every other rule off.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    std::fs::write(
        &path,
        "[[rule]]\nname = \"typo\"\nenabled = true\nlanguage = \"traceql\"\nquery = \"{ nme = 1 \"\n",
    )
    .unwrap();

    let rules = RulesFile::load(&path).unwrap().unwrap();

    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].query, "{ nme = 1 ");
}

#[test]
fn unreadable_path_is_an_error_not_none() {
    // A directory where the file should be: not "missing".
    let dir = tempfile::tempdir().unwrap();

    let error = RulesFile::load(dir.path()).unwrap_err().to_string();

    assert!(error.contains("cannot read"), "{error}");
}

#[test]
fn default_path_sits_next_to_settings() {
    let path = RulesFile::default_path();

    assert_eq!(path.file_name().unwrap(), "rules.toml");
    assert_eq!(path.parent().unwrap().file_name().unwrap(), "loctel");
}

#[test]
fn rule_language_and_scope_serialize_as_lowercase_words() {
    // `serde(rename_all = "lowercase")`, seen through the file format.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    RulesFile::save(&path, &sample_rules()).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("TraceQl") && !text.contains("LogQl"),
        "{text}"
    );
    assert!(
        !text.contains("\"Trace\"") && !text.contains("\"Span\""),
        "{text}"
    );
}
