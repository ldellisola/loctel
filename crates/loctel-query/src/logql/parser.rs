//! The LogQL parser: recursive descent.
//!
//! The grammar is Loki's `pkg/logql/syntax/syntax.y`, restricted to what loctel
//! supports (design §6.2): a stream selector, then line filters, `| json`,
//! `| logfmt` and label filters. Everything else that Loki has is recognised
//! and reported as `"<feature> is not supported yet"` at its position.
//!
//! **Label filter precedence** is the one place the grammar needs reading with
//! care, because `syntax.y` leaves most of it to yacc's defaults:
//!
//! ```text
//! %left OR
//! %left AND UNLESS
//! labelFilter: labelFilter labelFilter | labelFilter AND labelFilter
//!            | labelFilter COMMA labelFilter | labelFilter OR labelFilter
//! ```
//!
//! `and` binds tighter than `or`, and both are left-associative. A comma and
//! plain juxtaposition (`a="1" b="2"`) are also an `and`, but the grammar gives
//! neither a precedence, so wherever yacc cannot compare it shifts, and what
//! follows a comma or a space is grouped first: `a, b or c` is `a and (b or c)`
//! and `a or b, c` is `a or (b and c)`. `label_chain` reproduces
//! exactly that; the table in `tests/logql.rs` pins it, and it was checked by
//! running the label filter rules through goyacc.
//!
//! The parser recurses only on parentheses, and refuses past [`MAX_DEPTH`]
//! levels or [`MAX_OPERATORS`] operators, so a hostile or mistyped query cannot
//! overflow the stack here or in the evaluator that walks the tree (a chain of
//! commas is a tree as deep as it is long, which is what the operator limit
//! bounds).

use super::ast::{LabelFilter, LabelMatcher, LineFilter, LogQuery, NumberOp, Stage, StringMatcher};
use super::lexer::{Tok, Token, lex};
use crate::error::regex_error_message;
use crate::{Pattern, QueryError};

/// Deepest nesting of parentheses accepted.
const MAX_DEPTH: usize = 64;

/// Most operators (`and`, `or`, `,` and juxtaposition between label filters) in
/// one query.
const MAX_OPERATORS: usize = 1000;

/// Loki's metric functions and aggregations. A query that starts with one of
/// them (followed by `(`, or by `by (` or `without (`) is a metrics query.
const METRIC_FUNCTIONS: &[&str] = &[
    "rate",
    "rate_counter",
    "count_over_time",
    "bytes_rate",
    "bytes_over_time",
    "avg_over_time",
    "sum_over_time",
    "min_over_time",
    "max_over_time",
    "stdvar_over_time",
    "stddev_over_time",
    "quantile_over_time",
    "first_over_time",
    "last_over_time",
    "absent_over_time",
    "vector",
    "sum",
    "avg",
    "max",
    "min",
    "count",
    "stddev",
    "stdvar",
    "bottomk",
    "topk",
    "sort",
    "sort_desc",
    "label_replace",
    "approx_topk",
    "approx_count_distinct",
];

/// Stages Loki has after `|` that loctel does not implement, by the word that
/// starts them.
const UNSUPPORTED_STAGES: &[&str] = &[
    "line_format",
    "label_format",
    "unwrap",
    "decolorize",
    "drop",
    "keep",
    "unpack",
    "pattern",
    "regexp",
];

pub(super) fn parse(text: &str) -> Result<LogQuery, QueryError> {
    let chars: Vec<char> = text.chars().collect();
    let tokens = lex(&chars);
    let mut parser = Parser {
        chars: &chars,
        tokens,
        pos: 0,
        depth: 0,
        operators: 0,
    };
    parser.query()
}

/// What stands between two label filters.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Join {
    And,
    Or,
    /// `,`
    Comma,
    /// Nothing at all: `a="1" b="2"`.
    Space,
}

/// A number or duration literal.
enum Literal {
    Number(f64),
    Duration(i128),
}

struct Parser<'a> {
    chars: &'a [char],
    /// Never empty: the last token is `Eof`.
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
    operators: usize,
}

impl Parser<'_> {
    // ---- cursor ------------------------------------------------------------

    fn token(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn tok(&self) -> &Tok {
        &self.token().tok
    }

    fn peek_tok(&self, offset: usize) -> &Tok {
        let last = self.tokens.len() - 1;
        &self.tokens[(self.pos + offset).min(last)].tok
    }

    /// Moves past the current token. `Eof` stays put.
    fn bump(&mut self) {
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
    }

    fn expect(&mut self, tok: Tok, what: &str) -> Result<(), QueryError> {
        if *self.tok() == tok {
            self.bump();
            Ok(())
        } else {
            Err(self.unexpected(what))
        }
    }

    fn source_text(&self, token: &Token) -> String {
        self.chars[token.start..token.end].iter().collect()
    }

    /// The error for a token that is not what the grammar allows here.
    fn unexpected(&self, expected: &str) -> QueryError {
        let token = self.token();
        match &token.tok {
            Tok::Invalid(err) => err.clone(),
            Tok::Eof => QueryError::new(token.start, format!("expected {expected}")),
            _ => QueryError::new(
                token.start,
                format!("expected {expected}, found `{}`", self.source_text(token)),
            ),
        }
    }

    /// The word at the cursor, in lower case, if it is one.
    fn word(&self) -> Option<String> {
        match self.tok() {
            Tok::Ident(word) => Some(word.to_ascii_lowercase()),
            _ => None,
        }
    }

    // ---- limits ------------------------------------------------------------

    fn enter(&mut self) -> Result<(), QueryError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(QueryError::new(
                self.token().start,
                format!("query is too deeply nested (more than {MAX_DEPTH} levels)"),
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn count_operator(&mut self, position: usize) -> Result<(), QueryError> {
        self.operators += 1;
        if self.operators > MAX_OPERATORS {
            return Err(QueryError::new(
                position,
                format!("query has too many operators (more than {MAX_OPERATORS})"),
            ));
        }
        Ok(())
    }

    // ---- the query ---------------------------------------------------------

    fn query(&mut self) -> Result<LogQuery, QueryError> {
        // The whole query may be wrapped in parentheses: `({a="b"} |= "x")`.
        let mut open = 0;
        while matches!(self.tok(), Tok::LParen) {
            self.enter()?;
            self.bump();
            open += 1;
        }
        if self.starts_metrics_query() {
            // The plan's exact words, so not `QueryError::unsupported`, whose
            // "is" does not agree with the plural.
            return Err(QueryError::new(
                self.token().start,
                "metrics queries are not supported yet",
            ));
        }
        let matchers = self.selector()?;
        let mut stages = Vec::new();
        loop {
            match self.tok() {
                Tok::Eof => break,
                Tok::RParen if open > 0 => break,
                Tok::PipeEq | Tok::PipeRe | Tok::Neq | Tok::Nre => {
                    stages.push(Stage::LineFilter(self.line_filter()?));
                }
                Tok::PipePattern | Tok::NotPattern => {
                    let token = self.token();
                    return Err(QueryError::unsupported(
                        token.start,
                        &format!("the pattern filter `{}`", self.source_text(token)),
                    ));
                }
                Tok::Pipe => {
                    self.bump();
                    stages.push(self.pipe_stage()?);
                }
                _ => return Err(self.unexpected("a pipeline stage or the end of the query")),
            }
        }
        for _ in 0..open {
            self.expect(Tok::RParen, "`)`")?;
            self.leave();
        }
        if !matches!(self.tok(), Tok::Eof) {
            return Err(self.unexpected("the end of the query"));
        }
        Ok(LogQuery { matchers, stages })
    }

    /// `rate(`, `sum by (`, `topk(`...: the start of a metrics query.
    fn starts_metrics_query(&self) -> bool {
        let Some(word) = self.word() else {
            return false;
        };
        if !METRIC_FUNCTIONS.contains(&word.as_str()) {
            return false;
        }
        match self.peek_tok(1) {
            Tok::LParen => true,
            Tok::Ident(next) => {
                matches!(next.to_ascii_lowercase().as_str(), "by" | "without")
                    && matches!(self.peek_tok(2), Tok::LParen)
            }
            _ => false,
        }
    }

    // ---- the selector ------------------------------------------------------

    /// `{ matcher ("," matcher)* }`.
    fn selector(&mut self) -> Result<Vec<LabelMatcher>, QueryError> {
        let open = self.token().start;
        if !matches!(self.tok(), Tok::LBrace) {
            return Err(self.unexpected("`{`"));
        }
        self.bump();
        if matches!(self.tok(), Tok::RBrace) {
            return Err(QueryError::new(
                open,
                "a stream selector needs at least one matcher, for example `{service_name=\"api\"}`",
            ));
        }
        let mut matchers = Vec::new();
        loop {
            matchers.push(self.selector_matcher()?);
            match self.tok() {
                Tok::Comma => self.bump(),
                Tok::RBrace => {
                    self.bump();
                    return Ok(matchers);
                }
                _ => return Err(self.unexpected("`,` or `}`")),
            }
        }
    }

    /// `label ( = | != | =~ | !~ ) "value"`.
    fn selector_matcher(&mut self) -> Result<LabelMatcher, QueryError> {
        let Tok::Ident(label) = self.tok() else {
            return Err(self.unexpected("a label name"));
        };
        let label = label.clone();
        self.bump();
        let kind = match self.tok() {
            Tok::Eq => Tok::Eq,
            Tok::Neq => Tok::Neq,
            Tok::Re => Tok::Re,
            Tok::Nre => Tok::Nre,
            _ => return Err(self.unexpected("`=`, `!=`, `=~` or `!~`")),
        };
        self.bump();
        let matcher = self.string_matcher(&kind)?;
        Ok(LabelMatcher { label, matcher })
    }

    /// The string after `=`, `!=`, `=~` or `!~` (`kind`), as a matcher. A
    /// regex is compiled here, anchored, so a bad one is an error at the
    /// pattern.
    fn string_matcher(&mut self, kind: &Tok) -> Result<StringMatcher, QueryError> {
        let (text, at) = self.string()?;
        Ok(match kind {
            Tok::Eq => StringMatcher::Equal(text),
            Tok::Neq => StringMatcher::NotEqual(text),
            Tok::Re => StringMatcher::Regex(compile(Pattern::anchored(&text), at)?),
            _ => StringMatcher::NotRegex(compile(Pattern::anchored(&text), at)?),
        })
    }

    /// A string literal, and where it starts.
    fn string(&mut self) -> Result<(String, usize), QueryError> {
        match self.tok() {
            Tok::Str(text) => {
                let text = text.clone();
                let at = self.token().start;
                self.bump();
                Ok((text, at))
            }
            _ => {
                if let Some(error) = self.unsupported_ip() {
                    return Err(error);
                }
                Err(self.unexpected("a string"))
            }
        }
    }

    /// The error for `ip(` at the cursor: Loki's address filter.
    fn unsupported_ip(&self) -> Option<QueryError> {
        match (self.word().as_deref(), self.peek_tok(1)) {
            (Some("ip"), Tok::LParen) => Some(QueryError::unsupported(self.token().start, "ip")),
            _ => None,
        }
    }

    // ---- line filters ------------------------------------------------------

    /// `( |= | != | |~ | !~ ) "text"`.
    fn line_filter(&mut self) -> Result<LineFilter, QueryError> {
        let kind = self.tok().clone();
        self.bump();
        let (text, at) = self.string()?;
        // `|= "a" or "b"` is Loki's way to write several; not supported yet.
        if self.word().as_deref() == Some("or") {
            return Err(QueryError::unsupported(
                self.token().start,
                "or between line filters",
            ));
        }
        Ok(match kind {
            Tok::PipeEq => LineFilter::Contains(text),
            Tok::Neq => LineFilter::NotContains(text),
            Tok::PipeRe => LineFilter::Regex(compile(Pattern::unanchored(&text), at)?),
            _ => LineFilter::NotRegex(compile(Pattern::unanchored(&text), at)?),
        })
    }

    // ---- stages after `|` --------------------------------------------------

    /// What follows a `|`: a parser, or a label filter.
    fn pipe_stage(&mut self) -> Result<Stage, QueryError> {
        if let Some(word) = self.word() {
            match word.as_str() {
                "json" | "logfmt" => return self.parser_stage(&word),
                w if UNSUPPORTED_STAGES.contains(&w) => {
                    return Err(QueryError::unsupported(self.token().start, w));
                }
                _ => {}
            }
        }
        match self.tok() {
            Tok::Ident(_) | Tok::LParen => Ok(Stage::LabelFilter(self.label_chain()?)),
            _ => Err(self.unexpected("`json`, `logfmt` or a label filter")),
        }
    }

    /// `json` or `logfmt` (at the cursor), with nothing after it: Loki's
    /// parameters (`| json a="b.c"`, `| logfmt --strict`) are not supported.
    fn parser_stage(&mut self, word: &str) -> Result<Stage, QueryError> {
        self.bump();
        let feature = match (word, self.tok()) {
            ("json", Tok::Ident(_) | Tok::Str(_)) => Some("json label extraction"),
            ("logfmt", Tok::Minus) => Some("a logfmt flag"),
            ("logfmt", Tok::Ident(_) | Tok::Str(_)) => Some("logfmt label extraction"),
            _ => None,
        };
        if let Some(feature) = feature {
            return Err(QueryError::unsupported(self.token().start, feature));
        }
        Ok(if word == "json" {
            Stage::Json
        } else {
            Stage::Logfmt
        })
    }

    // ---- label filters -----------------------------------------------------

    /// `label_expr`: operands joined by `and`, `or`, `,` or nothing, by yacc's
    /// own conflict resolution (see the module docs).
    ///
    /// This is a shift-reduce loop over an explicit stack instead of recursion,
    /// because a long chain of commas (or spaces) is a right-deep tree: the
    /// `frames` hold each left operand with the join that waits for its right
    /// side, and the join on top is the pending one. When another join follows
    /// an operand, the pending join is *reduced* first only for `and` before
    /// `and` or `or`, and for `or` before `or`. In every other case, including
    /// every case with a comma or a space on either side, the operand goes on
    /// growing to the right: yacc has no precedence to compare and shifts.
    fn label_chain(&mut self) -> Result<LabelFilter, QueryError> {
        let mut frames: Vec<(LabelFilter, Join)> = Vec::new();
        let mut current = self.label_operand()?;
        loop {
            let join = match self.tok() {
                Tok::Comma => Some(Join::Comma),
                Tok::LParen => Some(Join::Space),
                Tok::Ident(word) if word.eq_ignore_ascii_case("and") => Some(Join::And),
                Tok::Ident(word) if word.eq_ignore_ascii_case("or") => Some(Join::Or),
                Tok::Ident(_) => Some(Join::Space),
                _ => None,
            };
            let reduce = match (frames.last(), join) {
                // Nothing follows: close everything that is still open.
                (Some(_), None) => true,
                (Some((_, Join::And)), Some(Join::And | Join::Or))
                | (Some((_, Join::Or)), Some(Join::Or)) => true,
                _ => false,
            };
            if reduce {
                if let Some((lhs, pending)) = frames.pop() {
                    current = join_filters(pending, lhs, current);
                }
                continue;
            }
            let Some(join) = join else {
                return Ok(current);
            };
            let position = self.token().start;
            if join != Join::Space {
                self.bump();
            }
            self.count_operator(position)?;
            frames.push((current, join));
            current = self.label_operand()?;
        }
    }

    /// `( label_expr )` or a single comparison.
    fn label_operand(&mut self) -> Result<LabelFilter, QueryError> {
        match self.tok() {
            Tok::LParen => {
                self.enter()?;
                self.bump();
                let inner = self.label_chain()?;
                self.expect(Tok::RParen, "`)`")?;
                self.leave();
                Ok(inner)
            }
            Tok::Ident(label) => {
                let label = label.clone();
                self.bump();
                self.label_comparison(label)
            }
            _ => Err(self.unexpected("a label filter")),
        }
    }

    /// What follows a label name: an operator and a string, a number or a
    /// duration. `=` and `!=` take any of the three (a string compares as text,
    /// the others as numbers); `=~` and `!~` take a string; `==`, `>`, `>=`,
    /// `<` and `<=` take a number or a duration.
    fn label_comparison(&mut self, label: String) -> Result<LabelFilter, QueryError> {
        let op = self.tok().clone();
        match op {
            Tok::Eq | Tok::Neq => {
                self.bump();
                let numeric = match self.tok() {
                    Tok::Number(_) | Tok::Duration(_) | Tok::Plus | Tok::Minus => true,
                    Tok::Str(_) => false,
                    _ => {
                        if let Some(error) = self.unsupported_ip() {
                            return Err(error);
                        }
                        return Err(self.unexpected("a string, number or duration"));
                    }
                };
                if numeric {
                    let op = if op == Tok::Eq {
                        NumberOp::Equal
                    } else {
                        NumberOp::NotEqual
                    };
                    self.numeric_filter(label, op)
                } else {
                    let matcher = self.string_matcher(&op)?;
                    Ok(LabelFilter::String(LabelMatcher { label, matcher }))
                }
            }
            Tok::Re | Tok::Nre => {
                self.bump();
                let matcher = self.string_matcher(&op)?;
                Ok(LabelFilter::String(LabelMatcher { label, matcher }))
            }
            Tok::EqEq | Tok::Gt | Tok::Gte | Tok::Lt | Tok::Lte => {
                self.bump();
                let op = match op {
                    Tok::EqEq => NumberOp::Equal,
                    Tok::Gt => NumberOp::Greater,
                    Tok::Gte => NumberOp::GreaterOrEqual,
                    Tok::Lt => NumberOp::Less,
                    _ => NumberOp::LessOrEqual,
                };
                self.numeric_filter(label, op)
            }
            _ => Err(self.unexpected("a comparison operator")),
        }
    }

    /// The number or duration of a numeric label filter.
    fn numeric_filter(&mut self, label: String, op: NumberOp) -> Result<LabelFilter, QueryError> {
        Ok(match self.literal()? {
            Literal::Number(value) => LabelFilter::Number { label, op, value },
            Literal::Duration(nanos) => LabelFilter::Duration { label, op, nanos },
        })
    }

    /// A number or a duration, with an optional sign.
    fn literal(&mut self) -> Result<Literal, QueryError> {
        let negative = match self.tok() {
            Tok::Minus => {
                self.bump();
                true
            }
            Tok::Plus => {
                self.bump();
                false
            }
            _ => false,
        };
        let literal = match self.tok() {
            Tok::Number(n) => Literal::Number(if negative { -*n } else { *n }),
            Tok::Duration(nanos) => {
                let nanos = i128::from(*nanos);
                Literal::Duration(if negative { -nanos } else { nanos })
            }
            _ => return Err(self.unexpected("a number or duration")),
        };
        self.bump();
        Ok(literal)
    }
}

/// `lhs join rhs` as a tree node: `or` is an `Or`, the rest are `And`s.
fn join_filters(join: Join, lhs: LabelFilter, rhs: LabelFilter) -> LabelFilter {
    match join {
        Join::Or => LabelFilter::Or(Box::new(lhs), Box::new(rhs)),
        Join::And | Join::Comma | Join::Space => LabelFilter::And(Box::new(lhs), Box::new(rhs)),
    }
}

/// A compiled pattern, or the error at the pattern's position.
fn compile(result: Result<Pattern, regex::Error>, at: usize) -> Result<Pattern, QueryError> {
    result.map_err(|err| QueryError::new(at, regex_error_message(&err)))
}
