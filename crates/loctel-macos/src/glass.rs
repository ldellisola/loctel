//! Liquid Glass behind the main window's sidebar and toolbar (macOS 26).
//!
//! GPUI draws the whole window into one `NSView` of its own and cannot draw
//! Apple's glass. So the glass is native: [`attach`] makes the window
//! transparent and puts one `NSGlassEffectView` per glass part *below* GPUI's
//! view, in the same window. The shell (`loctel_ui::app::MainWindow`) paints
//! nothing where the glass is (its sidebar and toolbar have no fill, and the
//! window colour is painted round them), so the glass shows through, and tells
//! the [`Backdrop`] where those parts are on every layout change.
//!
//! - **One host per region.** Each region is an `NSGlassEffectView` (style
//!   Regular, with the region's corner radius and an empty content view) inside
//!   a plain `NSView` host that clips to its bounds and is never hit: a click
//!   goes to GPUI's view above, whatever is under it.
//! - **Frames are applied later.** The shell calls `set_regions` from inside a
//!   GPUI frame, and a call into AppKit from there can call back into GPUI
//!   while GPUI holds its window state (the deadlock that others have hit with
//!   native views under GPUI). So `set_regions` only stores the regions, and
//!   the frames are set from the main dispatch queue, in the next turn of the
//!   run loop, outside any frame (see decision 0030).
//! - **No glass, no problem.** [`attach`] returns `None` when the class is
//!   missing (macOS before 26) or the window is not an AppKit one; the caller
//!   then falls back to GPUI's `Blurred` window background.
//!
//! Nothing here has run on a Mac. The part that holds a decision and no AppKit
//! (the y flip) is plain Rust and tested on every OS; the rest is only
//! type-checked off a Mac (`script/check-macos.sh`). See "Unverified until the
//! M3 Mac check" in `docs/areas/macos.md`. All `unsafe` of the glass is in this
//! file, each block with its reason.

/// A rectangle in AppKit's units (points), as plain numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The frame of the region at (`x`, `y`) of size `width` by `height`, measured
/// from the top left of a view `view_height` tall (GPUI's own coordinates), in
/// the coordinates of a parent view: y is kept when the parent is `flipped`
/// (its origin is its top left), and counted from the bottom edge otherwise,
/// which is AppKit's default.
pub(crate) fn appkit_frame(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    view_height: f64,
    flipped: bool,
) -> Frame {
    Frame {
        x,
        y: if flipped { y } else { view_height - y - height },
        width,
        height,
    }
}

/// The frame of `region` (in GPUI's window coordinates) as an `NSRect`:
/// `view_height` is the height of GPUI's view, and `flipped` whether the parent
/// view the frame is for has its origin at the top left. See [`appkit_frame`].
#[cfg(target_os = "macos")]
pub(crate) fn to_appkit_frame(
    region: gpui::Bounds<gpui::Pixels>,
    view_height: f64,
    flipped: bool,
) -> objc2_foundation::NSRect {
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let number = |pixels: gpui::Pixels| f64::from(f32::from(pixels));
    let frame = appkit_frame(
        number(region.origin.x),
        number(region.origin.y),
        number(region.size.width),
        number(region.size.height),
        view_height,
        flipped,
    );
    NSRect::new(
        NSPoint::new(frame.x, frame.y),
        NSSize::new(frame.width, frame.height),
    )
}

#[cfg(target_os = "macos")]
pub use appkit::attach;

#[cfg(target_os = "macos")]
mod appkit {
    use std::cell::{Cell, RefCell};
    use std::rc::{Rc, Weak};

    use dispatch2::DispatchQueue;
    use gpui::{Window, WindowBackgroundAppearance};
    use loctel_ui::app::{Backdrop, GlassRegion};
    use objc2::rc::Retained;
    use objc2::runtime::AnyClass;
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{NSGlassEffectView, NSGlassEffectViewStyle, NSView, NSWindowOrderingMode};
    use objc2_foundation::{NSPoint, NSRect};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::to_appkit_frame;

    /// How many regions the shell reports: the sidebar first, then the toolbar.
    const REGIONS: usize = 2;

    define_class!(
        // SAFETY:
        // - The superclass `NSView` is meant to be subclassed, and overriding
        //   `hitTest:` is the documented way to make a view ignore the mouse.
        // - The class has no instance variables and does not implement `Drop`.
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "LoctelGlassHost"]
        struct GlassHost;

        impl GlassHost {
            /// No point is ever in this view: AppKit goes on to the next view,
            /// GPUI's, which takes the event.
            #[unsafe(method(hitTest:))]
            fn hit_test(&self, _point: NSPoint) -> Option<&NSView> {
                None
            }
        }
    );

    impl GlassHost {
        fn new(mtm: MainThreadMarker) -> Retained<GlassHost> {
            let this = mtm.alloc::<GlassHost>().set_ivars(());
            // SAFETY: `initWithFrame:` is `NSView`'s designated initialiser and
            // the instance variables (none) are set.
            unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
        }
    }

    /// Puts glass behind the window's sidebar and toolbar, and returns the
    /// [`Backdrop`] the shell reports their places to. `None` when there is no
    /// glass to put: the `NSGlassEffectView` class does not exist (macOS before
    /// 26), this is not the main thread, or `window` is not an AppKit window.
    /// Then nothing about the window has changed.
    ///
    /// Otherwise the window is made `Transparent` (the glass is under GPUI's
    /// view, so the window must not be opaque), and each region gets its glass,
    /// still hidden: it appears when the first regions are applied.
    pub fn attach(window: &mut Window) -> Option<Box<dyn Backdrop>> {
        // The class is looked up by name, not linked: before macOS 26 it is
        // not there, and asking for it through the typed binding would panic.
        AnyClass::get(c"NSGlassEffectView")?;
        let mtm = MainThreadMarker::new()?;
        let gpui_view = gpui_view(window)?;
        // SAFETY: `gpui_view` is alive (we hold a strong reference), and the
        // result is retained by the `Retained` it is returned in.
        let parent = unsafe { gpui_view.superview() }?;

        window.set_background_appearance(WindowBackgroundAppearance::Transparent);

        let hosts = (0..REGIONS)
            .map(|_| Host::new(mtm, &parent, &gpui_view))
            .collect();
        Some(Box::new(GlassBackdrop(Rc::new(Glass {
            gpui_view,
            parent,
            hosts,
            pending: RefCell::new(None),
            queued: Cell::new(false),
        }))))
    }

    /// GPUI's own `NSView` for `window`, the one it draws the window into.
    fn gpui_view(window: &Window) -> Option<Retained<NSView>> {
        // Not `window.window_handle()`: GPUI's own method of that name is
        // something else (its window's handle in the app).
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // SAFETY: an `AppKitWindowHandle` holds a pointer to the window's
        // `NSView`, which stays valid while the handle does; `retain` takes a
        // strong reference of our own, so it stays valid after.
        unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }
    }

    /// One glass region: the host that clips and ignores the mouse, and the
    /// glass inside it.
    struct Host {
        view: Retained<GlassHost>,
        glass: Retained<NSGlassEffectView>,
    }

    impl Host {
        /// A hidden host with its glass, inserted into `parent` below GPUI's
        /// view.
        fn new(mtm: MainThreadMarker, parent: &NSView, gpui_view: &NSView) -> Host {
            let view = GlassHost::new(mtm);
            view.setClipsToBounds(true);
            view.setHidden(true);

            let glass = NSGlassEffectView::new(mtm);
            glass.setStyle(NSGlassEffectViewStyle::Regular);
            glass.setContentView(Some(&NSView::new(mtm)));
            view.addSubview(&glass);

            parent.addSubview_positioned_relativeTo(
                &view,
                NSWindowOrderingMode::Below,
                Some(gpui_view),
            );
            Host { view, glass }
        }
    }

    /// What the backdrop knows. Only ever touched on the main thread.
    struct Glass {
        /// GPUI's view: the regions are in its coordinates.
        gpui_view: Retained<NSView>,
        /// The view GPUI's view and the hosts are in.
        parent: Retained<NSView>,
        hosts: Vec<Host>,
        /// The latest regions that are not applied yet.
        pending: RefCell<Option<Vec<GlassRegion>>>,
        /// Whether a turn of the main queue is waiting to apply them.
        queued: Cell<bool>,
    }

    impl Glass {
        /// Sets the hosts' frames from the latest regions. Runs from the main
        /// queue, never inside a GPUI frame.
        fn apply(&self) {
            let Some(regions) = self.pending.take() else {
                return;
            };
            let view_height = self.gpui_view.bounds().size.height;
            let flipped = self.parent.isFlipped();
            // GPUI's view normally fills its parent; if it does not, the
            // regions are still where GPUI drew them.
            let origin = self.gpui_view.frame().origin;
            for (index, host) in self.hosts.iter().enumerate() {
                let Some(region) = regions.get(index) else {
                    host.view.setHidden(true);
                    continue;
                };
                let mut frame = to_appkit_frame(region.bounds, view_height, flipped);
                frame.origin.x += origin.x;
                frame.origin.y += origin.y;
                host.view.setFrame(frame);
                // The glass fills its host.
                host.glass.setFrame(host.view.bounds());
                host.glass
                    .setCornerRadius(f64::from(f32::from(region.corner_radius)));
                host.view.setHidden(false);
            }
        }
    }

    thread_local! {
        /// The backdrops with regions waiting for the main queue's next turn.
        /// A weak list rather than the backdrop itself in the queued closure:
        /// the closure must be `Send`, an AppKit view is not, and a window that
        /// closes meanwhile should not be kept alive by its queue entry.
        static DUE: RefCell<Vec<Weak<Glass>>> = const { RefCell::new(Vec::new()) };
    }

    /// Asks the main queue to apply `glass`'s pending regions, once however
    /// many times it is asked before the turn comes.
    fn queue(glass: &Rc<Glass>) {
        if glass.queued.replace(true) {
            return;
        }
        DUE.with(|due| due.borrow_mut().push(Rc::downgrade(glass)));
        DispatchQueue::main().exec_async(apply_due);
    }

    /// The main queue's turn: applies what is waiting.
    fn apply_due() {
        let due = DUE.with(|due| std::mem::take(&mut *due.borrow_mut()));
        for glass in due.iter().filter_map(Weak::upgrade) {
            glass.queued.set(false);
            glass.apply();
        }
    }

    /// The [`Backdrop`] the shell holds.
    struct GlassBackdrop(Rc<Glass>);

    impl Backdrop for GlassBackdrop {
        /// Stores the regions; the frames follow on the main queue. Called
        /// inside a GPUI frame, so it touches no view.
        fn set_regions(&self, regions: &[GlassRegion]) {
            *self.0.pending.borrow_mut() = Some(regions.to_vec());
            queue(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The toolbar of a 1440 by 900 window: 10 px from the top and the right
    /// edge, 48 px high, to the right of the 220 px sidebar.
    const TOOLBAR: (f64, f64, f64, f64) = (240.0, 10.0, 1190.0, 48.0);
    const VIEW_HEIGHT: f64 = 900.0;

    fn frame_of(flipped: bool) -> Frame {
        let (x, y, width, height) = TOOLBAR;
        appkit_frame(x, y, width, height, VIEW_HEIGHT, flipped)
    }

    #[test]
    fn frame_flips_y_when_not_flipped() {
        // AppKit's own origin is the bottom left: a region 10 px below the top
        // edge and 48 px high has its bottom edge at 900 - 10 - 48 = 842.
        let frame = frame_of(false);
        assert_eq!(frame.y, 842.0);
        assert_eq!((frame.x, frame.width, frame.height), (240.0, 1190.0, 48.0));
    }

    #[test]
    fn frame_keeps_y_when_flipped() {
        // A flipped view has GPUI's own origin (top left): nothing to convert.
        let frame = frame_of(true);
        assert_eq!(frame.y, 10.0);
        assert_eq!((frame.x, frame.width, frame.height), (240.0, 1190.0, 48.0));
    }

    #[test]
    fn a_region_that_fills_the_view_stays_at_the_origin_either_way() {
        for flipped in [false, true] {
            let frame = appkit_frame(0.0, 0.0, 1440.0, 900.0, 900.0, flipped);
            assert_eq!(
                frame,
                Frame {
                    x: 0.0,
                    y: 0.0,
                    width: 1440.0,
                    height: 900.0
                }
            );
        }
    }

    #[test]
    fn fractional_regions_keep_their_fractions() {
        let frame = appkit_frame(10.5, 10.25, 220.0, 100.5, 900.0, false);
        assert_eq!(frame.y, 900.0 - 10.25 - 100.5);
        assert_eq!(frame.x, 10.5);
    }

    #[test]
    fn flipping_twice_gives_the_region_back() {
        // The conversion is its own inverse: the top edge of the result
        // measured from the top of the view is where the region started.
        let frame = appkit_frame(240.0, 10.0, 1190.0, 48.0, 900.0, false);
        let top_down = 900.0 - frame.y - frame.height;
        assert_eq!(top_down, 10.0);
    }

    #[test]
    fn regions_that_share_a_top_edge_share_it_after_the_flip() {
        // The sidebar (880 high) and the toolbar (48 high) both start 10 px
        // from the top; in AppKit's bottom-up terms the sidebar's bottom edge
        // is 10 px up and the toolbar's is 842, and both top edges are at 890.
        let sidebar = appkit_frame(10.0, 10.0, 220.0, 880.0, 900.0, false);
        let toolbar = appkit_frame(240.0, 10.0, 1190.0, 48.0, 900.0, false);
        assert_eq!(sidebar.y, 10.0);
        assert_eq!(toolbar.y, 842.0);
        assert_eq!(sidebar.y + sidebar.height, toolbar.y + toolbar.height);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod mac_tests {
    use gpui::{Bounds, point, px, size};

    use super::to_appkit_frame;

    /// The sidebar of a 1440 by 900 window.
    fn sidebar() -> Bounds<gpui::Pixels> {
        Bounds::new(point(px(10.0), px(10.0)), size(px(220.0), px(880.0)))
    }

    #[test]
    fn to_appkit_frame_flips_y_when_not_flipped() {
        let toolbar = Bounds::new(point(px(240.0), px(10.0)), size(px(1190.0), px(48.0)));
        let frame = to_appkit_frame(toolbar, 900.0, false);
        assert_eq!(frame.origin.x, 240.0);
        assert_eq!(frame.origin.y, 842.0);
        assert_eq!(frame.size.width, 1190.0);
        assert_eq!(frame.size.height, 48.0);
        // A full-height region fills the view from the bottom.
        let frame = to_appkit_frame(sidebar(), 900.0, false);
        assert_eq!(frame.origin.y, 10.0);
        assert_eq!(frame.size.height, 880.0);
    }

    #[test]
    fn to_appkit_frame_keeps_y_when_flipped() {
        let toolbar = Bounds::new(point(px(240.0), px(10.0)), size(px(1190.0), px(48.0)));
        let frame = to_appkit_frame(toolbar, 900.0, true);
        assert_eq!(frame.origin.x, 240.0);
        assert_eq!(frame.origin.y, 10.0);
        assert_eq!(frame.size.width, 1190.0);
        assert_eq!(frame.size.height, 48.0);
    }
}
