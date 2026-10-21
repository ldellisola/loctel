//! The TraceQL parser: recursive descent, with precedence climbing at the
//! spanset level.
//!
//! The grammar is Tempo's `pkg/traceql/expr.y`, restricted to what loctel
//! supports (design §6.1). Precedence, from `expr.y`'s `%left` lines:
//!
//! - **Between spansets:** `&&` and `||` share one level and are
//!   left-associative; the structural operators (`> < >> << ~ !> !< !>> !<<
//!   !~`) bind tighter, also left-associative. So `A && B >> C` is
//!   `A && (B >> C)`, and `A || B && C` is `(A || B) && C`.
//! - **Inside a filter:** a comparison binds tighter than `&&` and `||`, which
//!   again share one level and are left-associative: `a || b && c` is
//!   `(a || b) && c`.
//!
//! The one place this parser is stricter than Tempo: `!` takes a whole
//! condition (`!(a = b)`, `!span.flag`). Tempo reads `!a = b` as `(!a) = b`,
//! which nobody means, so that is an error here.
//!
//! Everything else outside the supported subset is a [`QueryError`] that names
//! the feature and its position. The parser recurses only on parentheses and
//! `!`, and refuses past [`MAX_DEPTH`] levels or [`MAX_OPERATORS`] operators,
//! so a hostile or mistyped query cannot overflow the stack here or in the
//! evaluator that walks the tree.

use loctel_model::{SpanKind, StatusCode};

use super::ast::{
    Attribute, AttributeScope, CompareOp, FieldExpr, Intrinsic, Operand, PipelineStage,
    SpansetExpr, SpansetOp, Static, TraceQuery,
};
use super::lexer::{ScopeKind, Tok, Token, lex};
use crate::error::regex_error_message;
use crate::{Pattern, QueryError};

/// Deepest nesting of parentheses and `!` accepted.
const MAX_DEPTH: usize = 64;

/// Most operators (`&&`, `||`, `!`, structural operators) in one query.
const MAX_OPERATORS: usize = 1000;

/// Tempo functions that loctel recognises but does not implement. They are
/// only treated as such when followed by `(`.
const UNSUPPORTED_FUNCTIONS: &[&str] = &[
    "select",
    "by",
    "avg",
    "min",
    "max",
    "sum",
    "rate",
    "count_over_time",
    "quantile_over_time",
    "histogram_over_time",
    "min_over_time",
    "max_over_time",
    "avg_over_time",
    "sum_over_time",
    "coalesce",
    "with",
    "compare",
    "topk",
    "bottomk",
];

pub(super) fn parse(text: &str) -> Result<TraceQuery, QueryError> {
    let chars: Vec<char> = text.chars().collect();
    let tokens = lex(&chars);
    let mut parser = Parser {
        chars: &chars,
        tokens,
        pos: 0,
        depth: 0,
        operators: 0,
    };

    let spanset = parser.spanset(0)?;
    let mut stages = Vec::new();
    while matches!(parser.tok(), Tok::Pipe) {
        parser.bump();
        stages.push(parser.stage()?);
    }
    parser.expect_end()?;
    Ok(TraceQuery { spanset, stages })
}

/// A parsed operand: a value, or the `nil` keyword (only valid next to `=` or
/// `!=`, where it turns the comparison into an existence test).
enum Item {
    Value(Operand),
    Nil,
}

/// The comparison operator at the cursor.
#[derive(Clone, Copy)]
enum Cmp {
    Op(CompareOp),
    Re,
    NotRe,
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
            Tok::Arith(_) => QueryError::unsupported(token.start, "arithmetic"),
            Tok::UnionChild
            | Tok::UnionParent
            | Tok::UnionDesc
            | Tok::UnionAnce
            | Tok::UnionSibl => QueryError::unsupported(
                token.start,
                &format!(
                    "the structural union operator `{}`",
                    self.source_text(token)
                ),
            ),
            _ => QueryError::new(
                token.start,
                format!("expected {expected}, found `{}`", self.source_text(token)),
            ),
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

    // ---- spansets ----------------------------------------------------------

    /// `spanset_expr`, by precedence climbing. `min_prec` is the loosest
    /// operator this call may consume: 1 for `&&` and `||`, 2 for the
    /// structural operators.
    fn spanset(&mut self, min_prec: u8) -> Result<SpansetExpr, QueryError> {
        let mut lhs = self.spanset_primary()?;
        loop {
            let (op, prec) = match self.tok() {
                Tok::AndAnd => (SpansetOp::And, 1),
                Tok::OrOr => (SpansetOp::Or, 1),
                Tok::Gt => (SpansetOp::Child, 2),
                Tok::Lt => (SpansetOp::Parent, 2),
                Tok::Desc => (SpansetOp::Descendant, 2),
                Tok::Ance => (SpansetOp::Ancestor, 2),
                Tok::Sibl => (SpansetOp::Sibling, 2),
                Tok::NotChild => (SpansetOp::NotChild, 2),
                Tok::NotParent => (SpansetOp::NotParent, 2),
                Tok::NotDesc => (SpansetOp::NotDescendant, 2),
                Tok::NotAnce => (SpansetOp::NotAncestor, 2),
                Tok::Nre => (SpansetOp::NotSibling, 2),
                Tok::UnionChild
                | Tok::UnionParent
                | Tok::UnionDesc
                | Tok::UnionAnce
                | Tok::UnionSibl => return Err(self.unexpected("a spanset operator")),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            let position = self.token().start;
            self.bump();
            self.count_operator(position)?;
            // `prec + 1`: the right side takes only tighter operators, which
            // makes every level left-associative.
            let rhs = self.spanset(prec + 1)?;
            lhs = SpansetExpr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn spanset_primary(&mut self) -> Result<SpansetExpr, QueryError> {
        match self.tok() {
            Tok::LBrace => self.filter(),
            Tok::LParen => {
                self.enter()?;
                self.bump();
                let inner = self.spanset(0)?;
                if matches!(self.tok(), Tok::Pipe) {
                    return Err(QueryError::unsupported(
                        self.token().start,
                        "a pipeline inside parentheses",
                    ));
                }
                self.expect(Tok::RParen, "`)`")?;
                self.leave();
                Ok(inner)
            }
            _ => {
                if let Some(feature) = self.unsupported_call() {
                    return Err(QueryError::unsupported(self.token().start, &feature));
                }
                Err(self.unexpected("`{`"))
            }
        }
    }

    /// The name of an unsupported function at the cursor (`select(`, `rate(`).
    fn unsupported_call(&self) -> Option<String> {
        match (self.tok(), self.peek_tok(1)) {
            (Tok::Ident(word), Tok::LParen) if UNSUPPORTED_FUNCTIONS.contains(&word.as_str()) => {
                Some(word.clone())
            }
            _ => None,
        }
    }

    /// `{ }` or `{ field_expr }`.
    fn filter(&mut self) -> Result<SpansetExpr, QueryError> {
        self.bump();
        if matches!(self.tok(), Tok::RBrace) {
            self.bump();
            return Ok(SpansetExpr::Filter(FieldExpr::Operand(Operand::Static(
                Static::Bool(true),
            ))));
        }
        let expr = self.field_expr()?;
        if !matches!(self.tok(), Tok::RBrace) {
            return Err(self.unexpected("`}`"));
        }
        self.bump();
        Ok(SpansetExpr::Filter(expr))
    }

    // ---- pipeline ----------------------------------------------------------

    /// One stage after `|`: `count() CMP number`.
    fn stage(&mut self) -> Result<PipelineStage, QueryError> {
        if matches!(self.tok(), Tok::Ident(word) if word == "count") {
            self.bump();
            self.expect(Tok::LParen, "`(`")?;
            self.expect(Tok::RParen, "`)`")?;
            let op = match self.comparison() {
                Some(Cmp::Op(op)) => {
                    self.bump();
                    op
                }
                _ => return Err(self.unexpected("a comparison operator")),
            };
            let value = self.number()?;
            return Ok(PipelineStage::Count { op, value });
        }
        if let Some(feature) = self.unsupported_call() {
            return Err(QueryError::unsupported(self.token().start, &feature));
        }
        if matches!(self.tok(), Tok::LBrace) {
            return Err(QueryError::unsupported(
                self.token().start,
                "a spanset filter after `|`",
            ));
        }
        Err(self.unexpected("`count()`"))
    }

    /// A number: an integer or a float, with an optional leading `-`.
    fn number(&mut self) -> Result<f64, QueryError> {
        let negative = if matches!(self.tok(), Tok::Arith('-')) {
            if !matches!(self.peek_tok(1), Tok::Int(_) | Tok::Float(_)) {
                return Err(QueryError::unsupported(self.token().start, "arithmetic"));
            }
            self.bump();
            true
        } else {
            false
        };
        let value = match self.tok() {
            Tok::Int(n) => *n as f64,
            Tok::Float(f) => *f,
            _ => return Err(self.unexpected("a number")),
        };
        self.bump();
        Ok(if negative { -value } else { value })
    }

    fn expect_end(&self) -> Result<(), QueryError> {
        match self.tok() {
            Tok::Eof => Ok(()),
            Tok::Ident(word) if word == "with" && matches!(self.peek_tok(1), Tok::LParen) => {
                Err(QueryError::unsupported(self.token().start, "with"))
            }
            _ => Err(self.unexpected("`|` or end of query")),
        }
    }

    // ---- field expressions -------------------------------------------------

    /// `&&` and `||` chain left to right at one precedence level, as in
    /// Tempo's `%left AND OR`.
    fn field_expr(&mut self) -> Result<FieldExpr, QueryError> {
        let mut lhs = self.condition()?;
        loop {
            let make: fn(Box<FieldExpr>, Box<FieldExpr>) -> FieldExpr = match self.tok() {
                Tok::AndAnd => FieldExpr::And,
                Tok::OrOr => FieldExpr::Or,
                _ => break,
            };
            let position = self.token().start;
            self.bump();
            self.count_operator(position)?;
            let rhs = self.condition()?;
            lhs = make(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// One condition: `!cond`, `( field_expr )`, a comparison or a bare
    /// boolean.
    fn condition(&mut self) -> Result<FieldExpr, QueryError> {
        match self.tok() {
            Tok::Not => self.negation(),
            Tok::LParen => self.group(),
            _ => self.comparison_expr(),
        }
    }

    fn group(&mut self) -> Result<FieldExpr, QueryError> {
        self.enter()?;
        self.bump();
        let inner = self.field_expr()?;
        self.expect(Tok::RParen, "`)`")?;
        self.leave();
        Ok(inner)
    }

    fn negation(&mut self) -> Result<FieldExpr, QueryError> {
        let position = self.token().start;
        self.enter()?;
        self.bump();
        self.count_operator(position)?;
        let inner = match self.tok() {
            Tok::Not => self.negation()?,
            Tok::LParen => self.group()?,
            _ => {
                let (item, _) = self.item()?;
                if self.comparison().is_some() {
                    return Err(QueryError::new(
                        position,
                        "`!` applies to a whole condition; write `!(a = b)` to negate a comparison",
                    ));
                }
                self.bare_boolean(item)?
            }
        };
        if self.comparison().is_some() {
            return Err(QueryError::new(
                position,
                "`!` applies to a whole condition; write `!(a = b)` to negate a comparison",
            ));
        }
        self.leave();
        Ok(FieldExpr::Not(Box::new(inner)))
    }

    /// `operand CMP operand`, or a bare boolean operand.
    fn comparison_expr(&mut self) -> Result<FieldExpr, QueryError> {
        let (lhs, _) = self.item()?;
        let Some(cmp) = self.comparison() else {
            return self.bare_boolean(lhs);
        };
        let op_position = self.token().start;
        self.bump();
        let (rhs, rhs_position) = self.item()?;
        let expr = Self::build_comparison(cmp, op_position, lhs, rhs, rhs_position)?;
        if self.comparison().is_some() {
            return Err(QueryError::new(
                self.token().start,
                "comparisons cannot be chained; join them with `&&` or `||`",
            ));
        }
        Ok(expr)
    }

    /// A lone operand used as a condition: only an attribute (`{ span.hit }`)
    /// or `true`/`false` is a boolean on its own.
    fn bare_boolean(&self, item: Item) -> Result<FieldExpr, QueryError> {
        match item {
            Item::Value(operand @ (Operand::Attribute(_) | Operand::Static(Static::Bool(_)))) => {
                Ok(FieldExpr::Operand(operand))
            }
            _ => Err(self.unexpected("a comparison operator")),
        }
    }

    fn comparison(&self) -> Option<Cmp> {
        Some(match self.tok() {
            Tok::Eq => Cmp::Op(CompareOp::Eq),
            Tok::Neq => Cmp::Op(CompareOp::Neq),
            Tok::Gt => Cmp::Op(CompareOp::Gt),
            Tok::Gte => Cmp::Op(CompareOp::Gte),
            Tok::Lt => Cmp::Op(CompareOp::Lt),
            Tok::Lte => Cmp::Op(CompareOp::Lte),
            Tok::Re => Cmp::Re,
            Tok::Nre => Cmp::NotRe,
            _ => return None,
        })
    }

    fn build_comparison(
        cmp: Cmp,
        op_position: usize,
        lhs: Item,
        rhs: Item,
        rhs_position: usize,
    ) -> Result<FieldExpr, QueryError> {
        match (lhs, rhs) {
            (Item::Value(lhs), Item::Value(rhs)) => match cmp {
                Cmp::Op(op) => Ok(FieldExpr::Compare { op, lhs, rhs }),
                Cmp::Re | Cmp::NotRe => {
                    let negated = matches!(cmp, Cmp::NotRe);
                    let Operand::Static(Static::Str(source)) = rhs else {
                        return Err(QueryError::new(
                            rhs_position,
                            format!(
                                "expected a string pattern after `{}`",
                                if negated { "!~" } else { "=~" }
                            ),
                        ));
                    };
                    let pattern = Pattern::anchored(&source)
                        .map_err(|err| QueryError::new(rhs_position, regex_error_message(&err)))?;
                    Ok(FieldExpr::Regex {
                        negated,
                        lhs,
                        pattern,
                    })
                }
            },
            (Item::Nil, Item::Nil) => match cmp {
                Cmp::Op(CompareOp::Eq | CompareOp::Neq) => {
                    Ok(FieldExpr::Operand(Operand::Static(Static::Bool(false))))
                }
                _ => Err(nil_error(op_position)),
            },
            (Item::Value(operand), Item::Nil) | (Item::Nil, Item::Value(operand)) => match cmp {
                // `x = nil`: the attribute is missing. `x != nil`: present.
                Cmp::Op(CompareOp::Eq) => Ok(FieldExpr::NotExists(operand)),
                Cmp::Op(CompareOp::Neq) => Ok(FieldExpr::Exists(operand)),
                _ => Err(nil_error(op_position)),
            },
        }
    }

    // ---- operands ----------------------------------------------------------

    /// One operand, and the position it starts at.
    fn item(&mut self) -> Result<(Item, usize), QueryError> {
        let token = self.token().clone();
        let position = token.start;
        let item = match token.tok {
            Tok::Str(s) => Item::Value(Operand::Static(Static::Str(s))),
            Tok::Int(n) => Item::Value(Operand::Static(Static::Int(n))),
            Tok::Float(f) => Item::Value(Operand::Static(Static::Float(f))),
            Tok::Duration(d) => Item::Value(Operand::Static(Static::Duration(d))),
            Tok::Arith('-') => return self.negative_number(position),
            Tok::Ident(ref word) => self.word(word, &token)?,
            Tok::ScopedIntrinsic { scope, name } => Item::Value(Operand::Intrinsic(
                scoped_intrinsic(&scope, &name, position)?,
            )),
            Tok::Attr { scope, name } => {
                let scope = match scope {
                    ScopeKind::Span => AttributeScope::Span,
                    ScopeKind::Resource => AttributeScope::Resource,
                    ScopeKind::Any => AttributeScope::Any,
                    ScopeKind::Parent => {
                        return Err(QueryError::unsupported(position, "the `parent.` scope"));
                    }
                    ScopeKind::Event => {
                        return Err(QueryError::unsupported(position, "the `event.` scope"));
                    }
                    ScopeKind::Link => {
                        return Err(QueryError::unsupported(position, "the `link.` scope"));
                    }
                    ScopeKind::Instrumentation => {
                        return Err(QueryError::unsupported(
                            position,
                            "the `instrumentation.` scope",
                        ));
                    }
                };
                Item::Value(Operand::Attribute(Attribute { scope, name }))
            }
            _ => return Err(self.unexpected("a value")),
        };
        self.bump();
        Ok((item, position))
    }

    /// `-` in front of a number literal. Any other use of `-` is arithmetic.
    fn negative_number(&mut self, position: usize) -> Result<(Item, usize), QueryError> {
        let value = match self.peek_tok(1) {
            Tok::Int(n) => Static::Int(-*n),
            Tok::Float(f) => Static::Float(-*f),
            Tok::Duration(_) => {
                return Err(QueryError::unsupported(position, "a negative duration"));
            }
            _ => return Err(QueryError::unsupported(position, "arithmetic")),
        };
        self.bump();
        self.bump();
        Ok((Item::Value(Operand::Static(value)), position))
    }

    /// A bare word as an operand: a keyword value, an intrinsic, or an error.
    fn word(&self, word: &str, token: &Token) -> Result<Item, QueryError> {
        let value = match word {
            "true" => Static::Bool(true),
            "false" => Static::Bool(false),
            "nil" => return Ok(Item::Nil),
            "ok" => Static::Status(StatusCode::Ok),
            "error" => Static::Status(StatusCode::Error),
            "unset" => Static::Status(StatusCode::Unset),
            "unspecified" => Static::Kind(SpanKind::Unspecified),
            "internal" => Static::Kind(SpanKind::Internal),
            "server" => Static::Kind(SpanKind::Server),
            "client" => Static::Kind(SpanKind::Client),
            "producer" => Static::Kind(SpanKind::Producer),
            "consumer" => Static::Kind(SpanKind::Consumer),
            "name" => return Ok(intrinsic(Intrinsic::Name)),
            "status" => return Ok(intrinsic(Intrinsic::Status)),
            "statusMessage" => return Ok(intrinsic(Intrinsic::StatusMessage)),
            "kind" => return Ok(intrinsic(Intrinsic::Kind)),
            "duration" => return Ok(intrinsic(Intrinsic::Duration)),
            "rootName" => return Ok(intrinsic(Intrinsic::RootName)),
            "rootServiceName" => return Ok(intrinsic(Intrinsic::RootServiceName)),
            "traceDuration" => return Ok(intrinsic(Intrinsic::TraceDuration)),
            "parent" | "childCount" | "nestedSetLeft" | "nestedSetRight" | "nestedSetParent" => {
                return Err(QueryError::unsupported(token.start, &format!("`{word}`")));
            }
            _ => {
                // `http.status = 500`: a dotted name that forgot its scope.
                let message = if self.chars.get(token.end) == Some(&'.') {
                    format!(
                        "unknown intrinsic `{word}` (attributes need a scope: `span.{word}`, `resource.{word}` or `.{word}`)"
                    )
                } else {
                    format!("unknown intrinsic `{word}`")
                };
                return Err(QueryError::new(token.start, message));
            }
        };
        Ok(Item::Value(Operand::Static(value)))
    }
}

fn intrinsic(intrinsic: Intrinsic) -> Item {
    Item::Value(Operand::Intrinsic(intrinsic))
}

fn nil_error(position: usize) -> QueryError {
    QueryError::new(position, "`nil` can only be compared with `=` or `!=`")
}

/// `span:name`, `trace:rootService`, and the scoped intrinsics Tempo has that
/// loctel does not.
fn scoped_intrinsic(scope: &str, name: &str, position: usize) -> Result<Intrinsic, QueryError> {
    Ok(match (scope, name) {
        ("span", "name") => Intrinsic::Name,
        ("span", "status") => Intrinsic::Status,
        ("span", "statusMessage") => Intrinsic::StatusMessage,
        ("span", "kind") => Intrinsic::Kind,
        ("span", "duration") => Intrinsic::Duration,
        ("trace", "rootName") => Intrinsic::RootName,
        ("trace", "rootService") => Intrinsic::RootServiceName,
        ("trace", "duration") => Intrinsic::TraceDuration,
        ("span", "id" | "parentID" | "childCount")
        | ("trace", "id")
        | ("event", "name" | "timeSinceStart")
        | ("link", "traceID" | "spanID")
        | ("instrumentation", "name" | "version") => {
            return Err(QueryError::unsupported(
                position,
                &format!("`{scope}:{name}`"),
            ));
        }
        _ => {
            return Err(QueryError::new(
                position,
                format!("unknown intrinsic `{scope}:{name}`"),
            ));
        }
    })
}
