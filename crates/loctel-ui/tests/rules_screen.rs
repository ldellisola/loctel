//! The Discard rules screen (M4 plan, Task 5): the list of cards with their
//! switches and counts, and the editor with its live validation and the preview
//! of the last five minutes, in the real window over the fake mesh. The
//! references are `snapshots/rules_*.png`; build them with `cargo xtask
//! screenshots --update` and look at them before committing
//! (`docs/areas/ui.md`).
//!
//! The store holds the failing checkout, the successful one and 600 health
//! checks (5 services, 120 probes each, one trace and one log per probe), on a
//! clock two minutes after the last of them, so a rule on `GET /health` would
//! have dropped 600 spans in 600 traces of the last five minutes. The backend
//! is a `FixedBackend`: the rules are in memory (there is no file), and a test
//! that wants "the file changed behind the editor's back" calls
//! `set_rules` on it, which moves the rules generation as a reload does.
//!
//! What the Review Focus of the plan asks of the editor is pinned here:
//! `external_change_during_edit_warns` (item 2: the draft stays, a banner says
//! the file changed, and saving overwrites only after `Keep my draft`) and
//! `invalid_rule_saved_shows_error_and_no_preview` (item 3).

mod common;

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, Entity, Pixels, Point};
use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{Generated, Scenario, generate};
use loctel_model::LogRecord;
use loctel_rules::{DropScope, PREVIEW_WINDOW, Preview, RuleLanguage, RuleSpec, RuleStatus};
use loctel_store::{ManualClock, Store, StoreConfig};
use loctel_ui::Theme;
use loctel_ui::app::{
    MainWindow, PREVIEW_DEBOUNCE, RULES_REFRESH, RulesModel, Screen, UiBackend as _,
};
use loctel_ui::model::rules::{
    Draft, NO_RULES, NOTHING_DROPPED, PREVIEW_INVALID, file_error_line, preview_text, rows,
    suggest_from_log,
};
use loctel_ui::testing::{Driver, FixedBackend, assert_snapshot};
use loctel_ui::views::{EXTERNAL_CHANGE, RulesPart, RulesView};

use common::{BASE, SECOND, SEED, WINDOW, feed};

const HEALTH_QUERY: &str = r#"{ name = "GET /health" }"#;
const CHATTER_QUERY: &str = r#"{service_name="payments-api"} |= "handled request""#;
const STAGING_QUERY: &str = r#"{ resource.deployment.environment = "staging" }"#;

/// What the rules file is said to be wrong with, in the file-error screenshot.
const FILE_ERROR: &str = "unknown key `scpoe` in rule 2, expected one of `name`, `enabled`, \
                          `language`, `query`, `scope`";

// --- the data ----------------------------------------------------------------------------

/// The failing checkout, the successful one and 600 health checks, on a clock
/// that reads two minutes and twenty seconds after the checkout.
fn rules_store() -> Store {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    feed(&store, &generate(&Scenario::CheckoutFailure, SEED, BASE));
    feed(
        &store,
        &generate(&Scenario::CheckoutSuccess, SEED, BASE + 5 * SECOND),
    );
    feed(
        &store,
        &generate(
            &Scenario::HealthChecks { count: 120 },
            SEED,
            BASE + 10 * SECOND,
        ),
    );
    clock.advance(Duration::from_secs(140));
    store
}

/// The three rules of the screenshots: a trace rule that is on, a log rule that
/// is on and a trace rule that is off.
fn three_rules() -> Vec<RuleSpec> {
    vec![
        RuleSpec {
            name: "Health checks".to_owned(),
            enabled: true,
            language: RuleLanguage::TraceQl,
            query: HEALTH_QUERY.to_owned(),
            scope: DropScope::Trace,
        },
        RuleSpec {
            name: "Request chatter".to_owned(),
            enabled: true,
            language: RuleLanguage::LogQl,
            query: CHATTER_QUERY.to_owned(),
            scope: DropScope::Trace,
        },
        RuleSpec {
            name: "Staging traffic".to_owned(),
            enabled: false,
            language: RuleLanguage::TraceQl,
            query: STAGING_QUERY.to_owned(),
            scope: DropScope::Span,
        },
    ]
}

/// Sends `generated` through the rules the way the engine's sink does, so the
/// rules have dropped something to count.
fn drop_through(backend: &FixedBackend, store: &Store, generated: &Generated) {
    for batch in &generated.batches {
        if let Some(request) = &batch.traces {
            let spans = spans_from_request(request.clone()).items;
            let verdict = backend.rule_set().filter_spans(spans, store);
            backend.rule_set().remove_dropped(&verdict, store);
        }
        if let Some(request) = &batch.logs {
            let logs = logs_from_request(request.clone()).items;
            backend.rule_set().filter_logs(logs);
        }
    }
}

/// A backend over [`rules_store`] with `specs` as its rules and 31% of the
/// incoming data said to be dropped.
fn backend_with(specs: Vec<RuleSpec>) -> (FixedBackend, Store) {
    let store = rules_store();
    let mut backend = FixedBackend::new(store.clone());
    backend.status.dropped_fraction = 0.31;
    backend.set_rules(specs).expect("the rules are set");
    (backend, store)
}

/// [`backend_with`] the three rules, after 1,500 health checks and 40,000 log
/// lines (none of them in the store) went through them: the first card says
/// `1,500 spans dropped in 1,500 traces`, the second `1,2xx lines dropped`.
fn counted_backend() -> FixedBackend {
    let (backend, store) = backend_with(three_rules());
    drop_through(
        &backend,
        &store,
        &generate(
            &Scenario::HealthChecks { count: 300 },
            99,
            BASE + 20 * SECOND,
        ),
    );
    drop_through(
        &backend,
        &store,
        &generate(&Scenario::LogFlood { records: 40_000 }, 99, BASE),
    );
    backend
}

/// The window on `backend`, with the first frame drawn.
fn window_on(backend: FixedBackend, theme: &Theme) -> (Driver<MainWindow>, Rc<FixedBackend>) {
    let backend = Rc::new(backend);
    let shown = Rc::clone(&backend);
    let driver = Driver::new(WINDOW, theme, move |window, cx| {
        cx.new(|cx| MainWindow::new(shown, None, window, cx))
    });
    (driver, backend)
}

/// Draws until what is pending (a read, its result, the frame that shows it) is
/// done.
fn settle(driver: &mut Driver<MainWindow>) {
    for _ in 0..4 {
        driver.draw();
    }
}

/// Shows the rules screen (with `draft` open in the editor, unsaved), and
/// settles.
fn open_rules(driver: &mut Driver<MainWindow>, draft: Option<Draft>) {
    driver.update_root(|main, _, cx| main.navigate(Screen::Rules { draft }, cx));
    settle(driver);
}

/// The window on the rules screen over `backend`.
fn rules_window_on(backend: FixedBackend, theme: &Theme) -> (Driver<MainWindow>, Rc<FixedBackend>) {
    let (mut driver, backend) = window_on(backend, theme);
    open_rules(&mut driver, None);
    (driver, backend)
}

/// The window on the rules screen with the three rules (nothing counted).
fn plain_window() -> (Driver<MainWindow>, Rc<FixedBackend>) {
    rules_window_on(backend_with(three_rules()).0, &Theme::dark())
}

// --- reaching into the window ----------------------------------------------------------------

fn model(driver: &Driver<MainWindow>) -> Entity<RulesModel> {
    driver.read_root(|main| main.rules().clone())
}

fn view(driver: &Driver<MainWindow>) -> Entity<RulesView> {
    driver.read_root(|main| main.rules_view().clone())
}

fn read_model<R>(driver: &Driver<MainWindow>, f: impl FnOnce(&RulesModel) -> R) -> R {
    let model = model(driver);
    driver.read(|cx| f(model.read(cx)))
}

fn read_view<R>(driver: &Driver<MainWindow>, f: impl FnOnce(&RulesView, &gpui::App) -> R) -> R {
    let view = view(driver);
    driver.read(|cx| f(view.read(cx), cx))
}

fn bounds(driver: &Driver<MainWindow>, part: RulesPart) -> Option<gpui::Bounds<Pixels>> {
    read_view(driver, |view, cx| view.bounds_of(part, cx))
}

fn center(driver: &Driver<MainWindow>, part: RulesPart) -> Point<Pixels> {
    bounds(driver, part)
        .unwrap_or_else(|| panic!("the rules screen has no {part:?} on the screen"))
        .center()
}

fn has(driver: &Driver<MainWindow>, part: RulesPart) -> bool {
    bounds(driver, part).is_some()
}

fn click(driver: &mut Driver<MainWindow>, part: RulesPart) {
    driver.click(center(driver, part));
    settle(driver);
}

/// What the cards say: the rows of the model.
fn card_rows(driver: &Driver<MainWindow>) -> Vec<loctel_ui::model::rules::RuleRow> {
    read_model(driver, |rules| rules.rows().to_vec())
}

fn counts(driver: &Driver<MainWindow>) -> Vec<String> {
    card_rows(driver)
        .into_iter()
        .map(|row| row.counts)
        .collect()
}

/// The rules the backend holds, as the file would.
fn saved(backend: &FixedBackend) -> Vec<RuleSpec> {
    backend
        .rules()
        .into_iter()
        .map(|status| status.spec)
        .collect()
}

fn names(specs: &[RuleSpec]) -> Vec<&str> {
    specs.iter().map(|spec| spec.name.as_str()).collect()
}

/// The editor's draft, `None` while no editor is open.
fn draft(driver: &Driver<MainWindow>) -> Option<Draft> {
    read_model(driver, |rules| {
        rules.editor().map(|editor| editor.draft().clone())
    })
}

fn external_change(driver: &Driver<MainWindow>) -> bool {
    read_model(driver, |rules| {
        rules
            .editor()
            .is_some_and(|editor| editor.external_change())
    })
}

fn preview_line(driver: &Driver<MainWindow>) -> Option<String> {
    read_view(driver, |view, cx| view.preview_text(cx))
}

fn query_error(driver: &Driver<MainWindow>) -> Option<String> {
    read_view(driver, |view, cx| view.query_error_line(cx))
}

/// What the preview of `spec` is on the store, worked out here: the numbers the
/// editor must show.
fn expected_preview(store: &Store, spec: &RuleSpec) -> Preview {
    let window = u64::try_from(PREVIEW_WINDOW.as_nanos()).unwrap();
    let since = store.now_unix_nano().saturating_sub(window);
    loctel_rules::preview(spec, store, since).expect("the query parses")
}

/// Types `text` into the input `part`: a click puts the focus there first.
fn type_into(driver: &mut Driver<MainWindow>, part: RulesPart, text: &str) {
    driver.click(center(driver, part));
    driver.type_text(text);
    settle(driver);
}

// --- the references --------------------------------------------------------------------------

#[test]
fn rules_dark() {
    let (mut driver, _) = rules_window_on(counted_backend(), &Theme::dark());
    let image = driver.capture();
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("rules_dark", &image);
}

#[test]
fn rules_light() {
    let (mut driver, _) = rules_window_on(counted_backend(), &Theme::light());
    let image = driver.capture();
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("rules_light", &image);
}

#[test]
fn rules_editor_dark() {
    let (mut driver, store) = {
        let (backend, store) = backend_with(three_rules());
        drop_through(
            &backend,
            &store,
            &generate(
                &Scenario::HealthChecks { count: 300 },
                99,
                BASE + 20 * SECOND,
            ),
        );
        let (driver, _) = rules_window_on(backend, &Theme::dark());
        (driver, store)
    };
    click(&mut driver, RulesPart::Card(0));

    // The first rule is open, with its fields and the preview filled in.
    let expected = three_rules().remove(0);
    assert_eq!(draft(&driver), Some(Draft::from_spec(&expected)));
    assert!(has(&driver, RulesPart::Editor));
    let numbers = expected_preview(&store, &expected);
    assert_eq!(numbers.traces, 600, "the 600 health checks of the store");
    assert_eq!(
        preview_line(&driver),
        Some(
            "Would have dropped 600 spans in 600 traces. None of those traces had errors."
                .to_owned()
        )
    );
    assert_eq!(
        preview_line(&driver),
        Some(preview_text(RuleLanguage::TraceQl, &numbers))
    );
    assert_snapshot("rules_editor_dark", &driver.capture());
}

#[test]
fn rules_empty_dark() {
    let (mut backend, _) = backend_with(Vec::new());
    backend.status.dropped_fraction = 0.0;
    let (mut driver, _) = rules_window_on(backend, &Theme::dark());

    assert!(card_rows(&driver).is_empty());
    assert_eq!(
        read_view(&driver, |view, cx| view.banners(cx)),
        vec![NO_RULES.to_owned()],
        "the empty list says so, and nothing else"
    );
    assert_eq!(
        read_view(&driver, |view, cx| view.dropped_line(cx)),
        NOTHING_DROPPED
    );
    assert!(has(&driver, RulesPart::NewRule));
    assert_snapshot("rules_empty_dark", &driver.capture());
}

#[test]
fn rules_file_error_dark() {
    let (mut backend, _) = backend_with(three_rules());
    backend.status.rules_file_error = Some(FILE_ERROR.to_owned());
    let (mut driver, _) = rules_window_on(backend, &Theme::dark());

    // The rules it had are still listed, under the banner.
    assert_eq!(card_rows(&driver).len(), 3);
    assert_eq!(
        read_view(&driver, |view, cx| view.banners(cx)),
        vec![file_error_line(FILE_ERROR)]
    );
    assert!(
        file_error_line(FILE_ERROR).ends_with("loctel keeps using the rules it had."),
        "{}",
        file_error_line(FILE_ERROR)
    );
    assert_snapshot("rules_file_error_dark", &driver.capture());
}

#[test]
fn rules_editor_invalid_dark() {
    let (mut driver, _) = rules_window_on(counted_backend(), &Theme::dark());
    click(&mut driver, RulesPart::NewRule);
    type_into(&mut driver, RulesPart::NameInput, "Half a query");
    type_into(&mut driver, RulesPart::QueryInput, "{ name = ");

    // The error under the query, the preview that asks for a fix, and a new
    // rule that has nothing to delete.
    assert!(query_error(&driver).is_some_and(|line| line.starts_with("at column ")));
    assert_eq!(preview_line(&driver), Some(PREVIEW_INVALID.to_owned()));
    assert!(!has(&driver, RulesPart::Delete));
    assert_snapshot("rules_editor_invalid_dark", &driver.capture());
}

#[test]
fn rules_editor_changed_dark() {
    let (mut driver, backend) = rules_window_on(counted_backend(), &Theme::dark());
    edit_then_change_the_file(&mut driver, &backend);

    // The banner over the fields, Save off, the draft as typed, and the list
    // behind it showing what the file has now.
    assert!(external_change(&driver));
    assert!(has(&driver, RulesPart::KeepDraft));
    assert_eq!(card_rows(&driver).len(), 4);
    assert_snapshot("rules_editor_changed_dark", &driver.capture());
}

// --- the list -----------------------------------------------------------------------------------

#[test]
fn the_cards_say_what_each_rule_dropped() {
    let (driver, backend) = rules_window_on(counted_backend(), &Theme::dark());
    let statuses: Vec<RuleStatus> = backend.rules();
    assert_eq!(card_rows(&driver), rows(&statuses));
    let counts = counts(&driver);
    assert_eq!(counts[0], "1,500 spans dropped in 1,500 traces");
    assert!(
        counts[1].ends_with(" lines dropped") && counts[1].starts_with("1,"),
        "{}",
        counts[1]
    );
    assert_eq!(counts[2], "Off");
    for index in 0..3 {
        assert!(has(&driver, RulesPart::Card(index)), "card {index}");
        assert!(has(&driver, RulesPart::Switch(index)), "switch {index}");
    }
    assert!(!has(&driver, RulesPart::Card(3)));
    assert!(!has(&driver, RulesPart::Editor), "the editor is closed");
}

#[test]
fn the_header_says_how_much_of_the_incoming_data_was_dropped() {
    let (driver, _) = plain_window();
    assert_eq!(
        read_view(&driver, |view, cx| view.dropped_line(cx)),
        "31% of incoming data dropped"
    );
    assert!(has(&driver, RulesPart::NewRule));
}

#[test]
fn toggle_switch_disables_rule() {
    let (mut driver, backend) = plain_window();
    assert!(saved(&backend)[0].enabled);

    click(&mut driver, RulesPart::Switch(0));
    assert!(
        !saved(&backend)[0].enabled,
        "the switch turned the rule off"
    );
    assert_eq!(counts(&driver)[0], "Off", "and the card says so");
    assert!(
        !card_rows(&driver)[0].enabled,
        "the switch of the card is off"
    );
    assert!(
        saved(&backend)[1].enabled,
        "the other rules are left as they were"
    );
    assert!(
        draft(&driver).is_none(),
        "a click on the switch does not open the rule"
    );

    // And back on.
    click(&mut driver, RulesPart::Switch(0));
    assert!(saved(&backend)[0].enabled);
    assert_eq!(counts(&driver)[0], "0 spans dropped");
}

#[test]
fn clicking_a_card_opens_it_in_the_editor() {
    let (mut driver, _) = plain_window();
    click(&mut driver, RulesPart::Card(1));
    let rule = three_rules().remove(1);
    assert_eq!(draft(&driver), Some(Draft::from_spec(&rule)));
    assert_eq!(
        read_model(&driver, |rules| rules.selected()),
        Some(1),
        "the card of the open rule is the lit one"
    );
    // The inputs hold what the draft says.
    assert_eq!(
        read_view(&driver, |view, cx| view.name_text(cx)),
        "Request chatter"
    );
    assert_eq!(
        read_view(&driver, |view, cx| view.query_text(cx)),
        CHATTER_QUERY
    );
    // A log rule has no scope to choose.
    assert!(has(&driver, RulesPart::LanguageLogs));
    assert!(!has(&driver, RulesPart::ScopeTrace));
    assert!(!has(&driver, RulesPart::ScopeSpan));
    // Another card moves the editor to it.
    click(&mut driver, RulesPart::Card(2));
    assert_eq!(
        draft(&driver).map(|draft| draft.name),
        Some("Staging traffic".to_owned())
    );
    assert!(has(&driver, RulesPart::ScopeSpan), "a trace rule has one");
}

#[test]
fn the_list_refreshes_every_second() {
    let (mut driver, backend) = plain_window();
    assert_eq!(card_rows(&driver).len(), 3);

    // The file was edited: a rule more. Nothing changes until the next refresh.
    let mut specs = saved(&backend);
    specs.push(RuleSpec {
        name: "Edited by hand".to_owned(),
        enabled: true,
        language: RuleLanguage::LogQl,
        query: r#"{service_name="web-bff"} |= "ping""#.to_owned(),
        scope: DropScope::Trace,
    });
    backend.set_rules(specs).unwrap();
    driver.advance(RULES_REFRESH / 2);
    assert_eq!(card_rows(&driver).len(), 3, "half a second is not a second");
    driver.advance(RULES_REFRESH / 2);
    settle(&mut driver);
    let cards = card_rows(&driver);
    assert_eq!(cards.len(), 4);
    assert_eq!(cards[3].name, "Edited by hand");
    assert!(has(&driver, RulesPart::Card(3)), "and the card is drawn");
}

#[test]
fn a_hidden_rules_screen_catches_up_when_it_shows() {
    let (mut driver, backend) = window_on(backend_with(three_rules()).0, &Theme::dark());
    assert!(!read_model(&driver, |rules| rules.active()));
    let mut specs = saved(&backend);
    specs.pop();
    backend.set_rules(specs).unwrap();
    driver.advance(RULES_REFRESH * 3);
    assert_eq!(
        card_rows(&driver).len(),
        3,
        "a screen that is not showing reads nothing"
    );

    open_rules(&mut driver, None);
    assert!(read_model(&driver, |rules| rules.active()));
    assert_eq!(card_rows(&driver).len(), 2, "it reads as it comes back");
}

// --- the editor ---------------------------------------------------------------------------------

#[test]
fn new_rule_save_appears_in_list() {
    let (mut driver, backend) = plain_window();
    click(&mut driver, RulesPart::NewRule);

    // A blank rule, unsaved: traces, the whole trace, on.
    let blank = draft(&driver).expect("the editor is open");
    assert_eq!(
        blank,
        Draft {
            name: String::new(),
            language: RuleLanguage::TraceQl,
            query: String::new(),
            scope: DropScope::Trace,
            enabled: true,
        }
    );
    assert!(read_model(&driver, |rules| rules
        .editor()
        .is_some_and(|editor| editor.is_new())));
    assert_eq!(saved(&backend).len(), 3, "nothing is saved yet");

    // Save needs a name.
    click(&mut driver, RulesPart::Save);
    assert_eq!(saved(&backend).len(), 3, "Save does nothing without a name");
    assert!(draft(&driver).is_some(), "and the editor stays open");

    type_into(&mut driver, RulesPart::NameInput, "Drop charges");
    type_into(
        &mut driver,
        RulesPart::QueryInput,
        r#"{ name = "POST /charges" }"#,
    );
    assert_eq!(
        draft(&driver).map(|draft| (draft.name, draft.query)),
        Some((
            "Drop charges".to_owned(),
            r#"{ name = "POST /charges" }"#.to_owned()
        )),
        "what is typed is the draft"
    );

    // The preview follows once the typing has stopped.
    driver.advance(PREVIEW_DEBOUNCE);
    settle(&mut driver);
    let wanted = RuleSpec {
        name: "Drop charges".to_owned(),
        enabled: true,
        language: RuleLanguage::TraceQl,
        query: r#"{ name = "POST /charges" }"#.to_owned(),
        scope: DropScope::Trace,
    };
    let numbers = expected_preview(&rules_store(), &wanted);
    assert!(numbers.traces >= 1, "the checkouts have a POST /charges");
    assert_eq!(
        preview_line(&driver),
        Some(preview_text(RuleLanguage::TraceQl, &numbers))
    );

    click(&mut driver, RulesPart::Save);
    let specs = saved(&backend);
    assert_eq!(specs.len(), 4);
    assert_eq!(specs[3], wanted, "saved last, as typed");
    assert_eq!(
        names(&specs[..3]),
        ["Health checks", "Request chatter", "Staging traffic"],
        "the others are as they were, in their order"
    );
    assert!(draft(&driver).is_none(), "saving closes the editor");
    let cards = card_rows(&driver);
    assert_eq!(cards.len(), 4);
    assert_eq!(cards[3].name, "Drop charges");
    assert_eq!(cards[3].chip, "TRACEQL");
    assert_eq!(cards[3].counts, "0 spans dropped");
    assert!(has(&driver, RulesPart::Card(3)), "and the card is drawn");
}

#[test]
fn invalid_rule_saved_shows_error_and_no_preview() {
    let (mut driver, backend) = plain_window();
    click(&mut driver, RulesPart::NewRule);
    type_into(&mut driver, RulesPart::NameInput, "Half a query");
    type_into(&mut driver, RulesPart::QueryInput, "{ name = ");

    // While it is edited: the error under the query, and no numbers.
    let line = query_error(&driver).expect("the query does not parse");
    assert!(line.starts_with("at column "), "{line}");
    assert_eq!(
        preview_line(&driver),
        Some(PREVIEW_INVALID.to_owned()),
        "the preview does not wait for the typing to stop to say it"
    );

    // It can be saved as a draft, per M2.
    click(&mut driver, RulesPart::Save);
    let specs = saved(&backend);
    assert_eq!(specs.len(), 4, "an invalid query is allowed");
    assert_eq!(specs[3].name, "Half a query");
    assert!(draft(&driver).is_none());

    // Its card shows `Error: <message>` in the error colour and it drops
    // nothing.
    let card = card_rows(&driver).remove(3);
    let message = card.error.clone().expect("the card has an error");
    assert_eq!(card.counts, format!("Error: {message}"));
    assert!(backend.rules()[3].error.is_some());
    let checkout = generate(&Scenario::CheckoutFailure, SEED, BASE);
    let request = checkout.batches[0].traces.clone().expect("spans");
    let spans = spans_from_request(request).items;
    let sent = spans.len();
    let verdict = backend.rule_set().filter_spans(spans, &backend.store());
    assert!(
        verdict.drop_traces.is_empty() && verdict.keep.len() == sent,
        "a rule with an error drops nothing"
    );

    // Opened again, the editor still says so, and the preview shows no numbers.
    click(&mut driver, RulesPart::Card(3));
    assert!(query_error(&driver).is_some_and(|line| line.starts_with("at column ")));
    assert_eq!(preview_line(&driver), Some(PREVIEW_INVALID.to_owned()));
}

#[test]
fn the_preview_waits_for_the_typing_to_stop() {
    let (mut driver, _) = plain_window();
    click(&mut driver, RulesPart::NewRule);
    assert_eq!(
        preview_line(&driver),
        None,
        "an empty query has nothing yet"
    );
    type_into(&mut driver, RulesPart::NameInput, "Probes");
    type_into(&mut driver, RulesPart::QueryInput, HEALTH_QUERY);

    // Typed, and 300 ms have not passed: no result yet.
    assert_eq!(PREVIEW_DEBOUNCE, Duration::from_millis(300));
    assert_eq!(preview_line(&driver), None);
    driver.advance(PREVIEW_DEBOUNCE - Duration::from_millis(1));
    settle(&mut driver);
    assert_eq!(preview_line(&driver), None, "not yet");
    driver.advance(Duration::from_millis(1));
    settle(&mut driver);
    assert_eq!(
        preview_line(&driver),
        Some(
            "Would have dropped 600 spans in 600 traces. None of those traces had errors."
                .to_owned()
        )
    );
}

#[test]
fn an_edit_during_the_wait_restarts_it() {
    let (mut driver, _) = plain_window();
    click(&mut driver, RulesPart::NewRule);
    type_into(&mut driver, RulesPart::QueryInput, HEALTH_QUERY);
    driver.advance(Duration::from_millis(200));
    // Another letter: the wait starts over, and the old one does not run.
    driver.type_text(" ");
    settle(&mut driver);
    driver.advance(Duration::from_millis(200));
    settle(&mut driver);
    assert_eq!(
        preview_line(&driver),
        None,
        "400 ms after the first letter, 200 ms after the last"
    );
    driver.advance(Duration::from_millis(100));
    settle(&mut driver);
    assert!(preview_line(&driver).is_some());
}

#[test]
fn switching_the_language_changes_the_parser_the_scope_and_the_preview() {
    let (mut driver, _) = plain_window();
    // The staging rule: a TraceQL query LogQL cannot read (a label name has no
    // dots there), that drops only the matching spans.
    click(&mut driver, RulesPart::Card(2));
    assert!(has(&driver, RulesPart::ScopeTrace));
    assert!(has(&driver, RulesPart::ScopeSpan));
    assert!(query_error(&driver).is_none());

    // The same text read as LogQL is not a query.
    click(&mut driver, RulesPart::LanguageLogs);
    assert_eq!(
        draft(&driver).map(|draft| draft.language),
        Some(RuleLanguage::LogQl)
    );
    assert!(!has(&driver, RulesPart::ScopeTrace), "logs have no scope");
    assert!(query_error(&driver).is_some());
    assert_eq!(preview_line(&driver), Some(PREVIEW_INVALID.to_owned()));

    // And back: the query reads again, the scope is as it was and can be chosen.
    click(&mut driver, RulesPart::LanguageTraces);
    assert!(query_error(&driver).is_none());
    assert_eq!(
        draft(&driver).map(|draft| draft.scope),
        Some(DropScope::Span),
        "switching away and back keeps the scope"
    );
    click(&mut driver, RulesPart::ScopeTrace);
    assert_eq!(
        draft(&driver).map(|draft| draft.scope),
        Some(DropScope::Trace)
    );
    driver.advance(PREVIEW_DEBOUNCE);
    settle(&mut driver);
    assert_eq!(
        preview_line(&driver),
        Some("Would have dropped nothing.".to_owned()),
        "there is no staging traffic in the store"
    );
}

#[test]
fn saving_a_rule_unchanged_changes_nothing() {
    let (mut driver, backend) = rules_window_on(counted_backend(), &Theme::dark());
    let before_rules = saved(&backend);
    let before_counts = counts(&driver);
    click(&mut driver, RulesPart::Card(1));
    click(&mut driver, RulesPart::Save);
    assert!(draft(&driver).is_none(), "saved and closed");
    assert_eq!(saved(&backend), before_rules);
    assert_eq!(
        counts(&driver),
        before_counts,
        "the rule keeps its identity, so its counters"
    );
}

#[test]
fn editing_a_rule_replaces_it_where_it_stands() {
    let (mut driver, backend) = plain_window();
    click(&mut driver, RulesPart::Card(1));
    driver.click(center(&driver, RulesPart::NameInput));
    driver.key("ctrl-a");
    driver.type_text("Chatter");
    settle(&mut driver);
    assert_eq!(
        draft(&driver).map(|draft| draft.name),
        Some("Chatter".to_owned()),
        "typing over a selection replaces the name"
    );
    click(&mut driver, RulesPart::Save);
    let specs = saved(&backend);
    assert_eq!(
        names(&specs),
        ["Health checks", "Chatter", "Staging traffic"]
    );
    assert_eq!(specs[1].query, CHATTER_QUERY, "the rest of it is as it was");
}

#[test]
fn cancel_closes_the_editor_and_saves_nothing() {
    let (mut driver, backend) = plain_window();
    click(&mut driver, RulesPart::Card(0));
    type_into(&mut driver, RulesPart::QueryInput, " ");
    assert_ne!(
        draft(&driver).map(|draft| draft.query),
        Some(HEALTH_QUERY.to_owned()),
        "the draft has changed"
    );
    click(&mut driver, RulesPart::Cancel);
    assert!(draft(&driver).is_none());
    assert!(!has(&driver, RulesPart::Editor));
    assert_eq!(saved(&backend), three_rules());
}

#[test]
fn delete_asks_then_removes() {
    let (mut driver, backend) = plain_window();
    click(&mut driver, RulesPart::Card(1));
    assert!(!has(&driver, RulesPart::ConfirmDelete), "not asked yet");

    // Delete asks first: nothing is removed.
    click(&mut driver, RulesPart::Delete);
    assert_eq!(saved(&backend).len(), 3, "asking removes nothing");
    assert!(has(&driver, RulesPart::ConfirmDelete));
    assert!(has(&driver, RulesPart::KeepRule));
    assert!(read_model(&driver, |rules| rules
        .editor()
        .is_some_and(|editor| editor.confirming_delete())));

    // Keeping the rule goes back to the editor.
    click(&mut driver, RulesPart::KeepRule);
    assert_eq!(saved(&backend).len(), 3);
    assert!(!has(&driver, RulesPart::ConfirmDelete));
    assert!(has(&driver, RulesPart::Save), "the editor is as it was");

    // Confirming removes the rule, and only that one.
    click(&mut driver, RulesPart::Delete);
    click(&mut driver, RulesPart::ConfirmDelete);
    assert_eq!(
        names(&saved(&backend)),
        ["Health checks", "Staging traffic"]
    );
    assert!(draft(&driver).is_none(), "the editor closes");
    assert_eq!(card_rows(&driver).len(), 2);
}

#[test]
fn a_rule_that_is_not_saved_yet_has_nothing_to_delete() {
    let (mut driver, _) = plain_window();
    click(&mut driver, RulesPart::NewRule);
    assert!(!has(&driver, RulesPart::Delete));
}

// --- a rule edited behind the editor's back (plan Review Focus 2) -------------------------------

/// The editor is open on the first rule with a new name typed, and the rules
/// are replaced as a reload of an edited file does: the first rule's query
/// changed, a fourth rule added. Returns the rules the file has now.
fn edit_then_change_the_file(
    driver: &mut Driver<MainWindow>,
    backend: &FixedBackend,
) -> Vec<RuleSpec> {
    click(driver, RulesPart::Card(0));
    driver.click(center(driver, RulesPart::NameInput));
    driver.key("ctrl-a");
    driver.type_text("Health probes");
    settle(driver);
    assert_eq!(
        draft(driver).map(|draft| draft.name),
        Some("Health probes".to_owned())
    );
    assert!(
        !external_change(driver),
        "nothing has changed behind it yet"
    );

    let mut file = saved(backend);
    file[0].query = r#"{ name = "GET /healthz" }"#.to_owned();
    file.push(RuleSpec {
        name: "Added in the file".to_owned(),
        enabled: true,
        language: RuleLanguage::LogQl,
        query: r#"{service_name="web-bff"} |= "ping""#.to_owned(),
        scope: DropScope::Trace,
    });
    backend.set_rules(file.clone()).unwrap();
    driver.advance(RULES_REFRESH);
    settle(driver);
    file
}

#[test]
fn external_change_during_edit_warns() {
    let (mut driver, backend) = plain_window();
    let file = edit_then_change_the_file(&mut driver, &backend);

    // The draft stays, and a banner says the file changed.
    assert!(external_change(&driver));
    assert_eq!(
        draft(&driver).map(|draft| draft.name),
        Some("Health probes".to_owned()),
        "the draft is not touched"
    );
    assert_eq!(
        read_view(&driver, |view, cx| view.name_text(cx)),
        "Health probes",
        "nor is what the input holds"
    );
    assert_eq!(
        EXTERNAL_CHANGE,
        "The rules file changed while you were editing."
    );
    assert_eq!(
        read_view(&driver, |view, cx| view.editor_banner(cx)),
        Some(EXTERNAL_CHANGE)
    );
    assert!(has(&driver, RulesPart::KeepDraft));
    assert!(has(&driver, RulesPart::DiscardDraft));
    // The list shows what the file has now.
    assert_eq!(card_rows(&driver).len(), 4);
    assert_eq!(card_rows(&driver)[0].query, file[0].query);

    // Saving overwrites the file only after confirming: until the draft is
    // kept, Save does nothing.
    click(&mut driver, RulesPart::Save);
    assert_eq!(saved(&backend), file, "the file is as it was changed");
    assert!(draft(&driver).is_some(), "and the draft is still there");

    // `Keep my draft` is the confirmation. The banner goes, the draft stays.
    click(&mut driver, RulesPart::KeepDraft);
    assert!(!external_change(&driver));
    assert!(!has(&driver, RulesPart::KeepDraft));
    assert_eq!(
        draft(&driver).map(|draft| draft.name),
        Some("Health probes".to_owned())
    );
    assert_eq!(saved(&backend), file, "keeping the draft saves nothing");

    // And now Save overwrites the file with the rules as the editor saw them,
    // the draft in place of its rule: what was added in the file is gone.
    click(&mut driver, RulesPart::Save);
    let specs = saved(&backend);
    let mut wanted = three_rules();
    wanted[0].name = "Health probes".to_owned();
    assert_eq!(specs, wanted);
    assert!(draft(&driver).is_none());
}

#[test]
fn discarding_the_draft_closes_the_editor_and_keeps_the_file() {
    let (mut driver, backend) = plain_window();
    let file = edit_then_change_the_file(&mut driver, &backend);
    assert!(external_change(&driver));

    click(&mut driver, RulesPart::DiscardDraft);
    assert!(draft(&driver).is_none(), "the draft is gone");
    assert!(!has(&driver, RulesPart::Editor));
    assert_eq!(saved(&backend), file, "the file is as it was changed");
    assert_eq!(card_rows(&driver).len(), 4);
}

#[test]
fn a_change_made_here_is_not_an_external_one() {
    let (mut driver, backend) = plain_window();
    click(&mut driver, RulesPart::Card(0));
    // The switch of another card, while the editor is open: the rules change,
    // and it is this window's own doing.
    click(&mut driver, RulesPart::Switch(1));
    assert!(!saved(&backend)[1].enabled);
    driver.advance(RULES_REFRESH * 2);
    settle(&mut driver);
    assert!(
        !external_change(&driver),
        "no banner for what this window did"
    );

    // Saving keeps the switch where the user put it.
    click(&mut driver, RulesPart::Save);
    let specs = saved(&backend);
    assert_eq!(specs.len(), 3);
    assert!(!specs[1].enabled, "the switch survived the save");
    assert!(draft(&driver).is_none());
}

#[test]
fn a_rule_turned_off_while_open_stays_off_when_saved() {
    let (mut driver, backend) = plain_window();
    click(&mut driver, RulesPart::Card(0));
    click(&mut driver, RulesPart::Switch(0));
    assert!(!saved(&backend)[0].enabled);
    click(&mut driver, RulesPart::Save);
    assert!(
        !saved(&backend)[0].enabled,
        "the draft of the open rule follows its switch"
    );
}

// --- Discard lines like this… -----------------------------------------------------------------------------

/// The failing checkout's log "Charge failed for order ...".
fn charge_failed_log() -> LogRecord {
    let generated = generate(&Scenario::CheckoutFailure, SEED, BASE);
    generated
        .batches
        .iter()
        .filter_map(|batch| batch.logs.clone())
        .flat_map(|request| logs_from_request(request).items)
        .find(|log| log.body.starts_with("Charge failed"))
        .expect("the checkout fails with a charge")
}

#[test]
fn the_suggestion_opens_in_the_editor_unsaved() {
    let (mut driver, backend) = window_on(backend_with(three_rules()).0, &Theme::dark());
    let suggestion = suggest_from_log(&charge_failed_log());
    open_rules(&mut driver, Some(suggestion.clone()));

    assert_eq!(draft(&driver), Some(suggestion.clone()));
    assert!(read_model(&driver, |rules| rules
        .editor()
        .is_some_and(|editor| editor.is_new())));
    assert_eq!(
        read_view(&driver, |view, cx| view.name_text(cx)),
        suggestion.name,
        "the inputs hold it"
    );
    assert_eq!(
        read_view(&driver, |view, cx| view.query_text(cx)),
        suggestion.query
    );
    assert_eq!(saved(&backend).len(), 3, "it is not saved");
    assert_eq!(
        read_model(&driver, |rules| rules.selected()),
        None,
        "no card is lit for a rule that is not in the list"
    );

    // Its preview is a log rule's: the lines it would have dropped.
    let spec = suggestion.to_spec();
    let numbers = expected_preview(&rules_store(), &spec);
    assert_eq!(numbers.logs, 1, "the line it came from");
    assert_eq!(
        preview_line(&driver),
        Some("Would have dropped 1 line. It was an error.".to_owned())
    );

    // Saving it is how it becomes a rule.
    click(&mut driver, RulesPart::Save);
    let specs = saved(&backend);
    assert_eq!(specs.len(), 4);
    assert_eq!(specs[3], spec);
    assert_eq!(specs[3].language, RuleLanguage::LogQl);
    assert!(specs[3].enabled);
}

#[test]
fn a_new_suggestion_replaces_the_draft_in_the_editor() {
    let (mut driver, _) = plain_window();
    click(&mut driver, RulesPart::Card(0));
    let suggestion = suggest_from_log(&charge_failed_log());
    open_rules(&mut driver, Some(suggestion.clone()));
    assert_eq!(draft(&driver), Some(suggestion));
}
