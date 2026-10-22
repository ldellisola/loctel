//! The Settings window (M4 plan, Task 6): three port fields with the state of
//! their endpoints, the `Start loctel at login` switch where there is a login
//! item, `Apply` and `Close`, in a 520 by 360 window of its own. (Task 8 adds
//! the popover's tests to this file.) The references are
//! `snapshots/settings_*.png`; build them with `cargo xtask screenshots
//! --update` and look at them before committing (`docs/areas/ui.md`).
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

use gpui::{AppContext as _, Bounds, Pixels, Point};
use loctel_engine::{ListenerState, Settings};
use loctel_store::{ManualClock, Store, StoreConfig};
use loctel_ui::Theme;
use loctel_ui::app::{LoginItem, UiBackend as _, open_settings_window};
use loctel_ui::model::settings::{DUPLICATE_PORTS, Field, PORT_RANGE};
use loctel_ui::testing::{Driver, FixedBackend, assert_snapshot, headless_app};
use loctel_ui::views::{SettingsPart, SettingsView};

use common::BASE;

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
