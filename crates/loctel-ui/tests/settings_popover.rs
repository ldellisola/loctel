//! The Settings window (M4 plan, Task 6): three port fields with the state of
//! their endpoints, the `Start loctel at login` switch where there is a login
//! item, `Apply` and `Close`, in a 520 by 360 window of its own; and the menu
//! bar popover (Task 8), below the Settings tests. The references are
//! `snapshots/settings_*.png` and `snapshots/popover_*.png`; build them with
//! `cargo xtask screenshots --update` and look at them before committing
//! (`docs/areas/ui.md`).
//!
//! The backend is a `FixedBackend`: the settings are in memory and
//! `apply_settings` makes up three listening listeners at the ports it is
//! given. A test that wants a port to fail sets `fail_bind` on it (the listener
//! then reports `Failed`, as one whose port is in use does), and one that wants
//! the file not to be writable sets `fail_save`. The login item is a
//! `FakeLogin` the test can see and move.
//!
//! What the Review Focus of the plan asks of the form is pinned here:
//! `duplicate_ports_rejected` (item 4: the same port twice is refused in the
//! form, before anything is saved) and `apply_rebinds_and_shows_failure` (a
//! port in use is saved and shown as failed, with the reason, and the other
//! listeners are untouched).

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, Bounds, Pixels, Point, point, px, size};
use loctel_engine::{ListenerState, Settings};
use loctel_meshgen::{Scenario, generate};
use loctel_rules::{DropScope, RuleLanguage, RuleSpec};
use loctel_store::{ManualClock, Store, StoreConfig};
use loctel_ui::Theme;
use loctel_ui::app::{Backdrop, LoginItem, Screen, UiBackend as _, open_settings_window};
use loctel_ui::model::popover::{Popover, Tone};
use loctel_ui::model::settings::{DUPLICATE_PORTS, Field, PORT_RANGE};
use loctel_ui::testing::{Driver, FixedBackend, RecordingBackdrop, assert_snapshot, headless_app};
use loctel_ui::views::{
    POPOVER_RADIUS, POPOVER_WIDTH, PopoverActions, PopoverPart, PopoverView, SettingsPart,
    SettingsView, popover_height,
};

use common::{BASE, SEED, feed};

/// The size the plan fixes for the window, in logical pixels.
const SIZE: (u32, u32) = (520, 360);

/// What the OS says when a port is taken (Linux's wording; macOS's differs by a
/// word, and the window shortens both to `address in use`).
const IN_USE: &str = "Address already in use (os error 98)";

// --- the data ----------------------------------------------------------------------------

/// The login item of a test: the system's state, which the test can change
/// behind the window's back, a way to make the system refuse, and a record of
/// every call to `set_enabled`.
struct FakeLogin {
    enabled: Cell<bool>,
    refuse: RefCell<Option<&'static str>>,
    calls: RefCell<Vec<bool>>,
}

impl FakeLogin {
    fn new(enabled: bool) -> Rc<FakeLogin> {
        Rc::new(FakeLogin {
            enabled: Cell::new(enabled),
            refuse: RefCell::new(None),
            calls: RefCell::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<bool> {
        self.calls.borrow().clone()
    }
}

impl LoginItem for FakeLogin {
    fn is_enabled(&self) -> bool {
        self.enabled.get()
    }

    fn set_enabled(&self, on: bool) -> anyhow::Result<()> {
        self.calls.borrow_mut().push(on);
        if let Some(reason) = *self.refuse.borrow() {
            anyhow::bail!(reason);
        }
        self.enabled.set(on);
        Ok(())
    }
}

/// The settings window does not read the store; an empty one stands in.
fn empty_store() -> Store {
    Store::new(StoreConfig::default(), Arc::new(ManualClock::new(BASE)))
}

/// A backend with the default settings (4317, 4318, 4319) and no login item,
/// like Linux.
fn plain_backend() -> FixedBackend {
    FixedBackend::new(empty_store())
}

/// A backend with `login` as its login item, like macOS.
fn backend_with(login: &Rc<FakeLogin>) -> FixedBackend {
    let mut backend = plain_backend();
    backend.login = Some(Rc::clone(login) as Rc<dyn LoginItem>);
    backend
}

/// The window on `backend`, with the first frame drawn.
fn window_on(backend: FixedBackend, theme: &Theme) -> (Driver<SettingsView>, Rc<FixedBackend>) {
    let backend = Rc::new(backend);
    let shown = Rc::clone(&backend);
    let driver = Driver::new(SIZE, theme, move |window, cx| {
        cx.new(|cx| SettingsView::new(shown, window, cx))
    });
    (driver, backend)
}

/// The window on the default backend, in the dark theme.
fn plain_window() -> (Driver<SettingsView>, Rc<FixedBackend>) {
    window_on(plain_backend(), &Theme::dark())
}

// --- reaching into the window ------------------------------------------------------------

/// Draws until what is pending is done.
fn settle(driver: &mut Driver<SettingsView>) {
    for _ in 0..3 {
        driver.draw();
    }
}

fn read<R>(driver: &Driver<SettingsView>, f: impl FnOnce(&SettingsView, &gpui::App) -> R) -> R {
    let view = driver.view().clone();
    driver.read(|cx| f(view.read(cx), cx))
}

fn bounds(driver: &Driver<SettingsView>, part: SettingsPart) -> Option<Bounds<Pixels>> {
    read(driver, |view, _| view.bounds_of(part))
}

fn center(driver: &Driver<SettingsView>, part: SettingsPart) -> Point<Pixels> {
    bounds(driver, part)
        .unwrap_or_else(|| panic!("the settings window has no {part:?} on the screen"))
        .center()
}

fn click(driver: &mut Driver<SettingsView>, part: SettingsPart) {
    driver.click(center(driver, part));
    settle(driver);
}

/// Replaces what the field holds with `text`: a click puts the focus there,
/// select all, then the typing replaces the selection.
fn type_into(driver: &mut Driver<SettingsView>, field: Field, text: &str) {
    driver.click(center(driver, SettingsPart::Input(field)));
    driver.key("ctrl-a");
    if text.is_empty() {
        driver.key("backspace");
    } else {
        driver.type_text(text);
    }
    settle(driver);
}

fn can_apply(driver: &Driver<SettingsView>) -> bool {
    read(driver, |view, _| view.can_apply())
}

fn endpoint(driver: &Driver<SettingsView>, field: Field) -> Option<(bool, String)> {
    read(driver, |view, _| view.endpoint(field)).map(|state| (state.ok, state.text))
}

fn message(driver: &Driver<SettingsView>, field: Field) -> Option<String> {
    read(driver, |view, _| view.message(field))
}

fn notice(driver: &Driver<SettingsView>) -> Vec<String> {
    read(driver, |view, _| view.notice())
}

/// What the three inputs hold, as drawn.
fn input_texts(driver: &Driver<SettingsView>) -> [String; 3] {
    read(driver, |view, cx| {
        Field::ALL.map(|field| view.input_text(field, cx))
    })
}

fn login_switch(driver: &Driver<SettingsView>) -> Option<bool> {
    read(driver, |view, _| view.form().start_at_login)
}

fn listening() -> (bool, String) {
    (true, "listening".to_owned())
}

fn failed_in_use() -> (bool, String) {
    (false, "failed: address in use".to_owned())
}

// --- the references ----------------------------------------------------------------------

#[test]
fn settings_dark() {
    let login = FakeLogin::new(true);
    let (mut driver, _) = window_on(backend_with(&login), &Theme::dark());
    let image = driver.capture();
    assert_eq!(image.dimensions(), (1040, 720));
    assert_snapshot("settings_dark", &image);
}

#[test]
fn settings_light() {
    let login = FakeLogin::new(true);
    let (mut driver, _) = window_on(backend_with(&login), &Theme::light());
    let image = driver.capture();
    assert_eq!(image.dimensions(), (1040, 720));
    assert_snapshot("settings_light", &image);
}

#[test]
fn settings_failed_dark() {
    // HTTP moved to a port in use: saved, shown as failed with its reason, in
    // the row and in the line under the endpoints; the other two untouched.
    let login = FakeLogin::new(false);
    let mut backend = backend_with(&login);
    backend.fail_bind = Some(("otlp-http", IN_USE.to_owned()));
    let (mut driver, _) = window_on(backend, &Theme::dark());
    type_into(&mut driver, Field::Http, "4400");
    click(&mut driver, SettingsPart::Apply);
    assert_snapshot("settings_failed_dark", &driver.capture());
}

#[test]
fn settings_edited_dark() {
    // An edit not applied yet: `Apply` lit, and the line of the endpoint still
    // says what is true of the port it listens on.
    let login = FakeLogin::new(false);
    let (mut driver, _) = window_on(backend_with(&login), &Theme::dark());
    type_into(&mut driver, Field::Http, "4400");
    click(&mut driver, SettingsPart::LoginSwitch);
    assert!(can_apply(&driver));
    assert_snapshot("settings_edited_dark", &driver.capture());
}

#[test]
fn settings_invalid_dark() {
    // The worst case for the height: all three fields have a message.
    let login = FakeLogin::new(true);
    let (mut driver, _) = window_on(backend_with(&login), &Theme::dark());
    type_into(&mut driver, Field::Http, "4317");
    type_into(&mut driver, Field::Mcp, "70000");
    assert_snapshot("settings_invalid_dark", &driver.capture());
}

// --- what the window opens with ------------------------------------------------------------

#[test]
fn the_form_opens_on_the_saved_ports_with_every_endpoint_listening() {
    let backend = plain_backend();
    backend
        .apply_settings(Settings {
            otlp_grpc_port: 14_317,
            otlp_http_port: 14_318,
            mcp_port: 14_319,
            ..Settings::default()
        })
        .expect("the settings are saved");
    let (driver, _) = window_on(backend, &Theme::dark());
    assert_eq!(input_texts(&driver), ["14317", "14318", "14319"]);
    for field in Field::ALL {
        assert_eq!(endpoint(&driver, field), Some(listening()), "{field:?}");
        assert_eq!(message(&driver, field), None, "{field:?}");
    }
    assert!(!can_apply(&driver), "nothing was changed yet");
    assert!(notice(&driver).is_empty());
}

#[test]
fn an_endpoint_that_is_already_failing_says_so_when_the_window_opens() {
    let mut backend = plain_backend();
    backend.status.listeners[1].state = ListenerState::Failed(IN_USE.to_owned());
    let (driver, _) = window_on(backend, &Theme::dark());
    assert_eq!(endpoint(&driver, Field::Grpc), Some(listening()));
    assert_eq!(endpoint(&driver, Field::Http), Some(failed_in_use()));
    assert_eq!(endpoint(&driver, Field::Mcp), Some(listening()));
    assert!(
        notice(&driver).is_empty(),
        "the line under the endpoints reports what an Apply did, not the state"
    );
}

#[test]
fn the_sections_are_endpoints_and_where_there_is_a_login_item_startup() {
    let (driver, _) = plain_window();
    assert_eq!(read(&driver, |view, _| view.sections()), ["Endpoints"]);
    let login = FakeLogin::new(false);
    let (driver, _) = window_on(backend_with(&login), &Theme::dark());
    assert_eq!(
        read(&driver, |view, _| view.sections()),
        ["Endpoints", "Startup"]
    );
}

// --- editing and validating ----------------------------------------------------------------

#[test]
fn apply_is_enabled_only_while_the_form_differs_and_is_valid() {
    let (mut driver, backend) = plain_window();
    assert!(!can_apply(&driver));

    type_into(&mut driver, Field::Http, "4400");
    assert_eq!(read(&driver, |view, _| view.form().http.clone()), "4400");
    assert!(can_apply(&driver), "a different, valid port");

    type_into(&mut driver, Field::Http, "4318");
    assert!(!can_apply(&driver), "back to what was saved");

    type_into(&mut driver, Field::Http, "abc");
    assert!(!can_apply(&driver), "not a port");

    // A click on the dimmed button does nothing.
    click(&mut driver, SettingsPart::Apply);
    assert_eq!(backend.settings(), Settings::default());
}

#[test]
fn a_text_that_is_not_a_port_gets_the_range_message_under_its_field() {
    let (mut driver, _) = plain_window();
    for text in ["", "0", "65536", "70000", "abc", "43 18"] {
        type_into(&mut driver, Field::Mcp, text);
        assert_eq!(
            message(&driver, Field::Mcp),
            Some(PORT_RANGE.to_owned()),
            "{text:?}"
        );
        assert_eq!(message(&driver, Field::Grpc), None, "{text:?}");
        assert!(!can_apply(&driver), "{text:?}");
    }
    type_into(&mut driver, Field::Mcp, "65535");
    assert_eq!(message(&driver, Field::Mcp), None);
    assert!(can_apply(&driver));
}

#[test]
fn duplicate_ports_rejected() {
    let (mut driver, backend) = plain_window();
    type_into(&mut driver, Field::Http, "4317");
    assert_eq!(
        message(&driver, Field::Grpc),
        Some(DUPLICATE_PORTS.to_owned()),
        "the field that has the port first"
    );
    assert_eq!(
        message(&driver, Field::Http),
        Some(DUPLICATE_PORTS.to_owned()),
        "and the one that was just typed"
    );
    assert_eq!(message(&driver, Field::Mcp), None);
    assert!(!can_apply(&driver));

    // Refused in the form: nothing is saved and nothing moves.
    click(&mut driver, SettingsPart::Apply);
    assert_eq!(backend.settings(), Settings::default());
    assert_eq!(endpoint(&driver, Field::Http), Some(listening()));
    assert_eq!(
        backend
            .status()
            .listeners
            .iter()
            .map(|listener| listener.requested.port())
            .collect::<Vec<_>>(),
        [4317, 4318, 4319],
        "the listeners are where they were"
    );

    // A different port for one of them clears both messages.
    type_into(&mut driver, Field::Http, "4400");
    assert_eq!(message(&driver, Field::Grpc), None);
    assert_eq!(message(&driver, Field::Http), None);
    assert!(can_apply(&driver));
}

// --- Apply ---------------------------------------------------------------------------------

#[test]
fn apply_saves_the_ports_and_shows_the_listeners_it_got() {
    let (mut driver, backend) = plain_window();
    type_into(&mut driver, Field::Http, "4400");
    type_into(&mut driver, Field::Mcp, " 4500 ");
    click(&mut driver, SettingsPart::Apply);

    let saved = backend.settings();
    assert_eq!(
        (saved.otlp_grpc_port, saved.otlp_http_port, saved.mcp_port),
        (4317, 4400, 4500)
    );
    assert_eq!(
        backend
            .status()
            .listeners
            .iter()
            .map(|listener| listener.requested.port())
            .collect::<Vec<_>>(),
        [4317, 4400, 4500]
    );
    for field in Field::ALL {
        assert_eq!(endpoint(&driver, field), Some(listening()), "{field:?}");
    }
    assert!(notice(&driver).is_empty());
    assert!(!can_apply(&driver), "what is shown is what is saved");
}

#[test]
fn apply_rebinds_and_shows_failure() {
    // Review Focus 4: a port in use is saved, shown as failed with the reason,
    // and the other listeners keep running.
    let mut backend = plain_backend();
    backend.fail_bind = Some(("otlp-http", IN_USE.to_owned()));
    let (mut driver, backend) = window_on(backend, &Theme::dark());
    type_into(&mut driver, Field::Http, "4400");
    click(&mut driver, SettingsPart::Apply);

    assert_eq!(
        backend.settings().otlp_http_port,
        4400,
        "the setting is saved although the port could not be bound"
    );
    assert_eq!(endpoint(&driver, Field::Grpc), Some(listening()));
    assert_eq!(endpoint(&driver, Field::Http), Some(failed_in_use()));
    assert_eq!(endpoint(&driver, Field::Mcp), Some(listening()));
    assert_eq!(notice(&driver), ["otlp-http: address in use"]);
    assert!(
        !can_apply(&driver),
        "the form is what was saved, failed or not"
    );
    // The window shows what the backend says, a second later as well.
    driver.advance(Duration::from_secs(2));
    assert_eq!(endpoint(&driver, Field::Http), Some(failed_in_use()));
    assert_eq!(notice(&driver), ["otlp-http: address in use"]);
}

#[test]
fn the_line_under_the_endpoints_goes_when_the_form_changes_again() {
    let mut backend = plain_backend();
    backend.fail_bind = Some(("otlp-http", IN_USE.to_owned()));
    let (mut driver, _) = window_on(backend, &Theme::dark());
    type_into(&mut driver, Field::Http, "4400");
    click(&mut driver, SettingsPart::Apply);
    assert_eq!(notice(&driver).len(), 1);
    type_into(&mut driver, Field::Mcp, "4500");
    assert!(
        notice(&driver).is_empty(),
        "it reported an Apply that is over"
    );
    assert_eq!(
        endpoint(&driver, Field::Http),
        Some(failed_in_use()),
        "the row still says what is true"
    );
}

#[test]
fn a_settings_file_that_cannot_be_written_is_reported_and_the_form_is_kept() {
    let mut backend = plain_backend();
    backend.fail_save = Some("Permission denied (os error 13)".to_owned());
    let (mut driver, backend) = window_on(backend, &Theme::dark());
    type_into(&mut driver, Field::Http, "4400");
    click(&mut driver, SettingsPart::Apply);

    assert_eq!(
        notice(&driver),
        ["Could not save the settings: Permission denied (os error 13)"]
    );
    assert_eq!(backend.settings(), Settings::default(), "nothing was saved");
    assert_eq!(
        read(&driver, |view, _| view.form().http.clone()),
        "4400",
        "what was typed stays, to try again"
    );
    assert!(can_apply(&driver), "and Apply stays on for that");
    assert_eq!(endpoint(&driver, Field::Http), Some(listening()));
}

#[test]
fn the_statuses_follow_the_engine_while_the_window_is_open() {
    let mut backend = plain_backend();
    backend.fail_bind = Some(("otlp-http", IN_USE.to_owned()));
    let (mut driver, backend) = window_on(backend, &Theme::dark());
    type_into(&mut driver, Field::Mcp, "4500");
    assert_eq!(endpoint(&driver, Field::Http), Some(listening()));

    // Something moves the listeners behind the window's back (the popover's
    // pause does not, but the engine can fail a listener on its own).
    backend
        .apply_settings(Settings {
            otlp_http_port: 4400,
            ..Settings::default()
        })
        .expect("the settings are saved");
    assert_eq!(
        endpoint(&driver, Field::Http),
        Some(listening()),
        "nothing refreshes before the second is up"
    );
    driver.advance(Duration::from_secs(1));
    assert_eq!(endpoint(&driver, Field::Http), Some(failed_in_use()));
    assert_eq!(
        input_texts(&driver),
        ["4317", "4318", "4500"],
        "a refresh never rewrites what the user is typing"
    );
}

// --- Start at login ------------------------------------------------------------------------

#[test]
fn login_switch_hidden_without_login_item() {
    let (driver, _) = plain_window();
    assert!(bounds(&driver, SettingsPart::LoginSwitch).is_none());
    assert_eq!(login_switch(&driver), None);
    assert_eq!(read(&driver, |view, _| view.sections()), ["Endpoints"]);
}

#[test]
fn the_login_switch_shows_what_the_system_says() {
    for enabled in [false, true] {
        let login = FakeLogin::new(enabled);
        let (driver, _) = window_on(backend_with(&login), &Theme::dark());
        assert!(bounds(&driver, SettingsPart::LoginSwitch).is_some());
        assert_eq!(login_switch(&driver), Some(enabled));
        assert!(!can_apply(&driver));
    }
}

#[test]
fn the_login_switch_takes_effect_on_apply() {
    let login = FakeLogin::new(false);
    let (mut driver, backend) = window_on(backend_with(&login), &Theme::dark());
    let saved_before = backend.settings();

    click(&mut driver, SettingsPart::LoginSwitch);
    assert_eq!(login_switch(&driver), Some(true));
    assert!(can_apply(&driver), "the switch is part of the form");
    assert!(login.calls().is_empty(), "nothing happens before Apply");

    click(&mut driver, SettingsPart::Apply);
    assert_eq!(login.calls(), [true]);
    assert!(login.enabled.get());
    assert_eq!(login_switch(&driver), Some(true));
    assert!(!can_apply(&driver));
    assert!(notice(&driver).is_empty());
    assert_eq!(
        backend.settings(),
        saved_before,
        "the system is the source of truth: nothing is mirrored in the settings file"
    );

    // And back again.
    click(&mut driver, SettingsPart::LoginSwitch);
    click(&mut driver, SettingsPart::Apply);
    assert_eq!(login.calls(), [true, false]);
    assert_eq!(login_switch(&driver), Some(false));
}

#[test]
fn the_login_switch_is_not_called_when_it_did_not_change() {
    let login = FakeLogin::new(true);
    let (mut driver, _) = window_on(backend_with(&login), &Theme::dark());
    type_into(&mut driver, Field::Http, "4400");
    click(&mut driver, SettingsPart::Apply);
    assert!(login.calls().is_empty());
}

#[test]
fn a_login_item_the_system_refuses_is_reported_and_the_switch_follows_the_system() {
    let login = FakeLogin::new(false);
    *login.refuse.borrow_mut() = Some("cannot register the login item: not allowed");
    let (mut driver, backend) = window_on(backend_with(&login), &Theme::dark());
    type_into(&mut driver, Field::Http, "4400");
    click(&mut driver, SettingsPart::LoginSwitch);
    click(&mut driver, SettingsPart::Apply);

    assert_eq!(login.calls(), [true]);
    assert_eq!(
        notice(&driver),
        ["Could not change Start at login: cannot register the login item: not allowed"]
    );
    assert_eq!(
        login_switch(&driver),
        Some(false),
        "the switch shows what the system holds"
    );
    assert_eq!(
        backend.settings().otlp_http_port,
        4400,
        "the ports were applied all the same"
    );
    assert!(!can_apply(&driver));
}

#[test]
fn the_switch_follows_the_system_while_it_is_not_being_edited() {
    let login = FakeLogin::new(false);
    let (mut driver, _) = window_on(backend_with(&login), &Theme::dark());
    // The user switches it in System Settings.
    login.enabled.set(true);
    driver.advance(Duration::from_secs(1));
    assert_eq!(login_switch(&driver), Some(true));
    assert!(!can_apply(&driver), "that is not an edit of the form");

    // A switch the user has changed is theirs until they apply or close.
    click(&mut driver, SettingsPart::LoginSwitch);
    assert_eq!(login_switch(&driver), Some(false));
    login.enabled.set(false);
    driver.advance(Duration::from_secs(1));
    assert_eq!(login_switch(&driver), Some(false));
}

// --- Close and the window ------------------------------------------------------------------

#[test]
fn close_closes_the_window() {
    let (mut driver, _) = plain_window();
    assert!(driver.is_open());
    assert_eq!(driver.read(|cx| cx.windows().len()), 1);
    click(&mut driver, SettingsPart::Close);
    assert!(!driver.is_open());
    assert_eq!(driver.read(|cx| cx.windows().len()), 0);
}

#[test]
fn the_settings_window_opens_once_at_520_by_360_and_is_focused_when_asked_again() {
    let mut cx = headless_app();
    let backend: Rc<dyn loctel_ui::app::UiBackend> = Rc::new(plain_backend());

    let first = cx
        .update(|cx| open_settings_window(Rc::clone(&backend), cx))
        .expect("the window opens");
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    let size = cx
        .update(|cx| first.update(cx, |_, window, _| window.bounds().size))
        .expect("the window is open");
    assert_eq!(
        (f32::from(size.width), f32::from(size.height)),
        (520.0, 360.0)
    );

    let second = cx
        .update(|cx| open_settings_window(Rc::clone(&backend), cx))
        .expect("asking again is not an error");
    assert_eq!(second, first, "it is the window that is already open");
    assert_eq!(cx.update(|cx| cx.windows().len()), 1, "no second window");

    // Once it is closed, asking opens a new one.
    cx.update(|cx| first.update(cx, |_, window, _| window.remove_window()))
        .expect("the window closes");
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    let third = cx
        .update(|cx| open_settings_window(Rc::clone(&backend), cx))
        .expect("the window opens again");
    assert_ne!(third, first);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
}

// ===========================================================================================
// The menu bar popover (M4 plan, Task 8)
// ===========================================================================================
//
// A `PopoverView` in a `Driver`, in a window of the popover's own size (336 wide, as
// tall as its rows: `popover_height`), over a `FixedBackend`. Its actions are a
// `Recorder`: it writes every call down, and does to the backend what the app's
// actions do (pause, clear, the login item), so a test sees the view read the
// result back. The view asks its actions for everything it does outside itself,
// so what a click or a key reaches is what these tests pin:
//
// - `popover_actions_dispatch` and `the_keyboard_works_in_the_popover`: which
//   action each row, button and key calls, and that a row that opens a window
//   closes the popover after it
// - `popover_clear_confirms`: `Clear all data…` asks first, inline
// - `the_login_switch_follows_the_system`: the switch shows what the system
//   says, not what was asked
// - `popover_reports_one_glass_region`: the one `GlassRegion` it gives a backdrop

/// What the popover asked of its actions, in order.
#[derive(Clone, Debug, PartialEq)]
enum Call {
    OpenMain(Screen),
    OpenSettings,
    TogglePause,
    ClearAll,
    CopyMcp,
    SetStartAtLogin(bool),
    Quit,
    Close,
}

/// The `PopoverActions` of a test.
struct Recorder {
    backend: Rc<FixedBackend>,
    calls: RefCell<Vec<Call>>,
}

impl Recorder {
    fn new(backend: Rc<FixedBackend>) -> Rc<Recorder> {
        Rc::new(Recorder {
            backend,
            calls: RefCell::new(Vec::new()),
        })
    }

    fn push(&self, call: Call) {
        self.calls.borrow_mut().push(call);
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    /// The calls so far, forgotten.
    fn take(&self) -> Vec<Call> {
        std::mem::take(&mut *self.calls.borrow_mut())
    }
}

impl PopoverActions for Recorder {
    fn open_main(&self, screen: Screen) {
        self.push(Call::OpenMain(screen));
    }

    fn open_settings(&self) {
        self.push(Call::OpenSettings);
    }

    fn toggle_pause(&self) {
        self.push(Call::TogglePause);
        let paused = self.backend.status().paused;
        self.backend.set_paused(!paused);
    }

    fn clear_all(&self) {
        self.push(Call::ClearAll);
        self.backend.clear_all();
    }

    fn copy_mcp(&self) {
        self.push(Call::CopyMcp);
    }

    fn set_start_at_login(&self, on: bool) {
        self.push(Call::SetStartAtLogin(on));
        // As the app does: the system may refuse, and the popover then shows what it holds.
        if let Some(login) = self.backend.login_item() {
            let _ = login.set_enabled(on);
        }
    }

    fn quit(&self) {
        self.push(Call::Quit);
    }

    fn close(&self) {
        self.push(Call::Close);
    }
}

/// What a test of the popover holds: the window, the backend it shows and the
/// actions it calls.
type Popup = (Driver<PopoverView>, Rc<FixedBackend>, Rc<Recorder>);

/// The popover's window size, in logical pixels, for a backend that has a login
/// item or not (the `Start at login` row is one row more).
fn popover_size(backend: &FixedBackend) -> (u32, u32) {
    (
        POPOVER_WIDTH as u32,
        popover_height(backend.login.is_some()) as u32,
    )
}

/// The popover on `backend` in a window of its size, with the first frames drawn.
fn popover_on(backend: FixedBackend, theme: &Theme) -> Popup {
    popover_behind(backend, theme, None)
}

/// [`popover_on`] with `backdrop` attached, as `glass::attach` does on a Mac.
fn popover_behind(
    backend: FixedBackend,
    theme: &Theme,
    backdrop: Option<RecordingBackdrop>,
) -> Popup {
    let window = popover_size(&backend);
    let backend = Rc::new(backend);
    let actions = Recorder::new(Rc::clone(&backend));
    let (shown, acting) = (Rc::clone(&backend), Rc::clone(&actions));
    let mut driver = Driver::new(window, theme, move |_window, cx| {
        cx.new(|cx| {
            PopoverView::new(shown, acting, cx)
                .with_backdrop(backdrop.map(|backdrop| Box::new(backdrop) as Box<dyn Backdrop>))
        })
    });
    settle_popover(&mut driver);
    (driver, backend, actions)
}

/// Draws until what is pending is done: the first frame asks for the focus and
/// the second has it.
fn settle_popover(driver: &mut Driver<PopoverView>) {
    for _ in 0..3 {
        driver.draw();
    }
}

/// A rule that is on, for the line the popover shows about them.
fn health_rule() -> RuleSpec {
    RuleSpec {
        name: "Health checks".to_owned(),
        enabled: true,
        language: RuleLanguage::TraceQl,
        query: r#"{ name = "GET /health" }"#.to_owned(),
        scope: DropScope::Trace,
    }
}

/// A healthy engine with traffic, one rule on that has dropped 31%, and `login`
/// as the login item (a Mac).
fn busy_backend(login: &Rc<FakeLogin>) -> FixedBackend {
    let mut backend = backend_with(login);
    backend.status.spans_per_sec = 412.0;
    backend.status.logs_per_sec = 1_234.0;
    backend.status.stats.traces = 1_204;
    backend.status.stats.logs = 96_433;
    backend.status.dropped_fraction = 0.31;
    backend
        .set_rules(vec![health_rule()])
        .expect("the rule is kept");
    backend
}

fn popover_of(driver: &Driver<PopoverView>) -> Popover {
    driver.read_root(|view| view.popover().clone())
}

fn rows_of(driver: &Driver<PopoverView>) -> Vec<(&'static str, Option<&'static str>)> {
    driver.read_root(|view| view.action_rows())
}

fn place(driver: &Driver<PopoverView>, part: PopoverPart) -> Option<Bounds<Pixels>> {
    driver.read_root(|view| view.bounds_of(part))
}

fn click_part(driver: &mut Driver<PopoverView>, part: PopoverPart) {
    let at = place(driver, part)
        .unwrap_or_else(|| panic!("the popover has no {part:?} on the screen"))
        .center();
    driver.click(at);
    settle_popover(driver);
}

fn confirming(driver: &Driver<PopoverView>) -> bool {
    driver.read_root(|view| view.confirming_clear())
}

/// A popover whose `otlp-http` and `mcp` ports are taken, as after an Apply of
/// ports in use: the listeners are `Failed` in its status.
fn port_problem_backend(failing: &'static str, login: &Rc<FakeLogin>) -> FixedBackend {
    let mut backend = busy_backend(login);
    backend.fail_bind = Some((failing, IN_USE.to_owned()));
    backend
        .apply_settings(Settings::default())
        .expect("the settings are kept");
    backend
}

// --- the references ----------------------------------------------------------------------

#[test]
fn popover_dark() {
    let login = FakeLogin::new(true);
    let (mut driver, _, _) = popover_on(busy_backend(&login), &Theme::dark());
    let image = driver.capture();
    assert_eq!(image.dimensions(), (672, 2 * popover_height(true) as u32));
    assert_snapshot("popover_dark", &image);
}

#[test]
fn popover_light() {
    let login = FakeLogin::new(true);
    let (mut driver, _, _) = popover_on(busy_backend(&login), &Theme::light());
    let image = driver.capture();
    assert_eq!(image.dimensions(), (672, 2 * popover_height(true) as u32));
    assert_snapshot("popover_light", &image);
}

#[test]
fn popover_paused_dark() {
    // Paused, nothing flowing, no rules, the login item off: the amber pill, the
    // zeros, `Resume receiving` and the grey rules line.
    let login = FakeLogin::new(false);
    let backend = backend_with(&login);
    backend.set_paused(true);
    let (mut driver, _, _) = popover_on(backend, &Theme::dark());
    assert_snapshot("popover_paused_dark", &driver.capture());
}

#[test]
fn popover_port_problem_dark() {
    // `otlp-http` could not be bound: the red pill and the red row, and a rule
    // with a query that does not parse in the rules line.
    let login = FakeLogin::new(true);
    let backend = port_problem_backend("otlp-http", &login);
    backend
        .set_rules(vec![RuleSpec {
            query: r#"{ name = "#.to_owned(),
            ..health_rule()
        }])
        .expect("a draft with a bad query is kept");
    let (mut driver, _, _) = popover_on(backend, &Theme::dark());
    assert_snapshot("popover_port_problem_dark", &driver.capture());
}

#[test]
fn popover_clear_asks_dark() {
    // `Clear all data…` clicked: the row asks `Clear all traces and logs?`, with
    // `Clear` and `Cancel`, and nothing else moves.
    let login = FakeLogin::new(true);
    let (mut driver, _, _) = popover_on(busy_backend(&login), &Theme::dark());
    click_part(&mut driver, PopoverPart::ClearAll);
    assert_snapshot("popover_clear_asks_dark", &driver.capture());
}

/// What the compositor puts behind a window with glass on a Mac: the desktop. The
/// headless renderer cannot draw Apple's glass and its capture is opaque (it
/// clears to black), so a flat colour stands in for it, as in `screens.rs`.
fn desktop(theme: &Theme) -> gpui::Rgba {
    if theme.dark {
        gpui::rgb(0x2a4663)
    } else {
        gpui::rgb(0xc4d6e8)
    }
}

/// A window the popover sits in over the stand-in desktop.
struct Desktop {
    popover: gpui::Entity<PopoverView>,
    color: gpui::Rgba,
}

impl gpui::Render for Desktop {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        gpui::div()
            .size_full()
            .bg(self.color)
            .child(self.popover.clone())
    }
}

/// The popover as it is on a Mac, as far as a headless window can show it: a
/// backdrop attached, over the [`desktop`] stand-in.
fn glass_popover(theme: &Theme) -> (Driver<Desktop>, RecordingBackdrop) {
    let login = FakeLogin::new(true);
    let backend = Rc::new(busy_backend(&login));
    let actions = Recorder::new(Rc::clone(&backend));
    let backdrop = RecordingBackdrop::default();
    let recorder = backdrop.clone();
    let color = desktop(theme);
    let mut driver = Driver::new(
        (POPOVER_WIDTH as u32, popover_height(true) as u32),
        theme,
        move |_window, cx| {
            let popover = cx.new(|cx| {
                PopoverView::new(backend, actions, cx)
                    .with_backdrop(Some(Box::new(recorder) as Box<dyn Backdrop>))
            });
            cx.new(|_| Desktop { popover, color })
        },
    );
    for _ in 0..3 {
        driver.draw();
    }
    (driver, backdrop)
}

#[test]
fn popover_glass_dark() {
    let (mut driver, _) = glass_popover(&Theme::dark());
    assert_snapshot("popover_glass_dark", &driver.capture());
}

#[test]
fn popover_glass_light() {
    let (mut driver, _) = glass_popover(&Theme::light());
    assert_snapshot("popover_glass_light", &driver.capture());
}

/// The pixel at a position of a 336 wide window, in logical pixels (the capture is
/// drawn at scale 2).
fn pixel_at(image: &image::RgbaImage, x: f32, y: f32) -> image::Rgba<u8> {
    *image.get_pixel((x * 2.0) as u32, (y * 2.0) as u32)
}

/// Whether `found` is the opaque `color`, give or take rounding in the renderer.
fn is_colour(found: image::Rgba<u8>, color: gpui::Rgba) -> bool {
    let byte = |channel: f32| (channel * 255.0).round() as u8;
    let expected = [byte(color.r), byte(color.g), byte(color.b)];
    found[3] == 255 && (0..3).all(|channel| found[channel].abs_diff(expected[channel]) <= 2)
}

#[test]
fn the_glass_shows_through_where_the_popover_has_no_fill() {
    // The stand-in desktop shows in the padding of the panel (here, left of the
    // tiles) and outside its rounded corner: the popover paints neither a fill
    // nor the window colour over the glass.
    for theme in [Theme::dark(), Theme::light()] {
        let (mut driver, _) = glass_popover(&theme);
        let image = driver.capture();
        let ground = desktop(&theme);
        assert!(
            is_colour(pixel_at(&image, 6.0, 100.0), ground),
            "the padding: {:?}",
            pixel_at(&image, 6.0, 100.0)
        );
        assert!(
            is_colour(pixel_at(&image, 0.5, 0.5), ground),
            "the corner: {:?}",
            pixel_at(&image, 0.5, 0.5)
        );
    }
}

#[test]
fn without_a_backdrop_the_popover_is_opaque() {
    // No glass: the panel has its own fill, and the window colour is the shell's
    // (`Root`'s), so nothing is see-through.
    for theme in [Theme::dark(), Theme::light()] {
        let login = FakeLogin::new(true);
        let (mut driver, _, _) = popover_on(busy_backend(&login), &theme);
        let image = driver.capture();
        assert!(
            is_colour(pixel_at(&image, 6.0, 100.0), theme.bg),
            "the padding: {:?}",
            pixel_at(&image, 6.0, 100.0)
        );
        assert!(
            is_colour(pixel_at(&image, 0.5, 0.5), theme.bg),
            "the corner: {:?}",
            pixel_at(&image, 0.5, 0.5)
        );
    }
}

#[test]
fn with_a_backdrop_the_window_colour_is_cleared() {
    // The popover alone in its window, with a backdrop: Kit's `Root` would paint
    // the window colour over the glass, so the popover clears it. What is left
    // unpainted is black in a capture.
    let (mut driver, _, _) = popover_behind(
        busy_backend(&FakeLogin::new(true)),
        &Theme::dark(),
        Some(RecordingBackdrop::default()),
    );
    let image = driver.capture();
    let black = image::Rgba([0, 0, 0, 255]);
    assert_eq!(pixel_at(&image, 6.0, 100.0), black, "the padding");
    assert_eq!(pixel_at(&image, 0.5, 0.5), black, "the corner");
}

// --- what it shows -----------------------------------------------------------------------

#[test]
fn the_popover_shows_what_the_engine_says() {
    let login = FakeLogin::new(true);
    let (driver, _, _) = popover_on(busy_backend(&login), &Theme::dark());
    let popover = popover_of(&driver);
    assert_eq!(popover.pill.text, "Receiving");
    assert_eq!(popover.pill.tone, Tone::Ok);
    assert_eq!(
        popover.stats,
        [
            ("412".to_owned(), "spans per second"),
            ("1.2k".to_owned(), "log lines per second"),
            ("1,204".to_owned(), "traces held"),
            ("96,433".to_owned(), "log lines held"),
        ]
    );
    assert_eq!(
        popover.endpoints,
        [
            ("OTLP gRPC", "127.0.0.1:4317".to_owned(), true),
            ("OTLP HTTP", "127.0.0.1:4318".to_owned(), true),
            ("MCP", "http://127.0.0.1:4319/mcp".to_owned(), true),
        ]
    );
    assert_eq!(popover.rules_line, "1 discard rule on \u{b7} 31% dropped");
    assert!(!popover.paused);
    assert_eq!(popover.start_at_login, Some(true));
}

#[test]
fn the_rows_run_in_the_order_of_the_plan() {
    let login = FakeLogin::new(false);
    let (driver, _, _) = popover_on(busy_backend(&login), &Theme::dark());
    assert_eq!(
        rows_of(&driver),
        [
            ("Open loctel", Some("\u{2318}O")),
            ("Pause receiving", None),
            ("Clear all data\u{2026}", None),
            ("Start at login", None),
            ("Settings\u{2026}", Some("\u{2318},")),
            ("Quit loctel", Some("\u{2318}Q")),
        ]
    );
    // Top to bottom on the screen as well.
    let parts = [
        PopoverPart::Pill,
        PopoverPart::Stat(0),
        PopoverPart::Endpoint(0),
        PopoverPart::Endpoint(1),
        PopoverPart::Endpoint(2),
        PopoverPart::RulesLine,
        PopoverPart::OpenMain,
        PopoverPart::Pause,
        PopoverPart::ClearAll,
        PopoverPart::LoginSwitch,
        PopoverPart::Settings,
        PopoverPart::Quit,
    ];
    let tops: Vec<Pixels> = parts
        .iter()
        .map(|part| {
            place(&driver, *part)
                .unwrap_or_else(|| panic!("{part:?} is not on the screen"))
                .origin
                .y
        })
        .collect();
    assert!(
        tops.windows(2).all(|pair| pair[0] < pair[1]),
        "the parts run down the popover in order: {tops:?}"
    );
}

#[test]
fn the_stats_are_a_grid_of_two_by_two() {
    let login = FakeLogin::new(true);
    let (driver, _, _) = popover_on(busy_backend(&login), &Theme::dark());
    let tile = |index| place(&driver, PopoverPart::Stat(index)).expect("a stat tile");
    let (a, b, c, d) = (tile(0), tile(1), tile(2), tile(3));
    assert_eq!(
        a.origin.y, b.origin.y,
        "spans and logs per second share a row"
    );
    assert_eq!(c.origin.y, d.origin.y, "traces and logs held share a row");
    assert!(a.origin.x < b.origin.x && a.origin.x == c.origin.x && b.origin.x == d.origin.x);
    assert!(c.origin.y > a.origin.y);
    assert_eq!(a.size, b.size, "the tiles are the same size");
    assert_eq!(a.size, d.size);
}

#[test]
fn the_endpoint_rows_show_the_addresses_and_say_failed_in_words() {
    let login = FakeLogin::new(true);
    let (driver, _, _) = popover_on(busy_backend(&login), &Theme::dark());
    let lines = driver.read_root(|view| view.endpoint_lines());
    assert_eq!(
        lines,
        [
            ("OTLP gRPC", "127.0.0.1:4317".to_owned()),
            ("OTLP HTTP", "127.0.0.1:4318".to_owned()),
            // The URL without its scheme: `Copy` copies the whole of it.
            ("MCP", "127.0.0.1:4319/mcp".to_owned()),
        ]
    );

    // A listener that could not be bound is red, and says so.
    let (driver, _, _) = popover_on(port_problem_backend("otlp-http", &login), &Theme::dark());
    let lines = driver.read_root(|view| view.endpoint_lines());
    assert_eq!(
        lines[1],
        ("OTLP HTTP", "127.0.0.1:4318 \u{b7} failed".to_owned())
    );
    assert_eq!(lines[0].1, "127.0.0.1:4317");
}

#[test]
fn the_pause_row_says_resume_while_paused_and_the_pill_says_why() {
    let login = FakeLogin::new(false);
    let backend = backend_with(&login);
    backend.set_paused(true);
    let (driver, _, _) = popover_on(backend, &Theme::dark());
    let popover = popover_of(&driver);
    assert_eq!(
        (popover.pill.text, popover.pill.tone),
        ("Paused", Tone::Warn)
    );
    assert_eq!(rows_of(&driver)[1], ("Resume receiving", None));
}

#[test]
fn a_failed_listener_is_a_port_problem_and_a_dead_mcp_address_is_not_offered_to_copy() {
    let login = FakeLogin::new(true);
    let (driver, _, _) = popover_on(port_problem_backend("otlp-http", &login), &Theme::dark());
    let popover = popover_of(&driver);
    assert_eq!(
        (popover.pill.text, popover.pill.tone),
        ("Port problem", Tone::Error)
    );
    assert_eq!(
        popover.endpoints[1],
        ("OTLP HTTP", "127.0.0.1:4318".to_owned(), false)
    );
    assert!(
        place(&driver, PopoverPart::CopyMcp).is_some(),
        "MCP is up, so it can be copied"
    );

    let (driver, _, _) = popover_on(port_problem_backend("mcp", &login), &Theme::dark());
    assert!(!popover_of(&driver).endpoints[2].2);
    assert!(
        place(&driver, PopoverPart::CopyMcp).is_none(),
        "there is no MCP address to copy while it is not listening"
    );
}

#[test]
fn login_row_hidden_without_login_item_in_the_popover() {
    let (driver, _, _) = popover_on(plain_backend(), &Theme::dark());
    assert_eq!(popover_of(&driver).start_at_login, None);
    assert!(place(&driver, PopoverPart::LoginSwitch).is_none());
    assert_eq!(
        rows_of(&driver)
            .into_iter()
            .map(|(label, _)| label)
            .collect::<Vec<_>>(),
        [
            "Open loctel",
            "Pause receiving",
            "Clear all data\u{2026}",
            "Settings\u{2026}",
            "Quit loctel"
        ]
    );
}

#[test]
fn the_popover_is_336_wide_and_exactly_as_tall_as_its_rows() {
    assert_eq!(POPOVER_WIDTH, 336.0);
    assert_eq!(POPOVER_RADIUS, 18.0);
    assert!(
        popover_height(true) > popover_height(false),
        "the login row is one row more"
    );
    for with_login in [true, false] {
        let backend = if with_login {
            busy_backend(&FakeLogin::new(true))
        } else {
            plain_backend()
        };
        let (driver, _, _) = popover_on(backend, &Theme::dark());
        assert_eq!(
            place(&driver, PopoverPart::Panel),
            Some(Bounds::new(
                point(px(0.0), px(0.0)),
                size(px(336.0), px(popover_height(with_login)))
            )),
            "with a login item: {with_login}"
        );
    }
}

#[test]
fn every_part_lies_inside_the_panel_and_copy_inside_its_row() {
    let login = FakeLogin::new(true);
    let (driver, _, _) = popover_on(busy_backend(&login), &Theme::dark());
    let panel = place(&driver, PopoverPart::Panel).expect("the panel");
    let inside = |outer: Bounds<Pixels>, inner: Bounds<Pixels>| {
        inner.left() >= outer.left()
            && inner.right() <= outer.right()
            && inner.top() >= outer.top()
            && inner.bottom() <= outer.bottom()
    };
    let parts = [
        PopoverPart::Pill,
        PopoverPart::Stat(0),
        PopoverPart::Stat(1),
        PopoverPart::Stat(2),
        PopoverPart::Stat(3),
        PopoverPart::Endpoint(0),
        PopoverPart::Endpoint(1),
        PopoverPart::Endpoint(2),
        PopoverPart::CopyMcp,
        PopoverPart::RulesLine,
        PopoverPart::OpenMain,
        PopoverPart::Pause,
        PopoverPart::ClearAll,
        PopoverPart::LoginSwitch,
        PopoverPart::Settings,
        PopoverPart::Quit,
    ];
    for part in parts {
        let bounds = place(&driver, part).unwrap_or_else(|| panic!("{part:?} is not drawn"));
        assert!(
            inside(panel, bounds),
            "{part:?} {bounds:?} inside {panel:?}"
        );
    }
    let mcp = place(&driver, PopoverPart::Endpoint(2)).expect("the MCP row");
    let copy = place(&driver, PopoverPart::CopyMcp).expect("Copy");
    assert!(
        inside(mcp, copy),
        "Copy {copy:?} sits in the MCP row {mcp:?}"
    );
    assert!(
        copy.left() > mcp.center().x,
        "and at its right end: {copy:?} in {mcp:?}"
    );
}

#[test]
fn the_height_does_not_change_while_it_asks_to_clear() {
    let login = FakeLogin::new(true);
    let (mut driver, _, _) = popover_on(busy_backend(&login), &Theme::dark());
    let before = place(&driver, PopoverPart::Panel);
    click_part(&mut driver, PopoverPart::ClearAll);
    assert!(confirming(&driver));
    assert_eq!(place(&driver, PopoverPart::Panel), before);
    // And the question and its two buttons lie inside the panel, on one line.
    let panel = before.expect("the panel");
    let clear = place(&driver, PopoverPart::ConfirmClear).expect("Clear");
    let cancel = place(&driver, PopoverPart::CancelClear).expect("Cancel");
    for button in [clear, cancel] {
        assert!(
            button.left() >= panel.left() && button.right() <= panel.right(),
            "{button:?} inside {panel:?}"
        );
    }
    assert!(clear.right() <= cancel.left(), "Clear, then Cancel");
    assert_eq!(clear.center().y, cancel.center().y);
}

// --- it follows the engine -----------------------------------------------------------------

#[test]
fn the_popover_refreshes_every_second() {
    let login = FakeLogin::new(true);
    let (mut driver, backend, _) = popover_on(busy_backend(&login), &Theme::dark());
    assert_eq!(popover_of(&driver).pill.text, "Receiving");

    backend.set_paused(true);
    driver.advance(Duration::from_millis(900));
    assert_eq!(
        popover_of(&driver).pill.text,
        "Receiving",
        "nothing is read before the second is up"
    );
    driver.advance(Duration::from_millis(200));
    assert_eq!(popover_of(&driver).pill.text, "Paused");
    assert_eq!(rows_of(&driver)[1].0, "Resume receiving");

    // And back again.
    backend.set_paused(false);
    driver.advance(Duration::from_secs(1));
    assert_eq!(popover_of(&driver).pill.text, "Receiving");
}

#[test]
fn pause_and_resume_show_at_once_and_leave_the_popover_open() {
    let login = FakeLogin::new(false);
    let (mut driver, _, actions) = popover_on(busy_backend(&login), &Theme::dark());
    click_part(&mut driver, PopoverPart::Pause);
    assert_eq!(actions.take(), [Call::TogglePause], "and nothing closes it");
    assert_eq!(popover_of(&driver).pill.text, "Paused");
    assert_eq!(rows_of(&driver)[1].0, "Resume receiving");

    click_part(&mut driver, PopoverPart::Pause);
    assert_eq!(actions.take(), [Call::TogglePause]);
    assert_eq!(popover_of(&driver).pill.text, "Receiving");
    assert_eq!(rows_of(&driver)[1].0, "Pause receiving");
}

#[test]
fn the_login_switch_follows_the_system() {
    let login = FakeLogin::new(false);
    let (mut driver, _, actions) = popover_on(busy_backend(&login), &Theme::dark());
    assert_eq!(popover_of(&driver).start_at_login, Some(false));

    // The switch acts at once (there is no Apply here) and shows what the system holds.
    click_part(&mut driver, PopoverPart::LoginSwitch);
    assert_eq!(actions.take(), [Call::SetStartAtLogin(true)]);
    assert_eq!(login.calls(), [true]);
    assert_eq!(popover_of(&driver).start_at_login, Some(true));

    // A system that refuses leaves it as it was, whatever was asked.
    *login.refuse.borrow_mut() = Some("not allowed");
    click_part(&mut driver, PopoverPart::LoginSwitch);
    assert_eq!(actions.take(), [Call::SetStartAtLogin(false)]);
    assert_eq!(popover_of(&driver).start_at_login, Some(true));

    // Switched in System Settings: it follows within a second.
    login.enabled.set(false);
    driver.advance(Duration::from_secs(1));
    assert_eq!(popover_of(&driver).start_at_login, Some(false));
}

// --- what a click and a key do ---------------------------------------------------------------

#[test]
fn popover_clear_confirms() {
    let login = FakeLogin::new(true);
    let backend = busy_backend(&login);
    feed(
        &backend.store,
        &generate(&Scenario::CheckoutFailure, SEED, BASE),
    );
    let (mut driver, backend, actions) = popover_on(backend, &Theme::dark());
    assert_eq!(
        backend.store().stats().logs,
        7,
        "the failing checkout's logs"
    );
    assert!(!confirming(&driver));
    assert!(place(&driver, PopoverPart::ConfirmClear).is_none());

    // The first click only asks.
    click_part(&mut driver, PopoverPart::ClearAll);
    assert!(
        actions.calls().is_empty(),
        "nothing is cleared by the first click"
    );
    assert_eq!(backend.store().stats().logs, 7);
    assert!(confirming(&driver));
    assert_eq!(rows_of(&driver)[2], ("Clear all traces and logs?", None));
    assert!(place(&driver, PopoverPart::ClearAll).is_none());

    // `Cancel` puts the row back and clears nothing.
    click_part(&mut driver, PopoverPart::CancelClear);
    assert!(actions.calls().is_empty());
    assert_eq!(backend.store().stats().logs, 7);
    assert!(!confirming(&driver));
    assert_eq!(rows_of(&driver)[2], ("Clear all data\u{2026}", None));

    // `Clear` clears, once, and the popover stays open.
    click_part(&mut driver, PopoverPart::ClearAll);
    click_part(&mut driver, PopoverPart::ConfirmClear);
    assert_eq!(actions.take(), [Call::ClearAll]);
    assert!(!confirming(&driver));
    assert_eq!(rows_of(&driver)[2], ("Clear all data\u{2026}", None));
    assert_eq!(backend.store().stats().logs, 0);
}

#[test]
fn popover_actions_dispatch() {
    let login = FakeLogin::new(false);
    let (mut driver, _, actions) = popover_on(busy_backend(&login), &Theme::dark());
    let cases: [(PopoverPart, Vec<Call>); 6] = [
        // The rows that open a window close the popover after it.
        (
            PopoverPart::OpenMain,
            vec![Call::OpenMain(Screen::Home), Call::Close],
        ),
        (
            PopoverPart::RulesLine,
            vec![Call::OpenMain(Screen::Rules { draft: None }), Call::Close],
        ),
        (PopoverPart::Settings, vec![Call::OpenSettings, Call::Close]),
        // The others leave it open.
        (PopoverPart::CopyMcp, vec![Call::CopyMcp]),
        (PopoverPart::LoginSwitch, vec![Call::SetStartAtLogin(true)]),
        (PopoverPart::Quit, vec![Call::Quit]),
    ];
    for (part, expected) in cases {
        click_part(&mut driver, part);
        assert_eq!(actions.take(), expected, "{part:?}");
    }
    // Pause and the confirmed clear, which the other tests follow.
    click_part(&mut driver, PopoverPart::Pause);
    assert_eq!(actions.take(), [Call::TogglePause]);
    click_part(&mut driver, PopoverPart::ClearAll);
    click_part(&mut driver, PopoverPart::ConfirmClear);
    assert_eq!(actions.take(), [Call::ClearAll]);
}

#[test]
fn the_keyboard_works_in_the_popover() {
    let login = FakeLogin::new(true);
    let (mut driver, _, actions) = popover_on(busy_backend(&login), &Theme::dark());
    driver.key("escape");
    assert_eq!(actions.take(), [Call::Close], "Esc closes it");
    driver.key("cmd-o");
    assert_eq!(actions.take(), [Call::OpenMain(Screen::Home), Call::Close]);
    driver.key("cmd-,");
    assert_eq!(actions.take(), [Call::OpenSettings, Call::Close]);
    driver.key("cmd-q");
    assert_eq!(actions.take(), [Call::Quit]);
}

#[test]
fn esc_while_it_asks_forgets_the_question_and_closes() {
    let login = FakeLogin::new(true);
    let (mut driver, _, actions) = popover_on(busy_backend(&login), &Theme::dark());
    click_part(&mut driver, PopoverPart::ClearAll);
    assert!(confirming(&driver));
    driver.key("escape");
    assert_eq!(actions.take(), [Call::Close], "nothing was cleared");
    assert!(
        !confirming(&driver),
        "a popover that is shown again asks again"
    );
}

#[test]
fn the_keyboard_works_after_clicks_on_its_rows_and_buttons() {
    let login = FakeLogin::new(true);
    let (mut driver, _, actions) = popover_on(busy_backend(&login), &Theme::dark());
    click_part(&mut driver, PopoverPart::ClearAll);
    click_part(&mut driver, PopoverPart::CancelClear);
    driver.key("escape");
    assert_eq!(actions.take(), [Call::Close]);
    click_part(&mut driver, PopoverPart::CopyMcp);
    click_part(&mut driver, PopoverPart::LoginSwitch);
    actions.take();
    driver.key("cmd-q");
    assert_eq!(actions.take(), [Call::Quit]);
}

#[test]
fn the_popover_takes_the_keyboard_back_when_what_had_it_is_gone() {
    // Whatever had the keyboard and is not drawn any more leaves nothing in the
    // panel focused, and a key binding looks for its context from the focused
    // element up: Esc and the shortcuts would be dead (decision 0040 found the
    // same in the main window). The popover takes the keyboard back.
    let login = FakeLogin::new(true);
    let (mut driver, _, actions) = popover_on(busy_backend(&login), &Theme::dark());
    driver.update_root(|view, window, cx| {
        let stray = cx.focus_handle();
        window.focus(&stray, cx);
        // The view never draws it.
        let _ = view;
    });
    settle_popover(&mut driver);
    driver.key("escape");
    assert_eq!(actions.take(), [Call::Close]);
    driver.key("cmd-q");
    assert_eq!(actions.take(), [Call::Quit]);
}

// --- the glass -----------------------------------------------------------------------------

#[test]
fn popover_reports_one_glass_region() {
    let (mut driver, backdrop) = glass_popover(&Theme::dark());
    // More frames find nothing changed and report nothing.
    driver.draw();
    driver.draw();
    let calls = backdrop.calls();
    assert_eq!(calls.len(), 1, "one report for all the frames: {calls:?}");
    assert_eq!(calls[0].len(), 1, "the popover is one region");
    assert_eq!(
        calls[0][0].bounds,
        Bounds::new(
            point(px(0.0), px(0.0)),
            size(px(POPOVER_WIDTH), px(popover_height(true)))
        ),
        "it covers the whole popover"
    );
    assert_eq!(calls[0][0].corner_radius, px(18.0));
}

#[test]
fn the_region_is_the_popover_even_in_a_bigger_window_and_changes_only_with_it() {
    let login = FakeLogin::new(true);
    let backdrop = RecordingBackdrop::default();
    let (mut driver, _, _) =
        popover_behind(busy_backend(&login), &Theme::dark(), Some(backdrop.clone()));
    click_part(&mut driver, PopoverPart::ClearAll);
    click_part(&mut driver, PopoverPart::CancelClear);
    driver.advance(Duration::from_secs(2));
    assert_eq!(
        backdrop.calls().len(),
        1,
        "asking and refreshing change nothing"
    );
    // A window taller than the popover leaves the rest empty: the popover is
    // what the glass covers, not the window.
    driver.resize((336, 700));
    driver.draw();
    assert_eq!(backdrop.calls().len(), 1, "the popover did not change");
    assert_eq!(
        place(&driver, PopoverPart::Panel).map(|panel| panel.size.height),
        Some(px(popover_height(true)))
    );
}

#[test]
fn a_popover_with_no_backdrop_reports_nothing() {
    // Nothing to assert on the recorder: this is that the view runs without one
    // (every other test above does) and that attaching `None` is the same.
    let (driver, _, _) = popover_behind(plain_backend(), &Theme::dark(), None);
    assert!(place(&driver, PopoverPart::Panel).is_some());
}
