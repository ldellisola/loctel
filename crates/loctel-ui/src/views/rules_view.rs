//! The Discard rules screen (design 9.2 and 7, M4 plan Task 5): the list of
//! rules, and next to it the editor of the one that is open.
//!
//! [`RulesView`] is one floating panel, like the home screen's, and the editor
//! beside it ([`RuleEditorView`], 380 px wide, while one is open). The list
//! panel holds, from the top:
//!
//! 1. the header: the title `Discard rules` and under it the subtitle (`Anything a
//!    rule matches is dropped as it arrives and never stored. Rules are kept when
//!    loctel restarts.`); on the right the share of the incoming data the rules
//!    dropped (`31% of incoming data dropped`, `Nothing dropped yet`) and the
//!    `New rule` button
//! 2. the banners, when there are any: the rules file's error (`The rules file
//!    has an error: <msg>. loctel keeps using the rules it had.`, in the error
//!    colour), why a switch could not be saved, and, with no rules at all, `No
//!    discard rules. Nothing is dropped unless you add one.`
//! 3. a card per rule, in the file's order: the switch, the name, a `TRACEQL` or
//!    `LOGQL` chip and, at the right, what it dropped (`48,210 spans dropped in
//!    602 traces`, `96,433 lines dropped`, `Off`, or `Error: <message>` in the
//!    error colour), and the query under them in the monospace face. The card of
//!    the rule that is open in the editor has an accent ring.
//!
//! A click on a card opens it in the editor; a click on its switch turns the rule
//! on or off and nothing else.
//!
//! The view holds no data and no rules of its own: the cards, the banners and the
//! editor session are the [`RulesModel`]'s, and what the user does goes back into
//! it. It emits no events: the editor is part of the screen, and the shell hears
//! nothing from it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Point, Render, Rgba, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas, div,
    prelude::FluentBuilder as _, px,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};

use super::rule_editor_view::RuleEditorView;
use crate::Theme;
use crate::app::{INSET, PANEL_RADIUS, RulesModel};
use crate::model::rules::{NO_RULES, RuleRow, SUBTITLE};

/// The screen's title.
pub const TITLE: &str = "Discard rules";
/// The header's button.
pub const NEW_RULE: &str = "New rule";

/// The padding of the panel's sides.
const PAD_X: f32 = 20.0;
/// The width of the switch column of a card, and the gap that follows it: the
/// query under the name starts where the name does.
const SWITCH_COLUMN: f32 = 36.0;
const CARD_GAP: f32 = 12.0;

pub(super) fn rgba(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

/// The parts of the Rules screen a test can ask the place of: the list's, and
/// the editor's (present only while one is open).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RulesPart {
    /// The `New rule` button.
    NewRule,
    /// The card at this index of the list.
    Card(usize),
    /// The switch of the card at this index.
    Switch(usize),
    /// The editor panel.
    Editor,
    /// The `Name` input.
    NameInput,
    /// The `Traces (TraceQL)` side of the `Applies to` control.
    LanguageTraces,
    /// The `Logs (LogQL)` side of it.
    LanguageLogs,
    /// The `Query` input.
    QueryInput,
    /// The radio `the whole trace it belongs to`; trace rules only.
    ScopeTrace,
    /// The radio `only the matching span and its children`; trace rules only.
    ScopeSpan,
    /// The preview box.
    Preview,
    /// `Save`.
    Save,
    /// `Cancel`.
    Cancel,
    /// `Delete`; a saved rule only.
    Delete,
    /// The yes of the question `Delete` asks.
    ConfirmDelete,
    /// The `Keep rule` of the question.
    KeepRule,
    /// `Keep my draft` of the banner for a file that changed.
    KeepDraft,
    /// `Discard my draft` of the same banner.
    DiscardDraft,
}

/// Where the parts of the latest frame are, in window coordinates.
pub(super) type Frame = RefCell<HashMap<RulesPart, Bounds<Pixels>>>;

/// An invisible canvas over its parent that writes the parent's bounds into
/// `frame` while the frame is prepainted. Add it as a child of a `relative` box
/// that is the size of the part.
pub(super) fn measure(frame: &Rc<Frame>, part: RulesPart) -> impl IntoElement {
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

/// A banner over the list: its words, and whether it is an error.
struct Banner {
    text: String,
    error: bool,
}

/// The Discard rules screen. Make it with
/// `cx.new(|cx| RulesView::new(model, window, cx))`; it reads and writes `model`.
pub struct RulesView {
    model: Entity<RulesModel>,
    editor: Entity<RuleEditorView>,
    frame: Rc<Frame>,
    /// Keeps the observation of the model alive.
    _model: Subscription,
}

impl RulesView {
    pub fn new(model: Entity<RulesModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| RuleEditorView::new(model.clone(), window, cx));
        let subscription = cx.observe(&model, |_, _, cx| cx.notify());
        RulesView {
            model,
            editor,
            frame: Rc::new(Frame::default()),
            _model: subscription,
        }
    }

    /// The editor beside the list.
    pub fn editor_view(&self) -> &Entity<RuleEditorView> {
        &self.editor
    }

    /// Where `part` is in the window, as of the latest frame. `None` while it is
    /// not on the screen (the editor's parts with no editor open, a card that
    /// is not in the list).
    pub fn bounds_of(&self, part: RulesPart, cx: &App) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied().or_else(|| {
            // The editor is not drawn while none is open, and then what it
            // measured in its last frame is no longer on the screen.
            let editing = self.model.read(cx).editor().is_some();
            editing
                .then(|| self.editor.read(cx).bounds_of(part))
                .flatten()
        })
    }

    /// The middle of `part`, as of the latest frame.
    pub fn center_of(&self, part: RulesPart, cx: &App) -> Option<Point<Pixels>> {
        self.bounds_of(part, cx).map(|bounds| bounds.center())
    }

    /// The words of the banners over the list, top to bottom.
    pub fn banners(&self, cx: &App) -> Vec<String> {
        banners_of(self.model.read(cx))
            .into_iter()
            .map(|banner| banner.text)
            .collect()
    }

    /// The line on the right of the header.
    pub fn dropped_line(&self, cx: &App) -> String {
        self.model.read(cx).dropped_line()
    }

    /// The preview box's sentence, `None` while it has nothing to say or no
    /// editor is open.
    pub fn preview_text(&self, cx: &App) -> Option<String> {
        self.model.read(cx).editor()?.preview_text()
    }

    /// The line under the query input, `None` when there is nothing to say.
    pub fn query_error_line(&self, cx: &App) -> Option<String> {
        crate::model::rules::draft_error_line(self.model.read(cx).editor()?.draft())
    }

    /// The banner over the editor for a file that changed, `None` otherwise.
    pub fn editor_banner(&self, cx: &App) -> Option<&'static str> {
        self.model
            .read(cx)
            .editor()?
            .external_change()
            .then_some(super::rule_editor_view::EXTERNAL_CHANGE)
    }

    /// What the `Name` input holds.
    pub fn name_text(&self, cx: &App) -> String {
        self.editor.read(cx).name_text(cx)
    }

    /// What the `Query` input holds.
    pub fn query_text(&self, cx: &App) -> String {
        self.editor.read(cx).query_text(cx)
    }
}

/// The banners the model's state calls for, in the order they show.
fn banners_of(model: &RulesModel) -> Vec<Banner> {
    let mut banners = Vec::new();
    if let Some(text) = model.file_error_line() {
        banners.push(Banner { text, error: true });
    }
    if let Some(text) = model.action_error() {
        banners.push(Banner {
            text: text.to_owned(),
            error: true,
        });
    }
    if model.is_empty() {
        banners.push(Banner {
            text: NO_RULES.to_owned(),
            error: false,
        });
    }
    banners
}

impl Render for RulesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame.borrow_mut().clear();
        let theme = *cx.global::<Theme>();
        let (rows, selected, dropped, banners, editing) = {
            let model = self.model.read(cx);
            (
                model.rows().to_vec(),
                model.selected(),
                model.dropped_line(),
                banners_of(model),
                model.editor().is_some(),
            )
        };
        let frame = &self.frame;
        let mono = cx.theme().mono_font_family.clone();

        // --- the header --------------------------------------------------------------
        let new_rule = div()
            .relative()
            .flex_none()
            .child(
                Button::new("rules-new")
                    .label(NEW_RULE)
                    .primary()
                    .on_click({
                        let model = self.model.clone();
                        move |_, _window, cx| model.update(cx, |model, cx| model.new_rule(cx))
                    }),
            )
            .child(measure(frame, RulesPart::NewRule));

        let header = div()
            .flex_none()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(16.0))
            .px(px(PAD_X))
            .pt(px(18.0))
            .pb(px(14.0))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(20.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(TITLE),
                    )
                    .child(
                        div()
                            .max_w(px(560.0))
                            .text_size(px(13.0))
                            .text_color(theme.text_secondary)
                            .child(SUBTITLE),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme.text_secondary)
                            .child(dropped),
                    )
                    .child(new_rule),
            );

        // --- the banners -------------------------------------------------------------
        let banner_rows = banners.into_iter().map(|banner| {
            let (fill, border, ink) = if banner.error {
                (
                    rgba(theme.error, 0.10),
                    rgba(theme.error, 0.40),
                    theme.error_text,
                )
            } else {
                (rgba(theme.text, 0.04), theme.border, theme.text_secondary)
            };
            div()
                .w_full()
                .px(px(14.0))
                .py(px(10.0))
                .rounded(px(10.0))
                .bg(fill)
                .border_1()
                .border_color(border)
                .text_size(px(12.5))
                .text_color(ink)
                .child(banner.text)
        });

        // --- the cards ---------------------------------------------------------------
        let cards = rows.iter().enumerate().map(|(index, row)| {
            card(
                &theme,
                row,
                index,
                selected == Some(index),
                &mono,
                &self.model,
                frame,
            )
        });

        let content = div()
            .id("rules-scroll")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .px(px(PAD_X))
            .pb(px(PAD_X))
            .children(banner_rows)
            .children(cards)
            .overflow_y_scrollbar();

        let list_panel = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .rounded(px(PANEL_RADIUS))
            .bg(theme.panel)
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(header)
            .child(div().relative().flex_1().min_h_0().child(content));

        div()
            .size_full()
            .flex()
            .flex_row()
            .gap(px(INSET))
            .child(list_panel)
            .when(editing, |screen| screen.child(self.editor.clone()))
    }
}

/// One card: the switch, the name, the language chip and the count on the top
/// row, and the query in the monospace face under them.
fn card(
    theme: &Theme,
    row: &RuleRow,
    index: usize,
    selected: bool,
    mono: &SharedString,
    model: &Entity<RulesModel>,
    frame: &Rc<Frame>,
) -> impl IntoElement {
    let id = row.id;
    let count_ink = if row.error.is_some() {
        theme.error_text
    } else if row.enabled {
        theme.text_secondary
    } else {
        theme.text_tertiary
    };
    let name_ink = if row.enabled {
        theme.text
    } else {
        theme.text_secondary
    };

    // The switch swallows its own click, so the card does not open under it.
    let switch = div()
        .id(("rule-switch", index))
        .relative()
        .flex_none()
        .w(px(SWITCH_COLUMN))
        .on_click(|_, _window, cx| cx.stop_propagation())
        .child(
            Switch::new(("rule-switch-toggle", index))
                .checked(row.enabled)
                .small()
                .on_click({
                    let model = model.clone();
                    move |on, _window, cx| {
                        let on = *on;
                        model.update(cx, |model, cx| model.set_enabled(id, on, cx));
                    }
                }),
        )
        .child(measure(frame, RulesPart::Switch(index)));

    let chip = div()
        .flex_none()
        .h(px(20.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .bg(rgba(theme.accent, 0.14))
        .text_color(theme.accent)
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .child(row.chip);

    let top = div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(CARD_GAP))
        .child(switch)
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(name_ink)
                .child(row.name.clone()),
        )
        .child(chip)
        .child(div().flex_1())
        .child(
            div()
                .flex_none()
                .max_w(px(360.0))
                .truncate()
                .text_size(px(12.5))
                .text_color(count_ink)
                .child(row.counts.clone()),
        );

    let query = div()
        .w_full()
        .pl(px(SWITCH_COLUMN + CARD_GAP))
        .truncate()
        .font_family(mono.clone())
        .text_size(px(12.0))
        .text_color(theme.text_secondary)
        .child(row.query.clone());

    let hover = rgba(theme.text, 0.04);
    div()
        .id(("rule-card", index))
        .relative()
        .w_full()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .px(px(14.0))
        .py(px(12.0))
        .rounded(px(12.0))
        .border_1()
        .cursor_pointer()
        .when(selected, |card| {
            card.border_color(theme.accent).bg(rgba(theme.accent, 0.06))
        })
        .when(!selected, |card| {
            card.border_color(theme.border)
                .hover(move |style| style.bg(hover))
        })
        .on_click({
            let model = model.clone();
            move |_, _window, cx| model.update(cx, |model, cx| model.edit(id, cx))
        })
        .child(top)
        .child(query)
        .child(measure(frame, RulesPart::Card(index)))
}
