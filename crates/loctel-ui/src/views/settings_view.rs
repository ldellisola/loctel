//! The Settings window's form (design 9.2, M4 plan Task 6): 520 by 360, in the
//! window [`open_settings_window`](crate::app::open_settings_window) opens.
//!
//! From the top, over the scrolling part:
//!
//! 1. the title, `Settings`
//! 2. `Endpoints`: a row each for `OTLP gRPC port`, `OTLP HTTP port` and `MCP
//!    port`, with the input and, beside it, how that endpoint is doing now:
//!    `listening` (a green dot and the `ok` colour) or `failed: <reason>` (red,
//!    in the error colour). A text that is not a port, or a port two fields
//!    share, gets its message under its input (`Enter a port from 1 to 65535`,
//!    `Each endpoint needs its own port`)
//! 3. where the platform has a login item (macOS: `UiBackend::login_item` is
//!    `Some`), `Startup`: the `Start loctel at login` switch
//!
//! and pinned under it the buttons: `Apply` (on while the form differs from
//! what was saved and every field is valid) and `Close`, and beside them what
//! the last Apply could not do: the listeners that failed to bind
//! (`otlp-http: address in use`), a settings file that could not be written, a
//! login item the system refused.
//!
//! The rules are not here: the form is [`Form`] (its messages are exact and
//! pinned in `tests/model_misc.rs`), the words of the endpoint lines are
//! `model::settings`'s, and what `Apply` saves and rebinds is the backend's.
//! The view keeps the text of the three inputs (an input owns its text; they
//! are copied into the form as they are typed, and rewritten when an Apply
//! normalises them, so `" 4500 "` reads `4500`) and the switch, and refreshes
//! the endpoint lines, and a switch nobody has touched, every second.
//!
//! The listeners' lines show what the engine reports, which is the ports it
//! listens on; the inputs show the form. They differ while an edit is not
//! applied, and the line then still says what is true of the old port.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Point, Render, Styled as _, Subscription, Window,
    canvas, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _};
use loctel_engine::ListenerStatus;

use crate::app::{LoginItem, UiBackend, kit_theme};
use crate::model::settings::{EndpointState, Field, Form, endpoint_state, failures};
use crate::{FONT_FAMILY, Theme};

/// The window's title.
pub const TITLE: &str = "Settings";
/// The section of the three ports.
pub const ENDPOINTS: &str = "Endpoints";
/// The section of the login switch.
pub const STARTUP: &str = "Startup";
/// The login switch's words.
pub const LOGIN_LABEL: &str = "Start loctel at login";
/// The buttons.
pub const APPLY: &str = "Apply";
pub const CLOSE: &str = "Close";

/// How often the endpoint lines (and a switch nobody touched) are read again.
pub const REFRESH: Duration = Duration::from_secs(1);

/// The padding of the window's sides.
const PAD_X: f32 = 20.0;
/// The width of the captions of the three inputs, and the gap after them: the
/// message under an input starts where the input does.
const LABEL_WIDTH: f32 = 128.0;
const ROW_GAP: f32 = 12.0;
/// The width of a port input.
const INPUT_WIDTH: f32 = 96.0;

/// The parts of the Settings window a test can ask the place of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPart {
    /// The input of this field.
    Input(Field),
    /// The line of this field's endpoint (`listening`, `failed: ...`).
    Status(Field),
    /// The message under this field's input; there only while it has one.
    Message(Field),
    /// The `Start loctel at login` switch; there only with a login item.
    LoginSwitch,
    /// `Apply`.
    Apply,
    /// `Close`.
    Close,
}

/// Where the parts of the latest frame are, in window coordinates.
type Frame = RefCell<HashMap<SettingsPart, Bounds<Pixels>>>;

/// An invisible canvas over its parent that writes the parent's bounds into
/// `frame` while the frame is prepainted. Add it as a child of a `relative` box
/// that is the size of the part.
fn measure(frame: &Rc<Frame>, part: SettingsPart) -> impl IntoElement {
    let frame = Rc::clone(frame);
    canvas(
        move |bounds, _window, _cx| {
            frame.borrow_mut().insert(part, bounds);
        },
        |_, (), _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The Settings window's root view. Make it with
/// `cx.new(|cx| SettingsView::new(backend, window, cx))`;
/// [`open_settings_window`](crate::app::open_settings_window) does.
pub struct SettingsView {
    backend: Rc<dyn UiBackend>,
    login: Option<Rc<dyn LoginItem>>,
    /// The three port inputs, in [`Field::ALL`] order.
    inputs: [Entity<InputState>; 3],
    /// What the fields and the switch hold now.
    form: Form,
    /// What they held when the form was last loaded or applied: `Apply` is
    /// for the difference.
    saved: Form,
    /// The listeners as the engine last said.
    listeners: Vec<ListenerStatus>,
    /// What the last `Apply` could not do, one line each; empty until one
    /// fails, and again as soon as the form is edited.
    notice: Vec<String>,
    frame: Rc<Frame>,
    /// Keeps the inputs' change events heard.
    _inputs: Vec<Subscription>,
    /// Keeps the appearance observer alive.
    _appearance: Subscription,
}

impl SettingsView {
    /// The form for what the backend has saved, with the switch as the system
    /// holds it, and the endpoint lines as the engine reports them. It takes the
    /// theme from the window's appearance (Kit's too) and follows it, as the main
    /// window does, and refreshes the lines every second for as long as the view
    /// lives.
    pub fn new(backend: Rc<dyn UiBackend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        kit_theme::apply(&Theme::for_appearance(window.appearance()), cx);
        let appearance = window.observe_window_appearance(|window, cx| {
            kit_theme::apply(&Theme::for_appearance(window.appearance()), cx);
        });

        let login = backend.login_item();
        let form = Form::from(
            &backend.settings(),
            login.as_ref().map(|item| item.is_enabled()),
        );
        let listeners = backend.status().listeners;

        let inputs = Field::ALL.map(|field| {
            let text = text_of(&form, field).to_owned();
            cx.new(|cx| InputState::new(window, cx).default_value(text))
        });
        let subscriptions = Field::ALL
            .iter()
            .zip(&inputs)
            .map(|(&field, input)| {
                cx.subscribe_in(
                    input,
                    window,
                    move |view, input, event: &InputEvent, _, cx| {
                        if matches!(event, InputEvent::Change) {
                            let text = input.read(cx).value().to_string();
                            view.edited(field, text, cx);
                        }
                    },
                )
            })
            .collect();

        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(REFRESH).await;
                // The view is gone once the window closes; that ends the loop.
                if view.update(cx, |view, cx| view.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        SettingsView {
            backend,
            login,
            inputs,
            saved: form.clone(),
            form,
            listeners,
            notice: Vec::new(),
            frame: Rc::new(Frame::default()),
            _inputs: subscriptions,
            _appearance: appearance,
        }
    }

    // --- what a test reads ---------------------------------------------------------

    /// What the fields and the switch hold now.
    pub fn form(&self) -> &Form {
        &self.form
    }

    /// Whether `Apply` is on: the form differs from what was saved and every
    /// field is valid.
    pub fn can_apply(&self) -> bool {
        self.form != self.saved && self.form.validate().is_ok()
    }

    /// The state of `field`'s endpoint as drawn, `None` when the engine does not
    /// report that listener.
    pub fn endpoint(&self, field: Field) -> Option<EndpointState> {
        endpoint_state(&self.listeners, field)
    }

    /// The message under `field`'s input, `None` when it is fine.
    pub fn message(&self, field: Field) -> Option<String> {
        self.form.validate().err().and_then(|problems| {
            problems
                .into_iter()
                .find(|(problem, _)| *problem == field)
                .map(|(_, message)| message)
        })
    }

    /// What the last `Apply` could not do, one line each.
    pub fn notice(&self) -> Vec<String> {
        self.notice.clone()
    }

    /// The titles of the sections, top to bottom: `Endpoints`, and `Startup`
    /// where there is a login item.
    pub fn sections(&self) -> Vec<&'static str> {
        let mut sections = vec![ENDPOINTS];
        if self.login.is_some() {
            sections.push(STARTUP);
        }
        sections
    }

    /// What `field`'s input holds, as the input has it.
    pub fn input_text(&self, field: Field, cx: &App) -> String {
        self.inputs[index_of(field)].read(cx).value().to_string()
    }

    /// Where `part` is in the window, as of the latest frame. `None` while it
    /// is not on the screen.
    pub fn bounds_of(&self, part: SettingsPart) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied()
    }

    /// The middle of `part`, as of the latest frame.
    pub fn center_of(&self, part: SettingsPart) -> Option<Point<Pixels>> {
        self.bounds_of(part).map(|bounds| bounds.center())
    }

    // --- what the user does --------------------------------------------------------

    /// A field's text changed: the form takes it, and what the last Apply said
    /// is out of date.
    fn edited(&mut self, field: Field, text: String, cx: &mut Context<Self>) {
        match field {
            Field::Grpc => self.form.grpc = text,
            Field::Http => self.form.http = text,
            Field::Mcp => self.form.mcp = text,
        }
        self.notice.clear();
        cx.notify();
    }

    /// The switch was clicked.
    fn set_login(&mut self, on: bool, cx: &mut Context<Self>) {
        self.form.start_at_login = Some(on);
        self.notice.clear();
        cx.notify();
    }

    /// `Apply`: saves the ports and moves the listeners to them
    /// ([`UiBackend::apply_settings`]), then switches the login item if the
    /// switch changed, and shows what came of it. It does nothing while
    /// [`can_apply`](Self::can_apply) is false.
    ///
    /// - A port that cannot be bound is not a failure of the Apply: it is saved,
    ///   and its line says `failed: <reason>` (and the line beside the buttons
    ///   names it).
    /// - A file that cannot be written is: nothing moved, nothing was switched,
    ///   the form is kept and `Apply` stays on to try again.
    /// - A login item the system refuses is reported, and the switch shows what
    ///   the system holds, whatever was asked.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_apply() {
            return;
        }
        let Ok(ports) = self.form.validate() else {
            return;
        };
        // The other settings stay as they are on disk: the form only moves the
        // ports. The login item is the system's, and is not mirrored in the file.
        let mut settings = self.backend.settings();
        settings.otlp_grpc_port = ports.otlp_grpc;
        settings.otlp_http_port = ports.otlp_http;
        settings.mcp_port = ports.mcp;

        self.notice.clear();
        match self.backend.apply_settings(settings) {
            Err(error) => {
                self.notice
                    .push(format!("Could not save the settings: {error:#}"));
                cx.notify();
                return;
            }
            Ok(listeners) => {
                self.notice = failures(&listeners);
                self.listeners = listeners;
            }
        }

        if let Some(login) = &self.login
            && self.form.start_at_login != self.saved.start_at_login
            && let Some(on) = self.form.start_at_login
        {
            if let Err(error) = login.set_enabled(on) {
                self.notice
                    .push(format!("Could not change Start at login: {error:#}"));
            }
            // The system decides what the switch says.
            self.form.start_at_login = Some(login.is_enabled());
        }

        // The ports as they were saved: no blanks, no leading zeros.
        let canonical = [
            ports.otlp_grpc.to_string(),
            ports.otlp_http.to_string(),
            ports.mcp.to_string(),
        ];
        for (field, text) in Field::ALL.into_iter().zip(canonical) {
            if text_of(&self.form, field) != text {
                self.inputs[index_of(field)]
                    .update(cx, |input, cx| input.set_value(text.clone(), window, cx));
                match field {
                    Field::Grpc => self.form.grpc = text,
                    Field::Http => self.form.http = text,
                    Field::Mcp => self.form.mcp = text,
                }
            }
        }
        self.saved = self.form.clone();
        cx.notify();
    }

    /// Reads the endpoints (and the system's login item, while the switch is
    /// as it was loaded: the user may change it in System Settings) again.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let listeners = self.backend.status().listeners;
        if listeners != self.listeners {
            self.listeners = listeners;
            changed = true;
        }
        if let Some(login) = &self.login
            && self.form.start_at_login == self.saved.start_at_login
        {
            let now = Some(login.is_enabled());
            if now != self.saved.start_at_login {
                self.form.start_at_login = now;
                self.saved.start_at_login = now;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }
}

/// The text of `field` in `form`.
fn text_of(form: &Form, field: Field) -> &str {
    match field {
        Field::Grpc => &form.grpc,
        Field::Http => &form.http,
        Field::Mcp => &form.mcp,
    }
}

/// Where `field` is in [`Field::ALL`].
fn index_of(field: Field) -> usize {
    match field {
        Field::Grpc => 0,
        Field::Http => 1,
        Field::Mcp => 2,
    }
}

/// The small caption over a section.
fn caption(theme: &Theme, text: &'static str) -> impl IntoElement {
    div()
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_tertiary)
        .child(text)
}

/// A rounded panel for the rows of a section.
fn panel(theme: &Theme) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(10.0))
        .bg(theme.panel)
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame.borrow_mut().clear();
        let theme = *cx.global::<Theme>();
        let frame = &self.frame;
        let this = cx.entity();
        let validation = self.form.validate().err().unwrap_or_default();

        // --- the endpoints -----------------------------------------------------------
        let last = Field::ALL.len() - 1;
        let rows = Field::ALL.into_iter().enumerate().map(|(index, field)| {
            let state = endpoint_state(&self.listeners, field);
            let message = validation
                .iter()
                .find(|(problem, _)| *problem == field)
                .map(|(_, message)| message.clone());
            let (dot, ink) = match &state {
                Some(state) if state.ok => (theme.ok, theme.ok),
                Some(_) => (theme.error, theme.error_text),
                None => (theme.text_tertiary, theme.text_tertiary),
            };
            let status = div()
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(7.0))
                .child(div().flex_none().size(px(7.0)).rounded_full().bg(dot))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(12.5))
                        .text_color(ink)
                        .child(state.map(|state| state.text).unwrap_or_default()),
                )
                .child(measure(frame, SettingsPart::Status(field)));
            let line = div()
                .flex()
                .items_center()
                .gap(px(ROW_GAP))
                .child(
                    div()
                        .flex_none()
                        .w(px(LABEL_WIDTH))
                        .text_size(px(13.0))
                        .text_color(theme.text)
                        .child(field.label()),
                )
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .w(px(INPUT_WIDTH))
                        .child(Input::new(&self.inputs[index]).small())
                        .child(measure(frame, SettingsPart::Input(field))),
                )
                .child(status);
            let row = div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .px(px(14.0))
                .py(px(5.0))
                .child(line)
                .children(message.map(|message| {
                    div()
                        .relative()
                        .pl(px(LABEL_WIDTH + ROW_GAP))
                        .text_size(px(12.0))
                        .text_color(theme.error_text)
                        .child(message)
                        .child(measure(frame, SettingsPart::Message(field)))
                }));
            row.when(index != last, |row| {
                row.border_b_1().border_color(theme.border)
            })
        });
        let endpoints = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(caption(&theme, ENDPOINTS))
            .child(panel(&theme).children(rows));

        // --- the login switch --------------------------------------------------------
        let startup = self.form.start_at_login.map(|on| {
            let this = this.clone();
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(caption(&theme, STARTUP))
                .child(
                    panel(&theme).child(
                        div().w_full().px(px(14.0)).py(px(8.0)).child(
                            div()
                                .relative()
                                .flex_none()
                                .child(
                                    Switch::new("settings-login")
                                        .label(LOGIN_LABEL)
                                        .checked(on)
                                        .small()
                                        .on_click(move |on, _window, cx| {
                                            let on = *on;
                                            this.update(cx, |view, cx| view.set_login(on, cx));
                                        }),
                                )
                                .child(measure(frame, SettingsPart::LoginSwitch)),
                        ),
                    ),
                )
        });

        let content = div()
            .id("settings-scroll")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .px(px(PAD_X))
            .pt(px(12.0))
            .pb(px(12.0))
            .child(
                div()
                    .text_size(px(18.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(TITLE),
            )
            .child(endpoints)
            .children(startup)
            .overflow_y_scrollbar();

        // --- the buttons, and what the last Apply could not do -----------------------
        let apply = div()
            .relative()
            .flex_none()
            .child(
                Button::new("settings-apply")
                    .label(APPLY)
                    .primary()
                    .disabled(!self.can_apply())
                    .on_click({
                        let this = this.clone();
                        move |_, window, cx| this.update(cx, |view, cx| view.apply(window, cx))
                    }),
            )
            .child(measure(frame, SettingsPart::Apply));
        let close = div()
            .relative()
            .flex_none()
            .child(
                Button::new("settings-close")
                    .label(CLOSE)
                    .outline()
                    .on_click(|_, window, _cx| window.remove_window()),
            )
            .child(measure(frame, SettingsPart::Close));
        let footer = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(PAD_X))
            .py(px(8.0))
            .border_t_1()
            .border_color(theme.border)
            .child(apply)
            .child(close)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pl(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .text_size(px(12.0))
                    .text_color(theme.error_text)
                    .children(self.notice.iter().cloned().map(|line| div().child(line))),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(FONT_FAMILY)
            .text_size(px(13.0))
            .child(div().relative().flex_1().min_h_0().child(content))
            .child(footer)
    }
}
