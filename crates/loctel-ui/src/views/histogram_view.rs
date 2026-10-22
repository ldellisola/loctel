//! The volume histogram of the Logs screen (design 9.2, M4 plan Task 4): 48
//! bars 64 px high with 2 px between them, and four time labels under them.
//!
//! [`HistogramView`] holds no data: the bars are the [`LogsModel`]'s
//! ([`Bars`](crate::model::histogram::Bars), fractions of the tallest bar), and
//! this view multiplies them by a height. A bar is the lines of one bucket; the
//! errors are stacked at the bottom of it in the error colour and the rest of the
//! bar above them is the hairline colour. A bucket with a line is never thinner
//! than 2 px, so a single line shows; an empty bucket is a 1 px stub, so the
//! axis shows too.

use gpui::{
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div, px, relative,
};

use crate::Theme;
use crate::app::LogsModel;

/// The height of the bars, in pixels.
pub const HEIGHT: f32 = 64.0;
/// The gap between two bars, in pixels.
pub const GAP: f32 = 2.0;
/// The least a bar with a line in it is high, in pixels.
const MIN_BAR: f32 = 2.0;
/// The height of the stub that stands for an empty bucket.
const STUB: f32 = 1.0;
/// The room the label row takes, and the width a label has to sit in.
const LABEL_ROW: f32 = 18.0;
const LABEL_BOX: f32 = 96.0;

/// The histogram of the Logs screen. Make it with
/// `cx.new(|cx| HistogramView::new(logs, cx))`.
pub struct HistogramView {
    logs: Entity<LogsModel>,
    /// Keeps the observation of the model alive.
    _logs: Subscription,
}

impl HistogramView {
    pub fn new(logs: Entity<LogsModel>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&logs, |_, _, cx| cx.notify());
        HistogramView {
            logs,
            _logs: subscription,
        }
    }

    /// The heights of the bars in pixels, left to right: `(whole bar, the
    /// errors at its bottom)`. What the next frame draws.
    pub fn heights(&self, cx: &gpui::App) -> Vec<(f32, f32)> {
        heights(&self.logs.read(cx).histogram().bars)
    }
}

/// The heights of `bars` (`(total, errors)` as fractions of the tallest) in
/// pixels, with the 2 px minimum of a bar that has a line and the 1 px stub of
/// one that has none. The errors never stand higher than the bar.
fn heights(bars: &[(f32, f32)]) -> Vec<(f32, f32)> {
    bars.iter()
        .map(|&(total, errors)| {
            if total <= 0.0 {
                return (STUB, 0.0);
            }
            let whole = (total * HEIGHT).max(MIN_BAR);
            let errors = if errors > 0.0 {
                (errors * HEIGHT).max(MIN_BAR).min(whole)
            } else {
                0.0
            };
            (whole, errors)
        })
        .collect()
}

impl Render for HistogramView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let (bars, labels) = {
            let histogram = self.logs.read(cx).histogram();
            (heights(&histogram.bars), histogram.labels.clone())
        };

        let columns = div()
            .w_full()
            .h(px(HEIGHT))
            .flex()
            .items_end()
            .gap(px(GAP))
            .children(
                bars.into_iter()
                    .enumerate()
                    .map(|(index, (whole, errors))| {
                        let rest = whole - errors;
                        // Empty buckets are the stub, in the hairline colour like the
                        // rest of a bar; the errors are at the bottom.
                        div()
                            .id(("histogram-bar", index))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .justify_end()
                            .child(div().w_full().h(px(rest)).bg(theme.border))
                            .child(div().w_full().h(px(errors)).bg(theme.error))
                    }),
            );

        // The labels sit at the start, a third, two thirds and the end of the
        // bars, like the waterfall's ruler: the first and last flush with the
        // edges, the other two centred on their place.
        let label = |index: usize, text: &String| {
            let label = div()
                .absolute()
                .top_0()
                .h_full()
                .w(px(LABEL_BOX))
                .flex()
                .items_center()
                .text_size(px(11.0))
                .text_color(theme.text_tertiary);
            let label = match index {
                0 => label.left_0().justify_start(),
                3 => label.right_0().justify_end(),
                _ => label
                    .left(relative(index as f32 / 3.0))
                    .ml(px(-LABEL_BOX / 2.0))
                    .justify_center(),
            };
            label.child(text.clone())
        };
        let labels = div()
            .relative()
            .w_full()
            .h(px(LABEL_ROW))
            .children(labels.iter().enumerate().map(|(i, text)| label(i, text)));

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(columns)
            .child(labels)
    }
}
