//! Headless rendering and pixel-exact snapshots, for the screenshot tests.
//! Compiled only with the `screenshot` feature.
//!
//! [`render_png`] draws a view through GPUI's headless renderer (wgpu on the
//! CPU, via lavapipe, so no GPU or display is needed) and hands back the
//! pixels. [`assert_snapshot`] compares them with the reference PNG in
//! `tests/snapshots/`. The same inputs give the same bytes: the font is
//! bundled and the renderer is software, so a reference made on one machine
//! holds on another. `docs/areas/ui.md` has the workflow.
//!
//! The main window needs more than a bare view, and has its own entry points:
//!
//! - [`render_window_png`] and [`Driver`] put a view in GPUI Kit's `Root` with
//!   Kit initialised, the theme forced and motion reduced (the test platform
//!   always reports the light appearance, and Kit's scrollbar fade reads real
//!   time otherwise). [`Driver`] keeps the window, so a test can click, scroll,
//!   type, move the clock and capture.
//! - [`headless_app`] is the same app with no window, for the models that live
//!   in entities and are tested by moving the clock (`tests/live.rs`).
//! - [`FixedBackend`] is the [`UiBackend`](crate::app::UiBackend) of tests, and
//!   [`RecordingBackdrop`] the [`Backdrop`](crate::app::Backdrop) that writes
//!   down what it is told.

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyView, App, AppContext as _, Context, Entity, HeadlessAppContext, IntoElement, Keystroke,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    PlatformInput, PlatformTextSystem, Point, Render, ScrollDelta, ScrollWheelEvent, Styled,
    TouchPhase, Window, WindowHandle, div, px,
};
use gpui_kit::base::Root as KitRoot;
use image::{Rgba, RgbaImage};
use loctel_engine::{EngineStatus, ListenerState, ListenerStatus};
use loctel_store::{Store, StoreStats};

use crate::app::{self, Backdrop, GlassRegion, UiBackend, kit_theme};
use crate::theme::{FONT_FAMILY, Theme, register_fonts};

/// The window's root: the view the caller built, filling the window.
struct Root(AnyView);

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

/// On Linux the text system sees only the bundled fonts, so what is drawn
/// does not depend on what is installed. Elsewhere the platform's own is
/// used (the bundled fonts are added on top).
fn text_system() -> Arc<dyn PlatformTextSystem> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(gpui_wgpu::CosmicTextSystem::new_without_system_fonts(
            FONT_FAMILY,
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = FONT_FAMILY;
        gpui_platform::current_platform(true).text_system()
    }
}

/// Renders the view `build` returns into a window of `size` logical pixels
/// and returns the pixels. The headless platform draws at scale 2, like a
/// Retina display, so a 900 by 600 window comes back as a 1800 by 1200 image.
/// `build` runs inside the window, so it can create entities with
/// `cx.new(...)` and turn one into an `AnyView` with `.into()`.
///
/// Panics, with the likely cause, when there is no usable software renderer:
/// run `script/setup-linux.sh`, which installs Mesa's lavapipe.
pub fn render_png(
    size: (u32, u32),
    build: impl FnOnce(&mut Window, &mut App) -> AnyView,
) -> RgbaImage {
    let (width, height) = size;
    let mut cx = HeadlessAppContext::with_platform(
        text_system(),
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    cx.update(register_fonts);
    let window = cx
        .open_window(
            gpui::size(px(width as f32), px(height as f32)),
            |window, cx| {
                let view = build(window, cx);
                cx.new(|_| Root(view))
            },
        )
        .expect("the headless window opens");
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .expect("the window draws");
    cx.capture_screenshot(window.into())
        .unwrap_or_else(|error| {
            panic!(
                "cannot capture a screenshot: {error:#}\n\
             (headless rendering needs Vulkan or EGL: run `bash script/setup-linux.sh`)"
            )
        })
}

/// A headless app with no window, for the models that live in entities (the
/// home screen's data, Milestone 3 Task 5): GPUI's test platform with the
/// bundled text system and Kit's icons, motion reduced, and everything the main
/// window needs set up ([`app::init`]).
///
/// Time stands still on it until `advance_clock` moves it, so a timer of a model
/// fires when the test says, and `run_until_parked` runs every task that can
/// run: the foreground ones and the background executor's. Entities are made
/// with `cx.new(...)` (`use gpui::AppContext as _`), read with `read_entity` and
/// changed with `update_entity`.
pub fn headless_app() -> HeadlessAppContext {
    let mut cx = HeadlessAppContext::with_platform(
        text_system(),
        Arc::new(gpui_kit::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        app::init(cx);
        cx.set_reduce_motion(true);
    });
    cx
}

/// Renders a view of the main window into a window of `size` logical pixels
/// and returns the pixels (at scale 2, like [`render_png`]).
///
/// It is [`Driver::new`] and one [`capture`](Driver::capture): GPUI Kit is
/// initialised, the view sits in Kit's `Root`, motion is reduced and `theme` is
/// forced (set after `build` ran, so it wins over the appearance a view reads
/// from the window, which on the test platform is always light). `build` runs
/// inside the window, like [`render_png`]'s.
pub fn render_window_png<V: Render>(
    size: (u32, u32),
    theme: &Theme,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> RgbaImage {
    Driver::new(size, theme, build).capture()
}

/// A headless window of the main window kind that a test drives and captures.
///
/// Everything runs on the test platform's clock, which stands still until
/// [`advance`](Driver::advance) moves it, so timers (the refreshes) fire when
/// the test says. Input goes through GPUI's own event path (`dispatch_event`),
/// so hit testing, hover and focus behave as for a user. Positions are in
/// logical pixels from the window's top left corner.
pub struct Driver<V: 'static> {
    cx: HeadlessAppContext,
    window: WindowHandle<KitRoot>,
    view: Entity<V>,
}

impl<V: Render + 'static> Driver<V> {
    /// Opens the window (`size` is logical pixels) and draws the first frame.
    /// `build` runs inside it and returns the root view; see
    /// [`render_window_png`] for what is set up around it.
    pub fn new(
        size: (u32, u32),
        theme: &Theme,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Driver<V> {
        let mut cx = headless_app();
        let mut built = None;
        let window = cx
            .open_window(
                gpui::size(px(size.0 as f32), px(size.1 as f32)),
                |window, cx| {
                    let view = build(window, cx);
                    built = Some(view.clone());
                    cx.new(|cx| KitRoot::new(view, window, cx))
                },
            )
            .expect("the headless window opens");
        let view = built.expect("the window ran its build closure");
        // After the build: a view that reads the window's appearance (light on
        // the test platform) has set its own theme by now.
        cx.update(|cx| kit_theme::apply(theme, cx));
        let mut driver = Driver { cx, window, view };
        driver.draw();
        driver
    }

    /// The root view.
    pub fn view(&self) -> &Entity<V> {
        &self.view
    }

    /// Runs what is pending and draws one frame.
    pub fn draw(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.draw(cx).clear(cx)
            })
            .expect("the window draws");
    }

    /// Draws and returns the window's pixels.
    pub fn capture(&mut self) -> RgbaImage {
        self.draw();
        self.cx
            .capture_screenshot(self.window.into())
            .unwrap_or_else(|error| {
                panic!(
                    "cannot capture a screenshot: {error:#}\n\
                 (headless rendering needs Vulkan or EGL: run `bash script/setup-linux.sh`)"
                )
            })
    }

    /// Moves the pointer to `at`, presses the left button and lets go, then
    /// draws: one click.
    pub fn click(&mut self, at: Point<Pixels>) {
        let modifiers = Modifiers::default();
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers,
        }));
        self.dispatch(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }));
        self.dispatch(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers,
            click_count: 1,
        }));
        self.draw();
    }

    /// Moves the pointer to `at` (hover states, tooltips) and draws.
    pub fn hover(&mut self, at: Point<Pixels>) {
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }));
        self.draw();
    }

    /// Turns the wheel over `at` by `dy` pixels: positive shows what is below
    /// (later rows come into view), negative what is above. Then draws.
    pub fn scroll(&mut self, at: Point<Pixels>, dy: f32) {
        self.dispatch(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: at,
            // GPUI's wheel delta is the offset change: negative moves the
            // content up, which shows what is below.
            delta: ScrollDelta::Pixels(Point::new(px(0.0), px(-dy))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        }));
        self.draw();
    }

    /// Types `text` into whatever has the focus, a character at a time, as the
    /// keyboard would (key down, then the character to the input handler).
    pub fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            let typed = character.to_string();
            let key = match character {
                ' ' => "space".to_owned(),
                other => other.to_lowercase().to_string(),
            };
            let keystroke = Keystroke {
                modifiers: Modifiers::default(),
                key,
                key_char: Some(typed),
            };
            self.keystroke(keystroke);
        }
        self.draw();
    }

    /// Presses a key or a chord, in GPUI's keystroke syntax: `enter`,
    /// `escape`, `cmd-f`, `ctrl-shift-a`. Then draws.
    pub fn key(&mut self, keystroke: &str) {
        let keystroke = Keystroke::parse(keystroke)
            .unwrap_or_else(|error| panic!("`{keystroke}` is not a keystroke: {error:#}"));
        self.keystroke(keystroke);
        self.draw();
    }

    /// Moves the clock forward by `by`, runs the timers and tasks that fall
    /// due, and draws.
    pub fn advance(&mut self, by: Duration) {
        self.cx.advance_clock(by);
        self.draw();
    }

    /// Resizes the window to `size` logical pixels and draws.
    pub fn resize(&mut self, size: (u32, u32)) {
        let size = gpui::size(px(size.0 as f32), px(size.1 as f32));
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                // The test platform changes its bounds but does not say so.
                window.resize(size);
                window.bounds_changed(cx);
            })
            .expect("the window resizes");
        self.draw();
    }

    /// Runs `update` on the root view, with the window and its context: the
    /// way to call a method of the view, or to reach into it.
    pub fn update_root<R>(
        &mut self,
        update: impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> R,
    ) -> R {
        let view = self.view.clone();
        self.cx
            .update_window(self.window.into(), move |_, window, cx| {
                view.update(cx, |view, cx| update(view, window, cx))
            })
            .expect("the window is open")
    }

    /// Reads the root view.
    pub fn read_root<R>(&self, read: impl FnOnce(&V) -> R) -> R {
        self.cx.read_entity(&self.view, |view, _| read(view))
    }

    /// Reads the app: its globals (Kit's theme, ours), its windows.
    pub fn read<R>(&self, read: impl FnOnce(&App) -> R) -> R {
        let app = self.cx.app.borrow();
        read(&app)
    }

    fn dispatch(&mut self, event: PlatformInput) {
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .expect("the window is open");
        self.cx.run_until_parked();
    }

    fn keystroke(&mut self, keystroke: Keystroke) {
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            })
            .expect("the window is open");
        self.cx.run_until_parked();
    }
}

/// A [`UiBackend`] with a fixed status, for tests: the store they load (a
/// `ManualClock` store filled from meshgen, so what it holds is the same on
/// every run) and the status shown in the sidebar.
pub struct FixedBackend {
    pub store: Store,
    pub status: EngineStatus,
    /// What [`UiBackend::utc_offset`] returns; tests use 0 so clock times read
    /// in UTC.
    pub utc_offset: i32,
}

impl FixedBackend {
    /// A backend over `store` with [`listening_status`] and UTC.
    pub fn new(store: Store) -> FixedBackend {
        FixedBackend {
            store,
            status: listening_status(),
            utc_offset: 0,
        }
    }
}

impl UiBackend for FixedBackend {
    fn store(&self) -> Store {
        self.store.clone()
    }

    fn status(&self) -> EngineStatus {
        self.status.clone()
    }

    fn clear_all(&self) {
        self.store.clear();
    }

    fn utc_offset(&self) -> i32 {
        self.utc_offset
    }
}

/// The status of a healthy engine that has received nothing: three listening
/// endpoints (OTLP gRPC on 4317, OTLP HTTP on 4318, MCP on 4319), not paused,
/// no discard rules.
pub fn listening_status() -> EngineStatus {
    let listening = |name: &'static str, port: u16| {
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        ListenerStatus {
            name,
            requested: address,
            state: ListenerState::Listening(address),
        }
    };
    EngineStatus {
        paused: false,
        listeners: vec![
            listening("otlp-grpc", 4317),
            listening("otlp-http", 4318),
            listening("mcp", 4319),
        ],
        spans_per_sec: 0.0,
        logs_per_sec: 0.0,
        stats: StoreStats::default(),
        rules: Vec::new(),
        rules_file_error: None,
        dropped_spans: 0,
        dropped_logs: 0,
        dropped_fraction: 0.0,
    }
}

/// A [`Backdrop`] that writes down every [`set_regions`](Backdrop::set_regions)
/// call. Clones share the record: give one to the window and keep the other.
#[derive(Clone, Default)]
pub struct RecordingBackdrop {
    calls: Rc<RefCell<Vec<Vec<GlassRegion>>>>,
}

impl RecordingBackdrop {
    /// The regions of each call so far, oldest first.
    pub fn calls(&self) -> Vec<Vec<GlassRegion>> {
        self.calls.borrow().clone()
    }
}

impl Backdrop for RecordingBackdrop {
    fn set_regions(&self, regions: &[GlassRegion]) {
        self.calls.borrow_mut().push(regions.to_vec());
    }
}

/// Where the references live: `tests/snapshots/` in this crate.
fn snapshot_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

/// Where a failure's images go: `target/screenshot-diffs/` of the workspace,
/// which CI uploads.
fn diff_dir() -> PathBuf {
    // This crate is `<workspace>/crates/loctel-ui`.
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels below the workspace root");
    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        // Joining an absolute path replaces the base, so both forms work.
        Some(dir) => workspace.join(dir),
        None => workspace.join("target"),
    };
    target.join("screenshot-diffs")
}

fn updating() -> bool {
    std::env::var("UPDATE_SNAPSHOTS").is_ok_and(|value| value == "1" || value == "true")
}

/// Compares `img` with `tests/snapshots/{name}.png`, pixel for pixel.
///
/// - With `UPDATE_SNAPSHOTS=1` it writes the reference instead. Look at the
///   PNG before committing it.
/// - On a mismatch (or a missing reference) it writes
///   `target/screenshot-diffs/{name}.actual.png` and `{name}.diff.png` (red
///   where the pixels differ, the reference faded elsewhere) and panics.
pub fn assert_snapshot(name: &str, img: &RgbaImage) {
    if let Err(message) = check_snapshot(name, img, &snapshot_dir(), &diff_dir(), updating()) {
        panic!("{message}");
    }
}

/// [`assert_snapshot`] with the directories and the mode as arguments, so it
/// can be tested without touching the repository's references.
fn check_snapshot(
    name: &str,
    img: &RgbaImage,
    references: &Path,
    diffs: &Path,
    update: bool,
) -> Result<(), String> {
    let reference_path = references.join(format!("{name}.png"));

    if update {
        std::fs::create_dir_all(references)
            .map_err(|error| format!("cannot create {}: {error}", references.display()))?;
        img.save(&reference_path)
            .map_err(|error| format!("cannot write {}: {error}", reference_path.display()))?;
        eprintln!("updated reference {}", reference_path.display());
        return Ok(());
    }

    let actual_path = diffs.join(format!("{name}.actual.png"));
    let diff_path = diffs.join(format!("{name}.diff.png"));
    let save = |image: &RgbaImage, path: &Path| -> Result<(), String> {
        std::fs::create_dir_all(diffs)
            .map_err(|error| format!("cannot create {}: {error}", diffs.display()))?;
        image
            .save(path)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    };

    let reference = match image::open(&reference_path) {
        Ok(reference) => reference.to_rgba8(),
        Err(error) => {
            save(img, &actual_path)?;
            return Err(format!(
                "no usable reference {} ({error}).\n\
                 Create it with `UPDATE_SNAPSHOTS=1 cargo test -p loctel-ui --features screenshot`, \
                 then open the PNG and check it before committing.\n\
                 The render is at {}",
                reference_path.display(),
                actual_path.display()
            ));
        }
    };

    if reference.dimensions() == img.dimensions() && reference.as_raw() == img.as_raw() {
        return Ok(());
    }

    save(img, &actual_path)?;
    let what = match diff_image(&reference, img) {
        Some((diff, count, (x, y))) => {
            save(&diff, &diff_path)?;
            format!("{count} pixels differ, the first at ({x}, {y})")
        }
        None => format!(
            "the size changed: reference {:?}, render {:?}",
            reference.dimensions(),
            img.dimensions()
        ),
    };
    Err(format!(
        "screenshot `{name}` does not match its reference: {what}.\n\
         reference: {}\nactual:    {}\ndiff:      {}\n\
         If the change is intended, run `UPDATE_SNAPSHOTS=1 cargo test -p loctel-ui --features screenshot`, \
         view the new PNG and commit it.",
        reference_path.display(),
        actual_path.display(),
        diff_path.display()
    ))
}

/// The diff image, the number of differing pixels and the first one's place,
/// or `None` when the sizes differ (there is no pixel-wise diff then).
fn diff_image(reference: &RgbaImage, actual: &RgbaImage) -> Option<(RgbaImage, usize, (u32, u32))> {
    if reference.dimensions() != actual.dimensions() {
        return None;
    }
    let (width, height) = reference.dimensions();
    let mut diff = RgbaImage::new(width, height);
    let mut count = 0;
    let mut first = (0, 0);
    for (x, y, expected) in reference.enumerate_pixels() {
        if expected == actual.get_pixel(x, y) {
            let [r, g, b, _] = expected.0;
            // The reference at a quarter of its brightness: context, not noise.
            diff.put_pixel(x, y, Rgba([r / 4, g / 4, b / 4, 255]));
        } else {
            if count == 0 {
                first = (x, y);
            }
            count += 1;
            diff.put_pixel(x, y, Rgba([255, 0, 0, 255]));
        }
    }
    Some((diff, count, first))
}

#[cfg(test)]
mod tests {
    use super::*;

    use gpui::{
        FocusHandle, InteractiveElement as _, ScrollHandle, StatefulInteractiveElement as _, point,
    };

    /// A window with something to click, a scrolling list and a focused
    /// element that records the keys it gets.
    struct Probe {
        clicks: usize,
        keys: Vec<String>,
        focus: FocusHandle,
        scroll: ScrollHandle,
    }

    impl Render for Probe {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|probe, event: &gpui::KeyDownEvent, _, _| {
                    let keystroke = &event.keystroke;
                    let platform = if keystroke.modifiers.platform {
                        "platform+"
                    } else {
                        ""
                    };
                    probe.keys.push(match &keystroke.key_char {
                        Some(typed) => format!("char:{typed:?}"),
                        None => format!("key:{platform}{}", keystroke.key),
                    });
                }))
                .child(
                    div()
                        .id("button")
                        .w(px(100.0))
                        .h(px(40.0))
                        .on_click(cx.listener(|probe, _, _, _| probe.clicks += 1)),
                )
                .child(
                    div()
                        .id("list")
                        .absolute()
                        .top(px(100.0))
                        .left(px(0.0))
                        .w(px(200.0))
                        .h(px(100.0))
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(div().h(px(1000.0)).w(px(200.0))),
                )
        }
    }

    fn probe_driver() -> Driver<Probe> {
        Driver::new((400, 300), &Theme::dark(), |window, cx| {
            let probe = cx.new(|cx| Probe {
                clicks: 0,
                keys: Vec::new(),
                focus: cx.focus_handle(),
                scroll: ScrollHandle::new(),
            });
            let focus = probe.read(cx).focus.clone();
            window.focus(&focus, cx);
            probe
        })
    }

    #[test]
    fn driver_clicks_where_told() {
        let mut driver = probe_driver();
        driver.click(point(px(50.0), px(20.0)));
        assert_eq!(driver.read_root(|probe| probe.clicks), 1);
        driver.click(point(px(300.0), px(250.0)));
        assert_eq!(driver.read_root(|probe| probe.clicks), 1, "off the button");
    }

    #[test]
    fn driver_scrolls_down_for_positive_dy() {
        let mut driver = probe_driver();
        let list = point(px(50.0), px(150.0));
        driver.scroll(list, 60.0);
        let offset = driver.read_root(|probe| probe.scroll.offset().y);
        assert_eq!(offset, px(-60.0), "later content comes into view");
        driver.scroll(list, -20.0);
        let offset = driver.read_root(|probe| probe.scroll.offset().y);
        assert_eq!(offset, px(-40.0));
    }

    #[test]
    fn driver_types_text_and_presses_keys() {
        let mut driver = probe_driver();
        driver.type_text("Ab {");
        driver.key("enter");
        driver.key("cmd-f");
        let keys = driver.read_root(|probe| probe.keys.clone());
        // `enter` carries its newline, as a keyboard's does into a text input;
        // a chord with the command key carries no character.
        assert_eq!(
            keys,
            [
                "char:\"A\"",
                "char:\"b\"",
                "char:\" \"",
                "char:\"{\"",
                "char:\"\\n\"",
                "key:platform+f"
            ]
        );
    }

    #[test]
    fn driver_advances_the_clock_for_timers() {
        let mut driver = probe_driver();
        let fired = Rc::new(RefCell::new(false));
        let flag = Rc::clone(&fired);
        driver.update_root(move |_, _, cx| {
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                *flag.borrow_mut() = true;
            })
            .detach();
        });
        driver.advance(Duration::from_millis(400));
        assert!(!*fired.borrow(), "not yet");
        driver.advance(Duration::from_millis(200));
        assert!(*fired.borrow(), "after 600 ms");
    }

    fn flat(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba(color))
    }

    #[test]
    fn update_writes_the_reference_and_a_matching_render_passes() {
        let dir = tempfile::tempdir().unwrap();
        let (references, diffs) = (dir.path().join("refs"), dir.path().join("diffs"));
        let image = flat(4, 3, [10, 20, 30, 255]);

        check_snapshot("one", &image, &references, &diffs, true).unwrap();
        assert!(references.join("one.png").is_file());
        check_snapshot("one", &image, &references, &diffs, false).unwrap();
        assert!(!diffs.exists(), "a match writes no diff files");
    }

    #[test]
    fn a_single_changed_pixel_fails_and_writes_actual_and_diff_images() {
        let dir = tempfile::tempdir().unwrap();
        let (references, diffs) = (dir.path().join("refs"), dir.path().join("diffs"));
        let reference = flat(4, 3, [10, 20, 30, 255]);
        check_snapshot("one", &reference, &references, &diffs, true).unwrap();

        let mut actual = reference.clone();
        actual.put_pixel(2, 1, Rgba([10, 20, 31, 255]));
        let message = check_snapshot("one", &actual, &references, &diffs, false).unwrap_err();

        assert!(
            message.contains("1 pixels differ, the first at (2, 1)"),
            "{message}"
        );
        assert!(message.contains("one.actual.png") && message.contains("one.diff.png"));
        let saved = image::open(diffs.join("one.actual.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(saved.as_raw(), actual.as_raw());
        let diff = image::open(diffs.join("one.diff.png")).unwrap().to_rgba8();
        assert_eq!(diff.get_pixel(2, 1), &Rgba([255, 0, 0, 255]));
        assert_eq!(diff.get_pixel(0, 0), &Rgba([2, 5, 7, 255]));
    }

    #[test]
    fn a_different_size_fails_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let (references, diffs) = (dir.path().join("refs"), dir.path().join("diffs"));
        check_snapshot(
            "one",
            &flat(4, 3, [0, 0, 0, 255]),
            &references,
            &diffs,
            true,
        )
        .unwrap();

        let message = check_snapshot(
            "one",
            &flat(5, 3, [0, 0, 0, 255]),
            &references,
            &diffs,
            false,
        )
        .unwrap_err();
        assert!(message.contains("the size changed"), "{message}");
        assert!(diffs.join("one.actual.png").is_file());
    }

    #[test]
    fn a_missing_reference_fails_with_the_way_to_make_one() {
        let dir = tempfile::tempdir().unwrap();
        let (references, diffs) = (dir.path().join("refs"), dir.path().join("diffs"));
        let message = check_snapshot(
            "new",
            &flat(2, 2, [1, 2, 3, 255]),
            &references,
            &diffs,
            false,
        )
        .unwrap_err();
        assert!(message.contains("UPDATE_SNAPSHOTS=1"), "{message}");
        assert!(diffs.join("new.actual.png").is_file());
    }
}
