//! The status item's icon: the three-node glyph from the mockups, as a
//! template image.
//!
//! The artwork is black on transparent at 18 and 36 pixels
//! (`assets/status-icon.png` and `status-icon@2x.png`, drawn from
//! `status-icon.svg` by `script/gen-status-icon.py`). A template image is
//! coloured by AppKit from its alpha channel, so it is dark on a light menu
//! bar, light on a dark one and inverts while the menu is open.

/// The 18 by 18 pixel PNG, for 1x displays.
pub(crate) const STATUS_ICON_1X: &[u8] = include_bytes!("../assets/status-icon.png");
/// The 36 by 36 pixel PNG, for Retina displays.
pub(crate) const STATUS_ICON_2X: &[u8] = include_bytes!("../assets/status-icon@2x.png");

/// The status item's icon: both PNGs as one `NSImage` (AppKit picks the
/// representation for the screen), 18 by 18 points, marked as a template.
#[cfg(target_os = "macos")]
pub(crate) fn template_icon() -> anyhow::Result<gpui_tray::Icon> {
    use anyhow::Context as _;
    use objc2::AnyThread as _;
    use objc2_app_kit::{NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSData, NSSize};

    let points = NSSize::new(18.0, 18.0);

    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(STATUS_ICON_1X))
        .context("AppKit rejected the 1x status icon")?;
    image.setSize(points);

    // The 2x artwork covers the same 18 by 18 points with twice the pixels.
    let retina = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(STATUS_ICON_2X))
        .context("AppKit rejected the 2x status icon")?;
    retina.setSize(points);
    image.addRepresentation(&retina);

    image.setTemplate(true);
    Ok(gpui_tray::Icon::from_ns_image(image))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .expect("the embedded icon is a PNG")
            .to_rgba8()
    }

    #[test]
    fn the_two_sizes_are_18_and_36_pixels() {
        assert_eq!(decode(STATUS_ICON_1X).dimensions(), (18, 18));
        assert_eq!(decode(STATUS_ICON_2X).dimensions(), (36, 36));
    }

    #[test]
    fn both_are_template_artwork() {
        // A template image is black on transparent: AppKit uses only the alpha
        // channel and colours it for the menu bar (light, dark, selected).
        for png in [STATUS_ICON_1X, STATUS_ICON_2X] {
            let image = decode(png);
            let mut inked = 0;
            for pixel in image.pixels() {
                if pixel[3] > 0 {
                    inked += 1;
                    assert_eq!(&pixel.0[..3], &[0, 0, 0], "ink must be black");
                }
            }
            let total = (image.width() * image.height()) as usize;
            assert!(
                inked > total / 10,
                "the glyph is drawn ({inked} of {total})"
            );
            assert!(
                inked < total / 2,
                "the glyph is not a filled square ({inked} of {total})"
            );
            assert_eq!(image.get_pixel(0, 0)[3], 0, "the corner is clear");
        }
    }

    #[test]
    fn the_glyph_has_three_nodes() {
        // Each node is a ring: clear in the middle, inked around it. The
        // centres are the mockup's (3.5,8), (12.5,3.5), (12.5,12.5) on a
        // 16-unit grid.
        let image = decode(STATUS_ICON_2X);
        let scale = 36.0 / 16.0;
        for (x, y) in [(3.5, 8.0), (12.5, 3.5), (12.5, 12.5)] {
            let (cx, cy) = ((x * scale) as u32, (y * scale) as u32);
            assert_eq!(
                image.get_pixel(cx, cy)[3],
                0,
                "the centre of a node is hollow"
            );
            let ring_x = ((x + 2.0) * scale) as u32 - 1;
            assert!(
                image.get_pixel(ring_x.min(35), cy)[3] > 200,
                "its ring is inked"
            );
        }
    }
}
