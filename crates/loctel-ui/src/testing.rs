//! Headless rendering and pixel-exact snapshots, for the screenshot tests.
//! Compiled only with the `screenshot` feature.
//!
//! [`render_png`] draws a view through GPUI's headless renderer (wgpu on the
//! CPU, via lavapipe, so no GPU or display is needed) and hands back the
//! pixels. [`assert_snapshot`] compares them with the reference PNG in
//! `tests/snapshots/`. The same inputs give the same bytes: the font is
//! bundled and the renderer is software, so a reference made on one machine
//! holds on another. `docs/areas/ui.md` has the workflow.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyView, App, AppContext as _, Context, HeadlessAppContext, IntoElement, ParentElement,
    PlatformTextSystem, Render, Styled, Window, div, px,
};
use image::{Rgba, RgbaImage};

use crate::theme::{FONT_FAMILY, register_fonts};

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
