//! Where accumulated cells live until the sweep reads them.
//!
//! The sweep needs cells grouped by row and sorted by column within a row, and
//! it needs that ordering to be *total* — two cells at the same `(x, y)` must
//! come out in a deterministic order, or the same path could produce different
//! bytes on different runs. A plain sort by `(y, x)` gives exactly that, and
//! since the sweep merges equal-x cells by summing them, their relative order
//! within an x does not affect the result either.
//!
//! The store is a flat `Vec` rather than the oracle's block-allocated pool.
//! The pool exists to avoid reallocating while a path is being scanned, which
//! a growing `Vec` amortises anyway, and the flat buffer sorts faster and
//! carries no cell limit — the oracle's pool silently *stops recording cells*
//! past 1024 blocks, dropping geometry from a complex path, which is a damage
//! behaviour worth not reproducing.

use super::Cell;

/// A path's cells, sortable into row-major order.
#[derive(Debug, Default)]
pub struct CellStore {
    cells: Vec<Cell>,
    /// Row boundaries into `cells`, valid only after [`CellStore::sort`].
    rows: Vec<(i32, usize, usize)>,
    sorted: bool,
}

impl CellStore {
    /// Record one cell.
    pub fn push(&mut self, cell: Cell) {
        self.cells.push(cell);
        self.sorted = false;
    }

    /// Whether nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Drop every cell, keeping the allocations.
    pub fn clear(&mut self) {
        self.cells.clear();
        self.rows.clear();
        self.sorted = false;
    }

    /// Sort into row-major order and index the rows.
    pub fn sort(&mut self) {
        if self.sorted {
            return;
        }
        self.cells.sort_unstable_by_key(|c| (c.y, c.x));
        self.rows.clear();
        let mut start = 0usize;
        while start < self.cells.len() {
            let Some(first) = self.cells.get(start) else {
                break;
            };
            let y = first.y;
            let mut end = start + 1;
            while self.cells.get(end).is_some_and(|c| c.y == y) {
                end += 1;
            }
            self.rows.push((y, start, end));
            start = end;
        }
        self.sorted = true;
    }

    /// The rows, each a `(y, cells)` pair, in increasing y.
    ///
    /// Call [`CellStore::sort`] first; an unsorted store yields nothing rather
    /// than a wrong answer.
    pub fn rows(&self) -> impl Iterator<Item = (i32, &[Cell])> {
        let cells = &self.cells;
        let rows: &[(i32, usize, usize)] = if self.sorted { &self.rows } else { &[] };
        rows.iter()
            .filter_map(move |&(y, start, end)| cells.get(start..end).map(|row| (y, row)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(x: i32, y: i32) -> Cell {
        Cell {
            x,
            y,
            cover: 1,
            area: 1,
        }
    }

    #[test]
    fn rows_come_out_sorted_and_grouped() {
        let mut store = CellStore::default();
        store.push(cell(3, 1));
        store.push(cell(1, 0));
        store.push(cell(2, 1));
        store.push(cell(0, 0));
        store.sort();
        let rows: Vec<(i32, Vec<i32>)> = store
            .rows()
            .map(|(y, cells)| (y, cells.iter().map(|c| c.x).collect()))
            .collect();
        assert_eq!(rows, vec![(0, vec![0, 1]), (1, vec![2, 3])]);
    }

    #[test]
    fn an_unsorted_store_yields_no_rows() {
        let mut store = CellStore::default();
        store.push(cell(0, 0));
        assert_eq!(store.rows().count(), 0, "sort() is the gate");
        store.sort();
        assert_eq!(store.rows().count(), 1);
    }

    #[test]
    fn clearing_resets_the_sorted_flag() {
        let mut store = CellStore::default();
        store.push(cell(0, 0));
        store.sort();
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.rows().count(), 0);
    }

    #[test]
    fn negative_rows_sort_before_positive_ones() {
        // A path may extend above the target; the sweep clips, but the order
        // must still be monotone in y.
        let mut store = CellStore::default();
        store.push(cell(0, 5));
        store.push(cell(0, -3));
        store.sort();
        let ys: Vec<i32> = store.rows().map(|(y, _)| y).collect();
        assert_eq!(ys, vec![-3, 5]);
    }
}
