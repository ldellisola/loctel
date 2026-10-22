//! The menu bar popover: the panel that opens under the status item and
//! replaces the M1 and M2 menu (M4 plan, Task 9; decision 0042).
//!
//! `gpui-tray` calls back on a click on the icon only while the item has no
//! menu, and it does not say where the icon is (`on_activate_with` gets no
//! frame). So the popover is placed from the mouse: [`toggle`] reads
//! `NSEvent::mouseLocation()` at the click, finds the screen that holds it, and
//! [`anchor`] puts the popover's top 6 points under the menu bar, centred on the
//! click and kept 8 points inside the visible frame.
//!
//! - [`anchor`] and [`containing_screen`] are plain Rust with the numbers of the
//!   placement, tested on every OS (the Linux gate runs them).
//! - [`install`] gives the app what the popover needs (the engine and the
//!   settings path), and [`toggle`] is what the status item runs on a click:
//!   close the popover if it is open, otherwise open it.
//! - The popover is a GPUI `PopUp` window with no titlebar, 336 by
//!   `popover_height(true)` points, with `loctel_ui::views::PopoverView` in GPUI
//!   Kit's `Root`. Its background is transparent with the Liquid Glass of
//!   [`glass::attach`](crate::glass) behind it, or GPUI's blurred background when
//!   there is none (`window::attach_glass`).
//! - It closes when its window is deactivated (a click anywhere else), on Esc and
//!   after an action that opens a window; the view asks for the last two through
//!   `PopoverActions::close`.
//! - [`MacActions`] is what the view asks of the app: windows, the engine, the
//!   clipboard, the login item and quit. The view calls them from inside its event
//!   handlers, where GPUI's `App` is borrowed, so anything that needs the app runs
//!   on the foreground executor, after the handler (decision 0041).
//! - If the popover's window cannot be opened, the main window opens instead and
//!   the reason is logged once: loctel never becomes unreachable.
//!
//! Nothing here has run on a Mac. See "Unverified until the M4 Mac check" in
//! `docs/areas/macos.md`.

/// A rectangle in AppKit's screen coordinates: points, y going up from the
/// bottom left of the primary screen (so a screen above the primary one has a
/// positive y).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn max_x(&self) -> f64 {
        self.x + self.width
    }

    fn max_y(&self) -> f64 {
        self.y + self.height
    }
}

/// How far the popover's top sits under the bottom edge of the menu bar (the top
/// of the visible frame), in points.
pub(crate) const TOP_GAP: f64 = 6.0;
/// How near the popover may come to the left and right edge of the visible
/// frame, in points.
pub(crate) const EDGE_MARGIN: f64 = 8.0;

/// Where the popover's top left corner goes, as GPUI's window bounds want it: the
/// distance from the left and the top edge of the screen with frame
/// `screen_frame`, which the window is opened on (`WindowOptions::display_id`).
///
/// - The popover's top is [`TOP_GAP`] below the top of `visible_frame`, which is
///   the screen less the menu bar and the Dock (so the popover hangs from under
///   the menu bar, and a Dock at the left or right does not matter).
/// - It is centred on `click_x` (the mouse's x, in the same coordinates as the
///   frames), then moved so that its whole width is inside `visible_frame`, with
///   [`EDGE_MARGIN`] to spare at each side. A visible frame too narrow for that
///   puts the popover at its left margin.
///
/// `size` is the popover's width and height. The height does not move it: the
/// popover hangs from its top.
pub(crate) fn anchor(
    click_x: f64,
    screen_frame: Rect,
    visible_frame: Rect,
    size: (f64, f64),
) -> (f64, f64) {
    let (width, _height) = size;
    let leftmost = visible_frame.x + EDGE_MARGIN;
    let rightmost = visible_frame.max_x() - EDGE_MARGIN - width;
    let left = if rightmost < leftmost {
        leftmost
    } else {
        (click_x - width / 2.0).clamp(leftmost, rightmost)
    };
    let top = visible_frame.max_y() - TOP_GAP;
    (left - screen_frame.x, screen_frame.max_y() - top)
}

/// The index of the first of `frames` that holds the mouse at `mouse`, as AppKit's
/// `NSMouseInRect` counts for a view that is not flipped: the left edge and the
/// top edge belong to a frame, the right and the bottom edge do not. The mouse
/// pushed against the top of a screen, where the menu bar is, reports exactly the
/// top edge, so it is the top edge that must count.
pub(crate) fn containing_screen(mouse: (f64, f64), frames: &[Rect]) -> Option<usize> {
    frames.iter().position(|frame| {
        mouse.0 >= frame.x
            && mouse.0 < frame.max_x()
            && mouse.1 > frame.y
            && mouse.1 <= frame.max_y()
    })
}

#[cfg(target_os = "macos")]
pub(crate) use appkit::{install, open_window_handle, toggle};

#[cfg(target_os = "macos")]
mod appkit {
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use anyhow::Context as _;
    use gpui::{
        AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, ClipboardItem, DisplayId, Global,
        WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, point, px, size,
    };
    use gpui_kit::base::Root;
    use loctel_engine::EngineHandle;
    use loctel_ui::app::{Screen, UiBackend};
    use loctel_ui::views::{POPOVER_WIDTH, PopoverActions, PopoverView, popover_height};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSScreen};
    use objc2_foundation::{NSNumber, NSRect, NSString};

    use super::{Rect, anchor, containing_screen};
    use crate::{login_item, status_item, window};

    /// Whether the fallback to the main window has been logged.
    static FALLBACK_LOGGED: AtomicBool = AtomicBool::new(false);

    /// What the popover needs from the app, and the window while it is open. A GPUI
    /// global: the status item's click handler gets only the `App`.
    struct PopoverState {
        engine: EngineHandle,
        settings_path: PathBuf,
        /// The popover's window. A closed window's handle stays here until the
        /// next open; it is checked against the open windows before it is used.
        window: Option<AnyWindowHandle>,
    }

    impl Global for PopoverState {}

    /// Gives the app what [`toggle`] needs. `status_item::install` calls it
    /// before the status item exists, so a click always finds it.
    pub(crate) fn install(cx: &mut App, engine: EngineHandle, settings_path: PathBuf) {
        cx.set_global(PopoverState {
            engine,
            settings_path,
            window: None,
        });
    }

    /// What a click on the status item does: closes the popover if it is open,
    /// otherwise opens it under the click. If it cannot be opened, the main window
    /// opens instead.
    pub(crate) fn toggle(cx: &mut App) {
        if close(cx) {
            return;
        }
        let Some(state) = cx.try_global::<PopoverState>() else {
            eprintln!("loctel: the popover was asked for before it was installed");
            return;
        };
        let engine = state.engine.clone();
        let settings_path = state.settings_path.clone();
        match open(cx, &engine, &settings_path) {
            Ok(handle) => {
                cx.global_mut::<PopoverState>().window = Some(handle);
                cx.activate(true);
                // `update` fails only for a window that is gone, which cannot be
                // the one just opened.
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
            Err(error) => {
                if !FALLBACK_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "loctel: cannot open the menu bar popover: {error:#}; \
                         opening the main window instead"
                    );
                }
                window::open_or_focus(cx, engine, settings_path);
            }
        }
    }

    /// Closes the popover if it is open. Returns whether it was: a second call, or
    /// a call after the window deactivated, finds it closed and does nothing.
    fn close(cx: &mut App) -> bool {
        let Some(handle) = cx
            .try_global::<PopoverState>()
            .and_then(|state| state.window)
        else {
            return false;
        };
        if !cx.windows().contains(&handle) {
            return false;
        }
        cx.global_mut::<PopoverState>().window = None;
        // `update` fails only for a window that is gone, which the check above
        // has just ruled out.
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        true
    }

    /// The popover's window, while it is open: `window.rs` leaves it out when it
    /// counts the windows that keep loctel in the Dock.
    pub(crate) fn open_window_handle(cx: &App) -> Option<AnyWindowHandle> {
        cx.try_global::<PopoverState>()
            .and_then(|state| state.window)
            .filter(|handle| cx.windows().contains(handle))
    }

    /// Opens the popover's window under the mouse and returns its handle.
    fn open(
        cx: &mut App,
        engine: &EngineHandle,
        settings_path: &Path,
    ) -> anyhow::Result<AnyWindowHandle> {
        let mtm = MainThreadMarker::new().context("the popover must open on the main thread")?;
        let width = f64::from(POPOVER_WIDTH);
        // The popover has the login switch: macOS has a login item.
        let height = f64::from(popover_height(true));
        let (display, (x, y)) = place(mtm, (width, height))?;

        loctel_ui::app::init(cx);
        let backend: Rc<dyn UiBackend> = window::backend(engine, settings_path);
        let actions = Rc::new(MacActions {
            cx: cx.to_async(),
            engine: engine.clone(),
            settings_path: settings_path.to_owned(),
        });
        let bounds = Bounds::new(
            point(px(x as f32), px(y as f32)),
            size(px(width as f32), px(height as f32)),
        );
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id: Some(DisplayId::new(display)),
            window_background: WindowBackgroundAppearance::Transparent,
            ..WindowOptions::default()
        };
        let handle = cx.open_window(options, move |window, cx| {
            let backdrop = window::attach_glass(window);
            let view = cx.new(|cx| {
                let view = PopoverView::new(backend, actions, cx).with_backdrop(backdrop);
                // Closes on a click anywhere else. The window starts inactive, so
                // only a deactivation after it has been active counts.
                let mut was_active = false;
                cx.observe_window_activation(window, move |_, window, _| {
                    if window.is_window_active() {
                        was_active = true;
                    } else if was_active {
                        window.remove_window();
                    }
                })
                .detach();
                view
            });
            cx.new(|cx| Root::new(view, window, cx))
        })?;
        Ok(handle.into())
    }

    /// The display to open the popover on and the window's origin on it, for a
    /// click now: the screen under the mouse (the primary one if none holds it),
    /// and [`anchor`] for it.
    fn place(mtm: MainThreadMarker, size: (f64, f64)) -> anyhow::Result<(u64, (f64, f64))> {
        let mouse = NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm);
        let count = screens.count();
        anyhow::ensure!(count > 0, "there is no screen to open the popover on");
        let frames: Vec<Rect> = (0..count)
            .map(|index| rect(screens.objectAtIndex(index).frame()))
            .collect();
        // The first screen is the one with the menu bar; the mouse is on it or on
        // a screen with its own bar, so this is only the fallback.
        let index = containing_screen((mouse.x, mouse.y), &frames).unwrap_or(0);
        let screen = screens.objectAtIndex(index);
        let origin = anchor(mouse.x, frames[index], rect(screen.visibleFrame()), size);
        let display = display_number(&screen)
            .context("cannot tell which display the screen under the mouse is")?;
        Ok((display, origin))
    }

    fn rect(frame: NSRect) -> Rect {
        Rect {
            x: frame.origin.x,
            y: frame.origin.y,
            width: frame.size.width,
            height: frame.size.height,
        }
    }

    /// The `CGDirectDisplayID` of `screen` (its `NSScreenNumber`), which is what
    /// GPUI's `DisplayId` holds on a Mac.
    fn display_number(screen: &NSScreen) -> Option<u64> {
        let description = screen.deviceDescription();
        let number = description.objectForKey(&NSString::from_str("NSScreenNumber"))?;
        let number = number.downcast::<NSNumber>().ok()?;
        Some(u64::from(number.unsignedIntValue()))
    }

    /// What the popover asks of the app (`PopoverActions`).
    ///
    /// The view calls these from inside its event handlers, with GPUI's `App`
    /// borrowed, so the ones that need the app (windows, the clipboard, quit) run
    /// later on the foreground executor, in the order they were asked. The ones
    /// that do not (pause, clear, the login item) run at once, so the view reads
    /// their result as soon as it has called them.
    struct MacActions {
        cx: AsyncApp,
        engine: EngineHandle,
        settings_path: PathBuf,
    }

    impl MacActions {
        /// Runs `work` on the app once the handler that asked is done.
        fn later(&self, work: impl FnOnce(&mut App) + 'static) {
            let cx = self.cx.clone();
            self.cx
                .foreground_executor()
                .spawn(async move { cx.update(work) })
                .detach();
        }
    }

    impl PopoverActions for MacActions {
        fn open_main(&self, screen: Screen) {
            let engine = self.engine.clone();
            let settings_path = self.settings_path.clone();
            self.later(move |cx| window::show_screen(cx, engine, settings_path, screen));
        }

        fn open_settings(&self) {
            let engine = self.engine.clone();
            let settings_path = self.settings_path.clone();
            self.later(move |cx| window::open_settings(cx, engine, settings_path));
        }

        fn toggle_pause(&self) {
            self.engine.set_paused(!self.engine.is_paused());
        }

        fn clear_all(&self) {
            self.engine.store().clear();
        }

        fn copy_mcp(&self) {
            let url = self.engine.mcp_url();
            self.later(move |cx| match url {
                Some(url) => cx.write_to_clipboard(ClipboardItem::new_string(url)),
                None => {
                    eprintln!("loctel: the MCP server is not listening, so there is no address")
                }
            });
        }

        fn set_start_at_login(&self, on: bool) {
            if let Err(error) = login_item::set_enabled(on) {
                eprintln!("loctel: {error:#}");
            }
        }

        fn quit(&self) {
            let engine = self.engine.clone();
            self.later(move |cx| status_item::quit(&engine, cx));
        }

        fn close(&self) {
            self.later(|cx| {
                close(cx);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: f64 = 336.0;
    const HEIGHT: f64 = 504.0;
    const SIZE: (f64, f64) = (WIDTH, HEIGHT);

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// A 1440 by 900 primary screen with a 37 point menu bar (the notch's) and
    /// a Dock at the bottom that takes 70 points.
    fn primary() -> (Rect, Rect) {
        (
            rect(0.0, 0.0, 1440.0, 900.0),
            rect(0.0, 70.0, 1440.0, 793.0),
        )
    }

    #[test]
    fn anchor_centres_under_click() {
        let (screen, visible) = primary();
        let (x, _) = anchor(700.0, screen, visible, SIZE);
        assert_eq!(x, 700.0 - WIDTH / 2.0);
        // The middle of the popover is the click, to the point.
        assert_eq!(x + WIDTH / 2.0, 700.0);
    }

    #[test]
    fn anchor_clamps_at_right_edge() {
        let (screen, visible) = primary();
        // A click 10 points from the right edge: centred, the popover would
        // stick out by 158 points. Its right edge stops 8 points inside.
        let (x, _) = anchor(1430.0, screen, visible, SIZE);
        assert_eq!(x, 1440.0 - 8.0 - WIDTH);
        assert_eq!(x + WIDTH, visible.x + visible.width - 8.0);
        // A click on the edge itself is no different.
        assert_eq!(anchor(1440.0, screen, visible, SIZE).0, x);
    }

    #[test]
    fn anchor_clamps_at_left_edge() {
        let (screen, visible) = primary();
        let (x, _) = anchor(10.0, screen, visible, SIZE);
        assert_eq!(x, 8.0);
        assert_eq!(anchor(0.0, screen, visible, SIZE).0, 8.0);
    }

    #[test]
    fn anchor_below_menu_bar() {
        let (screen, visible) = primary();
        let (_, y) = anchor(700.0, screen, visible, SIZE);
        // The menu bar is 900 - 863 = 37 points; the popover's top is 6 below it.
        assert_eq!(y, 37.0 + 6.0);
        // The bar of a screen without a notch is 24 points: the gap stays 6.
        let flat = rect(0.0, 70.0, 1440.0, 806.0);
        assert_eq!(anchor(700.0, screen, flat, SIZE).1, 24.0 + 6.0);
        // The Dock does not move the top: the same bar, no Dock.
        let no_dock = rect(0.0, 0.0, 1440.0, 863.0);
        assert_eq!(anchor(700.0, screen, no_dock, SIZE).1, y);
    }

    #[test]
    fn a_dock_at_the_side_moves_the_edge_the_popover_keeps_clear_of() {
        let screen = rect(0.0, 0.0, 1440.0, 900.0);
        // A Dock of 80 points on the left and one of 60 on the right.
        let left = rect(80.0, 0.0, 1360.0, 863.0);
        assert_eq!(anchor(10.0, screen, left, SIZE).0, 80.0 + 8.0);
        let right = rect(0.0, 0.0, 1380.0, 863.0);
        assert_eq!(anchor(1430.0, screen, right, SIZE).0, 1380.0 - 8.0 - WIDTH);
    }

    #[test]
    fn anchor_is_relative_to_the_screen_it_opens_on() {
        // A second screen to the right of the primary one and 180 points higher:
        // the origin is measured from its own top left, not from the primary's.
        let screen = rect(1440.0, 180.0, 1920.0, 1080.0);
        let visible = rect(1440.0, 180.0, 1920.0, 1043.0);
        let (x, y) = anchor(2400.0, screen, visible, SIZE);
        assert_eq!(x, 2400.0 - WIDTH / 2.0 - 1440.0);
        assert_eq!(y, 37.0 + 6.0);
        // And its edges are its own: a click 5 points inside its left edge.
        assert_eq!(anchor(1445.0, screen, visible, SIZE).0, 8.0);
        // A screen below the primary one has a negative y; the top is the same.
        let below = rect(0.0, -1080.0, 1920.0, 1080.0);
        let below_visible = rect(0.0, -1080.0, 1920.0, 1043.0);
        assert_eq!(anchor(960.0, below, below_visible, SIZE).1, 37.0 + 6.0);
    }

    #[test]
    fn a_visible_frame_too_narrow_for_the_popover_gives_its_left_margin() {
        let screen = rect(0.0, 0.0, 300.0, 600.0);
        let visible = rect(0.0, 0.0, 300.0, 563.0);
        assert_eq!(anchor(150.0, screen, visible, SIZE).0, 8.0);
    }

    #[test]
    fn the_height_does_not_move_the_popover() {
        let (screen, visible) = primary();
        assert_eq!(
            anchor(700.0, screen, visible, (WIDTH, 300.0)),
            anchor(700.0, screen, visible, (WIDTH, 800.0))
        );
    }

    #[test]
    fn the_mouse_is_on_the_screen_that_holds_it() {
        let frames = [
            rect(0.0, 0.0, 1440.0, 900.0),
            rect(1440.0, 180.0, 1920.0, 1080.0),
        ];
        assert_eq!(containing_screen((700.0, 450.0), &frames), Some(0));
        assert_eq!(containing_screen((2400.0, 700.0), &frames), Some(1));
        // Right of the first screen and below the second one's bottom edge: none.
        assert_eq!(containing_screen((1500.0, 100.0), &frames), None);
        assert_eq!(containing_screen((-1.0, 450.0), &frames), None);
    }

    #[test]
    fn the_top_edge_of_a_screen_is_on_the_screen() {
        // The mouse pushed against the menu bar reports the top edge exactly.
        let frames = [rect(0.0, 0.0, 1440.0, 900.0)];
        assert_eq!(containing_screen((700.0, 900.0), &frames), Some(0));
        // The bottom edge and the right edge are the next screen's.
        assert_eq!(containing_screen((700.0, 0.0), &frames), None);
        assert_eq!(containing_screen((1440.0, 450.0), &frames), None);
        assert_eq!(containing_screen((0.0, 450.0), &frames), Some(0));
    }

    #[test]
    fn the_shared_edge_of_two_screens_belongs_to_one() {
        // Side by side: x = 1440 is the second screen's left edge.
        let side = [
            rect(0.0, 0.0, 1440.0, 900.0),
            rect(1440.0, 0.0, 1920.0, 1080.0),
        ];
        assert_eq!(containing_screen((1440.0, 450.0), &side), Some(1));
        // Stacked: y = 900 is the top edge of the lower screen, so it is its.
        let stacked = [
            rect(0.0, 0.0, 1440.0, 900.0),
            rect(0.0, 900.0, 1440.0, 900.0),
        ];
        assert_eq!(containing_screen((700.0, 900.0), &stacked), Some(0));
        assert_eq!(containing_screen((700.0, 900.5), &stacked), Some(1));
    }
}
