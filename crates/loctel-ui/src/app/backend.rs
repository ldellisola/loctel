//! What the main window needs from the engine, as a trait, so the views never
//! name the engine and a test can hand them a fixed one.

use std::rc::Rc;

use loctel_engine::{EngineStatus, ListenerStatus, Settings};
use loctel_query::QueryError;
use loctel_rules::{Preview, RuleId, RuleSpec, RuleStatus};
use loctel_store::Store;

use super::app_backend::LoginItem;

/// The window's view of the running app.
///
/// The views read the store and the status, and ask for the actions below: the
/// discard rules (read, replace, switch, preview), the settings (read, save and
/// rebind), receiving (pause) and the login item. They hold it as
/// `Rc<dyn UiBackend>`: it lives on GPUI's foreground thread, and the parts that
/// cross threads are the [`Store`] (a cheap clone) a refresh moves onto the
/// background executor. [`AppBackend`](super::AppBackend) is the real one over
/// the engine.
pub trait UiBackend: 'static {
    /// The store the receivers fill. A clone is the same store.
    fn store(&self) -> Store;
    /// A snapshot of pause, endpoints, rates, what the store holds and the
    /// discard rules.
    fn status(&self) -> EngineStatus;
    /// Empties the store (traces and logs; the `*_received` totals stay).
    /// Receiving carries on.
    fn clear_all(&self);
    /// The offset of the system's time zone from UTC, in seconds, for the
    /// clock times the tables show. Tests return 0.
    fn utc_offset(&self) -> i32;

    /// The discard rules, in file order, with their query errors and what each
    /// has dropped.
    fn rules(&self) -> Vec<RuleStatus>;
    /// Makes `specs` the rules, in that order, and saves `rules.toml`. A rule
    /// that did not change keeps its ID and counters, and a query that does not
    /// parse is allowed (it shows as that rule's error).
    fn set_rules(&self, specs: Vec<RuleSpec>) -> anyhow::Result<()>;
    /// Turns the rule `id` on or off and saves. An unknown `id` is an error.
    fn set_rule_enabled(&self, id: RuleId, on: bool) -> anyhow::Result<()>;
    /// What `spec` would have dropped of the last five minutes of stored data,
    /// ignoring whether it is on. The error is the query's, with its position.
    fn preview_rule(&self, spec: &RuleSpec) -> Result<Preview, QueryError>;
    /// How many times the rules have been replaced since launch (a save, a
    /// switch or a reload of an edited file). The rules editor reads it when it
    /// opens a rule and compares it later: a different number means the rules
    /// changed behind its back.
    fn rules_generation(&self) -> u64;

    /// The saved settings (`settings.toml`, the defaults when there is no file).
    fn settings(&self) -> Settings;
    /// Saves `s` to `settings.toml` and moves the listeners to its ports.
    /// A port that cannot be bound is not an error: the setting is saved, and
    /// that listener is `Failed` in the statuses that come back (in the order
    /// `otlp-grpc`, `otlp-http`, `mcp`). An error means the file could not be
    /// written, and then nothing moved.
    fn apply_settings(&self, s: Settings) -> anyhow::Result<Vec<ListenerStatus>>;
    /// Start at login, on the platforms that have it (macOS); `None` elsewhere,
    /// where the settings window has no such switch. The system is the source of
    /// truth: ask it, do not mirror it.
    fn login_item(&self) -> Option<Rc<dyn LoginItem>>;

    /// Pauses (`true`) or resumes receiving. While paused everything that
    /// arrives is dropped.
    fn set_paused(&self, on: bool);
    /// Where agents reach the MCP server, or `None` when it could not bind.
    fn mcp_url(&self) -> Option<String>;
}
