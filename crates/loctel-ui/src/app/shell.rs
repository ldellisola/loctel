//! The main window's shell: the floating sidebar, the toolbar over the content
//! and the content panel. It owns which [`Screen`] is showing, follows the
//! system appearance and reports where the sidebar and the toolbar are, so a
//! platform backdrop (macOS's Liquid Glass) can sit behind them.
//!
//! With a backdrop attached the sidebar and the toolbar are the glass parts:
//! they paint no fill, only a hairline and an inner highlight, and the window
//! colour is painted round them instead of under them (see
//! [`window_colour`]), so the backdrop shows through. Without one they have the
//! panel fill.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, FillOptions, FillRule,
    InteractiveElement as _, IntoElement, ParentElement, PathBuilder, PathStyle, Pixels, Point,
    Render, Rgba, Styled, Subscription, Window, WindowAppearance, canvas, div, point, px, relative,
    transparent_black,
};
use gpui_kit::base::Root;
use loctel_model::{SpanId, TraceId};

use super::backend::UiBackend;
use super::data::HomeModel;
use super::kit_theme;
use super::logs_data::LogsModel;
use super::rules_data::RulesModel;
use super::sidebar::sidebar_contents;
use super::toolbar::{HomeControls, toolbar_contents};
use crate::model::rules::Draft;
use crate::theme::FONT_FAMILY;
use crate::views::{
    LogsView, LogsViewEvent, MapView, RulesView, TraceTableEvent, TraceTableView, TraceView,
    TraceViewEvent,
};
use crate::{StatusModel, Theme};

/// The gap between the window's edge and the panels, and between the panels,
/// in pixels.
pub const INSET: f32 = 10.0;
/// The sidebar's width, in pixels.
pub const SIDEBAR_WIDTH: f32 = 220.0;
/// The corner radius of the floating panels (the sidebar, the toolbar and the
/// content), in pixels.
pub const PANEL_RADIUS: f32 = 16.0;
/// The toolbar's height, in pixels.
pub const TOOLBAR_HEIGHT: f32 = 48.0;
/// The share of the content area's height the service map takes on the home
/// screen; the trace table has the rest. At the default window size the map has
/// 427 px, enough for the checkout's 416.
const MAP_SHARE: f32 = 0.52;

/// The width of the content area (the toolbar, and the panels under it) in a
/// window `window_width` pixels wide: the window less the inset on both sides,
/// the sidebar and the gap between the sidebar and the content.
pub fn content_width(window_width: f32) -> f32 {
    window_width - 3.0 * INSET - SIDEBAR_WIDTH
}

/// The window's default size, in logical pixels.
pub const DEFAULT_SIZE: (f32, f32) = (1440.0, 900.0);
/// The smallest the window can be made, in logical pixels.
pub const MIN_SIZE: (f32, f32) = (1000.0, 640.0);

/// How often the status box asks the backend for a fresh status.
const STATUS_REFRESH: Duration = Duration::from_secs(1);

/// What the content area shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The service map above the trace table.
    Home,
    /// One trace: the waterfall, the inspector and the selected span's logs.
    Trace(TraceId),
    /// The log list. With `query` the LogQL bar takes that text when the
    /// screen is shown (and the list searches it): `Open in Logs` of the trace
    /// view. `None` shows the screen as it was left.
    Logs { query: Option<String> },
    /// The discard rules. With `draft` the rules editor opens on it, unsaved:
    /// `Discard lines like this…` of the Logs screen. `None` shows the list.
    Rules { draft: Option<Draft> },
}

/// A part of the window that can have a platform backdrop behind it: where it
/// is, in window coordinates, and how round its corners are.
#[derive(Clone, Debug, PartialEq)]
pub struct GlassRegion {
    pub bounds: Bounds<Pixels>,
    pub corner_radius: Pixels,
}

/// What sits behind the window's glass parts. On macOS it is a native glass
/// view per region; elsewhere there is none and the panels are opaque. Passing
/// one to [`MainWindow::new`] is also what makes the shell leave those parts
/// see-through: the platform that attaches it has made the window transparent.
///
/// The shell calls [`set_regions`](Backdrop::set_regions) from inside a GPUI
/// frame, so an implementation must not call back into GPUI or AppKit's layout
/// from it: it stores the regions and applies them later.
pub trait Backdrop: 'static {
    /// The regions as laid out now: the sidebar first, then the toolbar. Called
    /// only when they differ from the last call.
    fn set_regions(&self, regions: &[GlassRegion]);
}

/// The trace screen that is open: which trace, its view and what keeps the
/// view's events heard.
struct OpenTrace {
    id: TraceId,
    view: Entity<TraceView>,
    _events: Subscription,
}

/// The root view of the main window.
///
/// Create it inside the window, as [`open_main_window`](super::open_main_window)
/// does: `MainWindow::new` takes the theme from the window's appearance, and
/// starts the status refresh.
pub struct MainWindow {
    backend: Rc<dyn UiBackend>,
    screen: Screen,
    status: StatusModel,
    /// The home screen's live data, read every 500 ms for as long as the window
    /// lives (the trace screen goes back to it, and the map keeps its place).
    home: Entity<HomeModel>,
    /// The service map of the home screen.
    map: Entity<MapView>,
    /// The trace table of the home screen.
    table: Entity<TraceTableView>,
    /// The controls in the home screen's toolbar: Live, the time range, Pause
    /// and Clear all.
    controls: Entity<HomeControls>,
    /// The trace screen, while [`Screen::Trace`] is showing: made when the
    /// screen is first drawn and dropped when the window goes to another screen
    /// (its refresh stops with it).
    trace: Option<OpenTrace>,
    /// The span the next trace screen opens with selected, asked for by
    /// [`open_trace_at`](Self::open_trace_at) and taken when the screen is made.
    pending_span: Option<SpanId>,
    /// The Logs screen's live data. It lives as long as the window, like the
    /// home screen's, so the query, the toggles and the tail stay while another
    /// screen shows; it reads only while the Logs screen is the one showing.
    logs: Entity<LogsModel>,
    /// The Logs screen.
    logs_view: Entity<LogsView>,
    /// The Discard rules screen's live data and editor session. Like the Logs
    /// screen's it lives as long as the window, so a draft in the editor stays
    /// while another screen shows; it reads only while its screen is showing.
    rules: Entity<RulesModel>,
    /// The Discard rules screen.
    rules_view: Entity<RulesView>,
    regions: Rc<RegionReport>,
    /// Keeps the appearance observer alive.
    _appearance: Subscription,
    /// Keeps the table's "open this trace" subscription alive.
    _open: Subscription,
    /// Keeps the Logs screen's requests (a trace to open, a rule to suggest)
    /// heard.
    _logs_events: Subscription,
}

impl MainWindow {
    /// The shell showing [`Screen::Home`], with the theme of `window`'s
    /// appearance applied (Kit's too), refreshing its status box from `backend`
    /// every second. It makes the home screen's [`HomeModel`] and the service
    /// map and the trace table over it; a click on a row of the table shows that
    /// trace's screen. With a `backdrop`, the sidebar's and the toolbar's bounds
    /// are reported to it when they change.
    pub fn new(
        backend: Rc<dyn UiBackend>,
        backdrop: Option<Box<dyn Backdrop>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> MainWindow {
        kit_theme::apply(&Theme::for_appearance(window.appearance()), cx);

        let weak = cx.weak_entity();
        let appearance = window.observe_window_appearance(move |window, cx| {
            let appearance = window.appearance();
            // The window may be closing: nothing to do then.
            weak.update(cx, |main, cx| main.set_appearance(appearance, cx))
                .ok();
        });

        cx.spawn(async move |main, cx| {
            loop {
                cx.background_executor().timer(STATUS_REFRESH).await;
                // The view is gone once the window closes; that ends the loop.
                if main.update(cx, |main, cx| main.refresh_status(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        if backdrop.is_some() {
            // Kit's `Root` paints the window colour over the whole window, which
            // would hide the backdrop. The shell paints it itself, round the
            // glass parts (`window_colour`), so the root's own is switched off.
            // The root is made after this view, hence the deferral.
            cx.defer_in(window, |_, window, cx| clear_root_background(window, cx));
        }

        let status = StatusModel::from_status(&backend.status());
        let home = cx.new(|cx| HomeModel::new(Rc::clone(&backend), cx));
        let map = cx.new(|cx| MapView::new(home.clone(), cx));
        let table = cx.new(|cx| TraceTableView::new(home.clone(), window, cx));
        let controls = cx.new(|cx| HomeControls::new(home.clone(), cx));
        let open = cx.subscribe(&table, |main, _table, event: &TraceTableEvent, cx| {
            let TraceTableEvent::Open(id) = event;
            main.navigate(Screen::Trace(*id), cx);
        });
        // The Logs screen colours services the way the home screen does.
        let logs =
            cx.new(|cx| LogsModel::with_colors(Rc::clone(&backend), home.read(cx).colors(), cx));
        let logs_view = cx.new(|cx| LogsView::new(logs.clone(), window, cx));
        let logs_events =
            cx.subscribe(
                &logs_view,
                |main, _view, event: &LogsViewEvent, cx| match event {
                    LogsViewEvent::OpenTrace { trace, span } => {
                        main.open_trace_at(*trace, *span, cx)
                    }
                    LogsViewEvent::Discard(draft) => main.navigate(
                        Screen::Rules {
                            draft: Some(draft.clone()),
                        },
                        cx,
                    ),
                },
            );
        let rules = cx.new(|cx| RulesModel::new(Rc::clone(&backend), cx));
        let rules_view = cx.new(|cx| RulesView::new(rules.clone(), window, cx));
        MainWindow {
            backend,
            screen: Screen::Home,
            status,
            home,
            map,
            table,
            controls,
            trace: None,
            pending_span: None,
            logs,
            logs_view,
            rules,
            rules_view,
            regions: Rc::new(RegionReport::new(backdrop)),
            _appearance: appearance,
            _open: open,
            _logs_events: logs_events,
        }
    }

    /// The screen showing now.
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Shows `screen`. Leaving a trace closes its screen: the next visit starts
    /// from the trace's initial selection again.
    ///
    /// `Screen::Logs` with a query puts it in the Logs screen's bar every time,
    /// even when that screen is the one showing: the query asked for is the one
    /// that is shown, whatever the user typed since. The Logs model reads the
    /// store only while its screen shows.
    ///
    /// `Screen::Rules` with a draft opens the rules editor on it, unsaved, in the
    /// same way (a draft that was open is replaced). Without one the screen is
    /// shown as it was left, an open editor and all. The rules model reads only
    /// while its screen shows.
    pub fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if let Screen::Logs { query: Some(query) } = &screen {
            self.logs.update(cx, |logs, cx| logs.set_query(query, cx));
        }
        if let Screen::Rules { draft: Some(draft) } = &screen {
            self.rules
                .update(cx, |rules, cx| rules.open_draft(draft.clone(), cx));
        }
        if !matches!(screen, Screen::Trace(_)) {
            self.pending_span = None;
        }
        if self.screen != screen {
            self.screen = screen;
            if self
                .trace
                .as_ref()
                .is_some_and(|open| self.screen != Screen::Trace(open.id))
            {
                self.trace = None;
            }
            let showing_logs = matches!(self.screen, Screen::Logs { .. });
            self.logs
                .update(cx, |logs, cx| logs.set_active(showing_logs, cx));
            let showing_rules = matches!(self.screen, Screen::Rules { .. });
            self.rules
                .update(cx, |rules, cx| rules.set_active(showing_rules, cx));
            cx.notify();
        }
    }

    /// Shows the trace screen of trace `id` with the span `span` selected when
    /// the trace has it: what `Open trace` of a log does. Without a span (or a
    /// trace screen of `id` that is already open) it is [`navigate`](Self::navigate).
    pub fn open_trace_at(&mut self, id: TraceId, span: Option<SpanId>, cx: &mut Context<Self>) {
        if self.screen != Screen::Trace(id) {
            self.pending_span = span;
        }
        self.navigate(Screen::Trace(id), cx);
    }

    /// The trace screen, while [`Screen::Trace`] is showing and has been drawn
    /// once (the screen is made when it is first drawn).
    pub fn trace_view(&self) -> Option<&Entity<TraceView>> {
        self.trace.as_ref().map(|open| &open.view)
    }

    /// Makes the trace screen of trace `id` if it is not open yet. It shares the
    /// home screen's service colours, so a service keeps its colour; its `Back`
    /// returns to the home screen, and `Traces through this span name` puts its
    /// query in the home screen's bar and goes there.
    fn open_trace(&mut self, id: TraceId, window: &mut Window, cx: &mut Context<Self>) {
        if self.trace.as_ref().is_some_and(|open| open.id == id) {
            return;
        }
        let backend = Rc::clone(&self.backend);
        let colors = self.home.read(cx).colors();
        let view = cx.new(|cx| TraceView::with_colors(backend, id, colors, window, cx));
        if let Some(span) = self.pending_span.take() {
            // Before the first read is applied: the model has nothing yet.
            let model = view.read(cx).model().clone();
            model.update(cx, |model, _| model.prefer_selection(span));
        }
        let events = cx.subscribe(
            &view,
            |main, _view, event: &TraceViewEvent, cx| match event {
                TraceViewEvent::Back => main.navigate(Screen::Home, cx),
                TraceViewEvent::Query(text) => {
                    main.home.update(cx, |home, cx| home.set_query(text, cx));
                    main.navigate(Screen::Home, cx);
                }
                TraceViewEvent::OpenLogs(query) => main.navigate(
                    Screen::Logs {
                        query: Some(query.clone()),
                    },
                    cx,
                ),
            },
        );
        self.trace = Some(OpenTrace {
            id,
            view,
            _events: events,
        });
    }

    /// What the status box shows.
    pub fn status(&self) -> &StatusModel {
        &self.status
    }

    /// The home screen's live data: the map and the table's rows, and the
    /// settings that shape them.
    pub fn home(&self) -> &Entity<HomeModel> {
        &self.home
    }

    /// The service map of the home screen.
    pub fn map(&self) -> &Entity<MapView> {
        &self.map
    }

    /// The trace table of the home screen.
    pub fn table(&self) -> &Entity<TraceTableView> {
        &self.table
    }

    /// The controls of the home screen's toolbar.
    pub fn toolbar(&self) -> &Entity<HomeControls> {
        &self.controls
    }

    /// The Logs screen's live data: the rows of the list, the histogram and the
    /// settings that shape them.
    pub fn logs(&self) -> &Entity<LogsModel> {
        &self.logs
    }

    /// The Logs screen.
    pub fn logs_view(&self) -> &Entity<LogsView> {
        &self.logs_view
    }

    /// The Discard rules screen's live data: the rules with their counts, and
    /// the editor session.
    pub fn rules(&self) -> &Entity<RulesModel> {
        &self.rules
    }

    /// The Discard rules screen.
    pub fn rules_view(&self) -> &Entity<RulesView> {
        &self.rules_view
    }

    /// Switches to the look of `appearance` (dark for `Dark` and
    /// `VibrantDark`, light for the others): ours and Kit's. This is what the
    /// window's appearance observer calls when the system switches.
    pub fn set_appearance(&mut self, appearance: WindowAppearance, cx: &mut Context<Self>) {
        kit_theme::apply(&Theme::for_appearance(appearance), cx);
        cx.notify();
    }

    /// Reads the backend's status again. Redraws only when it changed.
    fn refresh_status(&mut self, cx: &mut Context<Self>) {
        let status = StatusModel::from_status(&self.backend.status());
        if self.status != status {
            self.status = status;
            cx.notify();
        }
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        if let Screen::Trace(id) = self.screen {
            self.open_trace(id, window, cx);
        }
        let logs_controls = self.logs_view.read(cx).controls().clone();
        if self.screen != Screen::Home {
            // Not drawn now: a test (or anyone) asking where its parts are must
            // not be told where they were on the home screen.
            self.controls.read(cx).forget_places();
        }
        if !matches!(self.screen, Screen::Logs { .. }) {
            logs_controls.read(cx).forget_places();
        }
        let regions = &self.regions;
        let glass = regions.has_backdrop();

        let sidebar = glass_frame(&theme, Region::Sidebar, regions)
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .child(sidebar_contents(&theme, &self.screen, &self.status, cx));

        let toolbar = glass_frame(&theme, Region::Toolbar, regions)
            .w_full()
            .h(px(TOOLBAR_HEIGHT))
            .flex_none()
            .child(toolbar_contents(
                &theme,
                &self.screen,
                &self.controls,
                &logs_controls,
            ));

        let content = match &self.screen {
            Screen::Home => home_screen(&theme, &self.map, &self.table),
            Screen::Trace(_) => {
                let panel = panel(&theme).flex_1().w_full().min_h_0();
                match &self.trace {
                    Some(open) => panel.child(open.view.clone()).into_any_element(),
                    None => panel.into_any_element(),
                }
            }
            Screen::Logs { .. } => div()
                .flex_1()
                .w_full()
                .min_h_0()
                .child(self.logs_view.clone())
                .into_any_element(),
            Screen::Rules { .. } => div()
                .flex_1()
                .w_full()
                .min_h_0()
                .child(self.rules_view.clone())
                .into_any_element(),
        };

        let root = div()
            .id("main-window")
            .size_full()
            .relative()
            .flex()
            .flex_row()
            .gap(px(INSET))
            .p(px(INSET))
            .text_color(theme.text)
            .font_family(FONT_FAMILY)
            .text_size(px(13.0));
        // The window colour: under everything, except where the backdrop's glass
        // has to show.
        let root = if glass {
            root.child(window_colour(theme.bg, Rc::clone(regions)))
        } else {
            root.bg(theme.bg)
        };
        root.child(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(px(INSET))
                    .child(toolbar)
                    .child(content),
            )
            // Last in the tree, so it is prepainted after both frames above
            // have their bounds.
            .child(report_canvas(Rc::clone(regions)))
    }
}

/// Makes the window's Kit `Root` paint no background. A window that is not
/// inside a `Root` has nothing to clear.
fn clear_root_background(window: &Window, cx: &mut App) {
    let Some(root) = window.root::<Root>().flatten() else {
        return;
    };
    root.update(cx, |root, cx| {
        root.style().background = Some(transparent_black().into());
        cx.notify();
    });
}

/// A floating content panel: rounded, with the panel fill and a hairline. The
/// caller sets its size.
fn panel(theme: &Theme) -> Div {
    div()
        .relative()
        .rounded(px(PANEL_RADIUS))
        .bg(theme.panel)
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
}

/// The home screen: the service map on top and, under it, the panel of the
/// trace table, which fills the rest.
fn home_screen(theme: &Theme, map: &Entity<MapView>, table: &Entity<TraceTableView>) -> AnyElement {
    div()
        .flex_1()
        .w_full()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(INSET))
        .child(
            panel(theme)
                .w_full()
                .h(relative(MAP_SHARE))
                .flex_none()
                .child(map.clone()),
        )
        .child(
            panel(theme)
                .w_full()
                .flex_1()
                .min_h_0()
                .child(table.clone()),
        )
        .into_any_element()
}

/// The two regions the shell reports.
#[derive(Clone, Copy)]
enum Region {
    Sidebar,
    Toolbar,
}

/// A glass part's frame (the sidebar's or the toolbar's): rounded, over a canvas
/// that measures where it is laid out. The caller sets the size and adds the
/// contents.
///
/// - With a backdrop it has no fill, so the glass behind the window shows. It
///   has a hairline round it and a highlight just inside, both translucent.
/// - Without one it is a panel: the opaque fill and the panel hairline.
fn glass_frame(theme: &Theme, region: Region, regions: &Rc<RegionReport>) -> gpui::Div {
    let measure = {
        let regions = Rc::clone(regions);
        // A border-box measurement: the frame's own bounds, border and all.
        canvas(
            move |bounds, _window, _cx| regions.measured(region, bounds),
            |_, (), _, _| {},
        )
        .absolute()
        .top(px(-1.0))
        .left(px(-1.0))
        .right(px(-1.0))
        .bottom(px(-1.0))
    };
    let frame = div().relative().rounded(px(PANEL_RADIUS)).overflow_hidden();
    if regions.has_backdrop() {
        frame
            .border_1()
            .border_color(theme.glass_hairline())
            .child(measure)
            .child(
                // Inside the hairline, with the radius of its inner edge.
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
    } else {
        frame
            .bg(theme.panel)
            .border_1()
            .border_color(theme.border)
            .child(measure)
    }
}

/// The window colour for a window with glass parts: painted behind everything,
/// everywhere except inside the sidebar and the toolbar (their rounded border
/// boxes, as measured in the same frame). The backdrop is under the whole of
/// the window, so a plain background would hide it; this leaves exactly its
/// parts clear, and keeps the colour in the gaps, the margin and the slivers
/// outside the rounded corners.
fn window_colour(color: Rgba, regions: Rc<RegionReport>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _cx| {
            // The regions were measured in this frame's prepaint, which is
            // over before any paint.
            let holes = regions.glass_bounds();
            if let Some(path) = window_colour_path(bounds, &holes) {
                window.paint_path(path, color);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .right_0()
    .bottom_0()
}

/// The window rectangle with a rounded hole for each of `holes`, as one path
/// (filled with the even-odd rule, so each outline inside the rectangle is a
/// hole). `None` if it cannot be built.
fn window_colour_path(
    window: Bounds<Pixels>,
    holes: &[Bounds<Pixels>],
) -> Option<gpui::Path<Pixels>> {
    let mut path = PathBuilder::fill().with_style(PathStyle::Fill(
        FillOptions::default().with_fill_rule(FillRule::EvenOdd),
    ));
    path.move_to(window.origin);
    path.line_to(window.top_right());
    path.line_to(window.bottom_right());
    path.line_to(window.bottom_left());
    path.close();
    for hole in holes {
        rounded_outline(&mut path, *hole, px(PANEL_RADIUS));
    }
    path.build().ok()
}

/// Adds the outline of `bounds` with corners of `radius` to `path`, as a closed
/// sub-path.
fn rounded_outline(path: &mut PathBuilder, bounds: Bounds<Pixels>, radius: Pixels) {
    let radius = radius
        .min(bounds.size.width / 2.0)
        .min(bounds.size.height / 2.0);
    let (left, top) = (bounds.left(), bounds.top());
    let (right, bottom) = (bounds.right(), bounds.bottom());
    let corner: Point<Pixels> = point(radius, radius);
    path.move_to(point(left + radius, top));
    path.line_to(point(right - radius, top));
    path.arc_to(corner, px(0.0), false, true, point(right, top + radius));
    path.line_to(point(right, bottom - radius));
    path.arc_to(corner, px(0.0), false, true, point(right - radius, bottom));
    path.line_to(point(left + radius, bottom));
    path.arc_to(corner, px(0.0), false, true, point(left, bottom - radius));
    path.line_to(point(left, top + radius));
    path.arc_to(corner, px(0.0), false, true, point(left + radius, top));
    path.close();
}

/// The canvas that, once both frames are measured, passes them on.
fn report_canvas(regions: Rc<RegionReport>) -> impl IntoElement {
    canvas(
        move |_bounds, _window, _cx| regions.report(),
        |_, (), _, _| {},
    )
    .absolute()
    .size_0()
}

/// Where the sidebar and the toolbar were laid out in the latest frame, and
/// what was last passed to the backdrop. Shared by the canvases of a frame.
struct RegionReport {
    backdrop: Option<Box<dyn Backdrop>>,
    sidebar: Cell<Option<Bounds<Pixels>>>,
    toolbar: Cell<Option<Bounds<Pixels>>>,
    reported: RefCell<Vec<GlassRegion>>,
}

impl RegionReport {
    fn new(backdrop: Option<Box<dyn Backdrop>>) -> RegionReport {
        RegionReport {
            backdrop,
            sidebar: Cell::new(None),
            toolbar: Cell::new(None),
            reported: RefCell::new(Vec::new()),
        }
    }

    /// Whether a backdrop is attached: the sidebar and the toolbar are glass.
    fn has_backdrop(&self) -> bool {
        self.backdrop.is_some()
    }

    /// The border boxes of the sidebar and the toolbar as measured in the
    /// latest frame (the ones that are known).
    fn glass_bounds(&self) -> Vec<Bounds<Pixels>> {
        [self.sidebar.get(), self.toolbar.get()]
            .into_iter()
            .flatten()
            .collect()
    }

    fn measured(&self, region: Region, bounds: Bounds<Pixels>) {
        match region {
            Region::Sidebar => self.sidebar.set(Some(bounds)),
            Region::Toolbar => self.toolbar.set(Some(bounds)),
        }
    }

    /// Passes the regions to the backdrop when they differ from the last time.
    /// With no backdrop there is nothing to do.
    fn report(&self) {
        let Some(backdrop) = &self.backdrop else {
            return;
        };
        let (Some(sidebar), Some(toolbar)) = (self.sidebar.get(), self.toolbar.get()) else {
            return;
        };
        let regions = vec![
            GlassRegion {
                bounds: sidebar,
                corner_radius: px(PANEL_RADIUS),
            },
            GlassRegion {
                bounds: toolbar,
                corner_radius: px(PANEL_RADIUS),
            },
        ];
        let mut reported = self.reported.borrow_mut();
        if *reported != regions {
            backdrop.set_regions(&regions);
            *reported = regions;
        }
    }
}
