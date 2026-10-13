//! Compact cell storage.
//!
//! Parsed cells live in one contiguous string arena indexed by `u32` offsets,
//! which keeps a million-row file to a handful of allocations. Edits, inserted
//! rows and inserted columns live in a sparse overlay on top of the arena.

use std::collections::HashMap;

pub type RowId = u32;
pub type ColId = u32;

#[derive(Default)]
pub struct Store {
    arena: String,
    /// Start offset of every parsed field, followed by one end sentinel.
    /// Fields of consecutive rows are contiguous, so a row's end is the next
    /// row's first start.
    field_starts: Vec<u32>,
    /// Index into `field_starts` of each parsed row's first field, plus a
    /// trailing sentinel.
    row_starts: Vec<u32>,
    parsed_rows: u32,
    total_rows: u32,
    total_cols: u32,
    edits: HashMap<u64, Box<str>>,
}

#[inline]
fn key(row: RowId, col: ColId) -> u64 {
    ((row as u64) << 32) | col as u64
}

impl Store {
    pub fn get(&self, row: RowId, col: ColId) -> &str {
        if !self.edits.is_empty()
            && let Some(value) = self.edits.get(&key(row, col))
        {
            return value;
        }
        if row >= self.parsed_rows {
            return "";
        }
        let first = self.row_starts[row as usize] as usize;
        let next = self.row_starts[row as usize + 1] as usize;
        let ix = first + col as usize;
        if ix >= next {
            return "";
        }
        let start = self.field_starts[ix] as usize;
        let end = self.field_starts[ix + 1] as usize;
        &self.arena[start..end]
    }

    /// Sets a cell, returning the previous value.
    pub fn set(&mut self, row: RowId, col: ColId, value: &str) -> String {
        let old = self.get(row, col).to_string();
        if old != value {
            self.edits.insert(key(row, col), value.into());
        }
        old
    }

    pub fn add_row(&mut self) -> RowId {
        let id = self.total_rows;
        self.total_rows += 1;
        id
    }

    pub fn add_col(&mut self) -> ColId {
        let id = self.total_cols;
        self.total_cols += 1;
        id
    }

    pub fn total_rows(&self) -> u32 {
        self.total_rows
    }

    pub fn total_cols(&self) -> u32 {
        self.total_cols
    }
}

/// Incrementally builds a [`Store`] while parsing.
pub struct StoreBuilder {
    store: Store,
}

impl StoreBuilder {
    pub fn with_capacity(bytes: usize) -> Self {
        let mut store = Store::default();
        store.arena.reserve(bytes);
        store.field_starts.reserve(bytes / 8);
        store.row_starts.reserve(bytes / 64);
        store.row_starts.push(0);
        Self { store }
    }

    #[inline]
    pub fn begin_field(&mut self) {
        self.store.field_starts.push(self.store.arena.len() as u32);
    }

    #[inline]
    pub fn push_str(&mut self, s: &str) {
        self.store.arena.push_str(s);
    }

    #[inline]
    pub fn push_char(&mut self, c: char) {
        self.store.arena.push(c);
    }

    pub fn arena_len(&self) -> usize {
        self.store.arena.len()
    }

    /// Ends the current row.
    pub fn end_row(&mut self, field_count: usize) {
        self.store.parsed_rows += 1;
        self.store.total_cols = self.store.total_cols.max(field_count as u32);
        self.store
            .row_starts
            .push(self.store.field_starts.len() as u32);
    }

    /// Truncates the arena back to `len` and drops the fields pushed after the
    /// current row's `field_index`th field.
    pub fn rewind_row(&mut self, field_index: usize, arena_len: usize) {
        let start = *self.store.row_starts.last().unwrap() as usize;
        self.store.field_starts.truncate(start + field_index);
        self.store.arena.truncate(arena_len);
    }

    /// Takes the fields of the current (unfinished) row out of the builder.
    pub fn take_row(&mut self) -> Vec<String> {
        let start = *self.store.row_starts.last().unwrap() as usize;
        let starts = self.store.field_starts.split_off(start);
        let mut fields = Vec::with_capacity(starts.len());
        for (i, &s) in starts.iter().enumerate() {
            let end = starts
                .get(i + 1)
                .map(|&e| e as usize)
                .unwrap_or(self.store.arena.len());
            fields.push(self.store.arena[s as usize..end].to_string());
        }
        if let Some(&first) = starts.first() {
            self.store.arena.truncate(first as usize);
        }
        fields
    }

    pub fn rows(&self) -> u32 {
        self.store.parsed_rows
    }

    pub fn finish(mut self, min_cols: usize) -> Store {
        self.store.field_starts.push(self.store.arena.len() as u32);
        self.store.arena.shrink_to_fit();
        self.store.field_starts.shrink_to_fit();
        self.store.row_starts.shrink_to_fit();
        self.store.total_rows = self.store.parsed_rows;
        self.store.total_cols = self.store.total_cols.max(min_cols as u32);
        self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(rows: &[&[&str]]) -> Store {
        let mut b = StoreBuilder::with_capacity(64);
        for row in rows {
            for f in row.iter() {
                b.begin_field();
                b.push_str(f);
            }
            b.end_row(row.len());
        }
        b.finish(0)
    }

    #[test]
    fn get_ragged_and_overlay() {
        let mut s = build(&[&["a", "b", "c"], &["d"], &["", "f"]]);
        assert_eq!(s.total_cols(), 3);
        assert_eq!(s.get(0, 2), "c");
        assert_eq!(s.get(1, 0), "d");
        assert_eq!(s.get(1, 2), "");
        assert_eq!(s.get(2, 0), "");
        assert_eq!(s.get(2, 1), "f");
        assert_eq!(s.set(1, 2, "z"), "");
        assert_eq!(s.get(1, 2), "z");
        let r = s.add_row();
        assert_eq!(r, 3);
        assert_eq!(s.get(r, 0), "");
        let c = s.add_col();
        assert_eq!(c, 3);
        s.set(r, c, "new");
        assert_eq!(s.get(3, 3), "new");
    }
}
