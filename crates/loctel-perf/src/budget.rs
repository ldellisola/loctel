//! The budgets of design 10.3 (Milestone 5, "Performance budgets" in
//! `docs/areas/testing.md`), what a scenario measured against each, and the
//! table the check prints. Pure: nothing here measures anything.
//!
//! A budget is an `id` (what `--only` takes), a `measure` (what the number is)
//! and a [`Limit`]. Most ids have one budget; `ingest` has three (the rate,
//! what was dropped and the slowest batches). A [`Measurement`] names its budget,
//! so a number can never be compared with another one's limit, and a [`Report`]
//! puts the measurements in the table's order, judges each and says what the
//! exit code of the check is.

use std::fmt::Write as _;

use serde_json::{Value, json};

/// How a measurement is held to its budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Limit {
    /// The measurement must be at least this much (a rate).
    AtLeast(f64),
    /// The measurement must be at most this much (a count that must be zero).
    AtMost(f64),
    /// The measurement must be strictly below this much (a time or a size).
    Below(f64),
}

impl Limit {
    /// Whether `value` is within the limit. A value that is not a number never
    /// is.
    pub fn holds(self, value: f64) -> bool {
        match self {
            Limit::AtLeast(least) => value >= least,
            Limit::AtMost(most) => value <= most,
            Limit::Below(bound) => value < bound,
        }
    }
}

/// What a measurement is counted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Millis,
    PerSecond,
    Count,
    Bytes,
}

impl Unit {
    /// A short name for the JSON.
    pub fn name(self) -> &'static str {
        match self {
            Unit::Millis => "ms",
            Unit::PerSecond => "items/s",
            Unit::Count => "items",
            Unit::Bytes => "bytes",
        }
    }

    /// A measured value as the table shows it: milliseconds with one decimal (three
    /// under a millisecond),
    /// rates and counts whole, sizes in decimal units (what the status box
    /// calls a GB).
    pub fn format(self, value: f64) -> String {
        match self {
            // Under a millisecond a tenth is no information: the counts from
            // the metadata take microseconds.
            Unit::Millis if value.abs() < 1.0 => format!("{value:.3} ms"),
            Unit::Millis => format!("{} ms", group(&format!("{value:.1}"))),
            Unit::PerSecond => format!("{} items/s", group(&format!("{value:.0}"))),
            Unit::Count => match value {
                1.0 => "1 item".to_owned(),
                _ => format!("{} items", group(&format!("{value:.0}"))),
            },
            Unit::Bytes => {
                if value >= 1e9 {
                    format!("{:.2} GB", value / 1e9)
                } else if value >= 1e6 {
                    format!("{:.0} MB", value / 1e6)
                } else if value >= 1e3 {
                    format!("{:.0} KB", value / 1e3)
                } else {
                    format!("{value:.0} B")
                }
            }
        }
    }

    /// A limit as the table shows it: like [`format`](Self::format), without the
    /// decimal of a whole number (`< 250 ms`, not `< 250.0 ms`).
    fn format_limit(self, value: f64) -> String {
        match self {
            Unit::Millis if value.fract() == 0.0 => format!("{} ms", group(&format!("{value:.0}"))),
            _ => self.format(value),
        }
    }
}

/// Puts a comma between the thousands of the whole part of a number written
/// with a point (`1234.5` is `1,234.5`).
fn group(number: &str) -> String {
    let (sign, rest) = number
        .strip_prefix('-')
        .map_or(("", number), |rest| ("-", rest));
    let (whole, fraction) = rest
        .split_once('.')
        .map_or((rest, None), |(a, b)| (a, Some(b)));
    let mut grouped = String::with_capacity(number.len() + whole.len() / 3);
    for (i, digit) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

/// One thing the performance check holds to a limit.
#[derive(Debug, PartialEq)]
pub struct Budget {
    /// What `cargo xtask perf --only` takes. Not unique: `ingest` has three
    /// budgets.
    pub id: &'static str,
    /// What the number is, and how it is taken.
    pub measure: &'static str,
    pub limit: Limit,
    pub unit: Unit,
}

impl Budget {
    /// The limit as the table's budget column says it.
    pub fn describe(&self) -> String {
        match self.limit {
            Limit::AtLeast(least) => format!(">= {}", self.unit.format_limit(least)),
            Limit::AtMost(most) => format!("<= {}", self.unit.format_limit(most)),
            Limit::Below(bound) => format!("< {}", self.unit.format_limit(bound)),
        }
    }
}

// The values are the plan's (Milestone 5, Global Constraints), and the test
// `the_table_lists_the_budgets_of_the_plan` pins them: a budget is raised only
// with a decision file that records the measurements (`docs/decisions/`), never
// to get a run green.

pub static INGEST_RATE: Budget = Budget {
    id: "ingest",
    measure: "items accepted a second over 20 s",
    limit: Limit::AtLeast(50_000.0),
    unit: Unit::PerSecond,
};
pub static INGEST_DROPPED: Budget = Budget {
    id: "ingest",
    measure: "items sent and not accepted",
    limit: Limit::AtMost(0.0),
    unit: Unit::Count,
};
pub static INGEST_P99: Budget = Budget {
    id: "ingest",
    measure: "p99 latency of a batch",
    limit: Limit::Below(250.0),
    unit: Unit::Millis,
};
pub static TRACE_OPEN_HOT: Budget = Budget {
    id: "trace_open_hot",
    measure: "trace + Waterfall::new, 100k spans, in memory",
    limit: Limit::Below(250.0),
    unit: Unit::Millis,
};
pub static TRACE_OPEN_COLD: Budget = Budget {
    id: "trace_open_cold",
    measure: "trace + Waterfall::new, 100k spans, in a segment",
    limit: Limit::Below(250.0),
    unit: Unit::Millis,
};
pub static LOGS_COUNT: Budget = Budget {
    id: "logs_count",
    measure: "log_count, 10M logs, levels and service",
    limit: Limit::Below(50.0),
    unit: Unit::Millis,
};
pub static LOGS_AT: Budget = Budget {
    id: "logs_at",
    measure: "logs_at(offset, 50), 100 random offsets, p95",
    limit: Limit::Below(20.0),
    unit: Unit::Millis,
};
pub static HISTOGRAM: Budget = Budget {
    id: "histogram",
    measure: "48 buckets, 10M logs, levels and service",
    limit: Limit::Below(100.0),
    unit: Unit::Millis,
};
pub static TRACEQL_FIRST_PAGE: Budget = Budget {
    id: "traceql_first_page",
    measure: "{ status = error }, 10M spans, limit 20",
    limit: Limit::Below(1_000.0),
    unit: Unit::Millis,
};
pub static LOGQL_FIRST_PAGE: Budget = Budget {
    id: "logql_first_page",
    measure: "{service_name=\"payments-api\"} |= \"timeout\", 10M logs, limit 50",
    limit: Limit::Below(1_000.0),
    unit: Unit::Millis,
};
pub static STARTUP_HEADLESS: Budget = Budget {
    id: "startup_headless",
    measure: "process start to the last listener line",
    limit: Limit::Below(300.0),
    unit: Unit::Millis,
};
pub static FIRST_FRAME: Budget = Budget {
    id: "first_frame",
    measure: "main window and first render, a process each",
    limit: Limit::Below(1_500.0),
    unit: Unit::Millis,
};
pub static RSS_10M: Budget = Budget {
    id: "rss_10m",
    measure: "resident memory after 10M logs and 1M spans",
    limit: Limit::Below(1_500_000_000.0),
    unit: Unit::Bytes,
};

/// Every budget, in the order of the table.
pub static BUDGETS: [&Budget; 13] = [
    &INGEST_RATE,
    &INGEST_DROPPED,
    &INGEST_P99,
    &TRACE_OPEN_HOT,
    &TRACE_OPEN_COLD,
    &LOGS_COUNT,
    &LOGS_AT,
    &HISTOGRAM,
    &TRACEQL_FIRST_PAGE,
    &LOGQL_FIRST_PAGE,
    &STARTUP_HEADLESS,
    &FIRST_FRAME,
    &RSS_10M,
];

/// The budgets with this `id`, in table order.
pub fn budgets_of(id: &str) -> impl Iterator<Item = &'static Budget> + '_ {
    BUDGETS
        .iter()
        .copied()
        .filter(move |budget| budget.id == id)
}

/// Whether `id` is the id of a budget.
pub fn is_budget_id(id: &str) -> bool {
    budgets_of(id).next().is_some()
}

/// The ids of the budgets, each once, in table order.
pub fn budget_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = Vec::new();
    for budget in BUDGETS {
        if !ids.contains(&budget.id) {
            ids.push(budget.id);
        }
    }
    ids
}

/// What a scenario found for one budget.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Value(f64),
    /// It cannot be measured here, and why (the resident size of a process is
    /// read from `/proc`, which only Linux has). Never a breach.
    NotMeasured(String),
}

/// A scenario's number for a budget.
#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub budget: &'static Budget,
    pub outcome: Outcome,
}

impl Measurement {
    pub fn value(budget: &'static Budget, value: f64) -> Measurement {
        Measurement {
            budget,
            outcome: Outcome::Value(value),
        }
    }

    pub fn not_measured(budget: &'static Budget, why: impl Into<String>) -> Measurement {
        Measurement {
            budget,
            outcome: Outcome::NotMeasured(why.into()),
        }
    }
}

/// How a row of the table came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Breach,
    NotMeasured,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Verdict::Pass => "ok",
            Verdict::Breach => "BREACH",
            Verdict::NotMeasured => "n/a",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Breach => "breach",
            Verdict::NotMeasured => "not_measured",
        }
    }
}

/// One line of the table: a budget and what was measured against it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: &'static str,
    pub measure: &'static str,
    /// The limit as the table writes it (`< 250 ms`).
    pub budget: String,
    pub unit: Unit,
    pub measured: Option<f64>,
    /// The measurement as the table writes it, or why there is none.
    pub measured_text: String,
    pub note: Option<String>,
    pub verdict: Verdict,
}

/// The measurements of a run, judged, in the order of the table.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    rows: Vec<Row>,
}

impl Report {
    /// Judges every measurement against its budget. A budget nobody measured
    /// has no row (`--only` runs one scenario), and the rows come in the
    /// table's order whatever order the measurements arrive in.
    pub fn new(measurements: Vec<Measurement>) -> Report {
        let position = |budget: &Budget| {
            BUDGETS
                .iter()
                .position(|known| std::ptr::eq(*known, budget))
                .unwrap_or(BUDGETS.len())
        };
        let mut measurements = measurements;
        measurements.sort_by_key(|measurement| position(measurement.budget));
        let rows = measurements
            .into_iter()
            .map(|Measurement { budget, outcome }| {
                let (measured, measured_text, note, verdict) = match outcome {
                    Outcome::Value(value) => (
                        Some(value),
                        budget.unit.format(value),
                        None,
                        if budget.limit.holds(value) {
                            Verdict::Pass
                        } else {
                            Verdict::Breach
                        },
                    ),
                    Outcome::NotMeasured(why) => (
                        None,
                        "not measured".to_owned(),
                        Some(why),
                        Verdict::NotMeasured,
                    ),
                };
                Row {
                    id: budget.id,
                    measure: budget.measure,
                    budget: budget.describe(),
                    unit: budget.unit,
                    measured,
                    measured_text,
                    note,
                    verdict,
                }
            })
            .collect();
        Report { rows }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// How many rows broke their budget.
    pub fn breaches(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.verdict == Verdict::Breach)
            .count()
    }

    /// 1 when any budget was missed, else 0: the exit code of the check.
    pub fn exit_code(&self) -> i32 {
        i32::from(self.breaches() > 0)
    }

    /// The table: what was measured against its budget, and the verdict.
    pub fn table(&self) -> String {
        let header = ["id", "measure", "budget", "measured", "verdict"];
        let cells: Vec<[String; 5]> = self
            .rows
            .iter()
            .map(|row| {
                let measured = match &row.note {
                    Some(note) => format!("{} ({note})", row.measured_text),
                    None => row.measured_text.clone(),
                };
                [
                    row.id.to_owned(),
                    row.measure.to_owned(),
                    row.budget.clone(),
                    measured,
                    row.verdict.word().to_owned(),
                ]
            })
            .collect();
        let mut widths = header.map(str::len);
        for line in &cells {
            for (width, cell) in widths.iter_mut().zip(line) {
                *width = (*width).max(cell.chars().count());
            }
        }
        let mut out = String::new();
        let mut line = |cells: [&str; 5]| {
            let mut text = String::new();
            for (i, (cell, width)) in cells.iter().zip(widths).enumerate() {
                if i > 0 {
                    text.push_str("  ");
                }
                let _ = write!(text, "{cell:<width$}");
            }
            out.push_str(text.trim_end());
            out.push('\n');
        };
        line(header);
        line(
            widths
                .map(|width| "-".repeat(width))
                .each_ref()
                .map(String::as_str),
        );
        for cells in &cells {
            line(cells.each_ref().map(String::as_str));
        }

        let breaches = self.breaches();
        let not_measured = self
            .rows
            .iter()
            .filter(|row| row.verdict == Verdict::NotMeasured)
            .count();
        out.push('\n');
        if breaches > 0 {
            let _ = writeln!(out, "{breaches} of {} budgets breached", self.rows.len());
        } else if not_measured > 0 {
            let _ = writeln!(out, "no budget breached ({not_measured} not measured)");
        } else {
            let _ = writeln!(out, "no budget breached");
        }
        out
    }

    /// The report as JSON: a row per measurement (`id`, `measure`, `budget`,
    /// `measured` as a number in `unit` or `null`, `note`, `verdict` as `pass`,
    /// `breach` or `not_measured`) and the number of `breaches`.
    pub fn to_json(&self) -> Value {
        let rows: Vec<Value> = self
            .rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.id,
                    "measure": row.measure,
                    "budget": row.budget,
                    "unit": row.unit.name(),
                    "measured": row.measured.filter(|value| value.is_finite()),
                    "note": row.note,
                    "verdict": row.verdict.key(),
                })
            })
            .collect();
        json!({ "rows": rows, "breaches": self.breaches() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(rows: &'a [Row], id: &str) -> &'a Row {
        rows.iter()
            .find(|row| row.id == id)
            .unwrap_or_else(|| panic!("no row for {id}"))
    }

    #[test]
    fn budget_table_flags_breaches() {
        // A fake measurement over its budget (`logs_count` must be under 50 ms).
        let report = Report::new(vec![
            Measurement::value(&LOGS_COUNT, 80.0),
            Measurement::value(&HISTOGRAM, 12.0),
            Measurement::not_measured(&RSS_10M, "only measured on Linux"),
        ]);

        let rows = report.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(row(rows, "logs_count").verdict, Verdict::Breach);
        assert_eq!(row(rows, "histogram").verdict, Verdict::Pass);
        assert_eq!(row(rows, "rss_10m").verdict, Verdict::NotMeasured);
        assert_eq!(report.exit_code(), 1, "one breach fails the check");

        // The table says measured against budget, and which row broke it.
        let table = report.table();
        let breach = table
            .lines()
            .find(|line| line.contains("logs_count"))
            .expect("a line for logs_count");
        assert!(breach.contains("80.0 ms"), "{breach}");
        assert!(breach.contains("< 50 ms"), "{breach}");
        assert!(breach.contains("BREACH"), "{breach}");
        let fine = table
            .lines()
            .find(|line| line.contains("histogram"))
            .expect("a line for histogram");
        assert!(fine.contains("ok") && !fine.contains("BREACH"), "{fine}");
        assert!(table.contains("only measured on Linux"), "{table}");
        assert!(table.contains("1 of 3 budgets breached"), "{table}");
    }

    #[test]
    fn a_report_within_every_budget_exits_zero() {
        let report = Report::new(vec![
            Measurement::value(&LOGS_COUNT, 0.2),
            Measurement::value(&INGEST_RATE, 400_000.0),
            Measurement::value(&INGEST_DROPPED, 0.0),
            Measurement::not_measured(&RSS_10M, "not on this OS"),
        ]);

        assert_eq!(report.exit_code(), 0, "{}", report.table());
        assert!(!report.table().contains("BREACH"));
    }

    #[test]
    fn each_kind_of_limit_is_strict_where_it_says_so() {
        // "< 250 ms": 250 is a miss. ">= 50,000": 50,000 is a hit. "0 dropped": 1 is a miss.
        let at = |budget: &'static Budget, value: f64| {
            Report::new(vec![Measurement::value(budget, value)]).exit_code()
        };

        assert_eq!(at(&TRACE_OPEN_HOT, 249.9), 0);
        assert_eq!(at(&TRACE_OPEN_HOT, 250.0), 1);
        assert_eq!(at(&INGEST_RATE, 50_000.0), 0);
        assert_eq!(at(&INGEST_RATE, 49_999.0), 1);
        assert_eq!(at(&INGEST_DROPPED, 0.0), 0);
        assert_eq!(at(&INGEST_DROPPED, 1.0), 1);
    }

    #[test]
    fn a_value_that_is_not_a_number_is_a_breach() {
        let report = Report::new(vec![Measurement::value(&LOGS_COUNT, f64::NAN)]);

        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn the_table_lists_the_budgets_of_the_plan() {
        let ids: Vec<&str> = BUDGETS.iter().map(|budget| budget.id).collect();
        assert_eq!(
            ids,
            [
                "ingest",
                "ingest",
                "ingest",
                "trace_open_hot",
                "trace_open_cold",
                "logs_count",
                "logs_at",
                "histogram",
                "traceql_first_page",
                "logql_first_page",
                "startup_headless",
                "first_frame",
                "rss_10m",
            ]
        );
        let limits: Vec<Limit> = BUDGETS.iter().map(|budget| budget.limit).collect();
        assert_eq!(
            limits,
            [
                Limit::AtLeast(50_000.0),
                Limit::AtMost(0.0),
                Limit::Below(250.0),
                Limit::Below(250.0),
                Limit::Below(250.0),
                Limit::Below(50.0),
                Limit::Below(20.0),
                Limit::Below(100.0),
                Limit::Below(1_000.0),
                Limit::Below(1_000.0),
                Limit::Below(300.0),
                Limit::Below(1_500.0),
                Limit::Below(1_500_000_000.0),
            ]
        );
        // (id, measure) names a budget.
        for (i, a) in BUDGETS.iter().enumerate() {
            for b in &BUDGETS[i + 1..] {
                assert!(a.id != b.id || a.measure != b.measure, "{} twice", a.id);
            }
        }
    }

    #[test]
    fn rows_come_in_table_order_whatever_order_they_were_measured_in() {
        let report = Report::new(vec![
            Measurement::value(&RSS_10M, 1.0),
            Measurement::value(&LOGS_COUNT, 1.0),
            Measurement::value(&INGEST_P99, 1.0),
        ]);

        let ids: Vec<&str> = report.rows().iter().map(|row| row.id).collect();
        assert_eq!(ids, ["ingest", "logs_count", "rss_10m"]);
    }

    #[test]
    fn only_selects_every_budget_of_an_id() {
        assert_eq!(budgets_of("ingest").count(), 3);
        assert_eq!(budgets_of("logs_at").count(), 1);
        assert_eq!(budgets_of("nope").count(), 0);
        assert!(is_budget_id("first_frame"));
        assert!(!is_budget_id("first"));
    }

    #[test]
    fn the_json_has_a_row_per_measurement() {
        let report = Report::new(vec![
            Measurement::value(&LOGS_COUNT, 80.0),
            Measurement::not_measured(&RSS_10M, "not on this OS"),
        ]);

        let json = report.to_json();

        let rows = json["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], "logs_count");
        assert_eq!(rows[0]["measured"], 80.0);
        assert_eq!(rows[0]["unit"], "ms");
        assert_eq!(rows[0]["budget"], "< 50 ms");
        assert_eq!(rows[0]["verdict"], "breach");
        assert_eq!(rows[1]["measured"], serde_json::Value::Null);
        assert_eq!(rows[1]["note"], "not on this OS");
        assert_eq!(rows[1]["verdict"], "not_measured");
        assert_eq!(json["breaches"], 1);
    }

    #[test]
    fn formats_values_the_way_the_table_reads() {
        assert_eq!(Unit::Millis.format(12.34), "12.3 ms");
        assert_eq!(Unit::Millis.format(1_234.5), "1,234.5 ms");
        assert_eq!(Unit::Millis.format(0.0042), "0.004 ms");
        assert_eq!(Unit::Millis.format(0.0), "0.000 ms");
        assert_eq!(Unit::PerSecond.format(412_345.6), "412,346 items/s");
        assert_eq!(Unit::Count.format(0.0), "0 items");
        assert_eq!(Unit::Count.format(1.0), "1 item");
        assert_eq!(Unit::Bytes.format(1_234_000_000.0), "1.23 GB");
        assert_eq!(Unit::Bytes.format(512_000.0), "512 KB");
    }
}
