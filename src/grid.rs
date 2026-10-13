//! The spreadsheet grid. It is painted directly rather than built from
//! elements: only the visible rows *and* columns are touched each frame, so a
//! file with millions of rows or thousands of columns scrolls as smoothly as a
//! small one.

use std::ops::Range;

use gpui::{
    App, BorderStyle, Bounds, ContentMask, CursorStyle, DispatchPhase, Entity, Font, FontFeatures,
    FontWeight, Hitbox, HitboxBehavior, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, PathBuilder, Pixels, Point, ScrollWheelEvent, SharedString, TextRun,
    Window, canvas, fill, point, prelude::*, px, quad, size, transparent_black,
};

use crate::document::SortDir;
use crate::editor::{CsvEditor, Drag, SelKind, Selection};
use crate::theme::Theme;
use crate::value::parse_number;

pub const ROW_H: f32 = 26.0;
pub const HEADER_H: f32 = 30.0;
pub const PAD: f32 = 8.0;
const FONT_SIZE: f32 = 13.0;
const LINE_H: f32 = 18.0;
const BORDER_GRAB: f32 = 4.0;
const BUTTON_W: f32 = 20.0;
const SORT_W: f32 = 14.0;
const SCROLLBAR: f32 = 11.0;
const DRAG_THRESHOLD: f32 = 4.0;
const MAX_CELL_CHARS: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Corner,
    ColBorder(usize),
    ColButton(usize),
    ColHeader(usize),
    RowHeader(usize),
    Cell(usize, usize),
    VScroll,
    HScroll,
    Empty,
}

/// Everything needed to map between window coordinates and cells.
pub struct Geometry {
    pub origin: Point<f32>,
    pub size: Point<f32>,
    pub row_header_w: f32,
    /// Left edge of each column in content coordinates, plus the total width.
    pub col_x: Vec<f32>,
    pub rows: usize,
    pub scroll: Point<f32>,
}

impl Geometry {
    pub fn new(editor: &CsvEditor, bounds: Bounds<Pixels>) -> Self {
        let (rows, widths): (usize, Vec<f32>) = match editor.doc.as_ref() {
            Some(doc) => (
                doc.row_count(),
                doc.columns.iter().map(|c| c.width).collect(),
            ),
            None => (0, Vec::new()),
        };
        let mut col_x = Vec::with_capacity(widths.len() + 1);
        let mut x = 0.0;
        col_x.push(0.0);
        for w in widths {
            x += w;
            col_x.push(x);
        }
        let digits = (rows.max(1) as f64).log10().floor() as usize + 1;
        Self {
            origin: point(bounds.origin.x.into(), bounds.origin.y.into()),
            size: point(bounds.size.width.into(), bounds.size.height.into()),
            row_header_w: (digits as f32 * 7.5 + 22.0).max(44.0),
            col_x,
            rows,
            scroll: editor.scroll,
        }
    }

    pub fn cols(&self) -> usize {
        self.col_x.len() - 1
    }

    pub fn content_width(&self) -> f32 {
        *self.col_x.last().unwrap()
    }

    pub fn content_height(&self) -> f32 {
        self.rows as f32 * ROW_H
    }

    pub fn body_width(&self) -> f32 {
        (self.size.x - self.row_header_w).max(0.)
    }

    pub fn body_height(&self) -> f32 {
        (self.size.y - HEADER_H).max(0.)
    }

    pub fn max_scroll_x(&self) -> f32 {
        (self.content_width() + SCROLLBAR - self.body_width()).max(0.)
    }

    pub fn max_scroll_y(&self) -> f32 {
        (self.content_height() + SCROLLBAR - self.body_height()).max(0.)
    }

    pub fn col_width(&self, col: usize) -> f32 {
        self.col_x[col + 1] - self.col_x[col]
    }

    pub fn col_screen_x(&self, col: usize) -> f32 {
        self.origin.x + self.row_header_w + self.col_x[col.min(self.cols())] - self.scroll.x
    }

    pub fn row_screen_y(&self, row: usize) -> f32 {
        self.origin.y + HEADER_H + row as f32 * ROW_H - self.scroll.y
    }

    /// A cell's top-left corner relative to the grid's own origin.
    pub fn cell_origin_local(&self, row: usize, col: usize) -> Option<(f32, f32)> {
        (row < self.rows && col < self.cols()).then(|| {
            (
                self.row_header_w + self.col_x[col] - self.scroll.x,
                HEADER_H + row as f32 * ROW_H - self.scroll.y,
            )
        })
    }

    pub fn visible_rows(&self) -> Range<usize> {
        let first = (self.scroll.y / ROW_H).floor().max(0.) as usize;
        let last = ((self.scroll.y + self.body_height()) / ROW_H).ceil() as usize;
        first.min(self.rows)..last.min(self.rows)
    }

    pub fn visible_cols(&self) -> Range<usize> {
        let left = self.scroll.x;
        let right = self.scroll.x + self.body_width();
        let first = self.col_x[1..].partition_point(|&x| x <= left);
        let last = self.col_x.partition_point(|&x| x < right);
        first.min(self.cols())..last.min(self.cols())
    }

    fn col_at_content(&self, x: f32) -> Option<usize> {
        if x < 0. || x >= self.content_width() {
            return None;
        }
        Some(self.col_x.partition_point(|&v| v <= x) - 1)
    }

    fn has_vbar(&self) -> bool {
        self.content_height() + SCROLLBAR > self.body_height()
    }

    fn has_hbar(&self) -> bool {
        self.content_width() + SCROLLBAR > self.body_width()
    }

    /// The vertical scrollbar thumb as (top, height) in window coordinates.
    pub fn vthumb(&self) -> Option<(f32, f32)> {
        if !self.has_vbar() {
            return None;
        }
        let track_top = self.origin.y + HEADER_H + 2.;
        let track = self.body_height() - 4. - if self.has_hbar() { SCROLLBAR } else { 0. };
        let content = self.content_height() + SCROLLBAR;
        let len = (track * self.body_height() / content).clamp(24., track.max(24.));
        let max = self.max_scroll_y().max(1.);
        Some((track_top + (track - len) * (self.scroll.y / max), len))
    }

    pub fn hthumb(&self) -> Option<(f32, f32)> {
        if !self.has_hbar() {
            return None;
        }
        let track_left = self.origin.x + self.row_header_w + 2.;
        let track = self.body_width() - 4. - if self.has_vbar() { SCROLLBAR } else { 0. };
        let content = self.content_width() + SCROLLBAR;
        let len = (track * self.body_width() / content).clamp(24., track.max(24.));
        let max = self.max_scroll_x().max(1.);
        Some((track_left + (track - len) * (self.scroll.x / max), len))
    }

    pub fn scroll_y_for_thumb(&self, thumb_top: f32) -> f32 {
        let track_top = self.origin.y + HEADER_H + 2.;
        let track = self.body_height() - 4. - if self.has_hbar() { SCROLLBAR } else { 0. };
        let (_, len) = self.vthumb().unwrap_or((0., 0.));
        ((thumb_top - track_top) / (track - len).max(1.)) * self.max_scroll_y()
    }

    pub fn scroll_x_for_thumb(&self, thumb_left: f32) -> f32 {
        let track_left = self.origin.x + self.row_header_w + 2.;
        let track = self.body_width() - 4. - if self.has_vbar() { SCROLLBAR } else { 0. };
        let (_, len) = self.hthumb().unwrap_or((0., 0.));
        ((thumb_left - track_left) / (track - len).max(1.)) * self.max_scroll_x()
    }

    pub fn hit(&self, pos: Point<Pixels>) -> Hit {
        let x = f32::from(pos.x) - self.origin.x;
        let y = f32::from(pos.y) - self.origin.y;
        if x < 0. || y < 0. || x > self.size.x || y > self.size.y {
            return Hit::Empty;
        }
        if self.has_vbar() && x >= self.size.x - SCROLLBAR && y >= HEADER_H {
            return Hit::VScroll;
        }
        if self.has_hbar() && y >= self.size.y - SCROLLBAR && x >= self.row_header_w {
            return Hit::HScroll;
        }
        let cx = x - self.row_header_w + self.scroll.x;
        let cy = y - HEADER_H + self.scroll.y;
        if y < HEADER_H {
            if x < self.row_header_w {
                return Hit::Corner;
            }
            // Resize handles straddle each column's right edge.
            let i = self.col_x.partition_point(|&v| v < cx - BORDER_GRAB);
            if i >= 1 && i < self.col_x.len() && (self.col_x[i] - cx).abs() <= BORDER_GRAB {
                return Hit::ColBorder(i - 1);
            }
            return match self.col_at_content(cx) {
                Some(c) if self.col_x[c + 1] - cx <= BUTTON_W + 2. => Hit::ColButton(c),
                Some(c) => Hit::ColHeader(c),
                None => Hit::Empty,
            };
        }
        let row = (cy / ROW_H).floor();
        if row < 0. || row as usize >= self.rows {
            return Hit::Empty;
        }
        let row = row as usize;
        if x < self.row_header_w {
            return Hit::RowHeader(row);
        }
        match self.col_at_content(cx) {
            Some(c) => Hit::Cell(row, c),
            None => Hit::Empty,
        }
    }

    /// The nearest cell to a point, even outside the body.
    pub fn cell_clamped(&self, pos: Point<Pixels>) -> (usize, usize) {
        let cx = f32::from(pos.x) - self.origin.x - self.row_header_w + self.scroll.x;
        let cy = f32::from(pos.y) - self.origin.y - HEADER_H + self.scroll.y;
        let row = ((cy / ROW_H).floor().max(0.) as usize).min(self.rows.saturating_sub(1));
        let col = self
            .col_at_content(cx.clamp(0., (self.content_width() - 0.5).max(0.)))
            .unwrap_or(0);
        (row, col)
    }

    /// The column boundary a column being dragged at `x` would drop onto.
    pub fn drop_col(&self, x: f32) -> usize {
        let cx = x - self.origin.x - self.row_header_w + self.scroll.x;
        (0..self.cols())
            .filter(|&c| (self.col_x[c] + self.col_x[c + 1]) / 2. < cx)
            .count()
    }

    pub fn drop_row(&self, y: f32) -> usize {
        let cy = y - self.origin.y - HEADER_H + self.scroll.y;
        (((cy + ROW_H / 2.) / ROW_H).floor().max(0.) as usize).min(self.rows)
    }
}

fn b(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w.max(0.)), px(h.max(0.))))
}

#[derive(Clone, Copy, PartialEq)]
enum Weight {
    Regular,
    Semibold,
}

struct TextItem {
    text: SharedString,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    right: bool,
    color: Hsla,
    weight: Weight,
}

#[derive(Default)]
struct Layer {
    clip: Option<Bounds<Pixels>>,
    quads: Vec<PaintQuad>,
    texts: Vec<TextItem>,
    paths: Vec<(gpui::Path<Pixels>, Hsla)>,
    overlay: Vec<PaintQuad>,
}

pub struct PrepaintState {
    hitbox: Hitbox,
    borders: Vec<Hitbox>,
}

pub fn grid(entity: Entity<CsvEditor>) -> impl IntoElement {
    let prepaint_entity = entity.clone();
    canvas(
        move |bounds, window, cx| prepaint(&prepaint_entity, bounds, window, cx),
        move |bounds, state, window, cx| paint(&entity, bounds, state, window, cx),
    )
    .size_full()
}

fn prepaint(
    entity: &Entity<CsvEditor>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> PrepaintState {
    entity.update(cx, |editor, _| {
        editor.grid_bounds = bounds;
        let (x, y) = (editor.scroll.x, editor.scroll.y);
        editor.set_scroll(x, y);
    });
    let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
    let editor = entity.read(cx);
    let geo = editor.geometry(bounds);
    let mut borders = Vec::new();
    if editor.drag.is_none() {
        for c in geo.visible_cols() {
            let x = geo.col_screen_x(c + 1);
            borders.push(window.insert_hitbox(
                b(x - BORDER_GRAB, geo.origin.y, BORDER_GRAB * 2., HEADER_H),
                HitboxBehavior::Normal,
            ));
        }
    }
    PrepaintState { hitbox, borders }
}

fn display_text(s: &str) -> SharedString {
    let mut text: String =
        if s.len() > MAX_CELL_CHARS * 4 || s.chars().nth(MAX_CELL_CHARS).is_some() {
            s.chars().take(MAX_CELL_CHARS).collect()
        } else {
            s.to_string()
        };
    if text.contains(['\n', '\r']) {
        text = text
            .split(['\n', '\r'])
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" \u{21b5} ");
    }
    if text.contains('\t') {
        text = text.replace('\t', " ");
    }
    text.into()
}

fn build_layers(editor: &CsvEditor, geo: &Geometry, theme: &Theme) -> Vec<Layer> {
    let doc = editor.doc.as_ref().unwrap();
    let rows = geo.visible_rows();
    let cols = geo.visible_cols();
    let (ox, oy) = (geo.origin.x, geo.origin.y);
    let (w, h) = (geo.size.x, geo.size.y);
    let rh = geo.row_header_w;
    let body_x = ox + rh;
    let body_y = oy + HEADER_H;
    let content_right = (geo.col_screen_x(geo.cols())).min(ox + w);
    let content_bottom = (geo.row_screen_y(geo.rows)).min(oy + h);
    let selection = editor.selection;
    let n_rows = doc.row_count();
    let n_cols = doc.col_count();
    let sel_rows = selection.map(|s| s.rows(n_rows));
    let sel_cols = selection.map(|s| s.cols(n_cols));

    // ----- body -----
    let mut body = Layer {
        clip: Some(b(body_x, body_y, w - rh, h - HEADER_H)),
        ..Default::default()
    };
    for r in rows.clone() {
        let y = geo.row_screen_y(r);
        if r % 2 == 1 {
            body.quads.push(fill(
                b(body_x, y, content_right - body_x, ROW_H),
                theme.zebra,
            ));
        }
    }
    if let (Some(sel), Some(sr), Some(sc)) = (selection, sel_rows.clone(), sel_cols.clone())
        && !sel.is_single()
        && !sr.is_empty()
        && !sc.is_empty()
    {
        let x0 = geo.col_screen_x(sc.start);
        let x1 = geo.col_screen_x(sc.end);
        let y0 = geo.row_screen_y(sr.start).max(body_y - 2.);
        let y1 = geo.row_screen_y(sr.end).min(oy + h + 2.);
        body.quads
            .push(fill(b(x0, y0, x1 - x0, y1 - y0), theme.selection));
    }
    // Grid lines.
    for r in rows.start..=rows.end {
        let y = geo.row_screen_y(r);
        if r > 0 {
            body.quads.push(fill(
                b(body_x, y - 0.5, content_right - body_x, 1.),
                theme.grid_line,
            ));
        }
    }
    for c in cols.start..=cols.end.min(geo.cols()) {
        let x = geo.col_screen_x(c);
        if c > 0 {
            body.quads.push(fill(
                b(x - 0.5, body_y, 1., content_bottom - body_y),
                theme.grid_line,
            ));
        }
    }
    for r in rows.clone() {
        let y = geo.row_screen_y(r);
        for c in cols.clone() {
            let value = doc.cell(r, c);
            if value.is_empty() {
                continue;
            }
            let x = geo.col_screen_x(c);
            let width = geo.col_width(c);
            body.texts.push(TextItem {
                text: display_text(value),
                x: x + PAD,
                y,
                width: width - PAD * 2.,
                height: ROW_H,
                right: parse_number(value).is_some(),
                color: theme.text,
                weight: Weight::Regular,
            });
        }
    }
    if let (Some(sel), Some(sr), Some(sc)) = (selection, sel_rows.clone(), sel_cols.clone())
        && !sr.is_empty()
        && !sc.is_empty()
    {
        if !sel.is_single() {
            let x0 = geo.col_screen_x(sc.start);
            let x1 = geo.col_screen_x(sc.end);
            let y0 = geo.row_screen_y(sr.start).max(body_y - 2.);
            let y1 = geo.row_screen_y(sr.end).min(oy + h + 2.);
            body.overlay.push(quad(
                b(x0 - 0.5, y0 - 0.5, x1 - x0 + 1., y1 - y0 + 1.),
                px(0.),
                transparent_black(),
                px(1.),
                theme.accent,
                BorderStyle::Solid,
            ));
        }
        let (ar, ac) = sel.anchor;
        if editor.editing.is_none() && ar < n_rows && ac < n_cols {
            let x = geo.col_screen_x(ac);
            let y = geo.row_screen_y(ar);
            body.overlay.push(quad(
                b(x - 1., y - 1., geo.col_width(ac) + 2., ROW_H + 2.),
                px(1.),
                transparent_black(),
                px(2.),
                theme.accent,
                BorderStyle::Solid,
            ));
        }
    }

    // ----- column headers -----
    let mut header = Layer {
        clip: Some(b(body_x, oy, w - rh, HEADER_H)),
        ..Default::default()
    };
    header
        .quads
        .push(fill(b(body_x, oy, w - rh, HEADER_H), theme.header_bg));
    for c in cols.clone() {
        let x = geo.col_screen_x(c);
        let width = geo.col_width(c);
        let selected = sel_cols.as_ref().is_some_and(|r| r.contains(&c));
        let whole = selection.is_some_and(|s| s.kind == SelKind::Cols) && selected;
        if selected {
            header.quads.push(fill(
                b(x, oy, width, HEADER_H),
                if whole {
                    theme.selection_header
                } else {
                    theme.selection
                },
            ));
        }
        header.quads.push(fill(
            b(x + width - 0.5, oy + 6., 1., HEADER_H - 12.),
            theme.border,
        ));
        let sort = doc.sort_dir(c);
        let filtered = doc.filter(c).is_some();
        let hovered =
            editor.hover_col == Some(c) || editor.filter.as_ref().is_some_and(|f| f.col == c);
        let mut text_w = width - PAD * 2. - BUTTON_W;
        if let Some(dir) = sort {
            text_w -= SORT_W;
            header.texts.push(TextItem {
                text: if dir == SortDir::Asc {
                    "\u{2191}"
                } else {
                    "\u{2193}"
                }
                .into(),
                x: x + width - BUTTON_W - SORT_W - 2.,
                y: oy,
                width: SORT_W,
                height: HEADER_H,
                right: false,
                color: theme.accent,
                weight: Weight::Semibold,
            });
        }
        let name = &doc.columns[c].name;
        header.texts.push(TextItem {
            text: display_text(name),
            x: x + PAD,
            y: oy,
            width: text_w,
            height: HEADER_H,
            right: false,
            color: if selected {
                theme.accent
            } else {
                theme.header_text
            },
            weight: Weight::Semibold,
        });
        let bx = x + width - BUTTON_W - 2.;
        let cy = oy + HEADER_H / 2.;
        if filtered {
            header.quads.push(quad(
                b(bx, cy - 9., BUTTON_W - 2., 18.),
                px(4.),
                theme.selection_header,
                px(0.),
                transparent_black(),
                BorderStyle::Solid,
            ));
            if let Some(path) = funnel(bx + (BUTTON_W - 2.) / 2., cy) {
                header.paths.push((path, theme.accent));
            }
        } else if hovered {
            header.quads.push(quad(
                b(bx, cy - 9., BUTTON_W - 2., 18.),
                px(4.),
                theme.hover,
                px(0.),
                transparent_black(),
                BorderStyle::Solid,
            ));
            if let Some(path) = chevron(bx + (BUTTON_W - 2.) / 2., cy) {
                header.paths.push((path, theme.text_muted));
            }
        }
    }
    header.quads.push(fill(
        b(body_x, oy + HEADER_H - 1., w - rh, 1.),
        theme.border,
    ));

    // ----- row headers -----
    let mut row_header = Layer {
        clip: Some(b(ox, body_y, rh, h - HEADER_H)),
        ..Default::default()
    };
    row_header
        .quads
        .push(fill(b(ox, body_y, rh, h - HEADER_H), theme.header_bg));
    for r in rows.clone() {
        let y = geo.row_screen_y(r);
        let selected = sel_rows.as_ref().is_some_and(|s| s.contains(&r));
        let whole = selection.is_some_and(|s| s.kind == SelKind::Rows) && selected;
        if selected {
            row_header.quads.push(fill(
                b(ox, y, rh, ROW_H),
                if whole {
                    theme.selection_header
                } else {
                    theme.selection
                },
            ));
        }
        let has_issue = !editor.issue_rows.is_empty() && editor.issue_rows.contains(&doc.row_id(r));
        if has_issue {
            row_header
                .quads
                .push(fill(b(ox, y + 3., 3., ROW_H - 6.), theme.error));
        }
        if r > 0 {
            row_header
                .quads
                .push(fill(b(ox, y - 0.5, rh, 1.), theme.grid_line));
        }
        row_header.texts.push(TextItem {
            text: (r + 1).to_string().into(),
            x: ox + 4.,
            y,
            width: rh - 4. - PAD,
            height: ROW_H,
            right: true,
            color: if has_issue {
                theme.error
            } else if selected {
                theme.accent
            } else {
                theme.text_faint
            },
            weight: Weight::Regular,
        });
    }
    row_header.quads.push(fill(
        b(ox + rh - 1., body_y, 1., h - HEADER_H),
        theme.border,
    ));

    // ----- corner, drag feedback, scrollbars -----
    let mut top = Layer {
        clip: Some(b(ox, oy, w, h)),
        ..Default::default()
    };
    top.quads
        .push(fill(b(ox, oy, rh, HEADER_H), theme.header_bg));
    top.quads
        .push(fill(b(ox, oy + HEADER_H - 1., rh, 1.), theme.border));
    top.quads
        .push(fill(b(ox + rh - 1., oy, 1., HEADER_H), theme.border));

    match &editor.drag {
        Some(Drag::Resize { col, .. }) if *col < geo.cols() => {
            let x = geo.col_screen_x(col + 1);
            top.quads
                .push(fill(b(x - 1., oy, 2., content_bottom - oy), theme.accent));
        }
        Some(Drag::MoveCols {
            cols: moving,
            grab,
            pointer,
        }) => {
            let x = f32::from(pointer.x) - grab;
            let width: f32 = moving.clone().map(|c| geo.col_width(c)).sum();
            top.quads.push(quad(
                b(x, oy, width, content_bottom - oy),
                px(2.),
                theme.accent.opacity(0.08),
                px(1.),
                theme.accent.opacity(0.6),
                BorderStyle::Solid,
            ));
            top.quads
                .push(fill(b(x, oy, width, HEADER_H), theme.selection_header));
            top.texts.push(TextItem {
                text: display_text(&doc.columns[moving.start].name),
                x: x + PAD,
                y: oy,
                width: width - PAD * 2.,
                height: HEADER_H,
                right: false,
                color: theme.accent,
                weight: Weight::Semibold,
            });
            let to = geo.drop_col(f32::from(pointer.x));
            if to < moving.start || to > moving.end {
                let lx = geo.col_screen_x(to);
                top.quads
                    .push(fill(b(lx - 1.5, oy, 3., content_bottom - oy), theme.accent));
            }
        }
        Some(Drag::MoveRows {
            rows: moving,
            grab,
            pointer,
        }) => {
            let y = f32::from(pointer.y) - grab;
            let height = moving.len() as f32 * ROW_H;
            top.quads.push(quad(
                b(ox, y, content_right - ox, height),
                px(2.),
                theme.accent.opacity(0.08),
                px(1.),
                theme.accent.opacity(0.6),
                BorderStyle::Solid,
            ));
            let to = geo.drop_row(f32::from(pointer.y));
            if to < moving.start || to > moving.end {
                let ly = geo.row_screen_y(to);
                top.quads
                    .push(fill(b(ox, ly - 1.5, content_right - ox, 3.), theme.accent));
            }
        }
        _ => {}
    }

    let active_v = matches!(editor.drag, Some(Drag::ScrollV { .. }));
    let active_h = matches!(editor.drag, Some(Drag::ScrollH { .. }));
    if let Some((ty, len)) = geo.vthumb() {
        top.quads.push(quad(
            b(ox + w - SCROLLBAR + 3., ty, SCROLLBAR - 5., len),
            px(3.),
            if active_v {
                theme.scrollbar_active
            } else {
                theme.scrollbar
            },
            px(0.),
            transparent_black(),
            BorderStyle::Solid,
        ));
    }
    if let Some((tx, len)) = geo.hthumb() {
        top.quads.push(quad(
            b(tx, oy + h - SCROLLBAR + 3., len, SCROLLBAR - 5.),
            px(3.),
            if active_h {
                theme.scrollbar_active
            } else {
                theme.scrollbar
            },
            px(0.),
            transparent_black(),
            BorderStyle::Solid,
        ));
    }

    vec![body, header, row_header, top]
}

fn funnel(cx: f32, cy: f32) -> Option<gpui::Path<Pixels>> {
    let mut p = PathBuilder::fill();
    p.move_to(point(px(cx - 5.), px(cy - 4.)));
    p.line_to(point(px(cx + 5.), px(cy - 4.)));
    p.line_to(point(px(cx + 1.2), px(cy + 0.5)));
    p.line_to(point(px(cx + 1.2), px(cy + 4.5)));
    p.line_to(point(px(cx - 1.2), px(cy + 3.3)));
    p.line_to(point(px(cx - 1.2), px(cy + 0.5)));
    p.close();
    p.build().ok()
}

fn chevron(cx: f32, cy: f32) -> Option<gpui::Path<Pixels>> {
    let mut p = PathBuilder::stroke(px(1.5));
    p.move_to(point(px(cx - 3.5), px(cy - 1.5)));
    p.line_to(point(px(cx), px(cy + 2.)));
    p.line_to(point(px(cx + 3.5), px(cy - 1.5)));
    p.build().ok()
}

fn paint(
    entity: &Entity<CsvEditor>,
    bounds: Bounds<Pixels>,
    state: PrepaintState,
    window: &mut Window,
    cx: &mut App,
) {
    let theme = Theme::get(cx);
    let (layers, cursor) = {
        let editor = entity.read(cx);
        let geo = editor.geometry(bounds);
        let cursor = match editor.drag {
            Some(Drag::Resize { .. }) => Some(CursorStyle::ResizeLeftRight),
            Some(Drag::MoveCols { .. }) | Some(Drag::MoveRows { .. }) => {
                Some(CursorStyle::ClosedHand)
            }
            _ => None,
        };
        (build_layers(editor, &geo, &theme), cursor)
    };

    window.paint_quad(fill(bounds, theme.bg));
    let regular = Font {
        features: FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)])),
        ..gpui::font(".SystemUIFont")
    };
    let semibold = Font {
        weight: FontWeight::SEMIBOLD,
        ..regular.clone()
    };
    let font_size = px(FONT_SIZE);
    let ellipsis = "\u{2026}";
    let ellipsis_w = f32::from(
        window
            .text_system()
            .shape_line(
                ellipsis.into(),
                font_size,
                &[run(ellipsis.len(), &regular, theme.text)],
                None,
            )
            .width,
    );

    for layer in layers {
        window.with_content_mask(layer.clip.map(|bounds| ContentMask { bounds }), |window| {
            for q in layer.quads {
                window.paint_quad(q);
            }
            for item in layer.texts {
                let font = if item.weight == Weight::Semibold {
                    &semibold
                } else {
                    &regular
                };
                if item.width <= 4. {
                    continue;
                }
                let system = window.text_system().clone();
                let mut line = system.shape_line(
                    item.text.clone(),
                    font_size,
                    &[run(item.text.len(), font, item.color)],
                    None,
                );
                if f32::from(line.width) > item.width {
                    let target = (item.width - ellipsis_w).max(0.);
                    let mut ix = line.closest_index_for_x(px(target));
                    while ix > 0 && f32::from(line.x_for_index(ix)) > target {
                        ix = item.text[..ix]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                    }
                    let truncated: SharedString =
                        format!("{}{ellipsis}", item.text[..ix].trim_end()).into();
                    line = system.shape_line(
                        truncated.clone(),
                        font_size,
                        &[run(truncated.len(), font, item.color)],
                        None,
                    );
                }
                let x = if item.right {
                    item.x + item.width - f32::from(line.width)
                } else {
                    item.x
                };
                let y = item.y + (item.height - LINE_H) / 2.;
                line.paint(
                    point(px(x), px(y)),
                    px(LINE_H),
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            }
            for (path, color) in layer.paths {
                window.paint_path(path, color);
            }
            for q in layer.overlay {
                window.paint_quad(q);
            }
        });
    }

    for border in &state.borders {
        window.set_cursor_style(CursorStyle::ResizeLeftRight, border);
    }
    if let Some(cursor) = cursor {
        window.set_window_cursor_style(cursor);
    }

    let hitbox = state.hitbox;
    window.on_mouse_event({
        let entity = entity.clone();
        let hitbox = hitbox.clone();
        move |ev: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                return;
            }
            entity.update(cx, |this, cx| this.grid_mouse_down(ev, window, cx));
            // The grid manages focus itself (e.g. into the cell editor), so stop
            // the root view from taking it back.
            window.prevent_default();
        }
    });
    window.on_mouse_event({
        let entity = entity.clone();
        let hitbox = hitbox.clone();
        move |ev: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let hovered = hitbox.is_hovered(window);
            entity.update(cx, |this, cx| this.grid_mouse_move(ev, hovered, cx));
        }
    });
    window.on_mouse_event({
        let entity = entity.clone();
        move |ev: &MouseUpEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble || ev.button != MouseButton::Left {
                return;
            }
            entity.update(cx, |this, cx| this.finish_drag(cx));
        }
    });
    window.on_mouse_event({
        let entity = entity.clone();
        move |ev: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.should_handle_scroll(window) {
                return;
            }
            entity.update(cx, |this, cx| this.grid_scroll(ev, cx));
        }
    });
}

fn run(len: usize, font: &Font, color: Hsla) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

// ----- mouse handling -----

impl CsvEditor {
    fn grid_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.editing.is_some() {
            self.commit_edit(window, cx);
        }
        window.focus(&self.focus, cx);
        self.menu = None;
        let geo = self.geometry(self.grid_bounds);
        let hit = geo.hit(ev.position);
        let (rows, cols) = self
            .doc
            .as_ref()
            .map(|d| (d.row_count(), d.col_count()))
            .unwrap_or((0, 0));
        let pos_x = f32::from(ev.position.x);
        let pos_y = f32::from(ev.position.y);
        match ev.button {
            MouseButton::Left => match hit {
                Hit::VScroll => {
                    if let Some((top, len)) = geo.vthumb() {
                        let grab = if pos_y >= top && pos_y <= top + len {
                            pos_y - top
                        } else {
                            let grab = len / 2.;
                            let y = geo.scroll_y_for_thumb(pos_y - grab);
                            self.set_scroll(self.scroll.x, y);
                            grab
                        };
                        self.drag = Some(Drag::ScrollV { grab });
                    }
                }
                Hit::HScroll => {
                    if let Some((left, len)) = geo.hthumb() {
                        let grab = if pos_x >= left && pos_x <= left + len {
                            pos_x - left
                        } else {
                            let grab = len / 2.;
                            let x = geo.scroll_x_for_thumb(pos_x - grab);
                            self.set_scroll(x, self.scroll.y);
                            grab
                        };
                        self.drag = Some(Drag::ScrollH { grab });
                    }
                }
                Hit::Corner => self.select_all(cx),
                Hit::ColBorder(c) => {
                    if ev.click_count >= 2 {
                        self.autofit_column(c);
                    } else {
                        self.drag = Some(Drag::Resize {
                            col: c,
                            start_x: pos_x,
                            start_width: geo.col_width(c),
                        });
                    }
                }
                Hit::ColButton(c) => {
                    if self.filter.as_ref().is_some_and(|f| f.col == c) {
                        self.filter = None;
                    } else {
                        self.open_filter(c, window, cx);
                    }
                }
                Hit::ColHeader(c) => {
                    if ev.modifiers.shift || ev.modifiers.platform {
                        let anchor = match self.selection {
                            Some(s) if ev.modifiers.shift && s.kind == SelKind::Cols => s.anchor,
                            _ => (0, c),
                        };
                        self.selection = Some(Selection {
                            kind: SelKind::Cols,
                            anchor,
                            head: (0, c),
                        });
                    } else {
                        self.drag = Some(Drag::PressHeader {
                            col: c,
                            start: ev.position,
                        });
                    }
                }
                Hit::RowHeader(r) => {
                    let current = self.selection.filter(|s| s.kind == SelKind::Rows);
                    if ev.modifiers.shift {
                        let anchor = current.map(|s| s.anchor).unwrap_or((r, 0));
                        self.selection = Some(Selection {
                            kind: SelKind::Rows,
                            anchor,
                            head: (r, 0),
                        });
                    } else {
                        if !current.is_some_and(|s| s.rows(rows).contains(&r)) {
                            let col = self.selection.map(|s| s.anchor.1).unwrap_or(0);
                            self.selection = Some(Selection {
                                kind: SelKind::Rows,
                                anchor: (r, col.min(cols.saturating_sub(1))),
                                head: (r, 0),
                            });
                        }
                        self.drag = Some(Drag::PressRowHeader {
                            row: r,
                            start: ev.position,
                        });
                    }
                }
                Hit::Cell(r, c) => {
                    if ev.click_count >= 2 {
                        self.start_edit(crate::editor::EditTarget::Cell(r, c), None, window, cx);
                        return;
                    }
                    match self.selection {
                        Some(mut s) if ev.modifiers.shift => {
                            if s.kind != SelKind::Cells {
                                s = Selection {
                                    kind: SelKind::Cells,
                                    anchor: (s.rows(rows).start, s.cols(cols).start),
                                    head: s.head,
                                };
                            }
                            s.head = (r, c);
                            self.selection = Some(s);
                        }
                        _ => self.selection = Some(Selection::cell(r, c)),
                    }
                    self.drag = Some(Drag::Select);
                }
                Hit::Empty => {}
            },
            MouseButton::Right => {
                let entries = match hit {
                    Hit::ColHeader(c) | Hit::ColButton(c) | Hit::ColBorder(c) => {
                        self.column_menu(c)
                    }
                    Hit::RowHeader(r) => {
                        if !self.selection.is_some_and(|s| s.rows(rows).contains(&r)) {
                            self.selection = Some(Selection {
                                kind: SelKind::Rows,
                                anchor: (r, 0),
                                head: (r, 0),
                            });
                        }
                        self.row_menu(r)
                    }
                    Hit::Cell(r, c) => {
                        if !self.selection.is_some_and(|s| s.contains(r, c, rows, cols)) {
                            self.selection = Some(Selection::cell(r, c));
                        }
                        self.cell_menu(r, c)
                    }
                    _ => {
                        use crate::editor::MenuAction;
                        let mut entries = Vec::new();
                        entries.push(crate::editor::MenuEntry::Item {
                            label: "Add Row".into(),
                            shortcut: None,
                            action: MenuAction::InsertRowBelow(rows.saturating_sub(1)),
                            danger: false,
                        });
                        entries.push(crate::editor::MenuEntry::Item {
                            label: "Add Column".into(),
                            shortcut: None,
                            action: if cols == 0 {
                                MenuAction::InsertColLeft(0)
                            } else {
                                MenuAction::InsertColRight(cols - 1)
                            },
                            danger: false,
                        });
                        entries
                    }
                };
                self.open_menu(ev.position, entries, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn grid_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        hovered: bool,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.drag.is_some() && ev.pressed_button != Some(MouseButton::Left) {
            self.finish_drag(cx);
            return;
        }
        let geo = self.geometry(self.grid_bounds);
        let (rows, cols) = self
            .doc
            .as_ref()
            .map(|d| (d.row_count(), d.col_count()))
            .unwrap_or((0, 0));
        let moved = |start: Point<Pixels>| {
            let d = ev.position - start;
            f32::from(d.x).abs() > DRAG_THRESHOLD || f32::from(d.y).abs() > DRAG_THRESHOLD
        };
        match self.drag.clone() {
            Some(Drag::Resize {
                col,
                start_x,
                start_width,
            }) => {
                self.resize_column(col, start_width + f32::from(ev.position.x) - start_x);
                cx.notify();
            }
            Some(Drag::PressHeader { col, start }) => {
                if moved(start) {
                    let range = self
                        .selection
                        .filter(|s| s.kind == SelKind::Cols && s.cols(cols).contains(&col))
                        .map(|s| s.cols(cols))
                        .unwrap_or(col..col + 1);
                    let grab = f32::from(start.x) - geo.col_screen_x(range.start);
                    self.selection = Some(Selection {
                        kind: SelKind::Cols,
                        anchor: (0, range.start),
                        head: (0, range.end - 1),
                    });
                    self.drag = Some(Drag::MoveCols {
                        cols: range,
                        grab,
                        pointer: ev.position,
                    });
                    cx.notify();
                }
            }
            Some(Drag::PressRowHeader { row, start }) => {
                if moved(start) {
                    let range = self
                        .selection
                        .filter(|s| s.kind == SelKind::Rows && s.rows(rows).contains(&row))
                        .map(|s| s.rows(rows))
                        .unwrap_or(row..row + 1);
                    let grab = f32::from(start.y) - geo.row_screen_y(range.start);
                    self.drag = Some(Drag::MoveRows {
                        rows: range,
                        grab,
                        pointer: ev.position,
                    });
                    cx.notify();
                }
            }
            Some(Drag::MoveCols { cols, grab, .. }) => {
                self.drag = Some(Drag::MoveCols {
                    cols,
                    grab,
                    pointer: ev.position,
                });
                cx.notify();
            }
            Some(Drag::MoveRows { rows, grab, .. }) => {
                self.drag = Some(Drag::MoveRows {
                    rows,
                    grab,
                    pointer: ev.position,
                });
                cx.notify();
            }
            Some(Drag::Select) => {
                if rows == 0 || cols == 0 {
                    return;
                }
                let (r, c) = geo.cell_clamped(ev.position);
                if let Some(mut s) = self.selection
                    && s.head != (r, c)
                {
                    s.head = (r, c);
                    self.selection = Some(s);
                    self.scroll_cell_into_view(r, c);
                    cx.notify();
                }
            }
            Some(Drag::ScrollV { grab }) => {
                let y = geo.scroll_y_for_thumb(f32::from(ev.position.y) - grab);
                self.set_scroll(self.scroll.x, y);
                cx.notify();
            }
            Some(Drag::ScrollH { grab }) => {
                let x = geo.scroll_x_for_thumb(f32::from(ev.position.x) - grab);
                self.set_scroll(x, self.scroll.y);
                cx.notify();
            }
            None => {
                let hover = if hovered {
                    match geo.hit(ev.position) {
                        Hit::ColHeader(c) | Hit::ColButton(c) | Hit::ColBorder(c) => Some(c),
                        _ => None,
                    }
                } else {
                    None
                };
                if hover != self.hover_col {
                    self.hover_col = hover;
                    cx.notify();
                }
            }
        }
    }

    fn scroll_cell_into_view(&mut self, row: usize, col: usize) {
        let geo = self.geometry(self.grid_bounds);
        let (mut x, mut y) = (self.scroll.x, self.scroll.y);
        let top = row as f32 * ROW_H;
        if top < y {
            y = top;
        } else if top + ROW_H > y + geo.body_height() - SCROLLBAR {
            y = top + ROW_H - geo.body_height() + SCROLLBAR;
        }
        let (left, right) = (geo.col_x[col], geo.col_x[col + 1]);
        if left < x {
            x = left;
        } else if right > x + geo.body_width() - SCROLLBAR {
            x = (right - geo.body_width() + SCROLLBAR).min(left);
        }
        self.set_scroll(x, y);
    }

    fn grid_scroll(&mut self, ev: &ScrollWheelEvent, cx: &mut gpui::Context<Self>) {
        let delta = ev.delta.pixel_delta(px(ROW_H));
        let (mut dx, mut dy) = (f32::from(delta.x), f32::from(delta.y));
        if ev.modifiers.shift && dx == 0. {
            dx = dy;
            dy = 0.;
        }
        let before = self.scroll;
        self.set_scroll(self.scroll.x - dx, self.scroll.y - dy);
        if self.scroll != before {
            cx.notify();
        }
    }
}
