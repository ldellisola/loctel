//! The Settings window's lifecycle (M4 plan, Task 6): [`open_settings_window`]
//! opens it, 520 by 480 and not resizable, or focuses the one that is already
//! open. What it shows is [`SettingsView`](crate::views::SettingsView).

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, Global, TitlebarOptions, WindowBounds, WindowHandle,
    WindowOptions, px, size,
};
use gpui_kit::base::Root;

use super::backend::UiBackend;
use crate::views::{SETTINGS_TITLE, SettingsView};

/// The window's size, in logical pixels. It is as tall as the form needs with a
/// message under every field and a login item, the worst case
/// (`everything_fits_the_window_with_every_message_showing`); it was 360 until
/// the `Storage` section of Milestone 5 (decision 0052).
pub const SETTINGS_SIZE: (f32, f32) = (520.0, 480.0);

/// The Settings window, from when it is opened until it is closed. It is a
/// GPUI global so that every way into Settings (the main window's ⌘, and the
/// popover's `Settings…`) finds the same window.
#[derive(Default)]
struct SettingsWindowState {
    window: Option<WindowHandle<Root>>,
}

impl Global for SettingsWindowState {}

/// Opens the Settings window on `cx`: 520 by 480, centred, not resizable and
/// titled `Settings`, with the form of [`SettingsView`] over `backend` in GPUI
/// Kit's `Root`. The view takes the theme from the window's appearance, as the
/// main window does.
///
/// If a Settings window is already open it is brought to the front instead and
/// its handle returned, so asking twice never gives two windows; a window that
/// has been closed is forgotten, and asking again opens a new one.
///
/// Fails when the platform has no display to open a window on.
pub fn open_settings_window(
    backend: Rc<dyn UiBackend>,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<Root>> {
    if let Some(open) = cx.default_global::<SettingsWindowState>().window
        && cx.windows().contains(&open.into())
    {
        // `update` fails only for a window that is gone, which the check above
        // has just ruled out; either way the app is activated.
        let _ = open.update(cx, |_, window, _| window.activate_window());
        cx.activate(true);
        return Ok(open);
    }

    super::init(cx);
    let bounds = Bounds::centered(None, size(px(SETTINGS_SIZE.0), px(SETTINGS_SIZE.1)), cx);
    let handle = cx.open_window(options(bounds), move |window, cx| {
        let view = cx.new(|cx| SettingsView::new(backend, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })?;
    cx.default_global::<SettingsWindowState>().window = Some(handle);
    cx.activate(true);
    Ok(handle)
}

/// The window's options: `bounds`, the title, and no resizing.
fn options(bounds: Bounds<gpui::Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SETTINGS_TITLE.into()),
            ..TitlebarOptions::default()
        }),
        is_resizable: false,
        ..WindowOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use gpui::point;

    use super::*;

    #[test]
    fn the_window_is_520_by_480_titled_settings_and_cannot_be_resized() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(520.0), px(480.0)));
        let options = options(bounds);
        assert_eq!(options.window_bounds, Some(WindowBounds::Windowed(bounds)));
        assert!(!options.is_resizable);
        assert_eq!(
            options.titlebar.and_then(|titlebar| titlebar.title),
            Some("Settings".into())
        );
        assert_eq!(SETTINGS_SIZE, (520.0, 480.0));
    }
}
