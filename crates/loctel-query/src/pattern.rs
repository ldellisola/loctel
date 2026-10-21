//! Compiled regexes, anchored or not.

use regex::Regex;

/// A compiled regular expression (RE2-style syntax, close to Go's `regexp`,
/// which Tempo and Loki use).
///
/// Whether it is anchored depends on where it is used (design §6):
///
/// - **Anchored** (`^(?:src)$`): TraceQL `=~` and `!~`, and LogQL label
///   matchers. `name =~ "POST"` does not match `POST /charges`.
/// - **Unanchored:** LogQL line filters (`|~`, `!~` on the line), which match
///   anywhere in the line.
///
/// Two patterns are equal when they have the same source and the same
/// anchoring, so syntax trees that hold patterns can be compared in tests.
#[derive(Clone, Debug)]
pub struct Pattern {
    regex: Regex,
    source: String,
    anchored: bool,
}

impl Pattern {
    /// Compiles `^(?:src)$`: the whole text must match.
    pub fn anchored(src: &str) -> Result<Pattern, regex::Error> {
        // Check the source on its own first. Wrapping alone could turn an
        // unbalanced source such as `a)|(b` into a valid but different
        // pattern, and the error for a bad source should point at the source,
        // not at the wrapper.
        Regex::new(src)?;
        let regex = Regex::new(&format!("^(?:{src})$"))?;
        Ok(Pattern {
            regex,
            source: src.to_string(),
            anchored: true,
        })
    }

    /// Compiles `src` as written: it may match anywhere in the text.
    pub fn unanchored(src: &str) -> Result<Pattern, regex::Error> {
        Ok(Pattern {
            regex: Regex::new(src)?,
            source: src.to_string(),
            anchored: false,
        })
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.regex.is_match(text)
    }

    /// The pattern as the user wrote it, without the anchors added around it.
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn is_anchored(&self) -> bool {
        self.anchored
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Pattern) -> bool {
        self.anchored == other.anchored && self.source == other.source
    }
}

impl Eq for Pattern {}
