//! The trace table (design 9.2, M3 plan Task 7): [`TraceTableView`] is the lower
//! half of the home screen. A query row (a `TraceQL` chip, an input and `Run`),
//! a toggle row (`Errors only`, `Slower than 1 s` and a count) and a GPUI Kit
//! `DataTable` over the rows of [`HomeData`](crate::app::HomeData).
//!
//! It holds no data of its own. The rows, the query's state and the toggles are
//! the [`HomeModel`]'s; this view writes into the model what the user does (a
//! keystroke, a toggle, `Run`, `Clear filters`) and draws what the model says.
//! The one thing it keeps is the text of the input, because the input owns its
//! text: the model's text is copied into it when something else changed it (a
//! click on the map), and the input's into the model as it is typed.
//!
//! - **Typing validates live and applies live.** Every change of the text goes to
//!   [`HomeModel::set_query`]: an empty bar or a query that parses filters the
//!   table at once; one that does not leaves the last good rows and shows `at
//!   column N: message` under the input. Enter and `Run` read the store again,
//!   which matters while paused.
//! - **Rows.** 36 px each, `Started` in the monospace face, the root span's name
//!   in bold, the red `Failed in` text or the amber `errors inside`, a dot per
//!   service in its colour (the names in a tooltip), the span count, and the
//!   duration with a 4 px bar. A click on a row asks the window to open the
//!   trace ([`TraceTableEvent::Open`]; the view does not know the shell).
//! - **Empty.** `No traces yet` when nothing is filtered, `No traces match` and a
//!   `Clear filters` button when something is.
//!
//! The columns have fixed widths, except that `Trace` and `Failed in` share what
//! the others leave, computed from the window's width ([`column_widths`]), so
//! the table fills its panel and never scrolls sideways down to the smallest
//! window. Decision 0027 has the choices; `docs/areas/ui.md` ("The trace
//! table") the look.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, Rgba,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window,
    canvas, div, prelude::FluentBuilder as _, px, relative,
};
use gpui_kit::component::button::{Button, ButtonVariants as _, Toggle, ToggleVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use loctel_model::TraceId;

use crate::Theme;
use crate::app::{HomeModel, content_width};
use crate::model::query_bar::{self, EXAMPLE_QUERY};
use crate::model::trace_table::{self, ROOT_NOT_ARRIVED, TraceRow};

/// What the table says when nothing is filtered and there are no traces.
pub const EMPTY_NO_DATA: &str = "No traces yet";
/// What it says when a filter leaves nothing.
pub const EMPTY_NO_MATCH: &str = "No traces match";
/// The button under [`EMPTY_NO_MATCH`].
pub const CLEAR_FILTERS: &str = "Clear filters";

/// The names of the columns, left to right.
pub const COLUMNS: [&str; 6] = [
    "Started",
    "Trace",
    "Failed in",
    "Services",
    "Spans",
    "Duration",
];

/// The height of a row, in pixels.
pub const ROW_HEIGHT: f32 = 36.0;
/// The height of the header, in pixels.
pub const HEADER_HEIGHT: f32 = 32.0;

/// The widths of the columns that do not share what is left, in pixels.
const STARTED_WIDTH: f32 = 116.0;
const SERVICES_WIDTH: f32 = 92.0;
const SPANS_WIDTH: f32 = 60.0;
const DURATION_WIDTH: f32 = 140.0;
/// The room the table keeps after its last column (Kit's empty last column).
const LAST_COLUMN: f32 = 12.0;
/// The least `Trace` and `Failed in` have, and the share `Trace` takes of what
/// is left for the two.
const MIN_TRACE: f32 = 140.0;
const MIN_FAILED_IN: f32 = 170.0;
const TRACE_SHARE: f32 = 0.4;
/// How many dots the `Services` column draws before it says `+N`.
const MAX_DOTS: usize = 5;
/// The size of a service dot, and the gap between two.
const DOT_SIZE: f32 = 8.0;
const DOT_GAP: f32 = 4.0;
/// The duration bar's height, and the room left of the duration so it does not
/// touch the right-aligned span count before it.
const BAR_HEIGHT: f32 = 4.0;
const DURATION_INDENT: f32 = 12.0;
/// The padding of the panel's rows, left and right.
const PAD_X: f32 = 16.0;

/// The widths of the six columns ([`COLUMNS`]) for a table `inner` pixels wide:
/// the four fixed ones, and `Trace` (about 40%) and `Failed in` (the rest) over
/// what is left after them and Kit's last empty column. Whole pixels, and never
/// more than `inner` in all, so the table has no horizontal scroll: down to
/// `inner` of 730 (the smallest window's panel is 748), where the two flexible
/// columns are at their least. Narrower than that the table scrolls.
pub fn column_widths(inner: f32) -> [f32; 6] {
    let fixed = STARTED_WIDTH + SERVICES_WIDTH + SPANS_WIDTH + DURATION_WIDTH;
    let flexible = (inner - fixed - LAST_COLUMN).max(0.0);
    let trace = (flexible * TRACE_SHARE).floor().max(MIN_TRACE);
    let failed_in = (flexible - trace).floor().max(MIN_FAILED_IN);
    [
        STARTED_WIDTH,
        trace,
        failed_in,
        SERVICES_WIDTH,
        SPANS_WIDTH,
        DURATION_WIDTH,
    ]
}

/// The part of the table's panel a test can ask the place of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TablePart {
    /// The query input.
    QueryInput,
    /// The `Run` button.
    Run,
    /// The `Errors only` toggle.
    ErrorsOnly,
    /// The `Slower than 1 s` toggle.
    SlowOnly,
    /// The table: its header and its rows.
    Table,
    /// The `Clear filters` button of an empty table; present only then.
    ClearFilters,
}

/// What the table asks of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceTableEvent {
    /// The user clicked the row of this trace: open it.
    Open(TraceId),
}

/// Where the parts of the latest frame are, in window coordinates.
type Frame = RefCell<HashMap<TablePart, Bounds<Pixels>>>;

/// An invisible canvas over its parent that writes the parent's bounds into
/// `frame` while the frame is prepainted. Add it as a child of a `relative` box
/// that is the size of the part.
fn measure(frame: &Rc<Frame>, part: TablePart) -> impl IntoElement {
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

/// What an empty table says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Empty {
    NoData,
    NoMatch,
}

impl Empty {
    fn text(self) -> &'static str {
        match self {
            Empty::NoData => EMPTY_NO_DATA,
            Empty::NoMatch => EMPTY_NO_MATCH,
        }
    }
}

/// Why the table has no rows, `None` when it has some or when nothing has been
/// read yet (a first frame says nothing, rather than `No traces yet`).
fn empty_kind(home: &HomeModel) -> Option<Empty> {
    if !home.data().rows.is_empty() || !home.loaded() {
        return None;
    }
    Some(if home.has_filters() {
        Empty::NoMatch
    } else {
        Empty::NoData
    })
}

fn rgba(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

/// The trace table of the home screen. Make it with
/// `cx.new(|cx| TraceTableView::new(home, window, cx))`; it reads and writes
/// `home` and emits [`TraceTableEvent::Open`] when a row is clicked.
pub struct TraceTableView {
    home: Entity<HomeModel>,
    input: Entity<InputState>,
    table: Entity<TableState<Rows>>,
    frame: Rc<Frame>,
    /// The widths the table has now, to change them only when they change.
    widths: [f32; 6],
    /// Keeps the observation of the model and of the input alive.
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<TraceTableEvent> for TraceTableView {}

impl TraceTableView {
    pub fn new(home: Entity<HomeModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let frame = Rc::new(Frame::default());
        let widths = column_widths(table_width(window));
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(EXAMPLE_QUERY));
        let delegate = Rows {
            home: home.clone(),
            view: cx.weak_entity(),
            frame: Rc::clone(&frame),
            widths,
        };
        let table = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .col_resizable(false)
                .col_movable(false)
                .col_selectable(false)
                .sortable(false)
                // Rows come and go at the top while the table is open: a
                // selected index would point at another trace a moment later.
                .row_selectable(false)
        });
        let subscriptions = [
            cx.observe_in(&home, window, |view, home, window, cx| {
                view.home_changed(&home, window, cx);
            }),
            cx.subscribe_in(
                &input,
                window,
                |view, input, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        view.home.update(cx, |home, cx| home.set_query(&text, cx));
                    }
                    InputEvent::PressEnter { .. } => view.run(cx),
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            ),
        ];
        TraceTableView {
            home,
            input,
            table,
            frame,
            widths,
            _subscriptions: subscriptions,
        }
    }

    /// Where `part` is in the window, as of the latest frame. `None` while it is
    /// not on the screen (the `Clear filters` button of a table with rows).
    pub fn bounds_of(&self, part: TablePart) -> Option<Bounds<Pixels>> {
        self.frame.borrow().get(&part).copied()
    }

    /// The middle of `part`, as of the latest frame.
    pub fn center_of(&self, part: TablePart) -> Option<Point<Pixels>> {
        self.bounds_of(part).map(|bounds| bounds.center())
    }

    /// The middle of the row at `index` (0 is the newest), as of the latest
    /// frame and while the table is not scrolled. `None` when the table has no
    /// such row or is not on the screen.
    pub fn row_center(&self, index: usize) -> Option<Point<Pixels>> {
        let table = self.bounds_of(TablePart::Table)?;
        let y = HEADER_HEIGHT + ROW_HEIGHT * index as f32 + ROW_HEIGHT / 2.0;
        let center = table.origin + gpui::point(table.size.width / 2.0, px(y));
        (center.y < table.origin.y + table.size.height).then_some(center)
    }

    /// The text in the query input.
    pub fn input_text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// The line under the input: `at column N: message` while the text does not
    /// parse, `None` otherwise.
    pub fn error_line(&self, cx: &App) -> Option<String> {
        query_bar::error_line(&self.home.read(cx).data().query)
    }

    /// What the empty table says ([`EMPTY_NO_DATA`] or [`EMPTY_NO_MATCH`]),
    /// `None` while it has rows or has not read yet.
    pub fn empty_state(&self, cx: &App) -> Option<&'static str> {
        empty_kind(self.home.read(cx)).map(Empty::text)
    }

    /// The line on the right of the toggles: `7 traces · newest first`, or
    /// `Showing the newest 1,000`.
    pub fn count_line(&self, cx: &App) -> String {
        let data = self.home.read(cx).data();
        trace_table::count_line(data.rows.len(), data.truncated)
    }

    /// The widths of the columns now, in pixels.
    pub fn column_widths(&self) -> [f32; 6] {
        self.widths
    }

    /// Reads the store again now, whatever the timer and the pause say: what
    /// Enter and `Run` do.
    fn run(&mut self, cx: &mut Context<Self>) {
        self.home.update(cx, |home, cx| home.refresh_now(cx));
    }

    /// The model changed: when something else than the input changed the text
    /// (a click on the map, `Clear filters`), the input follows. The table's
    /// rows are read from the model when it draws.
    fn home_changed(
        &mut self,
        home: &Entity<HomeModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let wanted = home.read(cx).query_text().to_owned();
        if self.input.read(cx).value() != wanted.as_str() {
            self.input
                .update(cx, |input, cx| input.set_value(wanted, window, cx));
        }
        self.table.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    /// Gives the table the widths its window allows, when they changed.
    fn sync_widths(&mut self, window: &Window, cx: &mut Context<Self>) {
        let widths = column_widths(table_width(window));
        if widths != self.widths {
            self.widths = widths;
            self.table.update(cx, |table, cx| {
                table.delegate_mut().widths = widths;
                table.refresh(cx);
                cx.notify();
            });
        }
    }
}

/// The width of the table, in pixels: the content panel less its hairlines.
fn table_width(window: &Window) -> f32 {
    content_width(f32::from(window.viewport_size().width)) - 2.0
}

impl Render for TraceTableView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_widths(window, cx);
        // Every part draws itself again below and measures itself: what was
        // there last frame and is gone now must not be found.
        self.frame.borrow_mut().clear();

        let theme = *cx.global::<Theme>();
        let home = self.home.read(cx);
        let errors_only = home.errors_only();
        let slow_only = home.slow_only();
        let data = home.data();
        let error = query_bar::error_line(&data.query);
        let count = trace_table::count_line(data.rows.len(), data.truncated);
        let frame = &self.frame;

        let chip = div()
            .flex_none()
            .h(px(24.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .rounded(px(7.0))
            .bg(rgba(theme.accent, 0.16))
            .text_color(theme.accent)
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .child("TraceQL");

        let input = div()
            .relative()
            .flex_1()
            .min_w_0()
            .font_family(cx.theme().mono_font_family.clone())
            .child(Input::new(&self.input).cleanable(true))
            .child(measure(frame, TablePart::QueryInput));

        let run = div()
            .relative()
            .flex_none()
            .child(
                Button::new("trace-run")
                    .label("Run")
                    .primary()
                    .on_click(cx.listener(|view, _, _, cx| view.run(cx))),
            )
            .child(measure(frame, TablePart::Run));

        let query_row = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(PAD_X))
            .pt(px(14.0))
            .child(chip)
            .child(input)
            .child(run);

        let error_line = error.map(|line| {
            div()
                .flex_none()
                .px(px(PAD_X))
                .pt(px(6.0))
                .text_size(px(12.0))
                .text_color(theme.error_text)
                .child(line)
        });

        let toggles = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(PAD_X))
            .py(px(12.0))
            .child(toggle(
                &theme,
                frame,
                TablePart::ErrorsOnly,
                "errors-only",
                "Errors only",
                errors_only,
                {
                    let home = self.home.clone();
                    move |on, cx| home.update(cx, |home, cx| home.set_errors_only(on, cx))
                },
            ))
            .child(toggle(
                &theme,
                frame,
                TablePart::SlowOnly,
                "slow-only",
                "Slower than 1 s",
                slow_only,
                {
                    let home = self.home.clone();
                    move |on, cx| home.update(cx, |home, cx| home.set_slow_only(on, cx))
                },
            ))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.text_secondary)
                    .child(count),
            );

        let table = div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                DataTable::new(&self.table)
                    .bordered(false)
                    .scrollbar_visible(true, false)
                    .with_size(px(ROW_HEIGHT))
                    .header_row_height(px(HEADER_HEIGHT)),
            )
            .child(measure(frame, TablePart::Table));

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(query_row)
            .children(error_line)
            .child(toggles)
            .child(div().flex_none().h(px(1.0)).w_full().bg(theme.border))
            .child(table)
    }
}

/// One of the two toggles under the query bar. `set` runs with the new state.
fn toggle(
    theme: &Theme,
    frame: &Rc<Frame>,
    part: TablePart,
    id: &'static str,
    label: &'static str,
    on: bool,
    set: impl Fn(bool, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .relative()
        .flex_none()
        .child(
            Toggle::new(id)
                .label(label)
                .small()
                .outline()
                .checked(on)
                .when(on, |toggle| {
                    toggle
                        .bg(rgba(theme.accent, 0.16))
                        .text_color(theme.accent)
                        .border_color(theme.accent)
                })
                .on_click(move |on, _window, cx| set(*on, cx)),
        )
        .child(measure(frame, part))
}

/// The table's delegate: it reads its rows from the model each time it draws,
/// so there is no second copy to keep fresh.
struct Rows {
    home: Entity<HomeModel>,
    view: WeakEntity<TraceTableView>,
    frame: Rc<Frame>,
    widths: [f32; 6],
}

/// The column at an index of [`COLUMNS`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Started,
    Trace,
    FailedIn,
    Services,
    Spans,
    Duration,
}

impl Col {
    const ALL: [Col; 6] = [
        Col::Started,
        Col::Trace,
        Col::FailedIn,
        Col::Services,
        Col::Spans,
        Col::Duration,
    ];

    fn at(index: usize) -> Col {
        Col::ALL[index.min(Col::ALL.len() - 1)]
    }

    fn name(self) -> &'static str {
        COLUMNS[self as usize]
    }

    /// Right-aligned: a number.
    fn right(self) -> bool {
        self == Col::Spans
    }
}

impl TableDelegate for Rows {
    fn columns_count(&self, _: &App) -> usize {
        Col::ALL.len()
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.home.read(cx).data().rows.len()
    }

    fn column(&self, index: usize, _: &App) -> Column {
        let col = Col::at(index);
        let column = Column::new(col.name().to_lowercase().replace(' ', "-"), col.name())
            .width(px(self.widths[index]))
            .resizable(false)
            .movable(false)
            .selectable(false);
        if col.right() {
            column.text_right()
        } else {
            column
        }
    }

    fn render_th(
        &mut self,
        index: usize,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = Col::at(index);
        div()
            .size_full()
            .flex()
            .items_center()
            .when(col.right(), |header| header.justify_end())
            .when(col == Col::Duration, |header| {
                header.pl(px(DURATION_INDENT))
            })
            .text_size(px(12.0))
            .font_weight(FontWeight::MEDIUM)
            .child(col.name())
    }

    fn render_tr(
        &mut self,
        row: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> gpui::Stateful<Div> {
        div()
            .id(("trace-row", row))
            .cursor_pointer()
            .on_click(cx.listener(move |table, _, _, cx| {
                let delegate = table.delegate();
                let Some(id) = delegate
                    .home
                    .read(cx)
                    .data()
                    .rows
                    .get(row)
                    .map(|row| row.trace_id)
                else {
                    return;
                };
                let view = delegate.view.clone();
                view.update(cx, |_, cx| cx.emit(TraceTableEvent::Open(id)))
                    .ok();
            }))
    }

    fn render_td(
        &mut self,
        row: usize,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let mono = cx.theme().mono_font_family.clone();
        let home = self.home.read(cx);
        let Some(row) = home.data().rows.get(row) else {
            return div().into_any_element();
        };
        cell(&theme, mono, row, Col::at(index), cell_id(index))
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let Some(kind) = empty_kind(self.home.read(cx)) else {
            return div().size_full().into_any_element();
        };
        let home = self.home.clone();
        let frame = Rc::clone(&self.frame);
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(px(15.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(kind.text()),
            )
            .when(kind == Empty::NoMatch, |panel| {
                panel.child(
                    div()
                        .relative()
                        .child(
                            Button::new("clear-filters")
                                .label(CLEAR_FILTERS)
                                .small()
                                .on_click(move |_, _window, cx| {
                                    home.update(cx, |home, cx| home.clear_filters(cx));
                                }),
                        )
                        .child(measure(&frame, TablePart::ClearFilters)),
                )
            })
            .into_any_element()
    }
}

/// An id for the elements of a cell that need one, per column.
fn cell_id(index: usize) -> SharedString {
    SharedString::from(format!("trace-cell-{index}"))
}

/// What goes in the cell of `row` under `col`. Colours come from `theme`, the
/// text from the model's row.
fn cell(
    theme: &Theme,
    mono: SharedString,
    row: &TraceRow,
    col: Col,
    id: SharedString,
) -> AnyElement {
    let line = || div().size_full().flex().items_center();
    match col {
        Col::Started => line()
            .font_family(mono)
            .text_size(px(12.0))
            .text_color(theme.text_secondary)
            .child(row.started.clone())
            .into_any_element(),
        Col::Trace => {
            let arrived = row.root != ROOT_NOT_ARRIVED;
            line()
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.0))
                        .when(arrived, |name| {
                            name.font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                        })
                        .when(!arrived, |name| name.text_color(theme.text_tertiary))
                        .child(row.root.clone()),
                )
                .into_any_element()
        }
        Col::FailedIn => {
            if let Some(text) = &row.failed_in {
                line()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(theme.error_text)
                            .child(text.clone()),
                    )
                    .into_any_element()
            } else if row.errors_inside {
                line()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(theme.warn)
                            .child("errors inside"),
                    )
                    .into_any_element()
            } else {
                line().into_any_element()
            }
        }
        Col::Services => services(theme, row, id),
        Col::Spans => line()
            .justify_end()
            .text_size(px(12.5))
            .text_color(theme.text_secondary)
            .child(row.span_count.to_string())
            .into_any_element(),
        Col::Duration => duration(theme, row),
    }
}

/// The dots of a row's services in their colours, with the names in a tooltip.
fn services(theme: &Theme, row: &TraceRow, id: SharedString) -> AnyElement {
    let names = SharedString::from(row.services_text());
    let more = row.services.len().saturating_sub(MAX_DOTS);
    div()
        .id(id)
        .size_full()
        .flex()
        .items_center()
        .gap(px(DOT_GAP))
        .children(row.services.iter().take(MAX_DOTS).map(|(_, color)| {
            div()
                .flex_none()
                .size(px(DOT_SIZE))
                .rounded_full()
                .bg(theme.service_palette[color % theme.service_palette.len()])
        }))
        .when(more > 0, |dots| {
            dots.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.text_tertiary)
                    .child(format!("+{more}")),
            )
        })
        .when(!names.is_empty(), |dots| {
            dots.tooltip(move |window, cx| Tooltip::new(names.clone()).build(window, cx))
        })
        .into_any_element()
}

/// The duration as text, and under it a 4 px bar as long as the row's share of
/// the longest trace of the table: red for a failed trace, amber with errors
/// inside, the accent otherwise.
fn duration(theme: &Theme, row: &TraceRow) -> AnyElement {
    let color = if row.failed_in.is_some() {
        theme.error
    } else if row.errors_inside {
        theme.warn
    } else {
        theme.accent
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(3.0))
        .pl(px(DURATION_INDENT))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(theme.text)
                .child(row.duration.clone()),
        )
        .child(
            div()
                .w_full()
                .h(px(BAR_HEIGHT))
                .rounded_full()
                .bg(rgba(theme.text_tertiary, 0.25))
                .child(
                    div()
                        .h_full()
                        .w(relative(row.duration_fraction.clamp(0.0, 1.0)))
                        .min_w(px(2.0))
                        .rounded_full()
                        .bg(color),
                ),
        )
        .into_any_element()
}
