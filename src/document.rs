//! The editable document: cell storage plus the column layout, row order,
//! sort, filters and undo history that together define what is on screen.
//!
//! All public row and column indices are *view* indices: positions as
//! currently displayed, after filtering and sorting.

use std::collections::HashMap;
use std::ops::Range;

use crate::filter::Filter;
use crate::parse::{Encoding, LineEnding, Parsed};
use crate::store::{ColId, RowId, Store};
use crate::value::SortKey;

#[derive(Clone, Debug)]
pub struct Column {
    pub id: ColId,
    pub name: String,
    pub width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub col: ColId,
    pub dir: SortDir,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileFormat {
    pub delimiter: u8,
    pub line_ending: LineEnding,
    pub encoding: Encoding,
    pub has_bom: bool,
}

impl Default for FileFormat {
    fn default() -> Self {
        Self {
            delimiter: b',',
            line_ending: LineEnding::Lf,
            encoding: Encoding::Utf8,
            has_bom: false,
        }
    }
}

struct CellChange {
    row: RowId,
    col: ColId,
    old: String,
    new: String,
}

enum Change {
    Cells(Vec<CellChange>),
    Rows {
        before: Vec<RowId>,
        after: Vec<RowId>,
        sort_before: Option<Sort>,
    },
    Columns {
        before: Vec<Column>,
        after: Vec<Column>,
    },
}

/// Where to put the selection after an undo or redo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    pub row: Option<usize>,
    pub col: Option<usize>,
}

pub const DEFAULT_COLUMN_WIDTH: f32 = 120.0;

pub struct Document {
    store: Store,
    pub columns: Vec<Column>,
    /// Every row in "file order", which manual moves and inserts update.
    order: Vec<RowId>,
    /// Rows on screen: `order` filtered, then sorted.
    view: Vec<RowId>,
    sort: Option<Sort>,
    filters: Vec<(ColId, Filter)>,
    undo: Vec<Change>,
    redo: Vec<Change>,
    saved_at: Option<usize>,
    pub format: FileFormat,
}

impl Document {
    pub fn from_parsed(parsed: Parsed) -> Self {
        let Parsed {
            headers,
            store,
            delimiter,
            line_ending,
            encoding,
            has_bom,
            ..
        } = parsed;
        let columns = headers
            .into_iter()
            .enumerate()
            .map(|(i, name)| Column {
                id: i as ColId,
                name,
                width: DEFAULT_COLUMN_WIDTH,
            })
            .collect();
        let order: Vec<RowId> = (0..store.total_rows()).collect();
        Self {
            view: order.clone(),
            order,
            store,
            columns,
            sort: None,
            filters: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            saved_at: Some(0),
            format: FileFormat {
                delimiter,
                line_ending,
                encoding,
                has_bom,
            },
        }
    }

    /// A blank document with a few columns and rows to type into.
    pub fn blank() -> Self {
        let mut doc = Self::from_parsed(crate::parse::parse_str("A,B,C\n", b','));
        for _ in 0..1 {
            let id = doc.store.add_row();
            doc.order.push(id);
            doc.view.push(id);
        }
        doc
    }

    // ----- reading -----

    pub fn row_count(&self) -> usize {
        self.view.len()
    }

    pub fn total_row_count(&self) -> usize {
        self.order.len()
    }

    pub fn col_count(&self) -> usize {
        self.columns.len()
    }

    pub fn row_id(&self, row: usize) -> RowId {
        self.view[row]
    }

    pub fn view_index_of(&self, id: RowId) -> Option<usize> {
        self.view.iter().position(|&r| r == id)
    }

    pub fn cell(&self, row: usize, col: usize) -> &str {
        self.store.get(self.view[row], self.columns[col].id)
    }

    pub fn sort(&self) -> Option<Sort> {
        self.sort
    }

    pub fn sort_dir(&self, col: usize) -> Option<SortDir> {
        self.sort
            .filter(|s| s.col == self.columns[col].id)
            .map(|s| s.dir)
    }

    pub fn filter(&self, col: usize) -> Option<&Filter> {
        let id = self.columns[col].id;
        self.filters.iter().find(|(c, _)| *c == id).map(|(_, f)| f)
    }

    pub fn has_filters(&self) -> bool {
        !self.filters.is_empty()
    }

    pub fn is_dirty(&self) -> bool {
        self.saved_at != Some(self.undo.len())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The longest value in a sample of the column, in characters.
    pub fn sample_width_chars(&self, col: usize, sample: usize) -> usize {
        let id = self.columns[col].id;
        let header = self.columns[col].name.chars().count() + 3;
        self.view
            .iter()
            .take(sample)
            .map(|&r| {
                let s = self.store.get(r, id);
                let line = s.split(['\n', '\r']).next().unwrap_or("");
                line.chars().take(80).count()
            })
            .max()
            .unwrap_or(0)
            .max(header)
    }

    /// Distinct values of a column among rows that pass every *other* filter,
    /// sorted, with how often each occurs. Returns at most `limit` values and
    /// the total number of distinct values.
    pub fn distinct_values(&self, col: usize, limit: usize) -> (Vec<(String, usize)>, usize) {
        let id = self.columns[col].id;
        let others: Vec<_> = self
            .filters
            .iter()
            .filter(|(c, _)| *c != id)
            .map(|(c, f)| (*c, f.compile()))
            .collect();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for &r in &self.order {
            if others.iter().all(|(c, f)| f.matches(self.store.get(r, *c))) {
                *counts.entry(self.store.get(r, id)).or_default() += 1;
            }
        }
        let total = counts.len();
        let mut values: Vec<(&str, usize)> = counts.into_iter().collect();
        values.sort_by(|a, b| SortKey::new(a.0).cmp_asc(&SortKey::new(b.0)));
        values.truncate(limit);
        (
            values
                .into_iter()
                .map(|(s, n)| (s.to_string(), n))
                .collect(),
            total,
        )
    }

    // ----- view state -----

    /// Cycles a column through ascending, descending and file order.
    pub fn toggle_sort(&mut self, col: usize) {
        let next = match self.sort_dir(col) {
            None => Some(SortDir::Asc),
            Some(SortDir::Asc) => Some(SortDir::Desc),
            Some(SortDir::Desc) => None,
        };
        self.set_sort(col, next);
    }

    pub fn set_sort(&mut self, col: usize, dir: Option<SortDir>) {
        self.sort = dir.map(|dir| Sort {
            col: self.columns[col].id,
            dir,
        });
        self.rebuild_view();
    }

    pub fn set_filter(&mut self, col: usize, filter: Filter) {
        let id = self.columns[col].id;
        self.filters.retain(|(c, _)| *c != id);
        if filter.is_active() {
            self.filters.push((id, filter));
        }
        self.rebuild_view();
    }

    pub fn clear_filters(&mut self) {
        self.filters.clear();
        self.rebuild_view();
    }

    fn rebuild_view(&mut self) {
        let filters: Vec<_> = self
            .filters
            .iter()
            .map(|(c, f)| (*c, f.compile()))
            .collect();
        let store = &self.store;
        let mut view: Vec<RowId> = if filters.is_empty() {
            self.order.clone()
        } else {
            self.order
                .iter()
                .copied()
                .filter(|&r| filters.iter().all(|(c, f)| f.matches(store.get(r, *c))))
                .collect()
        };
        if let Some(sort) = self.sort {
            sort_rows(store, &mut view, sort);
        }
        self.view = view;
    }

    /// Makes the current on-screen order the file order, so manual row
    /// operations behave predictably on a sorted view.
    fn bake_sort(&mut self) {
        let Some(sort) = self.sort.take() else {
            return;
        };
        let mut visible = vec![false; self.store.total_rows() as usize];
        for &r in &self.view {
            visible[r as usize] = true;
        }
        let mut sorted = self.order.clone();
        sort_rows(&self.store, &mut sorted, sort);
        let mut next_visible = self.view.iter();
        for slot in sorted.iter_mut() {
            if visible[*slot as usize] {
                *slot = *next_visible.next().unwrap();
            }
        }
        self.order = sorted;
    }

    // ----- editing -----

    fn push(&mut self, change: Change) {
        if self.saved_at.is_some_and(|s| s > self.undo.len()) {
            self.saved_at = None;
        }
        self.undo.push(change);
        self.redo.clear();
    }

    pub fn set_cell(&mut self, row: usize, col: usize, value: &str) {
        self.set_cells(vec![(row, col, value.to_string())]);
    }

    /// Sets several cells as one undoable change.
    pub fn set_cells(&mut self, cells: Vec<(usize, usize, String)>) {
        let mut changes = Vec::with_capacity(cells.len());
        for (row, col, new) in cells {
            let (r, c) = (self.view[row], self.columns[col].id);
            let old = self.store.set(r, c, &new);
            if old != new {
                changes.push(CellChange {
                    row: r,
                    col: c,
                    old,
                    new,
                });
            }
        }
        if !changes.is_empty() {
            self.push(Change::Cells(changes));
        }
    }

    fn change_columns(&mut self, f: impl FnOnce(&mut Vec<Column>, &mut Store)) {
        let before = self.columns.clone();
        f(&mut self.columns, &mut self.store);
        let ids: Vec<ColId> = self.columns.iter().map(|c| c.id).collect();
        let had_filter = self.filters.len();
        self.filters.retain(|(c, _)| ids.contains(c));
        let lost_sort = self.sort.is_some_and(|s| !ids.contains(&s.col));
        if lost_sort {
            self.sort = None;
        }
        if had_filter != self.filters.len() || lost_sort {
            self.rebuild_view();
        }
        let after = self.columns.clone();
        self.push(Change::Columns { before, after });
    }

    pub fn rename_column(&mut self, col: usize, name: &str) {
        if self.columns[col].name != name {
            let name = name.to_string();
            self.change_columns(|cols, _| cols[col].name = name);
        }
    }

    pub fn insert_column(&mut self, at: usize) -> usize {
        let at = at.min(self.columns.len());
        let name = self.unique_column_name();
        self.change_columns(|cols, store| {
            let id = store.add_col();
            cols.insert(
                at,
                Column {
                    id,
                    name,
                    width: DEFAULT_COLUMN_WIDTH,
                },
            );
        });
        at
    }

    fn unique_column_name(&self) -> String {
        (self.columns.len() + 1..)
            .map(|n| format!("Column {n}"))
            .find(|name| !self.columns.iter().any(|c| &c.name == name))
            .unwrap()
    }

    pub fn delete_columns(&mut self, cols: Range<usize>) {
        if cols.is_empty() {
            return;
        }
        self.change_columns(|c, _| {
            c.drain(cols);
        });
    }

    /// Moves the columns in `cols` so they start at `to`, an index into the
    /// column list as it was before the move. Returns their new range.
    pub fn move_columns(&mut self, cols: Range<usize>, to: usize) -> Range<usize> {
        let len = cols.len();
        if len == 0 || (to >= cols.start && to <= cols.end) {
            return cols;
        }
        let dest = if to > cols.end { to - len } else { to };
        self.change_columns(|c, _| {
            let moved: Vec<Column> = c.drain(cols).collect();
            c.splice(dest..dest, moved);
        });
        dest..dest + len
    }

    fn change_rows(&mut self, f: impl FnOnce(&mut Self)) {
        let sort_before = self.sort;
        let before = self.order.clone();
        self.bake_sort();
        f(self);
        let after = self.order.clone();
        self.push(Change::Rows {
            before,
            after,
            sort_before,
        });
    }

    /// Inserts a blank row so it appears at view index `at`.
    pub fn insert_row(&mut self, at: usize) -> usize {
        let at = at.min(self.view.len());
        self.change_rows(|doc| {
            let id = doc.store.add_row();
            doc.insert_into_order(id, at);
            doc.view.insert(at, id);
        });
        at
    }

    pub fn duplicate_rows(&mut self, rows: Range<usize>) -> Range<usize> {
        let len = rows.len();
        let at = rows.end;
        self.change_rows(|doc| {
            for (i, row) in rows.enumerate() {
                let source = doc.view[row];
                let id = doc.store.add_row();
                for col in &doc.columns {
                    let value = doc.store.get(source, col.id).to_string();
                    if !value.is_empty() {
                        doc.store.set(id, col.id, &value);
                    }
                }
                doc.insert_into_order(id, at + i);
                doc.view.insert(at + i, id);
            }
        });
        at..at + len
    }

    /// Places `id` in the file order just before whatever row is currently at
    /// view index `at` (or after the last visible row).
    fn insert_into_order(&mut self, id: RowId, at: usize) {
        let pos = if let Some(&anchor) = self.view.get(at) {
            self.order.iter().position(|&r| r == anchor).unwrap()
        } else if let Some(&last) = self.view.last() {
            self.order.iter().position(|&r| r == last).unwrap() + 1
        } else {
            self.order.len()
        };
        self.order.insert(pos, id);
    }

    pub fn delete_rows(&mut self, rows: Range<usize>) {
        if rows.is_empty() {
            return;
        }
        self.change_rows(|doc| {
            let mut doomed = vec![false; doc.store.total_rows() as usize];
            for &r in &doc.view[rows.clone()] {
                doomed[r as usize] = true;
            }
            doc.order.retain(|&r| !doomed[r as usize]);
            doc.view.drain(rows);
        });
    }

    /// Moves the rows in `rows` so they start at view index `to`, measured
    /// before the move. Returns their new range.
    pub fn move_rows(&mut self, rows: Range<usize>, to: usize) -> Range<usize> {
        let len = rows.len();
        if len == 0 || (to >= rows.start && to <= rows.end) {
            return rows;
        }
        let dest = if to > rows.end { to - len } else { to };
        self.change_rows(|doc| {
            let moved: Vec<RowId> = doc.view.drain(rows).collect();
            let mut is_moved = vec![false; doc.store.total_rows() as usize];
            for &r in &moved {
                is_moved[r as usize] = true;
            }
            doc.order.retain(|&r| !is_moved[r as usize]);
            for (i, &id) in moved.iter().enumerate() {
                doc.insert_into_order(id, dest + i);
                doc.view.insert(dest + i, id);
            }
        });
        dest..dest + len
    }

    // ----- history -----

    pub fn undo(&mut self) -> Option<Focus> {
        let change = self.undo.pop()?;
        let (change, focus) = self.apply(change, true);
        self.redo.push(change);
        Some(focus)
    }

    pub fn redo(&mut self) -> Option<Focus> {
        let change = self.redo.pop()?;
        let (change, focus) = self.apply(change, false);
        self.undo.push(change);
        Some(focus)
    }

    fn apply(&mut self, change: Change, undo: bool) -> (Change, Focus) {
        let mut focus = Focus {
            row: None,
            col: None,
        };
        match &change {
            Change::Cells(cells) => {
                for cell in cells {
                    let value = if undo { &cell.old } else { &cell.new };
                    self.store.set(cell.row, cell.col, value);
                }
                if let Some(first) = cells.first() {
                    focus.row = self.view_index_of(first.row);
                    focus.col = self.columns.iter().position(|c| c.id == first.col);
                }
            }
            Change::Rows {
                before,
                after,
                sort_before,
            } => {
                if undo {
                    self.order = before.clone();
                    self.sort = *sort_before;
                } else {
                    self.order = after.clone();
                    self.sort = None;
                }
                self.rebuild_view();
            }
            Change::Columns { before, after } => {
                let widths: HashMap<ColId, f32> =
                    self.columns.iter().map(|c| (c.id, c.width)).collect();
                self.columns = if undo { before.clone() } else { after.clone() };
                for col in &mut self.columns {
                    if let Some(&w) = widths.get(&col.id) {
                        col.width = w;
                    }
                }
                let ids: Vec<ColId> = self.columns.iter().map(|c| c.id).collect();
                self.filters.retain(|(c, _)| ids.contains(c));
                if self.sort.is_some_and(|s| !ids.contains(&s.col)) {
                    self.sort = None;
                }
                self.rebuild_view();
            }
        }
        (change, focus)
    }

    // ----- saving -----

    /// Serializes exactly what is on screen: the visible columns in order and
    /// the visible rows in order.
    pub fn to_csv(&self) -> String {
        let delimiter = self.format.delimiter as char;
        let newline = self.format.line_ending.as_str();
        let mut out = String::with_capacity(self.view.len() * self.columns.len() * 8 + 64);
        if self.columns.is_empty() {
            return out;
        }
        let d = self.format.delimiter;
        let write_field = |out: &mut String, s: &str| {
            let b = s.as_bytes();
            let needs_quotes =
                memchr::memchr3(d, b'"', b'\n', b).is_some() || memchr::memchr(b'\r', b).is_some();
            if needs_quotes {
                out.push('"');
                for (i, part) in s.split('"').enumerate() {
                    if i > 0 {
                        out.push_str("\"\"");
                    }
                    out.push_str(part);
                }
                out.push('"');
            } else {
                out.push_str(s);
            }
        };
        for (i, col) in self.columns.iter().enumerate() {
            if i > 0 {
                out.push(delimiter);
            }
            write_field(&mut out, &col.name);
        }
        out.push_str(newline);
        for &r in &self.view {
            for (i, col) in self.columns.iter().enumerate() {
                if i > 0 {
                    out.push(delimiter);
                }
                write_field(&mut out, self.store.get(r, col.id));
            }
            out.push_str(newline);
        }
        out
    }

    pub fn encode(&self) -> Vec<u8> {
        let text = self.to_csv();
        match self.format.encoding {
            Encoding::Utf16Le | Encoding::Utf16Be => {
                let be = self.format.encoding == Encoding::Utf16Be;
                let mut out = Vec::with_capacity(text.len() * 2 + 2);
                out.extend_from_slice(if be { &[0xFE, 0xFF] } else { &[0xFF, 0xFE] });
                for unit in text.encode_utf16() {
                    out.extend_from_slice(&if be {
                        unit.to_be_bytes()
                    } else {
                        unit.to_le_bytes()
                    });
                }
                out
            }
            // Files that were not valid UTF-8 are written back as UTF-8.
            Encoding::Utf8 | Encoding::Windows1252 => {
                let mut out = Vec::with_capacity(text.len() + 3);
                if self.format.has_bom {
                    out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
                }
                out.extend_from_slice(text.as_bytes());
                out
            }
        }
    }

    /// Records that the on-screen data was written to disk: the view becomes
    /// the file order and filters and sorting are cleared, since the file now
    /// contains exactly those rows.
    pub fn mark_saved(&mut self) {
        self.order = self.view.clone();
        self.sort = None;
        self.filters.clear();
        if self.format.encoding == Encoding::Windows1252 {
            self.format.encoding = Encoding::Utf8;
        }
        self.saved_at = Some(self.undo.len());
    }
}

fn sort_rows(store: &Store, rows: &mut [RowId], sort: Sort) {
    let mut keyed: Vec<(SortKey<'_>, RowId)> = rows
        .iter()
        .map(|&r| (SortKey::new(store.get(r, sort.col)), r))
        .collect();
    match sort.dir {
        SortDir::Asc => keyed.sort_by(|a, b| a.0.cmp_asc(&b.0)),
        SortDir::Desc => keyed.sort_by(|a, b| {
            if a.0.is_empty() || b.0.is_empty() {
                a.0.cmp_asc(&b.0)
            } else {
                b.0.cmp_asc(&a.0)
            }
        }),
    }
    for (slot, (_, r)) in rows.iter_mut().zip(keyed) {
        *slot = r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::{Condition, FilterOp};
    use crate::parse::parse_str;

    fn doc(text: &str) -> Document {
        Document::from_parsed(parse_str(text, b','))
    }

    fn col(d: &Document, c: usize) -> Vec<String> {
        (0..d.row_count())
            .map(|r| d.cell(r, c).to_string())
            .collect()
    }

    #[test]
    fn sort_cycle() {
        let mut d = doc("n,s\n10,b\n9,a\n,c\nx,d\n");
        d.toggle_sort(0);
        assert_eq!(col(&d, 0), vec!["9", "10", "x", ""]);
        d.toggle_sort(0);
        assert_eq!(col(&d, 0), vec!["x", "10", "9", ""]);
        d.toggle_sort(0);
        assert_eq!(col(&d, 0), vec!["10", "9", "", "x"]);
        assert!(!d.is_dirty());
    }

    #[test]
    fn filters_and_save_what_you_see() {
        let mut d = doc("name,age\nann,30\nbob,40\ncat,50\n");
        d.set_filter(
            1,
            Filter {
                condition: Condition {
                    op: Some(FilterOp::GreaterOrEqual),
                    value: "40".into(),
                    value2: String::new(),
                },
                values: None,
            },
        );
        assert_eq!(col(&d, 0), vec!["bob", "cat"]);
        d.toggle_sort(0);
        d.toggle_sort(0);
        assert_eq!(col(&d, 0), vec!["cat", "bob"]);
        let moved = d.move_columns(1..2, 0);
        assert_eq!(moved, 0..1);
        assert_eq!(d.to_csv(), "age,name\n50,cat\n40,bob\n");
        d.mark_saved();
        assert!(!d.is_dirty());
        assert!(!d.has_filters());
        assert_eq!(d.total_row_count(), 2);
    }

    #[test]
    fn quoting_on_save() {
        let mut d = doc("a,b\n1,2\n");
        d.set_cell(0, 0, "x,y");
        d.set_cell(0, 1, "say \"hi\"\nthere");
        assert_eq!(d.to_csv(), "a,b\n\"x,y\",\"say \"\"hi\"\"\nthere\"\n");
        let reparsed = doc(&d.to_csv());
        assert_eq!(reparsed.cell(0, 1), "say \"hi\"\nthere");
    }

    #[test]
    fn row_ops_and_undo() {
        let mut d = doc("v\na\nb\nc\nd\n");
        assert_eq!(d.move_rows(0..1, 3), 2..3);
        assert_eq!(col(&d, 0), vec!["b", "c", "a", "d"]);
        assert!(d.is_dirty());
        d.undo();
        assert_eq!(col(&d, 0), vec!["a", "b", "c", "d"]);
        assert!(!d.is_dirty());
        d.redo();
        assert_eq!(col(&d, 0), vec!["b", "c", "a", "d"]);
        d.undo();
        d.delete_rows(1..3);
        assert_eq!(col(&d, 0), vec!["a", "d"]);
        let at = d.insert_row(1);
        d.set_cell(at, 0, "new");
        assert_eq!(col(&d, 0), vec!["a", "new", "d"]);
        d.duplicate_rows(0..1);
        assert_eq!(col(&d, 0), vec!["a", "a", "new", "d"]);
        d.undo();
        d.undo();
        d.undo();
        d.undo();
        assert_eq!(col(&d, 0), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn moving_rows_in_sorted_view_bakes_order() {
        let mut d = doc("v\nc\na\nb\n");
        d.toggle_sort(0);
        assert_eq!(col(&d, 0), vec!["a", "b", "c"]);
        d.move_rows(2..3, 0);
        assert_eq!(col(&d, 0), vec!["c", "a", "b"]);
        assert_eq!(d.sort(), None);
        d.undo();
        assert_eq!(col(&d, 0), vec!["a", "b", "c"]);
        assert!(d.sort().is_some());
    }

    #[test]
    fn column_ops() {
        let mut d = doc("a,b,c\n1,2,3\n");
        assert_eq!(d.move_columns(0..1, 3), 2..3);
        assert_eq!(d.to_csv(), "b,c,a\n2,3,1\n");
        d.delete_columns(1..2);
        assert_eq!(d.to_csv(), "b,a\n2,1\n");
        let at = d.insert_column(1);
        d.set_cell(0, at, "x");
        d.rename_column(at, "new");
        assert_eq!(d.to_csv(), "b,new,a\n2,x,1\n");
        d.undo();
        d.undo();
        d.undo();
        d.undo();
        d.undo();
        assert_eq!(d.to_csv(), "a,b,c\n1,2,3\n");
    }

    #[test]
    fn distinct_values_respect_other_filters() {
        let mut d = doc("k,v\na,1\nb,1\na,2\n");
        let (values, total) = d.distinct_values(0, 100);
        assert_eq!(total, 2);
        assert_eq!(values, vec![("a".to_string(), 2), ("b".to_string(), 1)]);
        d.set_filter(
            1,
            Filter {
                condition: Condition {
                    op: Some(FilterOp::Equals),
                    value: "2".into(),
                    value2: String::new(),
                },
                values: None,
            },
        );
        let (values, _) = d.distinct_values(0, 100);
        assert_eq!(values, vec![("a".to_string(), 1)]);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    /// `SEESVEE_BENCH=path/to/file.csv cargo test --release bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_large_file() {
        let Ok(path) = std::env::var("SEESVEE_BENCH") else {
            return;
        };
        let t = Instant::now();
        let bytes = std::fs::read(&path).unwrap();
        let read = t.elapsed();
        let t = Instant::now();
        let parsed = crate::parse::parse_bytes(&bytes, None);
        let parse = t.elapsed();
        let mut doc = Document::from_parsed(parsed);
        println!("rows {} read {read:?} parse {parse:?}", doc.row_count());
        let t = Instant::now();
        doc.toggle_sort(3);
        println!("numeric sort {:?}", t.elapsed());
        let t = Instant::now();
        doc.set_sort(1, Some(SortDir::Asc));
        println!("text sort {:?}", t.elapsed());
        let t = Instant::now();
        let (_, distinct) = doc.distinct_values(2, 5000);
        println!("distinct values ({distinct}) {:?}", t.elapsed());
        let t = Instant::now();
        let csv = doc.encode();
        println!("encode {} MB {:?}", csv.len() / 1_000_000, t.elapsed());
        let t = Instant::now();
        doc.move_rows(5..6, 0);
        println!("move row (bakes sort) {:?}", t.elapsed());
    }
}
