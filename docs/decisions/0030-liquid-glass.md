# 0030: Liquid Glass behind the sidebar and the toolbar

Date: 2026-10-22
Status: accepted (unverified: nothing here has run on a Mac)

## Context

Milestone 3 Task 10 puts Apple's real glass (`NSGlassEffectView`, macOS 26)
behind the main window's sidebar and toolbar (design §9.3, §11). GPUI draws the
window into a single `NSView` and has no glass of its own, so the glass has to
be native views under GPUI's. The design names the hazards: there is no
upstream support, and a native view next to GPUI deadlocked another project
(AppKit called back into GPUI while GPUI held its window state). So the glass is
kept in one file (`crates/loctel-macos/src/glass.rs`), with a fallback.

## Decision

- **Approach.** `glass::attach(window)` runs inside the new window, before the
  shell is built (`open_main_window`'s `attach_backdrop`). It makes the window
  `WindowBackgroundAppearance::Transparent` and adds two glass regions, one for
  the sidebar and one for the toolbar, each below GPUI's view in the content
  view (`addSubview:positioned:NSWindowBelow relativeTo:<GPUI's view>`). A
  region is a host `NSView` (class `LoctelGlassHost`) that clips to its bounds
  and returns `nil` from `hitTest:`, so it never takes a click, holding an
  `NSGlassEffectView` (style Regular, the region's corner radius, an empty
  content view) that fills it. GPUI's own view is found through the window's
  `raw-window-handle` (`AppKitWindowHandle::ns_view`), as GPUI Kit does.
  `attach` returns a `Backdrop` that remembers the regions it is told.
- **Frames are applied later, from the main queue.** The shell reports the
  regions from a canvas prepaint callback, inside a GPUI frame.
  `Backdrop::set_regions` therefore only stores the latest regions and, if none
  is queued already, asks the main dispatch queue (`dispatch2`'s
  `DispatchQueue::main().exec_async`) for a turn; there the hosts' frames, the
  glass's frames and corner radii are set. Nothing in `set_regions` touches a
  view, so AppKit can never call back into GPUI while GPUI is in a frame, which
  is the deadlock the design warns about. Several reports before the turn
  arrives apply once, with the last. The queued closure has to be `Send` and an
  AppKit view is not, so it carries nothing: the backdrops that are waiting are
  in a thread-local list of `Weak` references, which the closure drains. A
  window that closes meanwhile is not kept alive by its queue entry. The cost is
  that the glass follows a layout change one main-queue turn late: during a live
  resize it may trail GPUI's content by a frame.
- **The y flip is plain Rust.** GPUI measures from the top left; a normal
  `NSView` counts from the bottom. `to_appkit_frame(region, view_height,
  flipped)` is the conversion (the signature the plan fixes). Its arithmetic is
  `appkit_frame`, with no GPUI or AppKit types, compiled for the tests on every
  OS like the menu's rows, so the Linux gate runs its tests (`frame_flips_y_*`).
  The two tests of the plan's names (`to_appkit_frame_flips_y_when_not_flipped`
  and `to_appkit_frame_keeps_y_when_flipped`) check the `NSRect` itself and can
  only be compiled for a Mac (`script/check-macos.sh --all-targets` does that);
  nobody has run them.
- **The Blurred fallback.** `attach` returns `None` when the `NSGlassEffectView`
  class does not exist (macOS before 26; it is looked up by name, because the
  typed binding panics when the class is missing), when it is not on the main
  thread, or when the window handle is not an AppKit one, and in those cases it
  has changed nothing. `window.rs` then sets `WindowBackgroundAppearance::Blurred`
  and prints one line to stderr (once per run). The shell is given no backdrop,
  so it paints its opaque panels. loctel needs macOS 26, so the fallback should
  only ever be seen if the class goes away or attaching fails.
- **What the shell paints with a backdrop.** The sidebar and the toolbar have no
  fill, a 1 px hairline (`Theme::glass_hairline`, the text colour at a low
  alpha) and, just inside it, a 1 px ring of highlight (`Theme::glass_highlight`:
  light on both looks). Without a backdrop they keep the panel fill and
  hairline of the mockups.

### Where the plan was wrong about the existing code

The plan says only to give the sidebar and toolbar a transparent fill. That is
not enough: two layers above the glass paint the window colour over all of it.

1. The shell's own root `div` had `bg(theme.bg)`.
2. GPUI Kit's `Root` paints `bg(theme.background)` over the whole window (it is
   the window state plugin's `style`), and our `kit_theme` maps `background` to
   `Theme::bg`.

Either would hide the glass, since the glass is under GPUI's view. So, when a
backdrop is attached:

- The shell's root has no background. A canvas painted first
  (`shell::window_colour`) paints the window colour as one path, the window
  rectangle with a rounded hole for each glass region (even-odd fill), using the
  regions measured in the same frame. The gaps, the margin and the slivers
  outside the rounded corners keep the window colour, and the glass shows
  exactly through the two parts.
- `MainWindow::new` clears Kit's `Root` background (its `Styled` style is
  applied after the plugin's), deferred one effect cycle because the `Root` is
  made after the view. A window that is not in a `Root` is left alone.

### Tests, and what the headless renderer can show

The headless renderer cannot draw the glass, and its capture is always opaque
(it clears to opaque black), so the plan's "painted transparent" cannot be read
from the capture's alpha. `tests/screens.rs` therefore puts the window over a
`Desktop` view that paints a flat colour (dark blue for the dark look, pale blue
for the light one) and compares the capture:

- `glass_parts_show_what_is_behind_and_the_rest_does_not` checks pixel values:
  the inside of the sidebar and the toolbar is the desktop colour; the content
  panels, the gaps, the margin and the corner slivers are not; the hairline and
  the highlight are the theme's translucent colours over the desktop colour.
- `shell_glass_dark` (the plan's) and `shell_glass_light` (added, so the light
  look's hairline and highlight are pinned too) are the references.
- `without_a_backdrop_the_panels_have_their_fill` checks the other branch.

The stand-in colour is not what the glass looks like. It only shows where the
shell leaves the window clear.

### Dependencies

`objc2-app-kit` gets the features of the plan (`NSGlassEffectView`, `NSView`,
`NSResponder`, `NSColor`) and two more it needs: `NSGraphics`
(`NSWindowOrderingMode`, for `addSubview:positioned:relativeTo:`) and
`objc2-core-foundation` (`CGFloat`, for the corner radius). `NSColor` is enabled
as the plan lists it, though the glass is not tinted yet. `raw-window-handle`
0.6 and `dispatch2` 0.3 become workspace dependencies of `loctel-macos` (macOS
only); both were already in `Cargo.lock` through GPUI and objc2.

## Consequences

- **Unverified until the M3 Mac check** (`docs/areas/macos.md`): that the glass
  appears and is Liquid Glass and not plain blur; that nothing deadlocks while
  the window opens, resizes and closes; that clicks reach GPUI through the
  hosts; that the clear Kit `Root` and the holed window colour leave no seam
  (AppKit rounds the glass, GPUI rounds the hole; the hairline covers the edge);
  that the window's shadow and the traffic lights look right on a transparent
  window; and how far the glass trails a live resize.
- A later change to the shell's layout needs no glass change: the regions are
  measured, not assumed. A third glass part means a third region in `attach`
  (`REGIONS`) and in the shell's report.
- If the Mac check shows the glass should be drawn elsewhere in the view
  order, or the deferral flickers, this is the file and the decision to change.
