//! The main window's screens: the shell (M3 plan, Task 4: the sidebar, the
//! toolbar and the content area) and the home screen with the service map above
//! the trace table (Task 6), rendered through GPUI Kit's `Root` in the headless
//! harness. The references are `snapshots/shell_*.png` and `home_*.png`; build
//! them with `cargo xtask screenshots --update` and look at them before
//! committing (`docs/areas/ui.md`).

mod common;

use std::rc::Rc;
use std::sync::Arc;

use gpui::{AppContext as _, Bounds, Hsla, WindowAppearance, point, px, size};
use gpui_kit::component::ActiveTheme as _;
use loctel_model::TraceId;
use loctel_store::{ManualClock, Store, StoreConfig};
use loctel_ui::app::{
    GlassRegion, INSET, MainWindow, NAV_ITEM_HEIGHT, PANEL_RADIUS, SIDEBAR_WIDTH, Screen,
    TOOLBAR_HEIGHT, TRAFFIC_LIGHT_SPACE, toolbar_title,
};
use loctel_ui::testing::{
    Driver, FixedBackend, RecordingBackdrop, assert_snapshot, render_window_png,
};
use loctel_ui::views::{EMPTY_HINT, EMPTY_TITLE};
use loctel_ui::{StatusModel, Theme};

use common::{WINDOW, home_store};

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
