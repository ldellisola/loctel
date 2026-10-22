//! The service map (design 9.2, M3 plan Task 6): [`MapView`] draws the placed
//! map of [`HomeData`](crate::app::HomeData) on one canvas, and turns a click
//! on a service into the filter of the trace table.
//!
//! What it draws, in picture coordinates (the layout's own; the camera maps
//! them to the screen):
//!
//! - **Nodes**: a rounded quad (radius 14) filled with the panel colour, a dot
//!   (a circle for a service in its colour, a rounded square for a database, a
//!   queue or an external host), the name and the subtitle the layout wrote. A
//!   failing node has an `error` border and an `error_text` subtitle; a node
//!   that is not a service has a dashed border; the selected service has a 3 px
//!   `accent` ring.
//! - **Edges**: cubic béziers from the right-edge middle of the caller to the
//!   left-edge middle of the callee, 2 px, with a filled arrowhead; `error`
//!   when failing. The rate label of an edge sits above the curve's midpoint.
//!
//! **The camera** ([`Camera`]) is the zoom and the offset of the picture in the
//! view. A new map is *fitted*: full size from the top left corner, or scaled
//! down (never below 0.5) when the picture does not fit, so it holds still as
//! services appear and follows a resize. The first pan or zoom leaves the fit
//! and the camera stays where the user put it; `Fit` goes back.
//!
//! **Input.** Drag pans. Ctrl or cmd plus the wheel and a pinch zoom, from 0.5 to
//! 2.0, around the pointer; the wheel alone pans (a two-finger drag on a
//! trackpad). A press that moves less than a few pixels is a click: on a
//! service it selects it (`HomeModel::select_service`, which writes the filter
//! into the query bar), on the selected service or on empty space it clears the
//! selection, and on a database, a queue or an external host it does nothing
//! (they have no filter). Decision 0026 has the reasons.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, BorderStyle, Bounds, ContentMask, Context, Corners, Edges, Entity, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, PathBuilder, PinchEvent, Pixels, Point, Render, Rgba,
    ScrollWheelEvent, ShapedLine, SharedString, Styled as _, Subscription, TextAlign, TextRun,
    TruncateFrom, Window, canvas, div, font, point, px, quad, size,
};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use loctel_store::NodeKind;

use crate::Theme;
use crate::app::HomeModel;
use crate::model::map_layout::{MapLayout, PlacedEdge, PlacedNode};
use crate::theme::FONT_FAMILY;

/// The most the map zooms out.
pub const MIN_ZOOM: f32 = 0.5;
/// The most the map zooms in.
pub const MAX_ZOOM: f32 = 2.0;

/// What the map says while there is nothing to draw.
pub const EMPTY_TITLE: &str = "Waiting for telemetry";
/// And what to do about it.
pub const EMPTY_HINT: &str = "Point OTEL_EXPORTER_OTLP_ENDPOINT at http://localhost:4317";

/// The corner radius of a node, in picture pixels.
const NODE_RADIUS: f32 = 14.0;
/// The border of a node, and of a failing one.
const NODE_BORDER: f32 = 1.0;
const FAILING_BORDER: f32 = 2.0;
/// The ring round the selected node: its width, which is also how far it is
/// outside the node.
const RING_WIDTH: f32 = 3.0;
/// The stroke of an edge.
const EDGE_WIDTH: f32 = 2.0;
/// The arrowhead of an edge: how long it is and how wide (half of it).
const ARROW_LENGTH: f32 = 10.0;
const ARROW_HALF_WIDTH: f32 = 5.0;
/// An edge leaves and enters a node level; its control points are at least
/// this far out, so a loop that goes back still has a shape.
const MIN_BEND: f32 = 48.0;
/// The dot of a node: its size, and how far from the node's left edge and top
/// it starts.
const DOT_SIZE: f32 = 10.0;
const DOT_X: f32 = 17.0;
const DOT_Y: f32 = 17.0;
/// Where the texts of a node start, from its left edge, and the room kept at
/// the right.
const TEXT_X: f32 = 34.0;
const TEXT_RIGHT: f32 = 14.0;
/// The name's size and line, and the subtitle's, in picture pixels, and how far
/// down each line starts.
const TITLE_SIZE: f32 = 13.0;
const TITLE_LINE: f32 = 18.0;
const TITLE_Y: f32 = 13.0;
const SUBTITLE_SIZE: f32 = 11.0;
const SUBTITLE_LINE: f32 = 16.0;
const SUBTITLE_Y: f32 = 34.0;
/// An edge label: its size, its padding, how high its box is and how far above
/// the curve's midpoint the box ends.
const LABEL_SIZE: f32 = 11.0;
const LABEL_PAD: f32 = 6.0;
const LABEL_HEIGHT: f32 = 18.0;
const LABEL_LIFT: f32 = 8.0;
/// How far the pointer may travel between press and release and still be a
/// click, in pixels on the screen.
const CLICK_SLOP: f32 = 3.0;
/// How much a pixel of wheel zooms: the zoom changes by `e^(pixels x rate)`.
const WHEEL_ZOOM_RATE: f32 = 0.005;
/// A wheel that counts lines (a mouse) is this many pixels a line.
const LINE_PIXELS: f32 = 20.0;
/// The Fit button's distance from the corner of the map.
const FIT_INSET: f32 = 12.0;

/// Where the picture is in the view: its zoom and the offset of its origin
/// from the view's top left corner. A picture point `(x, y)` shows at
/// `(pan_x + x * zoom, pan_y + y * zoom)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Screen pixels per picture pixel, from [`MIN_ZOOM`] to [`MAX_ZOOM`].
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
}

impl Default for Camera {
    /// Full size, the picture's corner in the view's corner.
    fn default() -> Camera {
        Camera {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        }
    }
}

impl Camera {
    /// The camera of a map nobody has moved: full size from the top left
    /// corner when a `picture` (width, height) fits a `view`, otherwise scaled
    /// to fit, but not below [`MIN_ZOOM`]. A picture is never enlarged and
    /// never centred, so it does not move while services appear; with no
    /// picture or no view it is the default camera.
    pub fn fit(view: (f32, f32), picture: (f32, f32)) -> Camera {
        let ((view_w, view_h), (width, height)) = (view, picture);
        if !(width > 0.0 && height > 0.0 && view_w > 0.0 && view_h > 0.0) {
            return Camera::default();
        }
        Camera {
            zoom: (view_w / width).min(view_h / height).clamp(MIN_ZOOM, 1.0),
            ..Camera::default()
        }
    }

    /// Where the picture point `(x, y)` is in the view.
    pub fn to_view(self, x: f32, y: f32) -> (f32, f32) {
        (self.pan_x + x * self.zoom, self.pan_y + y * self.zoom)
    }

    /// The picture point that shows at the view point `(x, y)`.
    pub fn to_picture(self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.pan_x) / self.zoom, (y - self.pan_y) / self.zoom)
    }

    /// The camera zoomed by `factor` (above 1 zooms in) and kept within
    /// [`MIN_ZOOM`] to [`MAX_ZOOM`], with the picture point under the view point
    /// `pivot` staying under it. A `factor` that is not a positive number
    /// changes nothing.
    pub fn zoomed(self, factor: f32, pivot: (f32, f32)) -> Camera {
        if !(factor.is_finite() && factor > 0.0) {
            return self;
        }
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let ratio = zoom / self.zoom;
        Camera {
            zoom,
            pan_x: pivot.0 - (pivot.0 - self.pan_x) * ratio,
            pan_y: pivot.1 - (pivot.1 - self.pan_y) * ratio,
        }
    }

    /// The camera with the picture moved by `(dx, dy)` view pixels.
    pub fn panned(self, dx: f32, dy: f32) -> Camera {
        Camera {
            pan_x: self.pan_x + dx,
            pan_y: self.pan_y + dy,
            ..self
        }
    }
}

/// A press that may turn into a drag.
struct Drag {
    /// Where the button went down, in window coordinates.
    start: Point<Pixels>,
    /// The camera at that moment.
    camera: Camera,
    /// The pointer has gone beyond the click slop: this is a pan.
    moved: bool,
}

/// What the latest paint knew, for the handlers that run between frames.
#[derive(Default)]
struct Frame {
    /// The map's area, in window coordinates.
    bounds: Cell<Bounds<Pixels>>,
    /// The camera that frame was drawn with.
    camera: Cell<Camera>,
    /// The Fit button's area, `None` while there is no button.
    fit_button: Cell<Option<Bounds<Pixels>>>,
}

/// The service map of the home screen. Make it with
/// `cx.new(|cx| MapView::new(home, cx))`; it draws `home`'s map and redraws when
/// that changes.
pub struct MapView {
    home: Entity<HomeModel>,
    /// Where the user left the camera, `None` while the map is fitted.
    camera: Option<Camera>,
    drag: Option<Drag>,
    frame: Rc<Frame>,
    /// Keeps the observation of the home model alive.
    _home: Subscription,
}

impl MapView {
    pub fn new(home: Entity<HomeModel>, cx: &mut Context<Self>) -> MapView {
        let subscription = cx.observe(&home, |_, _, cx| cx.notify());
        MapView {
            home,
            camera: None,
            drag: None,
            frame: Rc::new(Frame::default()),
            _home: subscription,
        }
    }

    /// Whether the map is fitted: nobody has panned or zoomed it since it was
    /// made or since `Fit`.
    pub fn is_fitted(&self) -> bool {
        self.camera.is_none()
    }

    /// The camera of the latest frame.
    pub fn camera(&self) -> Camera {
        self.frame.camera.get()
    }

    /// The map's area in the window, as of the latest frame.
    pub fn view_bounds(&self) -> Bounds<Pixels> {
        self.frame.bounds.get()
    }

    /// Where the middle of the node `id` is in the window, as of the latest
    /// frame. `None` when the map has no such node. When a service and a
    /// database have the same id, the service.
    pub fn node_center(&self, id: &str, cx: &App) -> Option<Point<Pixels>> {
        let data = self.home.read(cx).data();
        let node = data
            .map
            .nodes
            .iter()
            .filter(|node| node.id == id)
            .min_by_key(|node| node.kind)?;
        let (x, y) = self
            .frame
            .camera
            .get()
            .to_view(node.x + node.w / 2.0, node.y + node.h / 2.0);
        let origin = self.frame.bounds.get().origin;
        Some(point(origin.x + px(x), origin.y + px(y)))
    }

    /// Where the middle of the Fit button is in the window, as of the latest
    /// frame. `None` while there is no button (an empty map).
    pub fn fit_button_center(&self) -> Option<Point<Pixels>> {
        self.frame.fit_button.get().map(|bounds| bounds.center())
    }

    /// What the map says instead of a picture: the title and the hint, while
    /// it has no node. `None` when there is a map.
    pub fn empty_message(&self, cx: &App) -> Option<(&'static str, &'static str)> {
        self.home
            .read(cx)
            .data()
            .map
            .nodes
            .is_empty()
            .then_some((EMPTY_TITLE, EMPTY_HINT))
    }

    /// Puts the map back in its fitted place.
    pub fn fit(&mut self, cx: &mut Context<Self>) {
        if self.camera.take().is_some() {
            cx.notify();
        }
    }

    /// The camera now: the user's, or the fitted one of the latest frame.
    fn effective_camera(&self) -> Camera {
        self.camera.unwrap_or_else(|| self.frame.camera.get())
    }

    /// `position` (window coordinates) in the map's own, from its corner.
    fn local(&self, position: Point<Pixels>) -> (f32, f32) {
        let origin = self.frame.bounds.get().origin;
        (
            f32::from(position.x - origin.x),
            f32::from(position.y - origin.y),
        )
    }

    /// The node under `position`, the one drawn on top when they overlap: its
    /// id and kind.
    fn node_at(&self, position: Point<Pixels>, cx: &App) -> Option<(String, NodeKind)> {
        let (x, y) = {
            let (local_x, local_y) = self.local(position);
            self.effective_camera().to_picture(local_x, local_y)
        };
        self.home
            .read(cx)
            .data()
            .map
            .nodes
            .iter()
            .rev()
            .find(|node| x >= node.x && x < node.x + node.w && y >= node.y && y < node.y + node.h)
            .map(|node| (node.id.clone(), node.kind))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent) {
        self.drag = Some(Drag {
            start: event.position,
            camera: self.effective_camera(),
            moved: false,
        });
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            // The button went up where nobody heard it.
            self.drag = None;
            return;
        }
        let travelled = event.position - drag.start;
        let (dx, dy) = (f32::from(travelled.x), f32::from(travelled.y));
        if !drag.moved && dx.hypot(dy) < CLICK_SLOP {
            return;
        }
        drag.moved = true;
        self.camera = Some(drag.camera.panned(dx, dy));
        cx.notify();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        match self.drag.take() {
            Some(drag) if !drag.moved => self.click(event.position, cx),
            _ => {}
        }
    }

    /// A click at `position`: select the service, or clear.
    fn click(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        match self.node_at(position, cx) {
            Some((id, NodeKind::Service)) => {
                let selected = self.home.read(cx).selected_service() == Some(id.as_str());
                let choice = (!selected).then_some(id);
                self.home
                    .update(cx, |home, cx| home.select_service(choice, cx));
            }
            // A database, a queue or an external host has no filter.
            Some(_) => {}
            None => self
                .home
                .update(cx, |home, cx| home.select_service(None, cx)),
        }
    }

    /// The wheel: zoom with ctrl or cmd held, otherwise pan.
    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(LINE_PIXELS));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        let camera = self.effective_camera();
        self.camera = Some(if event.modifiers.control || event.modifiers.platform {
            camera.zoomed((dy * WHEEL_ZOOM_RATE).exp(), self.local(event.position))
        } else {
            camera.panned(dx, dy)
        });
        cx.stop_propagation();
        cx.notify();
    }

    /// A pinch: `delta` is the zoom change, 0.1 for 10% more.
    fn pinch(&mut self, event: &PinchEvent, cx: &mut Context<Self>) {
        let camera = self.effective_camera();
        self.camera = Some(camera.zoomed(1.0 + event.delta, self.local(event.position)));
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for MapView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let home = self.home.read(cx);
        let layout = home.data().map.clone();
        let selected = home.selected_service().map(str::to_owned);

        let root = div().size_full().relative().overflow_hidden();
        if layout.nodes.is_empty() {
            self.frame.fit_button.set(None);
            return root.child(empty_state(&theme));
        }

        let frame = Rc::clone(&self.frame);
        let user_camera = self.camera;
        let picture = canvas(
            move |bounds, _window, _cx| {
                let camera = user_camera.unwrap_or_else(|| {
                    Camera::fit(
                        (f32::from(bounds.size.width), f32::from(bounds.size.height)),
                        (layout.width, layout.height),
                    )
                });
                frame.bounds.set(bounds);
                frame.camera.set(camera);
                (layout, camera)
            },
            move |bounds, (layout, camera), window, cx| {
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    Painter {
                        theme: &theme,
                        camera,
                        origin: bounds.origin,
                    }
                    .paint(&layout, selected.as_deref(), window, cx);
                });
            },
        )
        .absolute()
        .size_full();

        let fit_frame = Rc::clone(&self.frame);
        let fit = div()
            .absolute()
            .top(px(FIT_INSET))
            .right(px(FIT_INSET))
            // The button is not part of the surface: a press on it is not a
            // drag of the map.
            .occlude()
            .child(
                Button::new("map-fit")
                    .label("Fit")
                    .small()
                    .tooltip("Fit the map to the window")
                    .on_click(cx.listener(|map, _, _, cx| map.fit(cx))),
            )
            .child(
                canvas(
                    move |bounds, _window, _cx| fit_frame.fit_button.set(Some(bounds)),
                    |_, (), _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );

        root.cursor_grab()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|map, event: &MouseDownEvent, _, _| map.mouse_down(event)),
            )
            .on_mouse_move(cx.listener(|map, event: &MouseMoveEvent, _, cx| {
                map.mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|map, event: &MouseUpEvent, _, cx| map.mouse_up(event, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|map, _: &MouseUpEvent, _, _| map.drag = None),
            )
            .on_scroll_wheel(cx.listener(|map, event: &ScrollWheelEvent, _, cx| {
                map.scroll(event, cx);
            }))
            .on_pinch(cx.listener(|map, event: &PinchEvent, _, cx| map.pinch(event, cx)))
            .child(picture)
            .child(fit)
    }
}

/// What shows while the map has no node.
fn empty_state(theme: &Theme) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(EMPTY_TITLE),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(theme.text_secondary)
                .child(EMPTY_HINT),
        )
}

fn hsla(color: Rgba) -> Hsla {
    color.into()
}

/// Draws a [`MapLayout`] through a [`Camera`] into a view whose top left
/// corner is at `origin`.
struct Painter<'a> {
    theme: &'a Theme,
    camera: Camera,
    origin: Point<Pixels>,
}

impl Painter<'_> {
    /// Where the picture point `(x, y)` is in the window.
    fn at(&self, x: f32, y: f32) -> Point<Pixels> {
        let (x, y) = self.camera.to_view(x, y);
        point(self.origin.x + px(x), self.origin.y + px(y))
    }

    /// A length of the picture on the screen.
    fn scaled(&self, length: f32) -> Pixels {
        px(length * self.camera.zoom)
    }

    /// The window area of a node, grown by `grow` picture pixels on every side.
    fn node_bounds(&self, node: &PlacedNode, grow: f32) -> Bounds<Pixels> {
        Bounds::new(
            self.at(node.x - grow, node.y - grow),
            size(
                self.scaled(node.w + 2.0 * grow),
                self.scaled(node.h + 2.0 * grow),
            ),
        )
    }

    fn paint(&self, layout: &MapLayout, selected: Option<&str>, window: &mut Window, cx: &mut App) {
        // An edge's end is the first node with that id, and the kinds sort
        // service first.
        let mut by_id: HashMap<&str, &PlacedNode> = HashMap::new();
        for node in &layout.nodes {
            by_id
                .entry(node.id.as_str())
                .and_modify(|first| {
                    if node.kind < first.kind {
                        *first = node;
                    }
                })
                .or_insert(node);
        }

        // Failing edges last, so they are on top where lines cross.
        let mut labels = Vec::new();
        for failing in [false, true] {
            for edge in layout.edges.iter().filter(|edge| edge.failing == failing) {
                let (Some(from), Some(to)) =
                    (by_id.get(edge.from.as_str()), by_id.get(edge.to.as_str()))
                else {
                    continue;
                };
                if let Some(label) = self.paint_edge(edge, from, to, window) {
                    labels.push(label);
                }
            }
        }
        for node in &layout.nodes {
            self.paint_node(node, selected, window, cx);
        }
        for label in labels {
            self.paint_label(&label, window, cx);
        }
    }

    /// One edge and its arrowhead. Returns its label to draw once the nodes
    /// are, with where to put it.
    fn paint_edge(
        &self,
        edge: &PlacedEdge,
        from: &PlacedNode,
        to: &PlacedNode,
        window: &mut Window,
    ) -> Option<EdgeLabel> {
        // A call of a node to itself has no line to draw.
        if std::ptr::eq(from, to) {
            return None;
        }
        let color = if edge.failing {
            hsla(self.theme.error)
        } else {
            hsla(Rgba {
                a: 0.55,
                ..self.theme.text_tertiary
            })
        };
        let start = self.at(from.x + from.w, from.y + from.h / 2.0);
        let end = self.at(to.x, to.y + to.h / 2.0);
        let bend = ((end.x - start.x) / 2.0).max(self.scaled(MIN_BEND));
        let control_a = point(start.x + bend, start.y);
        let control_b = point(end.x - bend, end.y);

        // The line stops where the arrowhead starts, along the direction the
        // curve arrives in.
        let (dx, dy) = (
            f32::from(end.x - control_b.x),
            f32::from(end.y - control_b.y),
        );
        let length = dx.hypot(dy);
        let (ux, uy) = if length > f32::EPSILON {
            (dx / length, dy / length)
        } else {
            (1.0, 0.0)
        };
        let head = f32::from(self.scaled(ARROW_LENGTH));
        let half = f32::from(self.scaled(ARROW_HALF_WIDTH));
        let base = point(end.x - px(ux * head), end.y - px(uy * head));

        let width = self.scaled(EDGE_WIDTH).max(px(1.0));
        let mut line = PathBuilder::stroke(width);
        line.move_to(start);
        line.cubic_bezier_to(base, control_a, control_b);
        if let Ok(path) = line.build() {
            window.paint_path(path, color);
        }

        let mut arrow = PathBuilder::fill();
        arrow.add_polygon(
            &[
                end,
                point(base.x - px(uy * half), base.y + px(ux * half)),
                point(base.x + px(uy * half), base.y - px(ux * half)),
            ],
            true,
        );
        if let Ok(path) = arrow.build() {
            window.paint_path(path, color);
        }

        // The curve at t = 1/2.
        let middle = point(
            (start.x + control_a.x * 3.0 + control_b.x * 3.0 + end.x) / 8.0,
            (start.y + control_a.y * 3.0 + control_b.y * 3.0 + end.y) / 8.0,
        );
        edge.label.as_ref().map(|text| EdgeLabel {
            text: text.clone(),
            middle,
            failing: edge.failing,
        })
    }

    /// One node: the ring if it is the selected service, then the box, the dot,
    /// the name and the subtitle.
    fn paint_node(
        &self,
        node: &PlacedNode,
        selected: Option<&str>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let theme = self.theme;
        let radius = self.scaled(NODE_RADIUS);

        if node.kind == NodeKind::Service && selected == Some(node.id.as_str()) {
            window.paint_quad(quad(
                self.node_bounds(node, RING_WIDTH),
                Corners::all(self.scaled(NODE_RADIUS + RING_WIDTH)),
                hsla(Rgba {
                    a: 0.0,
                    ..theme.accent
                }),
                Edges::all(self.scaled(RING_WIDTH)),
                hsla(theme.accent),
                BorderStyle::Solid,
            ));
        }

        let (border, border_width) = if node.failing {
            (theme.error, FAILING_BORDER)
        } else {
            (theme.border, NODE_BORDER)
        };
        let style = if node.kind == NodeKind::Service {
            BorderStyle::Solid
        } else {
            BorderStyle::Dashed
        };
        window.paint_quad(quad(
            self.node_bounds(node, 0.0),
            Corners::all(radius),
            hsla(theme.panel),
            Edges::all(self.scaled(border_width)),
            hsla(border),
            style,
        ));

        let dot = Bounds::new(
            self.at(node.x + DOT_X, node.y + DOT_Y),
            size(self.scaled(DOT_SIZE), self.scaled(DOT_SIZE)),
        );
        let (dot_color, dot_radius) = match node.color {
            Some(index) => (
                theme.service_palette[index % theme.service_palette.len()],
                self.scaled(DOT_SIZE / 2.0),
            ),
            None => (theme.text_tertiary, self.scaled(3.0)),
        };
        window.paint_quad(quad(
            dot,
            Corners::all(dot_radius),
            hsla(dot_color),
            Edges::all(px(0.0)),
            hsla(dot_color),
            BorderStyle::Solid,
        ));

        let room = self.scaled(node.w - TEXT_X - TEXT_RIGHT);
        let title = shape(
            window,
            &node.id,
            self.scaled(TITLE_SIZE),
            FontWeight::SEMIBOLD,
            hsla(theme.text),
            Some(room),
        );
        let _ = title.paint(
            self.at(node.x + TEXT_X, node.y + TITLE_Y),
            self.scaled(TITLE_LINE),
            TextAlign::Left,
            None,
            window,
            cx,
        );
        let subtitle_color = if node.failing {
            theme.error_text
        } else {
            theme.text_secondary
        };
        let subtitle = shape(
            window,
            &node.subtitle,
            self.scaled(SUBTITLE_SIZE),
            FontWeight::NORMAL,
            hsla(subtitle_color),
            Some(room),
        );
        let _ = subtitle.paint(
            self.at(node.x + TEXT_X, node.y + SUBTITLE_Y),
            self.scaled(SUBTITLE_LINE),
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }

    /// An edge's rate, in a small box above the middle of its curve.
    fn paint_label(&self, label: &EdgeLabel, window: &mut Window, cx: &mut App) {
        let theme = self.theme;
        let color = if label.failing {
            theme.error_text
        } else {
            theme.text_secondary
        };
        let text = shape(
            window,
            &label.text,
            self.scaled(LABEL_SIZE),
            FontWeight::NORMAL,
            hsla(color),
            None,
        );
        let pad = self.scaled(LABEL_PAD);
        let height = self.scaled(LABEL_HEIGHT);
        let boxed = Bounds::new(
            point(
                label.middle.x - (text.width + pad * 2.0) / 2.0,
                label.middle.y - self.scaled(LABEL_LIFT) - height,
            ),
            size(text.width + pad * 2.0, height),
        );
        window.paint_quad(quad(
            boxed,
            Corners::all(height / 2.0),
            hsla(theme.panel),
            Edges::all(px(1.0)),
            hsla(theme.border),
            BorderStyle::Solid,
        ));
        let _ = text.paint(
            point(boxed.origin.x + pad, boxed.origin.y),
            height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

/// The rate on an edge, to draw above the middle of its curve.
struct EdgeLabel {
    text: String,
    middle: Point<Pixels>,
    failing: bool,
}

/// `text` as one line of `size` in the app's font, cut with an ellipsis to fit
/// `max_width` when there is one.
fn shape(
    window: &Window,
    text: &str,
    size: Pixels,
    weight: FontWeight,
    color: Hsla,
    max_width: Option<Pixels>,
) -> ShapedLine {
    let mut face = font(FONT_FAMILY);
    face.weight = weight;
    let text = SharedString::from(text.to_owned());
    let run = TextRun {
        len: text.len(),
        font: face.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let (text, runs): (SharedString, Cow<'_, [TextRun]>) = match max_width {
        Some(width) => window.text_system().line_wrapper(face, size).truncate_line(
            text,
            width,
            "\u{2026}",
            std::slice::from_ref(&run),
            TruncateFrom::End,
        ),
        None => (text, Cow::Owned(vec![run])),
    };
    window.text_system().shape_line(text, size, &runs, None)
}
