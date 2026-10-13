//! The window's root view: one CSV document with its grid, editing,
//! menus, filter panel and problem list.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, Corner, Entity, ExternalPaths, FocusHandle,
    Focusable, KeyDownEvent, Pixels, Point, PromptLevel, SharedString, Subscription,
    UniformListScrollHandle, Window, anchored, deferred, div, point, prelude::*, px, uniform_list,
};

use crate::actions::*;
use crate::document::{Document, SortDir};
use crate::filter::{Filter, ValueSet};
use crate::filter_panel::{FilterPanel, FilterPanelEvent};
use crate::grid::{self, Geometry, HEADER_H, ROW_H};
use crate::parse::{Issue, parse_bytes, parse_str};
use crate::store::RowId;
use crate::text_input::{Direction, TextInput, TextInputEvent};
use crate::theme::Theme;
use crate::value::parse_number;

pub const CONTEXT: &str = "CsvEditor";
const MIN_COL_WIDTH: f32 = 36.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelKind {
    Cells,
    Rows,
    Cols,
}

/// A rectangular selection. `anchor` is the active cell; `head` is the
/// corner that moves when the selection is extended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub kind: SelKind,
    pub anchor: (usize, usize),
    pub head: (usize, usize),
}

impl Selection {
    pub fn cell(row: usize, col: usize) -> Self {
        Self {
            kind: SelKind::Cells,
            anchor: (row, col),
            head: (row, col),
        }
    }

    pub fn rows(&self, row_count: usize) -> Range<usize> {
        match self.kind {
            SelKind::Cols => 0..row_count,
            _ => self.anchor.0.min(self.head.0)..self.anchor.0.max(self.head.0) + 1,
        }
    }

    pub fn cols(&self, col_count: usize) -> Range<usize> {
        match self.kind {
            SelKind::Rows => 0..col_count,
            _ => self.anchor.1.min(self.head.1)..self.anchor.1.max(self.head.1) + 1,
        }
    }

    pub fn contains(&self, row: usize, col: usize, rows: usize, cols: usize) -> bool {
        self.rows(rows).contains(&row) && self.cols(cols).contains(&col)
    }

    pub fn is_single(&self) -> bool {
        self.kind == SelKind::Cells && self.anchor == self.head
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTarget {
    Cell(usize, usize),
    Header(usize),
}

pub struct Editing {
    pub target: EditTarget,
    pub input: Entity<TextInput>,
    _subscription: Subscription,
}

#[derive(Clone, Debug)]
pub enum Drag {
    Resize {
        col: usize,
        start_x: f32,
        start_width: f32,
    },
    PressHeader {
        col: usize,
        start: Point<Pixels>,
    },
    MoveCols {
        cols: Range<usize>,
        grab: f32,
        pointer: Point<Pixels>,
    },
    PressRowHeader {
        row: usize,
        start: Point<Pixels>,
    },
    MoveRows {
        rows: Range<usize>,
        grab: f32,
        pointer: Point<Pixels>,
    },
    Select,
    ScrollV {
        grab: f32,
    },
    ScrollH {
        grab: f32,
    },
}

#[derive(Clone, Debug)]
pub enum MenuAction {
    SortAsc(usize),
    SortDesc(usize),
    ClearSort,
    OpenFilter(usize),
    ClearFilter(usize),
    FilterByValue(usize, usize),
    ClearAllFilters,
    InsertColLeft(usize),
    InsertColRight(usize),
    RenameCol(usize),
    DeleteCols(Range<usize>),
    AutofitCol(usize),
    SelectCol(usize),
    InsertRowAbove(usize),
    InsertRowBelow(usize),
    DuplicateRows(Range<usize>),
    DeleteRows(Range<usize>),
    Cut,
    Copy,
    Paste,
    ClearCells,
}

pub enum MenuEntry {
    Item {
        label: SharedString,
        shortcut: Option<&'static str>,
        action: MenuAction,
        danger: bool,
    },
    Separator,
}

fn item(label: impl Into<SharedString>, action: MenuAction) -> MenuEntry {
    MenuEntry::Item {
        label: label.into(),
        shortcut: None,
        action,
        danger: false,
    }
}

fn item_key(
    label: impl Into<SharedString>,
    shortcut: &'static str,
    action: MenuAction,
) -> MenuEntry {
    MenuEntry::Item {
        label: label.into(),
        shortcut: Some(shortcut),
        action,
        danger: false,
    }
}

fn danger(label: impl Into<SharedString>, action: MenuAction) -> MenuEntry {
    MenuEntry::Item {
        label: label.into(),
        shortcut: None,
        action,
        danger: true,
    }
}

pub struct ContextMenu {
    pub position: Point<Pixels>,
    pub entries: Vec<MenuEntry>,
}

pub struct OpenFilter {
    pub col: usize,
    pub panel: Entity<FilterPanel>,
    pub position: Point<Pixels>,
    _subscription: Subscription,
}

pub struct CsvEditor {
    pub focus: FocusHandle,
    pub doc: Option<Document>,
    pub path: Option<PathBuf>,
    pub loading: Option<PathBuf>,
    pub load_error: Option<String>,
    pub load_time: Option<Duration>,
    pub issues: Vec<Issue>,
    pub issue_count: usize,
    pub issue_rows: HashSet<RowId>,
    pub issues_open: bool,
    pub banner_dismissed: bool,
    pub scroll: Point<f32>,
    pub selection: Option<Selection>,
    pub editing: Option<Editing>,
    pub drag: Option<Drag>,
    pub hover_col: Option<usize>,
    pub menu: Option<ContextMenu>,
    pub filter: Option<OpenFilter>,
    pub grid_bounds: Bounds<Pixels>,
    pub force_close: bool,
    issue_scroll: UniformListScrollHandle,
    title: Option<(String, bool)>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for CsvEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CsvEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            cx.set_global(Theme::for_appearance(window.appearance()));
            cx.refresh_windows();
        });
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| this.should_close(window, cx))
                .unwrap_or(true)
        });
        Self {
            focus: cx.focus_handle(),
            doc: None,
            path: None,
            loading: None,
            load_error: None,
            load_time: None,
            issues: Vec::new(),
            issue_count: 0,
            issue_rows: HashSet::new(),
            issues_open: false,
            banner_dismissed: false,
            scroll: point(0., 0.),
            selection: None,
            editing: None,
            drag: None,
            hover_col: None,
            menu: None,
            filter: None,
            grid_bounds: Bounds::default(),
            force_close: false,
            issue_scroll: UniformListScrollHandle::new(),
            title: None,
            _subscriptions: vec![appearance],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.doc.is_none() && self.loading.is_none()
    }

    pub fn is_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.is_dirty())
    }

    // ----- loading and saving -----

    pub fn new_blank(&mut self, cx: &mut Context<Self>) {
        let mut doc = Document::blank();
        for col in &mut doc.columns {
            col.width = 120.;
        }
        self.reset_view();
        self.doc = Some(doc);
        self.selection = Some(Selection::cell(0, 0));
        cx.notify();
    }

    fn reset_view(&mut self) {
        self.scroll = point(0., 0.);
        self.selection = None;
        self.editing = None;
        self.drag = None;
        self.menu = None;
        self.filter = None;
        self.issues.clear();
        self.issue_count = 0;
        self.issue_rows.clear();
        self.issues_open = false;
        self.banner_dismissed = false;
        self.load_error = None;
    }

    pub fn open_path(
        &mut self,
        path: PathBuf,
        delimiter: Option<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delimiter = delimiter.or_else(|| delimiter_for_extension(&path));
        self.loading = Some(path.clone());
        self.load_error = None;
        cx.notify();
        let read_path = path.clone();
        let task = cx.background_executor().spawn(async move {
            let start = Instant::now();
            let bytes = std::fs::read(&read_path)?;
            let parsed = parse_bytes(&bytes, delimiter);
            anyhow::Ok((parsed, start.elapsed()))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.loading = None;
                match result {
                    Ok((parsed, elapsed)) => this.load_parsed(parsed, Some(path), Some(elapsed)),
                    Err(err) => {
                        this.load_error = Some(format!("Couldn't open {}: {err}", path.display()));
                    }
                }
                window.focus(&this.focus, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows a freshly parsed file.
    pub fn load_parsed(
        &mut self,
        mut parsed: crate::parse::Parsed,
        path: Option<PathBuf>,
        elapsed: Option<Duration>,
    ) {
        self.loading = None;
        self.reset_view();
        self.issue_count = parsed.issue_count;
        self.issues = std::mem::take(&mut parsed.issues);
        self.issue_rows = self.issues.iter().filter_map(|i| i.row).collect();
        self.issues_open = self.issues.iter().any(|i| i.is_error());
        let mut doc = Document::from_parsed(parsed);
        autofit_all(&mut doc);
        if doc.row_count() > 0 && doc.col_count() > 0 {
            self.selection = Some(Selection::cell(0, 0));
        }
        self.doc = Some(doc);
        self.path = path;
        self.load_time = elapsed;
    }

    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(window, cx);
        if self.doc.is_none() {
            return;
        }
        match self.path.clone() {
            Some(path) => self.write_to(path, window, cx),
            None => self.save_as(window, cx),
        }
    }

    pub fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(window, cx);
        if self.doc.is_none() {
            return;
        }
        let dir = self
            .path
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/"));
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.csv".into());
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update_in(cx, |this, window, cx| {
                    if let Some(doc) = this.doc.as_mut() {
                        match delimiter_for_extension(&path) {
                            Some(d) => doc.format.delimiter = d,
                            None if doc.format.delimiter == b'\t' => doc.format.delimiter = b',',
                            None => {}
                        }
                    }
                    this.write_to(path, window, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn write_to(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        let bytes = doc.encode();
        let tmp = path.with_extension(format!(
            "{}.seesvee-tmp",
            path.extension()
                .map(|e| e.to_string_lossy())
                .unwrap_or_default()
        ));
        let result = std::fs::write(&tmp, &bytes).and_then(|_| {
            if let Ok(meta) = std::fs::metadata(&path) {
                std::fs::set_permissions(&tmp, meta.permissions()).ok();
            }
            std::fs::rename(&tmp, &path)
        });
        match result {
            Ok(()) => {
                // Remember the selection by row identity across the reorder.
                doc.mark_saved();
                self.path = Some(path);
                self.issues.clear();
                self.issue_count = 0;
                self.issue_rows.clear();
                self.issues_open = false;
                self.filter = None;
                cx.notify();
            }
            Err(err) => {
                std::fs::remove_file(&tmp).ok();
                let message = format!("Couldn't save {}", path.display());
                let detail = err.to_string();
                drop(window.prompt(PromptLevel::Critical, &message, Some(&detail), &["OK"], cx));
            }
        }
    }

    pub fn reload_with(&mut self, delimiter: u8, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.is_dirty() {
            let rx = window.prompt(
                PromptLevel::Warning,
                "Reload and discard your changes?",
                Some("Reading the file with a different delimiter discards unsaved edits."),
                &["Reload", "Cancel"],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                if rx.await == Ok(0) {
                    this.update_in(cx, |this, window, cx| {
                        this.open_path(path, Some(delimiter), window, cx)
                    })
                    .ok();
                }
            })
            .detach();
        } else {
            self.open_path(path, Some(delimiter), window, cx);
        }
    }

    fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.force_close || !self.is_dirty() {
            return true;
        }
        let name = self.display_name();
        let rx = window.prompt(
            PromptLevel::Warning,
            &format!("Do you want to save the changes you made to \u{201c}{name}\u{201d}?"),
            Some("Your changes will be lost if you don't save them."),
            &["Save", "Don't Save", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = rx.await;
            this.update_in(cx, |this, window, cx| match answer {
                Ok(0) => {
                    if this.path.is_some() {
                        this.save(window, cx);
                        if !this.is_dirty() {
                            this.force_close = true;
                            window.remove_window();
                        }
                    } else {
                        this.save_as(window, cx);
                    }
                }
                Ok(1) => {
                    this.force_close = true;
                    window.remove_window();
                }
                _ => {}
            })
            .ok();
        })
        .detach();
        false
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .or(self.loading.as_ref())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                if self.doc.is_some() {
                    "Untitled".into()
                } else {
                    "Seesvee".into()
                }
            })
    }

    // ----- geometry and scrolling -----

    pub fn geometry(&self, bounds: Bounds<Pixels>) -> Geometry {
        Geometry::new(self, bounds)
    }

    pub fn set_scroll(&mut self, x: f32, y: f32) {
        let geo = self.geometry(self.grid_bounds);
        self.scroll = point(
            x.clamp(0., geo.max_scroll_x()),
            y.clamp(0., geo.max_scroll_y()),
        );
    }

    fn scroll_into_view(&mut self, row: Option<usize>, col: Option<usize>) {
        let geo = self.geometry(self.grid_bounds);
        let (mut x, mut y) = (self.scroll.x, self.scroll.y);
        if let Some(row) = row {
            let top = row as f32 * ROW_H;
            let view_h = geo.body_height();
            if top < y {
                y = top;
            } else if top + ROW_H > y + view_h {
                y = top + ROW_H - view_h;
            }
        }
        if let Some(col) = col
            && col < geo.col_x.len() - 1
        {
            let left = geo.col_x[col];
            let right = geo.col_x[col + 1];
            let view_w = geo.body_width();
            if left < x {
                x = left;
            } else if right > x + view_w {
                x = (right - view_w).min(left);
            }
        }
        self.set_scroll(x, y);
    }

    // ----- selection -----

    fn dims(&self) -> (usize, usize) {
        self.doc
            .as_ref()
            .map(|d| (d.row_count(), d.col_count()))
            .unwrap_or((0, 0))
    }

    fn move_cursor(&mut self, dr: isize, dc: isize, extend: bool, cx: &mut Context<Self>) {
        let (rows, cols) = self.dims();
        if rows == 0 || cols == 0 || self.overlay_open() {
            return;
        }
        let mut sel = self.selection.unwrap_or(Selection::cell(0, 0));
        let from = if extend { sel.head } else { sel.anchor };
        let to = (
            (from.0 as isize + dr).clamp(0, rows as isize - 1) as usize,
            (from.1 as isize + dc).clamp(0, cols as isize - 1) as usize,
        );
        if extend {
            if sel.kind != SelKind::Cells {
                sel = Selection {
                    kind: SelKind::Cells,
                    anchor: (sel.rows(rows).start, sel.cols(cols).start),
                    head: sel.head,
                };
            }
            sel.head = to;
        } else {
            sel = Selection::cell(to.0, to.1);
        }
        self.selection = Some(sel);
        self.scroll_into_view(Some(to.0), Some(to.1));
        cx.notify();
    }

    fn overlay_open(&self) -> bool {
        self.menu.is_some() || self.filter.is_some()
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let (rows, cols) = self.dims();
        if rows == 0 || cols == 0 {
            return;
        }
        self.selection = Some(Selection {
            kind: SelKind::Cells,
            anchor: (0, 0),
            head: (rows - 1, cols - 1),
        });
        cx.notify();
    }

    // ----- editing -----

    pub fn start_edit(
        &mut self,
        target: EditTarget,
        initial: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.doc.as_ref() else { return };
        let value = match target {
            EditTarget::Cell(r, c) => {
                if r >= doc.row_count() || c >= doc.col_count() {
                    return;
                }
                doc.cell(r, c).to_string()
            }
            EditTarget::Header(c) => {
                if c >= doc.col_count() {
                    return;
                }
                doc.columns[c].name.clone()
            }
        };
        let replace = initial.is_some();
        let text = initial.unwrap_or(value);
        let input = cx.new(|cx| TextInput::new(cx, &text, ""));
        if matches!(target, EditTarget::Header(_)) && !replace {
            input.update(cx, |input, cx| input.select_all_text(cx));
        }
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                TextInputEvent::Submit(direction) => {
                    let direction = *direction;
                    this.commit_edit(window, cx);
                    let (dr, dc) = match direction {
                        Direction::Up => (-1, 0),
                        Direction::Down => (1, 0),
                        Direction::Left => (0, -1),
                        Direction::Right => (0, 1),
                    };
                    this.move_cursor(dr, dc, false, cx);
                }
                TextInputEvent::Cancel => this.cancel_edit(window, cx),
                TextInputEvent::Changed => {}
            });
        match target {
            EditTarget::Cell(r, c) => {
                self.selection = Some(Selection::cell(r, c));
                self.scroll_into_view(Some(r), Some(c));
            }
            EditTarget::Header(c) => self.scroll_into_view(None, Some(c)),
        }
        window.focus(&input.focus_handle(cx), cx);
        self.editing = Some(Editing {
            target,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    pub fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        let text = editing.input.read(cx).text().to_string();
        if let Some(doc) = self.doc.as_mut() {
            match editing.target {
                EditTarget::Cell(r, c) if r < doc.row_count() && c < doc.col_count() => {
                    doc.set_cell(r, c, &text)
                }
                EditTarget::Header(c) if c < doc.col_count() => doc.rename_column(c, &text),
                _ => {}
            }
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    // ----- clipboard -----

    fn selection_tsv(&self) -> Option<String> {
        let doc = self.doc.as_ref()?;
        let sel = self.selection?;
        let rows = sel.rows(doc.row_count());
        let cols = sel.cols(doc.col_count());
        let mut out = String::new();
        if sel.kind == SelKind::Cols {
            for (i, c) in cols.clone().enumerate() {
                if i > 0 {
                    out.push('\t');
                }
                push_tsv_field(&mut out, &doc.columns[c].name);
            }
            out.push('\n');
        }
        for r in rows {
            for (i, c) in cols.clone().enumerate() {
                if i > 0 {
                    out.push('\t');
                }
                push_tsv_field(&mut out, doc.cell(r, c));
            }
            out.push('\n');
        }
        Some(out)
    }

    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selection_tsv() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn cut(&mut self, cx: &mut Context<Self>) {
        self.copy(cx);
        self.clear_cells(cx);
    }

    pub fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let (Some(doc), Some(sel)) = (self.doc.as_mut(), self.selection) else {
            return;
        };
        let (rows, cols) = (doc.row_count(), doc.col_count());
        if rows == 0 || cols == 0 {
            return;
        }
        let text = text.strip_suffix('\n').unwrap_or(&text);
        let text = text.strip_suffix('\r').unwrap_or(text);
        let delimiter = if text.contains('\t') { b'\t' } else { b'\n' };
        let parsed = parse_str(text, delimiter);
        let mut grid: Vec<Vec<String>> = vec![parsed.headers.clone()];
        for r in 0..parsed.store.total_rows() {
            grid.push(
                (0..parsed.store.total_cols())
                    .map(|c| parsed.store.get(r, c).to_string())
                    .collect(),
            );
        }
        if text.is_empty() {
            grid = vec![vec![String::new()]];
        }
        let sel_rows = sel.rows(rows);
        let sel_cols = sel.cols(cols);
        let mut cells = Vec::new();
        if grid.len() == 1 && grid[0].len() == 1 && !sel.is_single() {
            // One value pasted over a range fills the range.
            for r in sel_rows.clone() {
                for c in sel_cols.clone() {
                    cells.push((r, c, grid[0][0].clone()));
                }
            }
        } else {
            for (i, line) in grid.iter().enumerate() {
                for (j, value) in line.iter().enumerate() {
                    let (r, c) = (sel_rows.start + i, sel_cols.start + j);
                    if r < rows && c < cols {
                        cells.push((r, c, value.clone()));
                    }
                }
            }
            let last_r = (sel_rows.start + grid.len()).min(rows) - 1;
            let width = grid.iter().map(|l| l.len()).max().unwrap_or(1);
            let last_c = (sel_cols.start + width).min(cols) - 1;
            self.selection = Some(Selection {
                kind: SelKind::Cells,
                anchor: (sel_rows.start, sel_cols.start),
                head: (last_r, last_c),
            });
        }
        doc.set_cells(cells);
        cx.notify();
    }

    pub fn clear_cells(&mut self, cx: &mut Context<Self>) {
        let (Some(doc), Some(sel)) = (self.doc.as_mut(), self.selection) else {
            return;
        };
        let mut cells = Vec::new();
        for r in sel.rows(doc.row_count()) {
            for c in sel.cols(doc.col_count()) {
                if !doc.cell(r, c).is_empty() {
                    cells.push((r, c, String::new()));
                }
            }
        }
        doc.set_cells(cells);
        cx.notify();
    }

    // ----- structure -----

    pub fn run_menu_action(
        &mut self,
        action: MenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        self.commit_edit(window, cx);
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let rows = doc.row_count();
        match action {
            MenuAction::SortAsc(c) => doc.set_sort(c, Some(SortDir::Asc)),
            MenuAction::SortDesc(c) => doc.set_sort(c, Some(SortDir::Desc)),
            MenuAction::ClearSort => {
                if let Some(sort) = doc.sort()
                    && let Some(c) = doc.columns.iter().position(|col| col.id == sort.col)
                {
                    doc.set_sort(c, None);
                }
            }
            MenuAction::OpenFilter(c) => {
                self.open_filter(c, window, cx);
                return;
            }
            MenuAction::ClearFilter(c) => doc.set_filter(c, Filter::default()),
            MenuAction::ClearAllFilters => doc.clear_filters(),
            MenuAction::FilterByValue(r, c) => {
                let value = doc.cell(r, c).to_string();
                let mut filter = doc.filter(c).cloned().unwrap_or_default();
                filter.values = Some(ValueSet::Only([value].into()));
                doc.set_filter(c, filter);
            }
            MenuAction::InsertColLeft(c) | MenuAction::InsertColRight(c) => {
                let at = if matches!(action, MenuAction::InsertColLeft(_)) {
                    c
                } else {
                    c + 1
                };
                let at = doc.insert_column(at);
                let row = self.selection.map(|s| s.anchor.0).unwrap_or(0);
                self.selection = (doc.row_count() > 0).then(|| Selection::cell(row, at));
                self.start_edit(EditTarget::Header(at), None, window, cx);
            }
            MenuAction::RenameCol(c) => self.start_edit(EditTarget::Header(c), None, window, cx),
            MenuAction::DeleteCols(range) => {
                doc.delete_columns(range);
                self.clamp_selection();
            }
            MenuAction::AutofitCol(c) => autofit(doc, c, 2000),
            MenuAction::SelectCol(c) => {
                self.selection = Some(Selection {
                    kind: SelKind::Cols,
                    anchor: (0, c),
                    head: (0, c),
                })
            }
            MenuAction::InsertRowAbove(r) | MenuAction::InsertRowBelow(r) => {
                let at = if matches!(action, MenuAction::InsertRowAbove(_)) {
                    r
                } else {
                    (r + 1).min(rows)
                };
                let at = doc.insert_row(at);
                let col = self.selection.map(|s| s.anchor.1).unwrap_or(0);
                self.selection = Some(Selection::cell(
                    at,
                    col.min(doc.col_count().saturating_sub(1)),
                ));
                self.scroll_into_view(Some(at), None);
            }
            MenuAction::DuplicateRows(range) => {
                let new = doc.duplicate_rows(range);
                self.selection = Some(Selection {
                    kind: SelKind::Rows,
                    anchor: (new.start, 0),
                    head: (new.end - 1, 0),
                });
            }
            MenuAction::DeleteRows(range) => {
                doc.delete_rows(range);
                self.clamp_selection();
            }
            MenuAction::Cut => self.cut(cx),
            MenuAction::Copy => self.copy(cx),
            MenuAction::Paste => self.paste(cx),
            MenuAction::ClearCells => self.clear_cells(cx),
        }
        cx.notify();
    }

    fn clamp_selection(&mut self) {
        let (rows, cols) = self.dims();
        if rows == 0 || cols == 0 {
            self.selection = None;
            return;
        }
        if let Some(sel) = self.selection {
            let r = sel.rows(rows).start.min(rows - 1);
            let c = sel.cols(cols).start.min(cols - 1);
            self.selection = Some(Selection::cell(r, c));
        }
        self.set_scroll(self.scroll.x, self.scroll.y);
    }

    /// The selected rows if whole rows are selected, otherwise the rows the
    /// selection touches.
    fn selected_rows(&self) -> Option<Range<usize>> {
        let (rows, _) = self.dims();
        self.selection
            .map(|s| s.rows(rows))
            .filter(|r| !r.is_empty())
    }

    fn selected_cols(&self) -> Option<Range<usize>> {
        let (_, cols) = self.dims();
        self.selection
            .map(|s| s.cols(cols))
            .filter(|r| !r.is_empty())
    }

    pub fn column_menu(&self, col: usize) -> Vec<MenuEntry> {
        let Some(doc) = self.doc.as_ref() else {
            return Vec::new();
        };
        let mut entries = vec![
            item("Sort A \u{2192} Z", MenuAction::SortAsc(col)),
            item("Sort Z \u{2192} A", MenuAction::SortDesc(col)),
        ];
        if doc.sort().is_some() {
            entries.push(item("Clear Sort", MenuAction::ClearSort));
        }
        entries.push(MenuEntry::Separator);
        entries.push(item("Filter\u{2026}", MenuAction::OpenFilter(col)));
        if doc.filter(col).is_some() {
            entries.push(item("Clear Filter", MenuAction::ClearFilter(col)));
        }
        if doc.has_filters() {
            entries.push(item("Clear All Filters", MenuAction::ClearAllFilters));
        }
        entries.push(MenuEntry::Separator);
        entries.push(item("Insert Column Left", MenuAction::InsertColLeft(col)));
        entries.push(item("Insert Column Right", MenuAction::InsertColRight(col)));
        entries.push(item("Rename Column\u{2026}", MenuAction::RenameCol(col)));
        entries.push(item("Autofit Width", MenuAction::AutofitCol(col)));
        entries.push(item("Select Column", MenuAction::SelectCol(col)));
        entries.push(MenuEntry::Separator);
        let range = self
            .selection
            .filter(|s| s.kind == SelKind::Cols && s.cols(doc.col_count()).contains(&col))
            .map(|s| s.cols(doc.col_count()))
            .unwrap_or(col..col + 1);
        let label = if range.len() > 1 {
            format!("Delete {} Columns", range.len())
        } else {
            "Delete Column".to_string()
        };
        entries.push(danger(label, MenuAction::DeleteCols(range)));
        entries
    }

    pub fn row_menu(&self, row: usize) -> Vec<MenuEntry> {
        let Some(doc) = self.doc.as_ref() else {
            return Vec::new();
        };
        let range = self
            .selection
            .filter(|s| s.rows(doc.row_count()).contains(&row))
            .map(|s| s.rows(doc.row_count()))
            .unwrap_or(row..row + 1);
        let plural = range.len() > 1;
        vec![
            item("Insert Row Above", MenuAction::InsertRowAbove(range.start)),
            item(
                "Insert Row Below",
                MenuAction::InsertRowBelow(range.end - 1),
            ),
            item(
                if plural {
                    format!("Duplicate {} Rows", range.len())
                } else {
                    "Duplicate Row".into()
                },
                MenuAction::DuplicateRows(range.clone()),
            ),
            MenuEntry::Separator,
            danger(
                if plural {
                    format!("Delete {} Rows", range.len())
                } else {
                    "Delete Row".into()
                },
                MenuAction::DeleteRows(range),
            ),
        ]
    }

    pub fn cell_menu(&self, row: usize, col: usize) -> Vec<MenuEntry> {
        let mut entries = vec![
            item_key("Cut", "\u{2318}X", MenuAction::Cut),
            item_key("Copy", "\u{2318}C", MenuAction::Copy),
            item_key("Paste", "\u{2318}V", MenuAction::Paste),
            item_key("Clear Contents", "\u{232b}", MenuAction::ClearCells),
            MenuEntry::Separator,
            item("Filter by This Value", MenuAction::FilterByValue(row, col)),
            item("Sort A \u{2192} Z", MenuAction::SortAsc(col)),
            item("Sort Z \u{2192} A", MenuAction::SortDesc(col)),
            MenuEntry::Separator,
        ];
        entries.extend(self.row_menu(row));
        entries.push(item("Delete Column", MenuAction::DeleteCols(col..col + 1)));
        if let MenuEntry::Item { danger, .. } = entries.last_mut().unwrap() {
            *danger = true;
        }
        entries
    }

    pub fn open_menu(
        &mut self,
        position: Point<Pixels>,
        entries: Vec<MenuEntry>,
        cx: &mut Context<Self>,
    ) {
        self.filter = None;
        self.menu = (!entries.is_empty()).then_some(ContextMenu { position, entries });
        cx.notify();
    }

    pub fn open_filter(&mut self, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let Some(doc) = self.doc.as_ref() else { return };
        if col >= doc.col_count() {
            return;
        }
        let geo = self.geometry(self.grid_bounds);
        let x = geo.col_screen_x(col).max(self.grid_bounds.left().into());
        let position = point(px(x), self.grid_bounds.top() + px(HEADER_H));
        let panel = cx.new(|cx| FilterPanel::new(doc, col, window, cx));
        let subscription =
            cx.subscribe_in(
                &panel,
                window,
                move |this, _, event, window, cx| match event {
                    FilterPanelEvent::Apply(filter) => {
                        if let Some(doc) = this.doc.as_mut() {
                            doc.set_filter(col, filter.clone());
                        }
                        this.close_filter(window, cx);
                        this.clamp_selection();
                    }
                    FilterPanelEvent::Sort(dir) => {
                        if let Some(doc) = this.doc.as_mut() {
                            doc.set_sort(col, Some(*dir));
                        }
                        this.close_filter(window, cx);
                    }
                    FilterPanelEvent::Close => this.close_filter(window, cx),
                },
            );
        self.filter = Some(OpenFilter {
            col,
            panel,
            position,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn close_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn finish_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        let geo = self.geometry(self.grid_bounds);
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        match drag {
            Drag::PressHeader { col, .. } => doc.toggle_sort(col),
            Drag::MoveCols { cols, pointer, .. } => {
                let to = geo.drop_col(f32::from(pointer.x));
                let moved = doc.move_columns(cols, to);
                let row = self.selection.map(|s| s.anchor.0).unwrap_or(0);
                self.selection = Some(Selection {
                    kind: SelKind::Cols,
                    anchor: (row, moved.start),
                    head: (row, moved.end - 1),
                });
            }
            Drag::MoveRows { rows, pointer, .. } => {
                let to = geo.drop_row(f32::from(pointer.y));
                let moved = doc.move_rows(rows, to);
                self.selection = Some(Selection {
                    kind: SelKind::Rows,
                    anchor: (moved.start, 0),
                    head: (moved.end - 1, 0),
                });
            }
            _ => {}
        }
        cx.notify();
    }

    pub fn resize_column(&mut self, col: usize, width: f32) {
        if let Some(doc) = self.doc.as_mut()
            && let Some(c) = doc.columns.get_mut(col)
        {
            c.width = width.max(MIN_COL_WIDTH);
        }
    }

    pub fn autofit_column(&mut self, col: usize) {
        if let Some(doc) = self.doc.as_mut() {
            autofit(doc, col, 2000);
        }
    }

    pub fn jump_to_issue(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(issue) = self.issues.get(ix).cloned() else {
            return;
        };
        let Some(doc) = self.doc.as_ref() else { return };
        let col = issue
            .field
            .and_then(|f| doc.columns.iter().position(|c| c.id == f))
            .unwrap_or(0);
        match issue.row.and_then(|r| doc.view_index_of(r)) {
            Some(row) => {
                self.selection = Some(Selection::cell(row, col));
                self.scroll_into_view(Some(row), Some(col));
            }
            None if issue.row.is_none() => {
                self.scroll_into_view(Some(0), Some(col));
            }
            None => {}
        }
        cx.notify();
    }

    // ----- key handling -----

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.overlay_open() || self.doc.is_none() {
            return;
        }
        let ks = &event.keystroke;
        if ks.modifiers.platform || ks.modifiers.control || ks.modifiers.function {
            return;
        }
        let Some(ch) = ks.key_char.as_ref() else {
            return;
        };
        if ch.chars().any(|c| c.is_control()) || ch.is_empty() {
            return;
        }
        if let Some(sel) = self.selection {
            let (r, c) = sel.anchor;
            self.start_edit(EditTarget::Cell(r, c), Some(ch.clone()), window, cx);
            cx.stop_propagation();
        }
    }

    fn page_rows(&self) -> isize {
        let geo = self.geometry(self.grid_bounds);
        ((geo.body_height() / ROW_H) as isize - 1).max(1)
    }

    fn register_actions(
        &self,
        el: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        el.on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &SaveAs, window, cx| this.save_as(window, cx)))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                if this.should_close(window, cx) {
                    window.remove_window();
                }
            }))
            .on_action(cx.listener(|this, _: &Undo, window, cx| {
                if this.editing.is_some() {
                    this.cancel_edit(window, cx);
                    return;
                }
                if this.overlay_open() {
                    return;
                }
                if let Some(focus) = this.doc.as_mut().and_then(|d| d.undo()) {
                    this.apply_focus(focus);
                }
                this.clamp_selection();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                if this.editing.is_some() || this.overlay_open() {
                    return;
                }
                if let Some(focus) = this.doc.as_mut().and_then(|d| d.redo()) {
                    this.apply_focus(focus);
                }
                this.clamp_selection();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Cut, _, cx| this.cut(cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &ClearCells, _, cx| {
                if !this.overlay_open() {
                    this.clear_cells(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_cursor(-1, 0, false, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_cursor(1, 0, false, cx)))
            .on_action(cx.listener(|this, _: &MoveLeft, _, cx| this.move_cursor(0, -1, false, cx)))
            .on_action(cx.listener(|this, _: &MoveRight, _, cx| this.move_cursor(0, 1, false, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.move_cursor(-1, 0, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.move_cursor(1, 0, true, cx)))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.move_cursor(0, -1, true, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.move_cursor(0, 1, true, cx)))
            .on_action(cx.listener(|this, _: &MoveToTop, _, cx| {
                this.move_cursor(-(isize::MAX / 2), 0, false, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveToBottom, _, cx| {
                this.move_cursor(isize::MAX / 2, 0, false, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveToRowStart, _, cx| {
                this.move_cursor(0, -(isize::MAX / 2), false, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveToRowEnd, _, cx| {
                this.move_cursor(0, isize::MAX / 2, false, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectToTop, _, cx| {
                this.move_cursor(-(isize::MAX / 2), 0, true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectToBottom, _, cx| {
                this.move_cursor(isize::MAX / 2, 0, true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectToRowStart, _, cx| {
                this.move_cursor(0, -(isize::MAX / 2), true, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectToRowEnd, _, cx| {
                this.move_cursor(0, isize::MAX / 2, true, cx)
            }))
            .on_action(cx.listener(|this, _: &PageUp, _, cx| {
                let n = this.page_rows();
                this.move_cursor(-n, 0, false, cx)
            }))
            .on_action(cx.listener(|this, _: &PageDown, _, cx| {
                let n = this.page_rows();
                this.move_cursor(n, 0, false, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveNext, _, cx| this.move_cursor(0, 1, false, cx)))
            .on_action(cx.listener(|this, _: &MovePrev, _, cx| this.move_cursor(0, -1, false, cx)))
            .on_action(cx.listener(|this, _: &StartEdit, window, cx| {
                if this.overlay_open() {
                    return;
                }
                if let Some(sel) = this.selection {
                    this.start_edit(
                        EditTarget::Cell(sel.anchor.0, sel.anchor.1),
                        None,
                        window,
                        cx,
                    );
                }
            }))
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.menu.take().is_some() || this.drag.take().is_some() {
                    cx.notify();
                } else if this.filter.is_some() {
                    this.close_filter(window, cx);
                } else if let Some(sel) = this.selection
                    && !sel.is_single()
                {
                    this.selection = Some(Selection::cell(sel.anchor.0, sel.anchor.1));
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleProblems, _, cx| {
                if !this.issues.is_empty() {
                    this.issues_open = !this.issues_open;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ClearAllFilters, window, cx| {
                this.run_menu_action(MenuAction::ClearAllFilters, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ClearSort, window, cx| {
                this.run_menu_action(MenuAction::ClearSort, window, cx)
            }))
            .on_action(cx.listener(|this, _: &InsertRowAbove, window, cx| {
                let at = this.selected_rows().map(|r| r.start).unwrap_or(0);
                this.run_menu_action(MenuAction::InsertRowAbove(at), window, cx)
            }))
            .on_action(cx.listener(|this, _: &InsertRowBelow, window, cx| {
                let (rows, _) = this.dims();
                let at = this
                    .selected_rows()
                    .map(|r| r.end - 1)
                    .unwrap_or(rows.saturating_sub(1));
                if rows == 0 {
                    if let Some(doc) = this.doc.as_mut() {
                        doc.insert_row(0);
                        this.selection = Some(Selection::cell(0, 0));
                        cx.notify();
                    }
                    return;
                }
                this.run_menu_action(MenuAction::InsertRowBelow(at), window, cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteRows, window, cx| {
                if let Some(range) = this.selected_rows() {
                    this.run_menu_action(MenuAction::DeleteRows(range), window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &InsertColumnLeft, window, cx| {
                let at = this.selected_cols().map(|c| c.start).unwrap_or(0);
                this.run_menu_action(MenuAction::InsertColLeft(at), window, cx)
            }))
            .on_action(cx.listener(|this, _: &InsertColumnRight, window, cx| {
                let (_, cols) = this.dims();
                let at = this.selected_cols().map(|c| c.end - 1);
                match at {
                    Some(at) => this.run_menu_action(MenuAction::InsertColRight(at), window, cx),
                    None if cols == 0 => {
                        this.run_menu_action(MenuAction::InsertColLeft(0), window, cx)
                    }
                    None => {}
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteColumns, window, cx| {
                if let Some(range) = this.selected_cols() {
                    this.run_menu_action(MenuAction::DeleteCols(range), window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &FilterColumn, window, cx| {
                if let Some(sel) = this.selection {
                    this.open_filter(sel.anchor.1, window, cx)
                }
            }))
            .on_action(
                cx.listener(|this, _: &ReloadComma, window, cx| this.reload_with(b',', window, cx)),
            )
            .on_action(cx.listener(|this, _: &ReloadSemicolon, window, cx| {
                this.reload_with(b';', window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ReloadTab, window, cx| this.reload_with(b'\t', window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ReloadPipe, window, cx| this.reload_with(b'|', window, cx)),
            )
    }

    fn apply_focus(&mut self, focus: crate::document::Focus) {
        if let (Some(r), Some(c)) = (focus.row, focus.col) {
            self.selection = Some(Selection::cell(r, c));
            self.scroll_into_view(Some(r), Some(c));
        }
    }

    // ----- rendering -----

    fn update_title(&mut self, window: &mut Window) {
        let name = self.display_name();
        let dirty = self.is_dirty();
        if self.title.as_ref() != Some(&(name.clone(), dirty)) {
            window.set_window_title(&name);
            window.set_window_edited(dirty);
            self.title = Some((name, dirty));
        }
    }

    fn render_banner(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let errors = self.issues.iter().filter(|i| i.is_error()).count();
        if self.issue_count == 0 || self.banner_dismissed || self.doc.is_none() {
            return None;
        }
        let count = self.issue_count;
        let message = if errors == 0 {
            format!("{count} warning{} in this file.", plural(count))
        } else {
            format!(
                "{count} problem{} found while reading this file. Affected rows are marked in red.",
                plural(count)
            )
        };
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .h(px(34.))
                .flex_none()
                .bg(theme.warning_bg)
                .text_color(theme.warning_text)
                .border_b_1()
                .border_color(theme.border)
                .child(div().child("\u{26a0}\u{fe0e}"))
                .child(div().flex_1().truncate().child(message))
                .child(
                    small_button(
                        "toggle-problems",
                        if self.issues_open {
                            "Hide Details"
                        } else {
                            "Show Details"
                        },
                        theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.issues_open = !this.issues_open;
                        cx.notify();
                    })),
                )
                .child(
                    div()
                        .id("dismiss-banner")
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.hover))
                        .child("\u{2715}")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.banner_dismissed = true;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    fn render_problems(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.issues_open || self.issues.is_empty() || self.doc.is_none() {
            return None;
        }
        let count = self.issues.len();
        let more = self.issue_count.saturating_sub(count);
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(190.))
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.header_bg)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .px_3()
                        .h(px(30.))
                        .flex_none()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex_1()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.header_text)
                                .child(if more > 0 {
                                    format!(
                                        "Problems ({}, showing first {count})",
                                        self.issue_count
                                    )
                                } else {
                                    format!("Problems ({count})")
                                }),
                        )
                        .child(
                            div()
                                .id("close-problems")
                                .px_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_color(theme.text_muted)
                                .hover(|s| s.bg(theme.hover))
                                .child("\u{2715}")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.issues_open = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    uniform_list(
                        "problems",
                        count,
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            let theme = Theme::get(cx);
                            let headers: Vec<String> = this
                                .doc
                                .as_ref()
                                .map(|d| d.columns.iter().map(|c| c.name.clone()).collect())
                                .unwrap_or_default();
                            range
                                .map(|ix| {
                                    let issue = &this.issues[ix];
                                    let mut location = format!("Line {}", issue.line);
                                    if let Some(field) = issue.field {
                                        let name = headers
                                            .get(field as usize)
                                            .filter(|n| !n.is_empty())
                                            .cloned()
                                            .unwrap_or_else(|| format!("field {}", field + 1));
                                        location.push_str(&format!(" \u{00b7} {name}"));
                                    }
                                    let color = if issue.is_error() {
                                        theme.error
                                    } else {
                                        theme.warning_text
                                    };
                                    div()
                                        .id(("problem", ix))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_3()
                                        .px_3()
                                        .h(px(24.))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.hover))
                                        .child(
                                            div().size(px(6.)).rounded_full().bg(color).flex_none(),
                                        )
                                        .child(
                                            div()
                                                .w(px(150.))
                                                .flex_none()
                                                .truncate()
                                                .text_color(theme.text_muted)
                                                .child(location),
                                        )
                                        .child(div().flex_1().truncate().child(issue.to_string()))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.jump_to_issue(ix, cx)
                                        }))
                                })
                                .collect()
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.issue_scroll),
                )
                .into_any_element(),
        )
    }

    fn render_status(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut left: Vec<AnyElement> = Vec::new();
        let mut right = String::new();
        if let Some(doc) = self.doc.as_ref() {
            let total = doc.total_row_count();
            let shown = doc.row_count();
            let rows = if shown != total {
                format!("{} of {} rows", fmt_int(shown), fmt_int(total))
            } else {
                format!("{} row{}", fmt_int(total), plural(total))
            };
            left.push(
                div()
                    .child(format!(
                        "{rows} \u{00b7} {} column{}",
                        fmt_int(doc.col_count()),
                        plural(doc.col_count())
                    ))
                    .into_any_element(),
            );
            if doc.has_filters() {
                left.push(
                    div()
                        .id("filtered")
                        .px_1p5()
                        .rounded_sm()
                        .bg(theme.selection_header)
                        .text_color(theme.accent)
                        .cursor_pointer()
                        .child("Filtered \u{2014} only visible rows are saved  \u{2715}")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run_menu_action(MenuAction::ClearAllFilters, window, cx)
                        }))
                        .into_any_element(),
                );
            }
            if self.issue_count > 0 {
                left.push(
                    div()
                        .id("problems-count")
                        .px_1p5()
                        .rounded_sm()
                        .text_color(theme.error)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.hover))
                        .child(format!("\u{26a0}\u{fe0e} {}", fmt_int(self.issue_count)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.issues_open = !this.issues_open;
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            right = self.selection_summary(doc);
            let fmt = doc.format;
            if !right.is_empty() {
                right.push_str("   ");
            }
            right.push_str(&format!(
                "{} \u{00b7} {} \u{00b7} {}",
                delimiter_name(fmt.delimiter),
                fmt.encoding.label(),
                fmt.line_ending.label()
            ));
            if let Some(t) = self.load_time {
                right.push_str(&format!(" \u{00b7} opened in {} ms", t.as_millis()));
            }
        } else if self.loading.is_some() {
            left.push(div().child("Loading\u{2026}").into_any_element());
        }
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(26.))
            .px_3()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.header_bg)
            .text_size(px(11.5))
            .text_color(theme.text_muted)
            .children(left)
            .child(div().flex_1())
            .child(div().truncate().child(right))
            .into_any_element()
    }

    fn selection_summary(&self, doc: &Document) -> String {
        let Some(sel) = self.selection else {
            return String::new();
        };
        let (rows, cols) = (doc.row_count(), doc.col_count());
        if rows == 0 || cols == 0 {
            return String::new();
        }
        let rr = sel.rows(rows);
        let cr = sel.cols(cols);
        if sel.is_single() {
            let (r, c) = sel.anchor;
            if r >= rows || c >= cols {
                return String::new();
            }
            return format!("Row {} \u{00b7} {}", fmt_int(r + 1), doc.columns[c].name);
        }
        let cells = rr.len() * cr.len();
        let mut count = 0usize;
        let mut sum = 0f64;
        let mut nums = 0usize;
        if cells <= 500_000 {
            for r in rr.clone() {
                for c in cr.clone() {
                    let v = doc.cell(r, c);
                    if !v.trim().is_empty() {
                        count += 1;
                        if let Some(n) = parse_number(v) {
                            sum += n;
                            nums += 1;
                        }
                    }
                }
            }
        }
        let mut out = format!("{} \u{00d7} {}", fmt_int(rr.len()), fmt_int(cr.len()));
        if cells <= 500_000 {
            out.push_str(&format!(" \u{00b7} Count {}", fmt_int(count)));
            if nums > 0 {
                out.push_str(&format!(
                    " \u{00b7} Sum {} \u{00b7} Avg {}",
                    fmt_num(sum),
                    fmt_num(sum / nums as f64)
                ));
            }
        }
        out
    }

    fn render_empty(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let body: AnyElement =
            if self.loading.is_some() {
                div()
                    .text_color(theme.text_muted)
                    .child(format!("Opening {}\u{2026}", self.display_name()))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child("Seesvee"),
                    )
                    .when_some(self.load_error.clone(), |el, err| {
                        el.child(
                            div()
                                .max_w(px(520.))
                                .text_center()
                                .text_color(theme.error)
                                .child(err),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(primary_button("open", "Open File\u{2026}", theme).on_click(
                                |_, window, cx| window.dispatch_action(Box::new(Open), cx),
                            ))
                            .child(
                                small_button("new", "New Sheet", theme)
                                    .on_click(cx.listener(|this, _, _, cx| this.new_blank(cx))),
                            ),
                    )
                    .child(
                        div()
                            .text_color(theme.text_faint)
                            .child("or drop a CSV file here \u{00b7} \u{2318}O"),
                    )
                    .into_any_element()
            };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(body)
            .into_any_element()
    }

    fn render_edit_overlay(&self, theme: &Theme) -> Option<AnyElement> {
        let editing = self.editing.as_ref()?;
        let geo = self.geometry(self.grid_bounds);
        let (left, top, width, height) = match editing.target {
            EditTarget::Cell(r, c) => {
                let (x, y) = geo.cell_origin_local(r, c)?;
                (x, y, geo.col_width(c), ROW_H)
            }
            EditTarget::Header(c) => {
                let x = geo.col_x.get(c)? + geo.row_header_w - self.scroll.x;
                (x, 0., geo.col_width(c), HEADER_H)
            }
        };
        let width = width.max(140.);
        Some(
            div()
                .absolute()
                .left(px(left - 1.))
                .top(px(top - 1.))
                .w(px(width + 2.))
                .h(px(height + 2.))
                .flex()
                .items_center()
                .px(px(grid::PAD - 1.))
                .bg(theme.input_bg)
                .border_2()
                .border_color(theme.accent)
                .shadow_md()
                .line_height(px(18.))
                .text_color(theme.text)
                .child(editing.input.clone())
                .into_any_element(),
        )
    }

    fn render_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let items = menu
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| match entry {
                MenuEntry::Separator => div()
                    .h(px(1.))
                    .my_1()
                    .mx_2()
                    .bg(theme.border)
                    .into_any_element(),
                MenuEntry::Item {
                    label,
                    shortcut,
                    action,
                    danger,
                } => {
                    let action = action.clone();
                    let accent = theme.accent;
                    let text = if *danger { theme.error } else { theme.text };
                    div()
                        .id(("menu-item", i))
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(24.))
                        .px_2()
                        .mx_1()
                        .rounded(px(5.))
                        .text_color(text)
                        .hover(move |s| s.bg(accent).text_color(gpui::white()))
                        .child(div().flex_1().child(label.clone()))
                        .when_some(*shortcut, |el, key| {
                            el.child(div().pl_4().opacity(0.6).child(key))
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run_menu_action(action.clone(), window, cx)
                        }))
                        .into_any_element()
                }
            });
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .anchor(Corner::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("context-menu")
                            .occlude()
                            .min_w(px(210.))
                            .py_1()
                            .flex()
                            .flex_col()
                            .bg(theme.surface)
                            .border_1()
                            .border_color(theme.border)
                            .rounded(px(8.))
                            .shadow_lg()
                            .text_size(px(13.))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                cx.notify();
                            }))
                            .children(items),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    fn render_filter(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.filter.as_ref()?;
        Some(
            deferred(
                anchored()
                    .position(open.position)
                    .anchor(Corner::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("filter-popover")
                            .occlude()
                            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                                this.close_filter(window, cx);
                            }))
                            .child(open.panel.clone()),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

impl Render for CsvEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_title(window);
        let theme = Theme::get(cx);
        let entity = cx.entity();
        let has_doc = self.doc.is_some();

        let root = div()
            .id("csv-editor")
            .key_context(CONTEXT)
            .track_focus(&self.focus);
        self.register_actions(root, cx)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                let paths = paths.paths().to_vec();
                let mut iter = paths.into_iter();
                if let Some(first) = iter.next() {
                    if this.is_empty() {
                        this.open_path(first, None, window, cx);
                    } else {
                        crate::open_window(Some(first), false, cx);
                    }
                }
                for path in iter {
                    crate::open_window(Some(path), false, cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg)
            .text_color(theme.text)
            .text_size(px(13.))
            .line_height(px(18.))
            .font_family(".SystemUIFont")
            .children(self.render_banner(&theme, cx))
            .child(
                div()
                    .id("grid-area")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(theme.selection))
                    .when(has_doc, |el| el.child(grid::grid(entity.clone())))
                    .children(self.render_edit_overlay(&theme))
                    .when(!has_doc, |el| el.child(self.render_empty(&theme, cx))),
            )
            .children(self.render_problems(&theme, cx))
            .child(self.render_status(&theme, cx))
            .children(self.render_menu(&theme, cx))
            .children(self.render_filter(cx))
    }
}

// ----- helpers -----

pub fn small_button(
    id: &'static str,
    label: &'static str,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2p5()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface)
        .text_color(theme.text)
        .cursor_pointer()
        .hover(|s| s.bg(theme.hover))
        .child(label)
}

pub fn primary_button(
    id: &'static str,
    label: &'static str,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let accent = theme.accent;
    div()
        .id(id)
        .px_3()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .bg(accent)
        .text_color(theme.accent_text)
        .cursor_pointer()
        .hover(move |s| s.bg(accent.opacity(0.85)))
        .child(label)
}

fn push_tsv_field(out: &mut String, s: &str) {
    if s.contains(['\t', '\n', '\r', '"']) {
        out.push('"');
        out.push_str(&s.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(s);
    }
}

pub fn delimiter_for_extension(path: &Path) -> Option<u8> {
    match path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        Some("tsv") | Some("tab") => Some(b'\t'),
        Some("psv") => Some(b'|'),
        _ => None,
    }
}

pub fn delimiter_name(d: u8) -> &'static str {
    match d {
        b',' => "Comma",
        b'\t' => "Tab",
        b';' => "Semicolon",
        b'|' => "Pipe",
        _ => "Custom",
    }
}

fn estimate_width(chars: usize) -> f32 {
    (chars as f32 * 7.4 + grid::PAD * 2.0 + 18.0).clamp(56.0, 420.0)
}

fn autofit(doc: &mut Document, col: usize, sample: usize) {
    let chars = doc.sample_width_chars(col, sample);
    doc.columns[col].width = estimate_width(chars).min(560.);
}

fn autofit_all(doc: &mut Document) {
    for c in 0..doc.col_count() {
        let chars = doc.sample_width_chars(c, 300);
        doc.columns[c].width = estimate_width(chars).min(320.);
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

pub fn fmt_int(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        let s = fmt_int(n.abs() as usize);
        if n < 0.0 { format!("-{s}") } else { s }
    } else {
        let s = format!("{n:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}
