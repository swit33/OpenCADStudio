//! The in-place text editing surface: a canvas that draws a text run at its real
//! size (world units scaled to pixels) with a caret and a selection, and turns a
//! click into a caret offset. MText's editor and the table cell editor both draw
//! through it, so the two share one place for caret geometry, click placement and
//! selection painting.

use super::super::Message;
use iced::{Color, Theme};

pub(in crate::app) struct TextSurface {
    /// Inset between the widget's edge and the drawing area, in pixels.
    pub pad: f32,
    /// The cell's own corners in the local frame, [x0, y0, x1, y1]: stroked as
    /// the editor's frame. The table cell editor needs it because its fill
    /// covers the border the drawing put under the overlay.
    pub cell_frame: Option<[f32; 4]>,
    /// Disconnected polylines as (x, y) world points + colour (NaN-split done).
    pub segments: Vec<(Vec<(f32, f32)>, Color, f32)>,
    /// Per-visible-character boxes (world frame) for click-to-select.
    pub boxes: Vec<crate::entities::text_support::GlyphBox>,
    /// Current selection as a visible-char range.
    pub sel: Option<(usize, usize)>,
    /// Caret position as a visible-char offset.
    pub caret: usize,
    /// Whether the caret is in its visible blink phase.
    pub caret_on: bool,
    /// Colour of the caret. `None` falls back to the theme's warning colour,
    /// which suits a panel but disappears over a drawing (a pale warning colour
    /// on a light sheet leaves the user with no visible caret at all).
    pub caret_color: Option<Color>,
    /// Who owns a click on the surface. The same canvas serves the MText editor
    /// (whose caret is app-managed) and the in-place cell editor (whose typing
    /// goes to a focused sink), so a click must not be reported as the other
    /// one's.
    pub clicks: SurfaceClicks,
    /// World-space min corner (bbox) and pixels-per-world-unit scale.
    pub minx: f32,
    pub miny: f32,
    pub scale: f32,
    pub content_h: f32,
    /// On-screen width of the MText wrapping rectangle.
    pub wrap_width_px: f32,
}

impl TextSurface {
    /// The caret's colour: the caller's ink when it has one, else the theme's.
    fn caret_ink(&self, theme: &Theme) -> Color {
        self.caret_color
            .unwrap_or_else(|| theme.palette().warning.base.color)
    }

    /// Visible-char offset (0..=N) nearest the cursor point (bounds-local px).
    fn offset_at(&self, p: iced::Point) -> usize {
        if self.boxes.is_empty() {
            return 0;
        }
        let wx = self.minx + (p.x - self.pad) / self.scale;
        let wy = self.miny + (self.content_h - p.y - self.pad) / self.scale;
        let mut best = 0usize;
        let mut best_d = f32::MAX;
        for b in &self.boxes {
            let dx = if wx < b.xmin {
                b.xmin - wx
            } else if wx > b.xmax {
                wx - b.xmax
            } else {
                0.0
            };
            let dy = if wy < b.ymin {
                b.ymin - wy
            } else if wy > b.ymax {
                wy - b.ymax
            } else {
                0.0
            };
            let d = dy * 1000.0 + dx; // prefer the correct line first
            if d < best_d {
                best_d = d;
                if b.is_rtl {
                    if wx < (b.xmin + b.xmax) * 0.5 {
                        best = b.vis + 1;
                    } else {
                        best = b.vis;
                    }
                } else {
                    best = b.vis;
                    // After the glyph centre → caret sits after this char.
                    if wx > (b.xmin + b.xmax) * 0.5 {
                        best = b.vis + 1;
                    }
                }
            }
        }
        best
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::app) enum SurfaceClicks {
    /// The MText editor: clicks move its own caret and selection.
    MText,
    /// The in-place table cell editor: clicks move the focused sink's cursor.
    TableCell,
}

#[derive(Default)]
pub(in crate::app) struct TextSurfaceState {
    dragging: bool,
}

impl iced::widget::canvas::Program<Message> for TextSurface {
    type State = TextSurfaceState;

    fn update(
        &self,
        state: &mut TextSurfaceState,
        event: &iced::Event,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Option<iced::widget::canvas::Action<Message>> {
        use iced::mouse::{Button, Event as Me};
        use iced::widget::canvas::Action;
        use iced::Event;
        match event {
            Event::Mouse(Me::ButtonPressed(Button::Left)) => {
                if let Some(p) = cursor.position_in(bounds) {
                    state.dragging = true;
                    let off = self.offset_at(p);
                    let message = match self.clicks {
                        SurfaceClicks::MText => Message::MTextSelStart(off),
                        SurfaceClicks::TableCell => Message::TableCellSelStart(off),
                    };
                    return Some(Action::publish(message).and_capture());
                }
            }
            Event::Mouse(Me::CursorMoved { .. }) => {
                if state.dragging {
                    if let Some(p) = cursor.position_in(bounds) {
                        let off = self.offset_at(p);
                        let message = match self.clicks {
                            SurfaceClicks::MText => Message::MTextSelTo(off),
                            SurfaceClicks::TableCell => Message::TableCellSelTo(off),
                        };
                        return Some(Action::publish(message));
                    }
                }
            }
            Event::Mouse(Me::ButtonReleased(Button::Left)) => {
                if state.dragging {
                    state.dragging = false;
                    return Some(Action::capture());
                }
            }
            _ => {}
        }
        None
    }

    fn draw(
        &self,
        _state: &TextSurfaceState,
        renderer: &iced::Renderer,
        theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        use iced::widget::canvas::{Frame, Path, Stroke};
        let mut frame = Frame::new(renderer, bounds.size());
        let pad = self.pad;
        // Draw at the real size; flip Y (world up → screen down).
        let map = |x: f32, y: f32| {
            iced::Point::new(
                pad + (x - self.minx) * self.scale,
                self.content_h - (pad + (y - self.miny) * self.scale),
            )
        };
        // Wrap ruler: same screen-space position as the width slider handle.
        // Only the MText preview has one; a zero width means no ruler.
        if self.wrap_width_px > 0.0 {
            let wrap_x = (pad + self.wrap_width_px).clamp(pad, bounds.width.max(pad));
            let wrap_line = Path::new(|path| {
                path.move_to(iced::Point::new(wrap_x, 0.0));
                path.line_to(iced::Point::new(wrap_x, self.content_h));
            });
            frame.stroke(
                &wrap_line,
                Stroke::default()
                    .with_color(theme.palette().primary.base.color.scale_alpha(0.65))
                    .with_width(1.0),
            );
        }
        // The cell's own frame, at the exact place the table draws it.
        if let Some([x0, y0, x1, y1]) = self.cell_frame {
            let corners = [map(x0, y1), map(x1, y1), map(x1, y0), map(x0, y0)];
            let frame_path = Path::new(|p| {
                p.move_to(corners[0]);
                for corner in &corners[1..] {
                    p.line_to(*corner);
                }
                p.close();
            });
            frame.stroke(
                &frame_path,
                Stroke::default()
                    .with_color(theme.palette().primary.base.color)
                    .with_width(1.5),
            );
        }
        // Selection highlight behind the glyphs.
        if let Some((a, b)) = self.sel {
            for bx in &self.boxes {
                if bx.vis >= a && bx.vis < b {
                    let p0 = map(bx.xmin, bx.ymax);
                    let p1 = map(bx.xmax, bx.ymin);
                    let rect = Path::rectangle(
                        iced::Point::new(p0.x.min(p1.x), p0.y.min(p1.y)),
                        iced::Size::new((p1.x - p0.x).abs(), (p1.y - p0.y).abs()),
                    );
                    frame.fill(&rect, theme.palette().primary.base.color.scale_alpha(0.45));
                }
            }
        }
        for (seg, col, width) in &self.segments {
            if seg.len() < 2 {
                continue;
            }
            let path = Path::new(|p| {
                p.move_to(map(seg[0].0, seg[0].1));
                for &(x, y) in &seg[1..] {
                    p.line_to(map(x, y));
                }
            });
            frame.stroke(&path, Stroke::default().with_color(*col).with_width(*width));
        }
        // Caret — a vertical bar at the caret's glyph boundary, shown when the
        // selection is empty (a plain text cursor).
        // Caret is shown only when the selection is empty and the blink is in
        // its visible phase.
        let collapsed = self.caret_on && self.sel.map(|(a, b)| a == b).unwrap_or(true);
        if collapsed && self.boxes.is_empty() {
            // Empty text: show a caret at the top-left so the user can type.
            // Its height matches the preview's constant em (see EM_PX) so the
            // empty-editor caret isn't tiny.
            let path = Path::new(|p| {
                p.move_to(iced::Point::new(self.pad, self.pad));
                p.line_to(iced::Point::new(
                    self.pad,
                    (self.pad + 40.0).min(self.content_h),
                ));
            });
            frame.stroke(
                &path,
                Stroke::default()
                    .with_color(self.caret_ink(theme))
                    .with_width(2.0),
            );
        } else if collapsed {
            // The caret sits on a glyph's leading or trailing edge — the exact
            // world corners of that edge, so a rotated run gets a rotated
            // caret (the AABB would degenerate to a dot for a 90° run).
            let leading = |b: &crate::entities::text_support::GlyphBox| {
                // The cell's left edge [BL, TL], in reading order for RTL.
                if b.is_rtl {
                    (b.quad[1], b.quad[2])
                } else {
                    (b.quad[0], b.quad[3])
                }
            };
            let trailing = |b: &crate::entities::text_support::GlyphBox| {
                // The cell's right edge [BR, TR], in reading order for RTL.
                if b.is_rtl {
                    (b.quad[0], b.quad[3])
                } else {
                    (b.quad[1], b.quad[2])
                }
            };
            let bar = if let Some(b) = self.boxes.iter().find(|b| b.vis == self.caret) {
                Some(leading(b))
            } else if self.caret > 0 {
                self.boxes
                    .iter()
                    .find(|b| b.vis == self.caret - 1)
                    .map(trailing)
            } else {
                self.boxes.first().map(leading)
            };
            if let Some((p0, p1)) = bar {
                let s0 = map(p0.0, p0.1);
                let s1 = map(p1.0, p1.1);
                let path = Path::new(|p| {
                    p.move_to(s0);
                    p.line_to(s1);
                });
                frame.stroke(
                    &path,
                    Stroke::default()
                        .with_color(self.caret_ink(theme))
                        .with_width(2.0),
                );
            }
        }
        vec![frame.into_geometry()]
    }
}
