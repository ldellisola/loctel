//! `rules.toml`: the discard rules on disk.
//!
//! The file is TOML, an array of `[[rule]]` tables, written for people to edit
//! by hand. It lives next to `settings.toml`. A file that does not exist is
//! not an error (a first launch looks like that), and the engine then writes
//! one that holds no rules, only an explanation and a commented-out example:
//! loctel never decides what to discard (`docs/decisions/0016`).

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::spec::RuleSpec;

/// Reading and writing the rules file. A namespace for functions: there is no
/// state to hold, the engine keeps the path.
pub struct RulesFile;

/// The whole file as TOML sees it: `rule` is the `[[rule]]` tables.
#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    rule: Vec<RuleSpec>,
}

/// [`Document`], borrowing, for writing.
#[derive(Serialize)]
struct DocumentRef<'a> {
    rule: &'a [RuleSpec],
}

impl RulesFile {
    /// The health-check rule shown, commented out, at the top of a new file:
    /// one `[[rule]]` table as TOML text, ending in a newline. It is not
    /// applied by default; a person turns it on by removing the `# `.
    pub const EXAMPLE_RULE: &'static str = r#"[[rule]]
name = "Health checks"
enabled = true
language = "traceql"
scope = "trace"
query = '{ span.url.path =~ "/(health|healthz|alive|livez|ready|readyz)(/.*)?" || span.http.route =~ "/(health|healthz|alive|livez|ready|readyz)(/.*)?" }'
"#;

    /// `<config dir>/loctel/rules.toml`: the directory of the settings file
    /// (`Settings::default_path` in `loctel-engine`, which this crate cannot
    /// depend on, so the rule is repeated here) plus `rules.toml`. On macOS that
    /// is `~/Library/Application Support/loctel/rules.toml`. A machine with no
    /// config directory uses the system's temporary directory, as settings do.
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("loctel")
            .join("rules.toml")
    }

    /// Reads the rules, in file order. `Ok(None)` when the file does not
    /// exist. A file that cannot be read, or is not valid rules (bad TOML, an
    /// unknown `language` or `scope`, a rule without a name, `enabled`,
    /// `language` or `query`), is an error whose one-line text names the file
    /// and, for bad TOML, the line. The file is never rewritten by this call.
    ///
    /// The queries are not checked here: a rule whose query does not parse is
    /// returned as written, and `RuleSet` reports it.
    pub fn load(path: &Path) -> anyhow::Result<Option<Vec<RuleSpec>>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e).with_context(|| format!("cannot read {}", path.display()));
            }
        };
        let document: Document = toml::from_str(&text).map_err(|e| {
            let line = e
                .span()
                .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
            // The error's own `Display` is several lines with the text under
            // it; the message is the part worth showing next to a line number.
            let message = e.message().replace('\n', " ");
            match line {
                Some(line) => anyhow::anyhow!(
                    "invalid rules in {} (line {line}): {message}",
                    path.display()
                ),
                None => anyhow::anyhow!("invalid rules in {}: {message}", path.display()),
            }
        })?;
        Ok(Some(document.rule))
    }

    /// Writes the rules, creating the directories as needed: the comment header
    /// (what the file is, its keys, the commented-out example) and one
    /// `[[rule]]` table per rule. The file is written whole beside the target
    /// and then renamed over it, so a crash leaves the old file or the new one,
    /// never half of one. Comments a person added to the old file are not kept.
    pub fn save(path: &Path, rules: &[RuleSpec]) -> anyhow::Result<()> {
        let mut text = header();
        if !rules.is_empty() {
            text.push('\n');
            text.push_str(
                &toml::to_string_pretty(&DocumentRef { rule: rules })
                    .context("cannot serialize the rules")?,
            );
        }
        write_atomically(path, &text)
    }

    /// Writes a file that holds no rules: only the header. [`load`](Self::load)
    /// of it gives `Ok(Some(vec![]))`.
    pub fn write_empty(path: &Path) -> anyhow::Result<()> {
        Self::save(path, &[])
    }
}

/// The comment block at the top of the file. Every example line is the
/// matching line of [`RulesFile::EXAMPLE_RULE`] with `# ` in front.
fn header() -> String {
    let mut text = String::from(
        "\
# loctel discard rules
#
# Data that matches an enabled rule is dropped when it arrives and is gone for
# good. loctel ships no rules: nothing is discarded unless you write a rule
# here. Rules apply in the order they appear below. Changes to this file take
# effect within a few seconds, and a mistake in one rule's query switches off
# only that rule.
#
# Each rule is a [[rule]] table with these keys:
#
#   name      what you call the rule
#   enabled   true or false; a rule that is off stays here and does nothing
#   language  \"traceql\" (judges spans) or \"logql\" (judges logs)
#   query     the query, written as in Grafana (put it in single quotes, so
#             the double quotes inside need no escaping)
#   scope     TraceQL only: \"trace\" (the default) drops the whole trace, and
#             \"span\" drops only the matching spans and what is below them
#
# The format and what each rule does: docs/areas/rules.md in the loctel
# repository, https://github.com/ldellisola/loctel/blob/main/docs/areas/rules.md
#
# An example. Remove the leading \"# \" from each of its lines to use it:
#
",
    );
    for line in RulesFile::EXAMPLE_RULE.lines() {
        text.push_str("# ");
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// Writes `text` to `path` through a temporary file beside it, like
/// `Settings::save`.
fn write_atomically(path: &Path, text: &str) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;

    let name = path
        .file_name()
        .with_context(|| format!("{} does not name a file", path.display()))?;
    let mut temporary = name.to_os_string();
    temporary.push(".tmp");
    let temporary = dir.join(temporary);

    let written = fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    });
    written
        .and_then(|()| fs::rename(&temporary, path))
        .inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })
        .with_context(|| format!("cannot write {}", path.display()))
}
