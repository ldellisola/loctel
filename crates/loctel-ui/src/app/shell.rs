//! The main window's shell: the floating sidebar, the toolbar over the content
//! and the content panel. It owns which [`Screen`] is showing, follows the
//! system appearance and reports where the sidebar and the toolbar are, so a
//! platform backdrop (macOS's Liquid Glass) can sit behind them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Bounds, Context, InteractiveElement as _, IntoElement, ParentElement, Pixels, Render, Styled,
    Subscription, Window, WindowAppearance, canvas, div, px,
};
use loctel_model::TraceId;

use super::backend::UiBackend;
use super::kit_theme;
use super::sidebar::sidebar_contents;
use super::toolbar::toolbar_contents;
use crate::theme::FONT_FAMILY;
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
}

/// A part of the window that can have a platform backdrop behind it: where it
/// is, in window coordinates, and how round its corners are.
#[derive(Clone, Debug, PartialEq)]
pub struct GlassRegion {
    pub bounds: Bounds<Pixels>,
    pub corner_radius: Pixels,
}

/// What sits behind the window's glass parts. On macOS it is a native glass
/// view per region; elsewhere there is none and the panels are opaque.
///
/// The shell calls [`set_regions`](Backdrop::set_regions) from inside a GPUI
/// frame, so an implementation must not call back into GPUI or AppKit's layout
/// from it: it stores the regions and applies them later.
pub trait Backdrop: 'static {
    /// The regions as laid out now: the sidebar first, then the toolbar. Called
    /// only when they differ from the last call.
    fn set_regions(&self, regions: &[GlassRegion]);
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
    regions: Rc<RegionReport>,
    /// Keeps the appearance observer alive.
    _appearance: Subscription,
}

impl MainWindow {
    /// The shell showing [`Screen::Home`], with the theme of `window`'s
    /// appearance applied (Kit's too), refreshing its status box from `backend`
    /// every second. With a `backdrop`, the sidebar's and the toolbar's bounds
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

        let status = StatusModel::from_status(&backend.status());
        MainWindow {
            backend,
            screen: Screen::Home,
            status,
            regions: Rc::new(RegionReport::new(backdrop)),
            _appearance: appearance,
        }
    }

    /// The screen showing now.
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Shows `screen`.
    pub fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if self.screen != screen {
            self.screen = screen;
            cx.notify();
        }
    }

    /// What the status box shows.
    pub fn status(&self) -> &StatusModel {
        &self.status
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let regions = &self.regions;

        let sidebar = glass_frame(&theme, Region::Sidebar, regions)
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .child(sidebar_contents(&theme, &self.screen, &self.status, cx));

        let toolbar = glass_frame(&theme, Region::Toolbar, regions)
            .w_full()
            .h(px(TOOLBAR_HEIGHT))
            .flex_none()
            .child(toolbar_contents(&theme, &self.screen));

        let content = div()
            .flex_1()
            .w_full()
            .min_h_0()
            .rounded(px(PANEL_RADIUS))
            .bg(theme.panel)
            .border_1()
            .border_color(theme.border);

        div()
            .id("main-window")
            .size_full()
            .relative()
            .flex()
            .flex_row()
            .gap(px(INSET))
            .p(px(INSET))
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(FONT_FAMILY)
            .text_size(px(13.0))
            .child(sidebar)
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

/// The two regions the shell reports.
#[derive(Clone, Copy)]
enum Region {
    Sidebar,
    Toolbar,
}

/// A floating panel's frame: rounded, with a fill and a hairline, over a
/// canvas that measures where it is laid out. The caller sets the size and
/// adds the contents.
fn glass_frame(theme: &Theme, region: Region, regions: &Rc<RegionReport>) -> gpui::Div {
    let regions = Rc::clone(regions);
    div()
        .relative()
        .rounded(px(PANEL_RADIUS))
        .bg(theme.panel)
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .child(
            // A border-box measurement: the frame's own bounds, border and all.
            canvas(
                move |bounds, _window, _cx| regions.measured(region, bounds),
                |_, (), _, _| {},
            )
            .absolute()
            .top(px(-1.0))
            .left(px(-1.0))
            .right(px(-1.0))
            .bottom(px(-1.0)),
        )
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
