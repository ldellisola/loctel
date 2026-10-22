//! The editor of the Discard rules screen (design 7 and 9.2, M4 plan Task 5): the
//! panel beside the list while a rule is open, 380 px wide, in the glass look
//! (a translucent hairline and a highlight just inside it, over the panel fill).
//!
//! From the top, over the scrolling part:
//!
//! 1. the title (`Edit rule`, or `New rule` for one that is not saved yet) and,
//!    when the rules file changed while the editor was open, the banner `The
//!    rules file changed while you were editing.` with `Keep my draft` and
//!    `Discard my draft`
//! 2. `Name`: an input
//! 3. `Applies to`: a segmented control, `Traces (TraceQL)` or `Logs (LogQL)`
//! 4. `Query`: a multi-line input in the monospace face, with the error line
//!    (`at column N: message`) under it while the text does not parse
//! 5. for traces only, the radio `When a span matches, drop`: `the whole trace it
//!    belongs to` or `only the matching span and its children`
//! 6. the preview box, `Tried on the last 5 minutes`: what the draft would have
//!    dropped, or `Fix the query to see what it would drop.`
//!
//! and pinned under it the buttons: `Save` (enabled with a name, and while the
//! file did not change behind the editor's back), `Cancel` and, for a saved rule,
//! `Delete`, which asks (`Delete this rule?`, `Delete rule`, `Keep rule`) before it
//! removes anything.
//!
//! The view holds no rules. The draft, the preview and the banner are the
//! [`RulesModel`]'s editor session; the one thing it keeps is the text of the
//! two inputs, because an input owns its text: the draft's text is copied into
//! them when an editor opens (a different session), and theirs into the model as
//! it is typed.

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use loctel_rules::{DropScope, RuleLanguage};

use super::rules_view::{Frame, RulesPart, measure, rgba};
use crate::Theme;
use crate::app::{PANEL_RADIUS, RulesModel};
use crate::model::rules::{Draft, PREVIEW_TITLE, draft_error_line};

/// The editor panel's width, in pixels.
pub const EDITOR_WIDTH: f32 = 380.0;

/// The field labels.
pub const NAME_LABEL: &str = "Name";
pub const APPLIES_LABEL: &str = "Applies to";
pub const QUERY_LABEL: &str = "Query";

/// The two sides of the `Applies to` control.
pub const LANGUAGE_TRACES: &str = "Traces (TraceQL)";
pub const LANGUAGE_LOGS: &str = "Logs (LogQL)";

/// The radio of a trace rule, and its two answers.
pub const SCOPE_TITLE: &str = "When a span matches, drop";
pub const SCOPE_TRACE: &str = "the whole trace it belongs to";
pub const SCOPE_SPAN: &str = "only the matching span and its children";

/// The buttons.
pub const SAVE: &str = "Save";
pub const CANCEL: &str = "Cancel";
pub const DELETE: &str = "Delete";
/// What `Delete` asks, and the two answers.
pub const DELETE_ASK: &str = "Delete this rule?";
pub const DELETE_CONFIRM: &str = "Delete rule";
pub const DELETE_KEEP: &str = "Keep rule";

/// The banner for a rules file that changed while the editor was open, and its
/// two buttons.
pub const EXTERNAL_CHANGE: &str = "The rules file changed while you were editing.";
pub const KEEP_DRAFT: &str = "Keep my draft";
pub const DISCARD_DRAFT: &str = "Discard my draft";

/// The title of an editor for a rule that is saved, and for one that is not.
const TITLE_EDIT: &str = "Edit rule";
const TITLE_NEW: &str = "New rule";

/// The height of the query input, in pixels (about five lines of the monospace
/// face).
const QUERY_HEIGHT: f32 = 104.0;
/// The padding of the panel.
const PAD: f32 = 18.0;
/// The least the preview's sentence has room for, so the panel does not jump
/// as the sentence comes and goes (two lines).
const PREVIEW_BODY_MIN: f32 = 36.0;

/// What a frame needs of the editor session, copied out of the model so the
/// element tree can be built while the view is borrowed.
struct Shown {
    draft: Draft,
    is_new: bool,
    external_change: bool,
    confirming_delete: bool,
    error: Option<String>,
    preview: Option<String>,
    query_error: Option<String>,
    can_save: bool,
}

/// The editor panel. Make it with `cx.new(|cx| RuleEditorView::new(model, window,
/// cx))`; [`RulesView`](super::RulesView) does, and shows it while a rule is
/// open.
pub struct RuleEditorView {
    model: Entity<RulesModel>,
    name: Entity<InputState>,
    query: Entity<TextareaState>,
    frame: Rc<Frame>,
    /// The editor session whose text the inputs hold.
    synced: Option<u64>,
    /// Keeps the observation of the model and of the inputs alive.
    _subscriptions: [Subscription; 3],
}

impl RuleEditorView {
    pub fn new(model: Entity<RulesModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Name this rule"));
        let query = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("A TraceQL or LogQL query, as in Grafana")
        });
        let subscriptions = [
            cx.observe_in(&model, window, |view, model, window, cx| {
                view.model_changed(&model, window, cx);
            }),
            cx.subscribe_in(&name, window, |view, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    view.model.update(cx, |model, cx| model.set_name(&text, cx));
                }
            }),
            cx.subscribe_in(&query, window, |view, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    view.model
                        .update(cx, |model, cx| model.set_query(&text, cx));
                }
            }),
        ];
        RuleEditorView {
            model,
            name,
            query,
            frame: Rc::new(Frame::default()),
            synced: None,
            _subscriptions: subscriptions,
        }
    }

    /// Where `part` of the editor is in the window, as of the latest frame.
    /// `None` while it is not on the screen.
    pub fn bounds_of(&self, part: RulesPart) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied()
    }

    /// What the `Name` input holds.
    pub fn name_text(&self, cx: &App) -> String {
        self.name.read(cx).value().to_string()
    }

    /// What the `Query` input holds.
    pub fn query_text(&self, cx: &App) -> String {
        self.query.read(cx).value().to_string()
    }

    /// The model changed: when a different editor opened, its draft goes into
    /// the inputs (a rule that has no name yet puts the cursor in `Name`).
    fn model_changed(
        &mut self,
        model: &Entity<RulesModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opened = model.read(cx).editor().map(|editor| {
            (
                editor.session(),
                editor.draft().name.clone(),
                editor.draft().query.clone(),
            )
        });
        if let Some((session, name, query)) = opened
            && self.synced != Some(session)
        {
            self.synced = Some(session);
            let blank = name.is_empty();
            self.name
                .update(cx, |input, cx| input.set_value(name, window, cx));
            self.query
                .update(cx, |input, cx| input.set_value(query, window, cx));
            if blank {
                self.name.update(cx, |input, cx| input.focus(window, cx));
            }
        }
        cx.notify();
    }

    /// What the frame shows, `None` with no editor open.
    fn shown(&self, cx: &App) -> Option<Shown> {
        let model = self.model.read(cx);
        let editor = model.editor()?;
        Some(Shown {
            draft: editor.draft().clone(),
            is_new: editor.is_new(),
            external_change: editor.external_change(),
            confirming_delete: editor.confirming_delete(),
            error: editor.error().map(str::to_owned),
            preview: editor.preview_text(),
            query_error: draft_error_line(editor.draft()),
            can_save: editor.can_save(),
        })
    }
}

/// The small caption over a field.
fn label(theme: &Theme, text: &'static str) -> impl IntoElement {
    div()
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_tertiary)
        .child(text)
}

impl Render for RuleEditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame.borrow_mut().clear();
        let Some(shown) = self.shown(cx) else {
            return div().into_any_element();
        };
        let theme = *cx.global::<Theme>();
        let frame = &self.frame;
        let model = &self.model;
        let mono = cx.theme().mono_font_family.clone();

        // --- the title, and the banner for a file that changed -----------------------
        let title = div()
            .text_size(px(15.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.text)
            .child(if shown.is_new { TITLE_NEW } else { TITLE_EDIT });

        let banner = shown.external_change.then(|| {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .p(px(12.0))
                .rounded(px(10.0))
                .bg(rgba(theme.warn, 0.12))
                .border_1()
                .border_color(rgba(theme.warn, 0.45))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(theme.text)
                        .child(EXTERNAL_CHANGE),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .relative()
                                .flex_none()
                                .child(
                                    Button::new("rule-keep-draft")
                                        .label(KEEP_DRAFT)
                                        .small()
                                        .outline()
                                        .on_click({
                                            let model = model.clone();
                                            move |_, _window, cx| {
                                                model.update(cx, |model, cx| model.keep_draft(cx));
                                            }
                                        }),
                                )
                                .child(measure(frame, RulesPart::KeepDraft)),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_none()
                                .child(
                                    Button::new("rule-discard-draft")
                                        .label(DISCARD_DRAFT)
                                        .small()
                                        .ghost()
                                        .on_click({
                                            let model = model.clone();
                                            move |_, _window, cx| {
                                                model.update(cx, |model, cx| {
                                                    model.discard_draft(cx);
                                                });
                                            }
                                        }),
                                )
                                .child(measure(frame, RulesPart::DiscardDraft)),
                        ),
                )
        });

        // --- the fields --------------------------------------------------------------
        let name = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(label(&theme, NAME_LABEL))
            .child(
                div()
                    .relative()
                    .w_full()
                    .child(Input::new(&self.name))
                    .child(measure(frame, RulesPart::NameInput)),
            );

        let applies = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(label(&theme, APPLIES_LABEL))
            .child(segmented(&theme, shown.draft.language, model, frame));

        let query = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(label(&theme, QUERY_LABEL))
            .child(
                div()
                    .relative()
                    .w_full()
                    .font_family(mono)
                    .child(Textarea::new(&self.query).h(px(QUERY_HEIGHT)))
                    .child(measure(frame, RulesPart::QueryInput)),
            )
            .children(shown.query_error.clone().map(|line| {
                div()
                    .text_size(px(12.0))
                    .text_color(theme.error_text)
                    .child(line)
            }));

        let scope = (shown.draft.language == RuleLanguage::TraceQl).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(label(&theme, SCOPE_TITLE))
                .child(scope_radio(
                    shown.draft.scope,
                    DropScope::Trace,
                    SCOPE_TRACE,
                    RulesPart::ScopeTrace,
                    model,
                    frame,
                ))
                .child(scope_radio(
                    shown.draft.scope,
                    DropScope::Span,
                    SCOPE_SPAN,
                    RulesPart::ScopeSpan,
                    model,
                    frame,
                ))
        });

        let preview = div()
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(12.0))
            .rounded(px(10.0))
            .bg(rgba(theme.text, 0.04))
            .border_1()
            .border_color(theme.border)
            .child(label(&theme, PREVIEW_TITLE))
            .child(
                div()
                    .min_h(px(PREVIEW_BODY_MIN))
                    .text_size(px(12.5))
                    .text_color(theme.text)
                    .children(shown.preview.clone()),
            )
            .child(measure(frame, RulesPart::Preview));

        let content = div()
            .id("rule-editor-scroll")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .p(px(PAD))
            .child(title)
            .children(banner)
            .child(name)
            .child(applies)
            .child(query)
            .children(scope)
            .child(preview)
            .overflow_y_scrollbar();

        // --- the buttons -------------------------------------------------------------
        let buttons = if shown.confirming_delete {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(DELETE_ASK),
                )
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .child(
                            Button::new("rule-delete-confirm")
                                .label(DELETE_CONFIRM)
                                .danger()
                                .on_click({
                                    let model = model.clone();
                                    move |_, _window, cx| {
                                        model.update(cx, |model, cx| model.confirm_delete(cx));
                                    }
                                }),
                        )
                        .child(measure(frame, RulesPart::ConfirmDelete)),
                )
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .child(
                            Button::new("rule-delete-keep")
                                .label(DELETE_KEEP)
                                .outline()
                                .on_click({
                                    let model = model.clone();
                                    move |_, _window, cx| {
                                        model.update(cx, |model, cx| model.keep_rule(cx));
                                    }
                                }),
                        )
                        .child(measure(frame, RulesPart::KeepRule)),
                )
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .child(
                            Button::new("rule-save")
                                .label(SAVE)
                                .primary()
                                .disabled(!shown.can_save)
                                .on_click({
                                    let model = model.clone();
                                    move |_, _window, cx| {
                                        model.update(cx, |model, cx| model.save(cx));
                                    }
                                }),
                        )
                        .child(measure(frame, RulesPart::Save)),
                )
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .child(
                            Button::new("rule-cancel")
                                .label(CANCEL)
                                .outline()
                                .on_click({
                                    let model = model.clone();
                                    move |_, _window, cx| {
                                        model.update(cx, |model, cx| model.cancel(cx));
                                    }
                                }),
                        )
                        .child(measure(frame, RulesPart::Cancel)),
                )
                .child(div().flex_1())
                .when(!shown.is_new, |row| {
                    row.child(
                        div()
                            .relative()
                            .flex_none()
                            .child(
                                Button::new("rule-delete")
                                    .label(DELETE)
                                    .danger()
                                    .outline()
                                    .on_click({
                                        let model = model.clone();
                                        move |_, _window, cx| {
                                            model.update(cx, |model, cx| model.ask_delete(cx));
                                        }
                                    }),
                            )
                            .child(measure(frame, RulesPart::Delete)),
                    )
                })
        };

        let footer = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(PAD))
            .py(px(14.0))
            .border_t_1()
            .border_color(theme.border)
            .children(shown.error.clone().map(|error| {
                div()
                    .text_size(px(12.0))
                    .text_color(theme.error_text)
                    .child(error)
            }))
            .child(buttons);

        div()
            .relative()
            .w(px(EDITOR_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .rounded(px(PANEL_RADIUS))
            .bg(theme.panel)
            .border_1()
            .border_color(theme.glass_hairline())
            .overflow_hidden()
            .child(div().relative().flex_1().min_h_0().child(content))
            .child(footer)
            .child(
                // The glass look: the soft light edge just inside the hairline.
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .rounded(px(PANEL_RADIUS - 1.0))
                    .border_1()
                    .border_color(theme.glass_highlight()),
            )
            .child(measure(frame, RulesPart::Editor))
            .into_any_element()
    }
}

/// The `Applies to` control: two cells in a rounded track, the one that is
/// chosen tinted with the accent.
fn segmented(
    theme: &Theme,
    language: RuleLanguage,
    model: &Entity<RulesModel>,
    frame: &Rc<Frame>,
) -> impl IntoElement {
    let cell =
        |index: usize, this: RuleLanguage, text: &'static str, part: RulesPart| -> AnyElement {
            let on = language == this;
            let model = model.clone();
            div()
                .relative()
                .flex_1()
                .child(
                    div()
                        .id(("rule-language", index))
                        .h(px(28.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(7.0))
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .when(on, |cell| {
                            cell.bg(rgba(theme.accent, 0.16))
                                .text_color(theme.accent)
                                .font_weight(FontWeight::SEMIBOLD)
                        })
                        .when(!on, |cell| cell.text_color(theme.text_secondary))
                        .on_click(move |_, _window, cx| {
                            model.update(cx, |model, cx| model.set_language(this, cx));
                        })
                        .child(text),
                )
                .child(measure(frame, part))
                .into_any_element()
        };
    div()
        .w_full()
        .flex()
        .gap(px(2.0))
        .p(px(2.0))
        .rounded(px(9.0))
        .bg(rgba(theme.text, 0.06))
        .border_1()
        .border_color(theme.border)
        .child(cell(
            0,
            RuleLanguage::TraceQl,
            LANGUAGE_TRACES,
            RulesPart::LanguageTraces,
        ))
        .child(cell(
            1,
            RuleLanguage::LogQl,
            LANGUAGE_LOGS,
            RulesPart::LanguageLogs,
        ))
}

/// One answer of the scope radio.
fn scope_radio(
    chosen: DropScope,
    this: DropScope,
    text: &'static str,
    part: RulesPart,
    model: &Entity<RulesModel>,
    frame: &Rc<Frame>,
) -> impl IntoElement {
    let model = model.clone();
    let index = usize::from(this == DropScope::Span);
    div()
        .relative()
        .w_full()
        .child(
            Radio::new(("rule-scope", index))
                .label(text)
                .checked(chosen == this)
                .on_click(move |_, _window, cx| {
                    model.update(cx, |model, cx| model.set_scope(this, cx));
                }),
        )
        .child(measure(frame, part))
}
