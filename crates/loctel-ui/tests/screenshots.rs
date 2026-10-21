//! Pixel-exact screenshot tests of the status view, rendered through GPUI's
//! headless renderer. Build the references with
//! `UPDATE_SNAPSHOTS=1 cargo test -p loctel-ui --features screenshot`, and
//! look at the PNGs before committing them (`docs/areas/ui.md`).

use gpui::AppContext as _;
use image::RgbaImage;
use loctel_ui::testing::{assert_snapshot, render_png};
use loctel_ui::{ListenerLine, StatusModel, StatusView, Theme};

/// Three listeners, the HTTP one failed: the shape a developer sees when
/// something else already holds port 4318.
fn fixed_model() -> StatusModel {
    StatusModel {
        paused: false,
        listeners: vec![
            ListenerLine {
                name: "otlp-grpc".to_owned(),
                address: "127.0.0.1:4317".to_owned(),
                ok: true,
                error: None,
            },
            ListenerLine {
                name: "otlp-http".to_owned(),
                address: "127.0.0.1:4318".to_owned(),
                ok: false,
                error: Some("Address already in use (os error 98)".to_owned()),
            },
            ListenerLine {
                name: "mcp".to_owned(),
                address: "127.0.0.1:4319".to_owned(),
                ok: true,
                error: None,
            },
        ],
        spans_per_sec: 412.0,
        logs_per_sec: 1234.0,
        traces_held: 128,
        logs_held: 52_340,
    }
}

fn render(model: StatusModel, theme: Theme) -> RgbaImage {
    render_png((900, 600), move |_window, cx| {
        cx.new(|_| StatusView::new(model, theme)).into()
    })
}

#[test]
fn status_view_dark_matches_snapshot() {
    let image = render(fixed_model(), Theme::dark());
    assert_snapshot("status_view_dark", &image);
}

#[test]
fn status_view_light_matches_snapshot() {
    let mut model = fixed_model();
    model.paused = true;
    model.listeners[1].ok = true;
    model.listeners[1].error = None;
    let image = render(model, Theme::light());
    assert_snapshot("status_view_light", &image);
}

#[test]
fn render_is_deterministic() {
    let first = render(fixed_model(), Theme::dark());
    let second = render(fixed_model(), Theme::dark());
    assert_eq!(first.dimensions(), second.dimensions());
    assert_eq!(first.as_raw(), second.as_raw());
    // Two blank frames would be "deterministic" too.
    let background = *first.get_pixel(0, 0);
    assert!(
        first.pixels().any(|pixel| *pixel != background),
        "the render is a single flat colour"
    );
}
