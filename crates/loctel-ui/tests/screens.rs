//! The main window's screens: the shell (M3 plan, Task 4: the sidebar, the
//! toolbar and the content area), the home screen with the service map above
//! the trace table (Tasks 6 and 7) and the trace screen (Task 8: the header, the
//! waterfall, the inspector and the span's logs), rendered through GPUI Kit's
//! `Root` in the headless harness. The references are `snapshots/shell_*.png`,
//! `home_*.png`, `toolbar_*.png` (crops of the home toolbar, decision 0032) and
//! `trace_*.png` (and `huge_trace_middle_dark.png`, `long_values_dark.png`);
//! build them with `cargo xtask screenshots --update`
//! and look at them before committing (`docs/areas/ui.md`).

mod common;

use std::rc::Rc;
use std::sync::Arc;

use gpui::{AppContext as _, Bounds, Hsla, WindowAppearance, point, px, size};
use gpui_kit::component::ActiveTheme as _;
use loctel_meshgen::checkout::{EXCEPTION_MESSAGE, EXCEPTION_STACKTRACE, EXCEPTION_TYPE};
use loctel_meshgen::{Scenario, generate};
use loctel_model::{AttrValue, Resource, Span, SpanEvent, SpanId, SpanKind, StatusCode, TraceId};
use loctel_store::{ManualClock, Store, StoreConfig};
use loctel_ui::app::{
    GlassRegion, INSET, MainWindow, NAV_ITEM_HEIGHT, PANEL_RADIUS, SIDEBAR_WIDTH, Screen,
    TOOLBAR_HEIGHT, TRAFFIC_LIGHT_SPACE, TimeRange, ToolbarPart, toolbar_title,
};
use loctel_ui::testing::{
    Driver, FixedBackend, RecordingBackdrop, assert_snapshot, render_window_png,
};
use loctel_ui::views::{
    COLUMNS, EMPTY_HINT, EMPTY_TITLE, HEADER_HEIGHT, INSPECTOR_WIDTH, LOGS_HEIGHT, ROW_HEIGHT,
    TablePart, TracePart, WATERFALL_ROW_HEIGHT, column_widths,
};
use loctel_ui::{StatusModel, Theme};

use common::{
    BASE, SEED, WINDOW, failing_trace_id, feed, home_store, open_trace, part_bounds, part_center,
    row_center, row_named, row_names, selected_details, toolbar, toolbar_center, trace_model,
    trace_view, waterfall_view,
};

/// A store with nothing in it, on a clock that never moves.
fn empty_store() -> Store {
    Store::new(StoreConfig::default(), Arc::new(ManualClock::new(0)))
}

/// An empty store and the status of a healthy engine: three listening
/// endpoints (OTLP gRPC 4317, OTLP HTTP 4318, MCP 4319) and no discard rules.
fn backend() -> Rc<FixedBackend> {
    Rc::new(FixedBackend::new(empty_store()))
}

fn trace_id() -> TraceId {
    TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").unwrap()
}

fn hsla(color: gpui::Rgba) -> Hsla {
    color.into()
}

#[test]
fn shell_dark() {
    let backend = backend();
    let image = render_window_png(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    // The window is 1440 by 900 logical pixels, drawn at scale 2.
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("shell_dark", &image);
}

#[test]
fn shell_light() {
    let backend = backend();
    let image = render_window_png(WINDOW, &Theme::light(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("shell_light", &image);
}

#[test]
fn shell_reports_glass_regions_once() {
    let backdrop = RecordingBackdrop::default();
    let backend = backend();
    let recorder = backdrop.clone();
    let mut driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, Some(Box::new(recorder)), window, cx))
    });

    // Two frames: the second finds nothing changed and reports nothing.
    driver.draw();
    driver.draw();

    let calls = backdrop.calls();
    assert_eq!(calls.len(), 1, "one report for two frames: {calls:?}");
    let regions = &calls[0];
    assert_eq!(regions.len(), 2, "the sidebar and the toolbar: {regions:?}");
    for region in regions {
        assert!(
            region.bounds.size.width > px(0.0) && region.bounds.size.height > px(0.0),
            "{region:?} has no area"
        );
        assert_eq!(region.corner_radius, px(PANEL_RADIUS));
    }

    // The plan's numbers: a floating sidebar 220 wide inset 10 from the window,
    // and the toolbar to its right on the same inset.
    let (width, height) = (WINDOW.0 as f32, WINDOW.1 as f32);
    let sidebar = Bounds::new(
        point(px(INSET), px(INSET)),
        size(px(SIDEBAR_WIDTH), px(height - 2.0 * INSET)),
    );
    let toolbar_left = INSET + SIDEBAR_WIDTH + INSET;
    let toolbar = Bounds::new(
        point(px(toolbar_left), px(INSET)),
        size(px(width - toolbar_left - INSET), px(TOOLBAR_HEIGHT)),
    );
    assert_eq!(regions[0].bounds, sidebar, "the sidebar");
    assert_eq!(regions[1].bounds, toolbar, "the toolbar");
}

#[test]
fn glass_regions_follow_a_resize() {
    let backdrop = RecordingBackdrop::default();
    let backend = backend();
    let recorder = backdrop.clone();
    let mut driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, Some(Box::new(recorder)), window, cx))
    });
    driver.draw();
    driver.resize((1200, 700));
    driver.draw();
    driver.draw();

    let calls = backdrop.calls();
    assert_eq!(calls.len(), 2, "one report per layout: {calls:?}");
    assert_eq!(
        calls[1][0].bounds.size.height,
        px(700.0 - 2.0 * INSET),
        "the sidebar follows the window"
    );
    assert_eq!(
        calls[1][1].bounds.size.width,
        px(1200.0 - 2.0 * INSET - SIDEBAR_WIDTH - INSET),
        "the toolbar follows the window"
    );
}

/// What the compositor puts behind a window with glass parts on a Mac: the
/// desktop. The headless renderer cannot draw Apple's glass, and its capture is
/// always opaque (it clears to black), so this stands in for the glass:
/// a flat colour that is nothing like the window's own, painted under the
/// window. Where the shell leaves the window see-through, the colour shows.
/// Real glass is dark under a dark appearance and pale under a light one, and so
/// is the stand-in (the text has to read on it).
fn desktop(theme: &Theme) -> gpui::Rgba {
    if theme.dark {
        gpui::rgb(0x2a4663)
    } else {
        gpui::rgb(0xc4d6e8)
    }
}

struct Desktop {
    main: gpui::Entity<MainWindow>,
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
            .child(self.main.clone())
    }
}

/// The window as `glass::attach` leaves it on a Mac, as far as a headless
/// window can show it: a backdrop that has been handed the regions, over the
/// [`desktop`] stand-in.
fn glass_capture(theme: &Theme) -> image::RgbaImage {
    let backdrop = RecordingBackdrop::default();
    let backend = backend();
    let color = desktop(theme);
    let image = render_window_png(WINDOW, theme, move |window, cx| {
        let main = cx.new(|cx| MainWindow::new(backend, Some(Box::new(backdrop)), window, cx));
        cx.new(|_| Desktop { main, color })
    });
    assert_eq!(image.dimensions(), (2880, 1800));
    image
}

/// The pixel at a position of a 1440 by 900 window, in logical pixels (the
/// capture is drawn at scale 2).
fn pixel_at(image: &image::RgbaImage, x: f32, y: f32) -> image::Rgba<u8> {
    *image.get_pixel((x * 2.0) as u32, (y * 2.0) as u32)
}

/// A colour as the 8 bit channels of an opaque pixel.
fn opaque(color: gpui::Rgba) -> image::Rgba<u8> {
    let byte = |channel: f32| (channel * 255.0).round() as u8;
    image::Rgba([byte(color.r), byte(color.g), byte(color.b), 255])
}

/// `line` (a translucent colour) drawn over `ground`, as 8 bit channels.
fn blended(line: gpui::Rgba, ground: gpui::Rgba) -> [u8; 3] {
    let mix = |over: f32, under: f32| ((over * line.a + under * (1.0 - line.a)) * 255.0).round();
    [
        mix(line.r, ground.r) as u8,
        mix(line.g, ground.g) as u8,
        mix(line.b, ground.b) as u8,
    ]
}

/// Whether `found` is `expected` give or take rounding in the renderer.
fn close(found: image::Rgba<u8>, expected: [u8; 3]) -> bool {
    (0..3).all(|channel| found[channel].abs_diff(expected[channel]) <= 2)
}

#[test]
fn shell_glass_dark() {
    assert_snapshot("shell_glass_dark", &glass_capture(&Theme::dark()));
}

#[test]
fn shell_glass_light() {
    assert_snapshot("shell_glass_light", &glass_capture(&Theme::light()));
}

#[test]
fn glass_parts_show_what_is_behind_and_the_rest_does_not() {
    for theme in [Theme::dark(), Theme::light()] {
        let image = glass_capture(&theme);
        let behind = desktop(&theme);
        let at = |x: f32, y: f32| pixel_at(&image, x, y);

        // The sidebar and the toolbar paint nothing of their own where they
        // are empty: what is behind the window shows through. That includes
        // Kit's root and the window colour, which would otherwise cover it.
        assert_eq!(at(120.0, 300.0), opaque(behind), "inside the sidebar");
        assert_eq!(at(1000.0, 34.0), opaque(behind), "inside the toolbar");

        // The content panels are solid, and so is the window colour around
        // every panel: the gaps, the margin, and the slivers outside the
        // rounded corners of the glass parts.
        assert_eq!(at(250.0, 80.0), opaque(theme.panel), "the map panel");
        assert_eq!(at(300.0, 860.0), opaque(theme.panel), "the table panel");
        for (name, x, y) in [
            ("between the sidebar and the content", 235.0, 400.0),
            ("between the toolbar and the map", 800.0, 63.0),
            ("between the map and the table", 800.0, 499.0),
            ("the margin", 5.0, 5.0),
            ("outside the sidebar's rounded corner", 11.0, 11.0),
            ("outside the toolbar's rounded corner", 241.0, 11.0),
        ] {
            assert_eq!(at(x, y), opaque(theme.bg), "{name} at ({x}, {y})");
        }

        // Each glass part has a hairline round it and, just inside it, a
        // highlight: both translucent, so what is behind shows through them.
        for (x, y) in [(10.5, 400.0), (800.0, 10.5)] {
            let expected = blended(theme.glass_hairline(), behind);
            assert!(
                close(at(x, y), expected),
                "the hairline at ({x}, {y}) is {:?}, not {expected:?}",
                at(x, y)
            );
        }
        for (x, y) in [(11.5, 400.0), (800.0, 11.5)] {
            let expected = blended(theme.glass_highlight(), behind);
            assert!(
                close(at(x, y), expected),
                "the highlight at ({x}, {y}) is {:?}, not {expected:?}",
                at(x, y)
            );
        }
    }
}

#[test]
fn without_a_backdrop_the_panels_have_their_fill() {
    // No glass behind the window (Linux, or the Blurred fallback of a Mac):
    // the sidebar and the toolbar are panels, and the window colour is
    // painted everywhere.
    for theme in [Theme::dark(), Theme::light()] {
        let backend = backend();
        let color = desktop(&theme);
        let image = render_window_png(WINDOW, &theme, move |window, cx| {
            let main = cx.new(|cx| MainWindow::new(backend, None, window, cx));
            cx.new(|_| Desktop { main, color })
        });
        let at = |x: f32, y: f32| pixel_at(&image, x, y);
        assert_eq!(at(120.0, 300.0), opaque(theme.panel), "the sidebar");
        assert_eq!(at(1000.0, 34.0), opaque(theme.panel), "the toolbar");
        assert_eq!(at(235.0, 400.0), opaque(theme.bg), "the gap");
        assert_eq!(at(11.0, 11.0), opaque(theme.bg), "outside the corner");
    }
}

#[test]
fn appearance_switch_changes_theme() {
    let backend = backend();
    // Start light, so that each switch below really changes the colour.
    let mut driver = Driver::new(WINDOW, &Theme::light(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    assert_eq!(
        driver.read(|cx| cx.theme().background),
        hsla(Theme::light().bg)
    );

    driver.update_root(|main, _window, cx| main.set_appearance(WindowAppearance::Dark, cx));
    assert_eq!(
        driver.read(|cx| cx.theme().background),
        hsla(Theme::dark().bg),
        "Dark switches the Kit's background to ours"
    );
    assert_eq!(driver.read(|cx| *cx.global::<Theme>()), Theme::dark());

    driver.update_root(|main, _window, cx| main.set_appearance(WindowAppearance::Light, cx));
    assert_eq!(
        driver.read(|cx| cx.theme().background),
        hsla(Theme::light().bg),
        "Light switches it back"
    );

    // The vibrant variants are the same two looks.
    driver.update_root(|main, _window, cx| main.set_appearance(WindowAppearance::VibrantDark, cx));
    assert_eq!(
        driver.read(|cx| cx.theme().background),
        hsla(Theme::dark().bg)
    );
}

#[test]
fn appearance_picks_the_theme() {
    assert_eq!(Theme::for_appearance(WindowAppearance::Dark), Theme::dark());
    assert_eq!(
        Theme::for_appearance(WindowAppearance::VibrantDark),
        Theme::dark()
    );
    assert_eq!(
        Theme::for_appearance(WindowAppearance::Light),
        Theme::light()
    );
    assert_eq!(
        Theme::for_appearance(WindowAppearance::VibrantLight),
        Theme::light()
    );
}

#[test]
fn kit_theme_carries_our_tokens() {
    let backend = backend();
    for theme in [Theme::dark(), Theme::light()] {
        let backend = Rc::clone(&backend);
        let driver = Driver::new(WINDOW, &theme, move |window, cx| {
            cx.new(|cx| MainWindow::new(backend, None, window, cx))
        });
        driver.read(|cx| {
            let kit = cx.theme();
            assert_eq!(kit.background, hsla(theme.bg));
            assert_eq!(kit.foreground, hsla(theme.text));
            assert_eq!(kit.border, hsla(theme.border));
            assert_eq!(kit.primary, hsla(theme.accent));
            assert_eq!(kit.font_family.as_ref(), loctel_ui::FONT_FAMILY);
        });
    }
}

#[test]
fn navigate_changes_the_screen_and_the_toolbar() {
    let backend = backend();
    let mut driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    assert_eq!(
        driver.update_root(|main, _, _| main.screen().clone()),
        Screen::Home
    );

    driver.update_root(|main, _, cx| main.navigate(Screen::Trace(trace_id()), cx));
    assert_eq!(
        driver.update_root(|main, _, _| main.screen().clone()),
        Screen::Trace(trace_id())
    );

    assert_eq!(toolbar_title(&Screen::Home), "Map & traces");
    assert_eq!(
        toolbar_title(&Screen::Trace(trace_id())),
        "Map & traces \u{203a} 4bf92f35\u{2026}0e0e4736"
    );
}

#[test]
fn clicking_the_nav_item_goes_home() {
    let backend = backend();
    let mut driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    driver.update_root(|main, _, cx| main.navigate(Screen::Trace(trace_id()), cx));
    driver.draw();

    // The centre of the one navigation item: below the traffic-light space of
    // the sidebar, in its column.
    let centre = point(
        px(INSET + SIDEBAR_WIDTH / 2.0),
        px(INSET + TRAFFIC_LIGHT_SPACE + NAV_ITEM_HEIGHT / 2.0),
    );
    driver.click(centre);

    assert_eq!(
        driver.update_root(|main, _, _| main.screen().clone()),
        Screen::Home
    );
}

#[test]
fn the_status_box_reads_the_backend() {
    let backend = backend();
    let driver = Driver::new(WINDOW, &Theme::dark(), {
        let backend = Rc::clone(&backend);
        move |window, cx| cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    let shown: StatusModel = driver.read_root(|main| main.status().clone());
    assert_eq!(shown, StatusModel::from_status(&backend.status));
    assert_eq!(shown.listeners.len(), 3);
    assert!(shown.listeners.iter().all(|listener| listener.ok));
    assert_eq!(shown.rules_on, 0);
}

#[test]
fn the_status_box_follows_the_engine() {
    use loctel_ui::app::UiBackend as _;
    use std::cell::RefCell;

    // A backend whose status can be changed from the test.
    struct Changing {
        store: Store,
        paused: RefCell<bool>,
    }
    impl loctel_ui::app::UiBackend for Changing {
        fn store(&self) -> Store {
            self.store.clone()
        }
        fn status(&self) -> loctel_engine::EngineStatus {
            let mut status = FixedBackend::new(self.store.clone()).status;
            status.paused = *self.paused.borrow();
            status
        }
        fn clear_all(&self) {
            self.store.clear();
        }
        fn utc_offset(&self) -> i32 {
            0
        }
    }

    let changing = Rc::new(Changing {
        store: empty_store(),
        paused: RefCell::new(false),
    });
    assert!(!changing.status().paused);
    let mut driver = Driver::new(WINDOW, &Theme::dark(), {
        let changing = Rc::clone(&changing);
        move |window, cx| cx.new(|cx| MainWindow::new(changing, None, window, cx))
    });
    assert!(!driver.read_root(|main| main.status().paused));

    *changing.paused.borrow_mut() = true;
    driver.advance(std::time::Duration::from_millis(1_100));
    assert!(
        driver.read_root(|main| main.status().paused),
        "the status box refreshes about once a second"
    );
}

#[test]
fn glass_region_is_plain_data() {
    // The type a macOS backdrop will receive: bounds and a corner radius.
    let region = GlassRegion {
        bounds: Bounds::new(point(px(1.0), px(2.0)), size(px(3.0), px(4.0))),
        corner_radius: px(16.0),
    };
    assert_eq!(region.clone(), region);
}

// --- the home toolbar ---------------------------------------------------------
//
// Live, the time range, Pause and Clear all (design 9.2). The references are
// crops of the window, 2x like the others: the strip of the toolbar (and, with
// the range menu open, what hangs under it), from the gap left of the toolbar
// to the window's right edge.

/// The window over the home data, with its first frame drawn.
fn toolbar_driver(theme: &Theme) -> Driver<MainWindow> {
    let backend = Rc::new(FixedBackend::new(home_store()));
    Driver::new(WINDOW, theme, move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    })
}

/// The toolbar's strip of `image` (a capture of the window) and `below`
/// logical pixels under it, with the window's inset round it.
fn toolbar_crop(image: &image::RgbaImage, below: f32) -> image::RgbaImage {
    let scale = image.width() as f32 / WINDOW.0 as f32;
    let left = SIDEBAR_WIDTH + INSET;
    let height = INSET + TOOLBAR_HEIGHT + below;
    image::imageops::crop_imm(
        image,
        (left * scale) as u32,
        0,
        ((WINDOW.0 as f32 - left) * scale) as u32,
        (height * scale) as u32,
    )
    .to_image()
}

/// The room under the toolbar the open range menu takes, with its shadow.
const MENU_ROOM: f32 = 190.0;

fn toolbar_running(theme: &Theme, name: &str) {
    let mut driver = toolbar_driver(theme);
    let image = toolbar_crop(&driver.capture(), INSET);
    assert_snapshot(name, &image);
}

fn toolbar_menu(theme: &Theme, name: &str) {
    let mut driver = toolbar_driver(theme);
    driver.click(toolbar_center(&driver, ToolbarPart::Range));
    let image = toolbar_crop(&driver.capture(), MENU_ROOM);
    assert_snapshot(name, &image);
}

fn toolbar_paused(theme: &Theme, name: &str) {
    let mut driver = toolbar_driver(theme);
    driver.click(toolbar_center(&driver, ToolbarPart::Pause));
    let image = toolbar_crop(&driver.capture(), INSET);
    assert_snapshot(name, &image);
}

#[test]
fn toolbar_dark() {
    toolbar_running(&Theme::dark(), "toolbar_dark");
}

#[test]
fn toolbar_light() {
    toolbar_running(&Theme::light(), "toolbar_light");
}

#[test]
fn toolbar_menu_dark() {
    toolbar_menu(&Theme::dark(), "toolbar_menu_dark");
}

#[test]
fn toolbar_menu_light() {
    toolbar_menu(&Theme::light(), "toolbar_menu_light");
}

#[test]
fn toolbar_paused_dark() {
    toolbar_paused(&Theme::dark(), "toolbar_paused_dark");
}

#[test]
fn toolbar_paused_light() {
    toolbar_paused(&Theme::light(), "toolbar_paused_light");
}

#[test]
fn the_toolbar_shows_the_range_the_model_has() {
    // The button says the range: a range set on the model (not by a click) is
    // what the next frame shows, so the control cannot disagree with the data.
    let mut driver = toolbar_driver(&Theme::dark());
    let home = driver.read_root(|main| main.home().clone());
    let before = driver.capture();
    driver.update_root(|_, _, cx| {
        home.update(cx, |home, cx| home.set_range(TimeRange::Everything, cx))
    });
    let after = driver.capture();
    assert_ne!(
        toolbar_crop(&before, INSET).as_raw(),
        toolbar_crop(&after, INSET).as_raw(),
        "`Everything` is a different label from `Last 15 minutes`"
    );
}

#[test]
fn the_trace_screen_has_a_toolbar_with_only_the_title() {
    // The controls belong to the home screen: on the trace screen the toolbar
    // is the title alone (the references `trace_dark` and `trace_light` did not
    // change when the controls came), and back home they are there again.
    let mut driver = toolbar_driver(&Theme::dark());
    let controls = toolbar(&driver);
    let has = |driver: &Driver<MainWindow>, part| {
        driver.read(|cx| controls.read(cx).bounds_of(part).is_some())
    };
    for part in [ToolbarPart::Live, ToolbarPart::Pause, ToolbarPart::ClearAll] {
        assert!(has(&driver, part), "{part:?} on the home screen");
    }
    open_trace(&mut driver, failing_trace_id());
    for part in [
        ToolbarPart::Live,
        ToolbarPart::Range,
        ToolbarPart::Pause,
        ToolbarPart::ClearAll,
    ] {
        assert!(!has(&driver, part), "no {part:?} on the trace screen");
    }
    driver.update_root(|main, _, cx| main.navigate(Screen::Home, cx));
    driver.draw();
    assert!(has(&driver, ToolbarPart::Pause), "back on the home screen");
}

// --- the home screen: the service map -----------------------------------------

/// The window over the home data: the failing and the successful checkout and
/// a health check of each service (`common::home_store`).
fn home_window(theme: &Theme) -> image::RgbaImage {
    let backend = Rc::new(FixedBackend::new(home_store()));
    render_window_png(WINDOW, theme, move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    })
}

#[test]
fn home_map_dark() {
    let image = home_window(&Theme::dark());
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("home_dark", &image);
}

#[test]
fn home_map_light() {
    let image = home_window(&Theme::light());
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("home_light", &image);
}

#[test]
fn home_map_has_every_node_of_the_checkout() {
    let backend = Rc::new(FixedBackend::new(home_store()));
    let driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    let home = driver.read_root(|main| main.home().clone());
    let map = driver.read_root(|main| main.map().clone());
    let ids: Vec<String> = driver.read(|cx| {
        home.read(cx)
            .data()
            .map
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect()
    });
    for id in [
        "web-bff",
        "orders-api",
        "payments-api",
        "inventory-api",
        "rabbitmq",
        "postgresql",
        "redis",
        "card-gateway",
        "notifications-worker",
    ] {
        assert!(ids.iter().any(|node| node == id), "no node {id}: {ids:?}");
        let centre = driver.read(|cx| map.read(cx).node_center(id, cx));
        let centre = centre.unwrap_or_else(|| panic!("`{id}` is not on the screen"));
        assert!(
            centre.x > px(0.0) && centre.x < px(WINDOW.0 as f32),
            "{id} at {centre:?}"
        );
        assert!(
            centre.y > px(0.0) && centre.y < px(WINDOW.1 as f32),
            "{id} at {centre:?}"
        );
    }
}

#[test]
fn empty_map_message() {
    // Nothing received yet: the map says what to do about it.
    let backend = backend();
    let driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    let map = driver.read_root(|main| main.map().clone());
    assert_eq!(
        driver.read(|cx| map.read(cx).empty_message(cx)),
        Some((EMPTY_TITLE, EMPTY_HINT))
    );
    assert_eq!(EMPTY_TITLE, "Waiting for telemetry");
    assert_eq!(
        EMPTY_HINT,
        "Point OTEL_EXPORTER_OTLP_ENDPOINT at http://localhost:4317"
    );

    // With traces in the store there is a map and no message.
    let backend = Rc::new(FixedBackend::new(home_store()));
    let driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    let map = driver.read_root(|main| main.map().clone());
    assert_eq!(driver.read(|cx| map.read(cx).empty_message(cx)), None);
}

// --- the home screen: the trace table ------------------------------------------

#[test]
fn the_table_has_the_planned_columns_and_rows() {
    assert_eq!(
        COLUMNS,
        [
            "Started",
            "Trace",
            "Failed in",
            "Services",
            "Spans",
            "Duration"
        ]
    );
    assert_eq!(ROW_HEIGHT, 36.0, "the plan's row height");
    const { assert!(HEADER_HEIGHT < ROW_HEIGHT) };
}

#[test]
fn the_columns_share_the_width_of_the_panel() {
    // The width the columns can have in a window of each size: they fit it
    // (never a horizontal scroll) and use it, down to the smallest window.
    for inner in [730.0, 748.0, 900.0, 1188.0, 1600.0, 2400.0] {
        let widths = column_widths(inner);
        let total: f32 = widths.iter().sum();
        assert!(total <= inner, "{inner}: the columns ({total}) fit");
        assert!(total >= inner - 12.0 - 2.0, "{inner}: and use it ({total})");
        assert!(widths.iter().all(|&w| w >= 56.0), "{inner}: {widths:?}");
    }
    // The two that share what the fixed ones leave: the failure text has more.
    let widths = column_widths(1188.0);
    assert!(
        widths[2] > widths[1],
        "Failed in is wider than Trace: {widths:?}"
    );
}

#[test]
fn the_table_panel_holds_the_query_bar_toggles_and_table() {
    let backend = Rc::new(FixedBackend::new(home_store()));
    let driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    let table = driver.read_root(|main| main.table().clone());
    let map = driver.read_root(|main| main.map().clone());
    let bounds = |part| {
        driver
            .read(|cx| table.read(cx).bounds_of(part))
            .unwrap_or_else(|| panic!("no {part:?} on the screen"))
    };
    let (input, run) = (bounds(TablePart::QueryInput), bounds(TablePart::Run));
    let (errors, slow) = (bounds(TablePart::ErrorsOnly), bounds(TablePart::SlowOnly));
    let list = bounds(TablePart::Table);
    let map_area = driver.read(|cx| map.read(cx).view_bounds());

    // Top to bottom: the map, the query row, the toggle row, the table.
    assert!(input.origin.y >= map_area.origin.y + map_area.size.height);
    assert!(errors.origin.y >= input.origin.y + input.size.height);
    assert!(list.origin.y >= errors.origin.y + errors.size.height);
    // The input and the button share a row, the toggles another.
    assert_eq!(input.origin.y, run.origin.y, "same row");
    assert!(input.origin.x + input.size.width <= run.origin.x);
    assert_eq!(errors.origin.y, slow.origin.y, "same row");
    assert!(errors.origin.x + errors.size.width <= slow.origin.x);
    // The table fills the rest of the panel, down to its bottom edge (the
    // window's inset away).
    let window_bottom = px(WINDOW.1 as f32 - INSET);
    assert!(
        list.origin.y + list.size.height >= window_bottom - px(2.0),
        "the table reaches the bottom of its panel: {list:?}"
    );
    assert!(list.origin.y + list.size.height <= window_bottom);
}

// --- the trace screen ------------------------------------------------------------------

/// The window over the home data, with the failing checkout opened.
fn trace_driver(theme: &Theme) -> Driver<MainWindow> {
    let backend = Rc::new(FixedBackend::new(home_store()));
    let mut driver = Driver::new(WINDOW, theme, move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    open_trace(&mut driver, failing_trace_id());
    driver
}

#[test]
fn trace_dark() {
    let mut driver = trace_driver(&Theme::dark());
    assert_eq!(
        driver.update_root(|main, _, _| main.screen().clone()),
        Screen::Trace(failing_trace_id())
    );

    // The failed-in span is selected and the inspector shows it: payments-api's
    // first call to the card gateway.
    let details = selected_details(&driver).expect("a span is selected");
    assert_eq!(details.name, "POST");
    assert_eq!(details.service, "payments-api");
    assert_eq!(details.offset, "starts at +236 ms");
    assert!(details.status.starts_with("Error"), "{}", details.status);

    let image = driver.capture();
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("trace_dark", &image);
}

#[test]
fn trace_light() {
    let mut driver = trace_driver(&Theme::light());
    let details = selected_details(&driver).expect("a span is selected");
    assert_eq!(details.name, "POST");
    assert_eq!(details.service, "payments-api");

    let image = driver.capture();
    assert_eq!(image.dimensions(), (2880, 1800));
    assert_snapshot("trace_light", &image);
}

#[test]
fn the_trace_screen_has_the_planned_layout() {
    let driver = trace_driver(&Theme::dark());
    let window = WINDOW.0 as f32;

    // The inspector is 340 wide on the right of the content, the waterfall's
    // list fills the rest, side by side.
    let inspector = part_bounds(&driver, TracePart::Inspector).expect("the inspector is drawn");
    let list = part_bounds(&driver, TracePart::List).expect("the waterfall is drawn");
    assert_eq!(INSPECTOR_WIDTH, 340.0);
    assert!(
        (f32::from(inspector.size.width) - INSPECTOR_WIDTH).abs() <= 1.5,
        "the inspector is {:?} wide",
        inspector.size.width
    );
    assert!(list.origin.x + list.size.width <= inspector.origin.x + px(1.0));
    // The content panel ends INSET and a hairline from the window's edge.
    let right = inspector.origin.x + inspector.size.width;
    assert!(
        right <= px(window - INSET) && right >= px(window - INSET - 2.0),
        "the inspector reaches the panel's edge: {right:?}"
    );

    // The logs sit under the waterfall's list, about 200 px tall, and the
    // header over both.
    let back = part_bounds(&driver, TracePart::Back).expect("the back button is drawn");
    assert!(back.origin.y < list.origin.y);
    assert_eq!(LOGS_HEIGHT, 200.0);
    assert_eq!(WATERFALL_ROW_HEIGHT, 28.0, "the plan's row height");
    let bottom = list.origin.y + list.size.height + px(1.0 + LOGS_HEIGHT);
    assert!(
        (bottom - px(WINDOW.1 as f32 - INSET)).abs() <= px(2.0),
        "list, hairline and logs fill the panel to its bottom: {bottom:?}"
    );
}

#[test]
fn the_waterfall_lists_the_checkout_in_tree_order() {
    let driver = trace_driver(&Theme::dark());
    let names = row_names(&driver);
    assert_eq!(names.len(), 15);
    assert_eq!(names[0], "POST /api/checkout");
    assert_eq!(names[1], "validate_token");
    assert_eq!(names[14], "order.failed process");
}

#[test]
fn trace_charges_selected_dark() {
    let mut driver = trace_driver(&Theme::dark());
    let charges = row_named(&driver, "POST /charges");

    driver.click(row_center(&driver, charges));
    driver.draw();

    // The inspector shows the span and its exception card, with the stack
    // trace; its logs are the four of the span.
    let details = selected_details(&driver).expect("a span is selected");
    assert_eq!(details.name, "POST /charges");
    assert_eq!(details.service, "payments-api");
    assert_eq!(details.offset, "starts at +200 ms");
    let exception = details.exception.expect("the span has an exception");
    assert_eq!(exception.type_name, EXCEPTION_TYPE);
    assert_eq!(exception.message, EXCEPTION_MESSAGE);
    assert_eq!(exception.stacktrace.as_deref(), Some(EXCEPTION_STACKTRACE));
    let model = trace_model(&driver);
    assert_eq!(driver.read(|cx| model.read(cx).logs().len()), 4);
    assert!(
        part_bounds(&driver, TracePart::CopyException).is_some(),
        "the card has its Copy button"
    );

    assert_snapshot("trace_charges_selected_dark", &driver.capture());
}

#[test]
fn trace_errors_path_only_dark() {
    let mut driver = trace_driver(&Theme::dark());
    assert_eq!(row_names(&driver).len(), 15);

    driver.click(part_center(&driver, TracePart::ErrorsPath));

    // The seven erroring spans and nothing else (every span above one is
    // itself erroring in this trace): the way from the root to the gateway.
    assert_eq!(
        row_names(&driver),
        [
            "POST /api/checkout",
            "POST",
            "POST /orders",
            "POST",
            "POST /charges",
            "POST",
            "POST",
        ]
    );
    // The selected span (the failed-in one) is on the path and stays.
    let details = selected_details(&driver).expect("still selected");
    assert_eq!(details.offset, "starts at +236 ms");
    assert_snapshot("trace_errors_path_only_dark", &driver.capture());
}

#[test]
fn an_erroring_row_has_its_marker_and_the_exception_its_diamond() {
    // The rows of the failing checkout that error say so with a symbol, not
    // only a colour; the screenshot shows it, the model says which.
    let driver = trace_driver(&Theme::dark());
    let model = trace_model(&driver);
    driver.read(|cx| {
        let rows = model.read(cx).waterfall().unwrap().rows();
        let erroring = rows.iter().filter(|row| row.is_error).count();
        assert_eq!(erroring, 7);
        let diamonds: usize = rows.iter().map(|row| row.exception_fracs.len()).sum();
        assert_eq!(diamonds, 1, "the one exception event, on /charges");
    });
}

#[test]
fn a_missing_trace_says_so_and_can_still_go_back() {
    let backend = Rc::new(FixedBackend::new(home_store()));
    let mut driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    open_trace(&mut driver, TraceId([0xab; 16]));

    let model = trace_model(&driver);
    assert_eq!(
        driver.read(|cx| model.read(cx).state()),
        loctel_ui::app::LoadState::Missing
    );
    let back = part_center(&driver, TracePart::Back);
    driver.click(back);
    assert_eq!(
        driver.update_root(|main, _, _| main.screen().clone()),
        Screen::Home
    );
}

// --- big and awkward traces ---------------------------------------------------------------

/// A store holding `HugeTrace { spans }` on a clock that has gone a minute on.
fn huge_store(spans: usize) -> (Store, TraceId) {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    let generated = generate(&Scenario::HugeTrace { spans }, SEED, BASE);
    feed(&store, &generated);
    clock.advance(std::time::Duration::from_secs(60));
    (store, generated.trace_ids[0])
}

#[test]
fn huge_trace_scrolls_to_middle() {
    let (store, id) = huge_store(100_000);
    let backend = Rc::new(FixedBackend::new(store));
    let mut driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    open_trace(&mut driver, id);

    let model = trace_model(&driver);
    let rows = driver.read(|cx| model.read(cx).waterfall().unwrap().rows().len());
    assert_eq!(rows, 100_000, "every span has a row");
    let waterfall = waterfall_view(&driver);
    assert_eq!(driver.read(|cx| waterfall.read(cx).top_row()), 0);

    // To row 50,000: the list puts it in the middle of its viewport.
    driver.update_root(|main, _, cx| {
        let view = main.trace_view().expect("open").clone();
        let waterfall = view.read(cx).waterfall_view().clone();
        waterfall.update(cx, |waterfall, _| waterfall.scroll_to_row(50_000));
    });
    driver.draw();
    driver.draw();

    let top = driver.read(|cx| waterfall.read(cx).top_row());
    let in_view = (top..top + 40).contains(&50_000);
    assert!(
        in_view,
        "row 50,000 is in the viewport, which starts at {top}"
    );
    assert!(
        top > 49_900,
        "and the list really is at the middle, not at the top: {top}"
    );
    let middle = driver.read(|cx| waterfall.read(cx).row_center(50_000));
    assert!(middle.is_some(), "the row is on the screen");

    assert_snapshot("huge_trace_middle_dark", &driver.capture());
}

/// A trace with one very awkward span (a 300-character name that has no
/// break in it, a 20 KB attribute with spaces, a 600-character value without
/// any) over a short normal child.
fn awkward_store() -> (Store, TraceId) {
    let trace = TraceId([0x7e; 16]);
    let resource = Arc::new(Resource {
        attributes: vec![(
            "service.name".to_owned(),
            AttrValue::Str("web-bff".to_owned()),
        )],
    });
    let long_name: String = std::iter::repeat_n("GET/very-long-route-segment", 12)
        .collect::<Vec<_>>()
        .join("/")
        .chars()
        .take(300)
        .collect();
    assert_eq!(long_name.chars().count(), 300);
    let payload = "the quick brown fox jumps over the lazy dog ".repeat(460);
    assert!(payload.len() >= 20_000);
    let root = Span {
        trace_id: trace,
        span_id: SpanId([1; 8]),
        parent_span_id: None,
        name: long_name,
        kind: SpanKind::Server,
        start_unix_nano: BASE,
        end_unix_nano: BASE + 800_000_000,
        status: StatusCode::Error,
        status_message: "a status message that goes on and on ".repeat(8),
        attributes: vec![
            ("payload".to_owned(), AttrValue::Str(payload)),
            (
                "session.token".to_owned(),
                AttrValue::Str("tok_".to_owned() + &"x9".repeat(300)),
            ),
            ("http.route".to_owned(), AttrValue::Str("/api/x".to_owned())),
        ],
        events: vec![SpanEvent {
            time_unix_nano: BASE + 700_000_000,
            name: "exception".to_owned(),
            attributes: vec![
                (
                    "exception.type".to_owned(),
                    AttrValue::Str(
                        "Some.Very.Long.Namespace.WithAnExceptionNameThatKeepsGoing".to_owned(),
                    ),
                ),
                (
                    "exception.message".to_owned(),
                    AttrValue::Str("m".repeat(400)),
                ),
                (
                    "exception.stacktrace".to_owned(),
                    AttrValue::Str(format!("   at {}\n   at short", "Frame.".repeat(60))),
                ),
            ],
        }],
        resource: Arc::clone(&resource),
        scope_name: "test".to_owned(),
    };
    let child = Span {
        span_id: SpanId([2; 8]),
        parent_span_id: Some(SpanId([1; 8])),
        name: "child".to_owned(),
        kind: SpanKind::Internal,
        start_unix_nano: BASE + 100_000_000,
        end_unix_nano: BASE + 300_000_000,
        status: StatusCode::Unset,
        status_message: String::new(),
        attributes: Vec::new(),
        events: Vec::new(),
        ..root.clone()
    };
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    store.ingest_spans(vec![root, child]);
    clock.advance(std::time::Duration::from_secs(60));
    (store, trace)
}

#[test]
fn long_values_do_not_break_layout() {
    let (store, id) = awkward_store();
    let backend = Rc::new(FixedBackend::new(store));
    let mut driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    open_trace(&mut driver, id);

    // The long span is the root, and the failed-in span: selected.
    let details = selected_details(&driver).expect("selected");
    assert_eq!(details.name.chars().count(), 300);

    // The layout is the normal one: the inspector 340 wide at the panel's right
    // edge, the list beside it, nothing pushed out of the window.
    let check = |driver: &Driver<MainWindow>, when: &str| {
        let inspector = part_bounds(driver, TracePart::Inspector).expect("the inspector");
        let list = part_bounds(driver, TracePart::List).expect("the list");
        assert!(
            (f32::from(inspector.size.width) - INSPECTOR_WIDTH).abs() <= 1.5,
            "{when}: the inspector is {:?} wide",
            inspector.size.width
        );
        let right = inspector.origin.x + inspector.size.width;
        assert!(
            right <= px(WINDOW.0 as f32 - INSET),
            "{when}: the inspector ends at {right:?}"
        );
        assert!(
            list.origin.x + list.size.width <= inspector.origin.x + px(1.0),
            "{when}: the list is beside the inspector"
        );
        let back = part_bounds(driver, TracePart::Back).expect("the header");
        assert!(
            back.origin.x < px(WINDOW.0 as f32 / 2.0),
            "{when}: the header"
        );
    };
    check(&driver, "cut");
    assert_snapshot("long_values_dark", &driver.capture());

    // Showing a long value whole makes the pane longer, never wider. The links
    // are far down a pane that scrolls: bring the last one into view first.
    let pane = part_center(&driver, TracePart::Inspector);
    driver.scroll(pane, 1_500.0);
    let show_all = part_center(&driver, TracePart::ShowAll);
    assert!(
        show_all.y < px(WINDOW.1 as f32),
        "the link scrolled into view: {show_all:?}"
    );
    driver.click(show_all);
    let inspector_view = trace_view(&driver);
    let expanded = driver.read(|cx| {
        inspector_view
            .read(cx)
            .inspector_view()
            .read(cx)
            .expanded_values()
    });
    assert_eq!(expanded, 1, "the click showed one value whole");
    check(&driver, "shown whole");
    assert_snapshot("long_values_shown_whole_dark", &driver.capture());
}
