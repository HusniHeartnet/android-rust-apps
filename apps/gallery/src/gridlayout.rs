//! Row layout of the photo grid: a header per day followed by rows of tiles. Pure arithmetic,
//! so it can be tested without a screen.

pub const HEADER_H: f32 = 40.0;

pub enum RowKind {
    /// A day heading covering view positions `start..end`.
    Header { label: String, start: usize, end: usize },
    /// One row of `n` tiles beginning at view position `start`.
    Cells { start: usize, n: usize },
}

pub struct Row {
    pub y: f32,
    pub h: f32,
    pub kind: RowKind,
}

pub struct Layout {
    pub rows: Vec<Row>,
    pub height: f32,
}

impl Layout {
    /// Rows that touch the vertical span `y0..y1`, as a range of row numbers.
    pub fn visible(&self, y0: f32, y1: f32) -> (usize, usize) {
        let first = self.rows.partition_point(|r| r.y + r.h < y0);
        let last = self.rows.partition_point(|r| r.y < y1);
        (first, last.max(first))
    }

    pub fn row_at(&self, y: f32) -> Option<usize> {
        let i = self.rows.partition_point(|r| r.y + r.h <= y);
        self.rows.get(i).filter(|r| r.y <= y).map(|_| i)
    }

    /// The heading of the day that the row at height `y` belongs to.
    pub fn label_at(&self, y: f32) -> Option<&str> {
        let at = self.row_at(y.max(0.0)).or_else(|| self.rows.len().checked_sub(1))?;
        self.rows[..=at].iter().rev().find_map(|r| match &r.kind {
            RowKind::Header { label, .. } => Some(label.as_str()),
            _ => None,
        })
    }
}

/// `days` holds one day number per item (newest first, so equal days are neighbours).
pub fn build(days: &[i32], label: impl Fn(i32) -> String, cols: usize, cell: f32, gap: f32) -> Layout {
    let mut rows = Vec::new();
    let mut y = 0.0;
    let mut i = 0;
    while i < days.len() {
        let day = days[i];
        let mut j = i;
        while j < days.len() && days[j] == day {
            j += 1;
        }
        rows.push(Row { y, h: HEADER_H, kind: RowKind::Header { label: label(day), start: i, end: j } });
        y += HEADER_H;
        let mut s = i;
        while s < j {
            let n = cols.min(j - s);
            rows.push(Row { y, h: cell + gap, kind: RowKind::Cells { start: s, n } });
            y += cell + gap;
            s += n;
        }
        i = j;
    }
    Layout { rows, height: y }
}

pub fn day_key(y: i32, m: u32, d: u32) -> i32 {
    y * 10_000 + m as i32 * 100 + d as i32
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

pub fn day_label(day: i32, today: i32, yesterday: i32) -> String {
    if day == today {
        return "Today".into();
    }
    if day == yesterday {
        return "Yesterday".into();
    }
    let (y, m, d) = (day / 10_000, (day / 100 % 100) as usize, day % 100);
    let month = MONTHS.get(m.wrapping_sub(1)).copied().unwrap_or("?");
    if y == today / 10_000 { format!("{d} {month}") } else { format!("{d} {month} {y}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_become_headers_followed_by_rows_of_tiles() {
        let days = [20261021, 20261021, 20261021, 20261020, 20261020, 20261020, 20261020, 20261020];
        let l = build(&days, |d| d.to_string(), 3, 100.0, 2.0);
        let kinds: Vec<String> = l
            .rows
            .iter()
            .map(|r| match &r.kind {
                RowKind::Header { label, start, end } => format!("H{label}:{start}-{end}"),
                RowKind::Cells { start, n } => format!("C{start}+{n}"),
            })
            .collect();
        assert_eq!(kinds, ["H20261021:0-3", "C0+3", "H20261020:3-8", "C3+3", "C6+2"]);
        assert!((l.height - (2.0 * HEADER_H + 3.0 * 102.0)).abs() < 0.01);
        assert!(l.rows.windows(2).all(|w| (w[0].y + w[0].h - w[1].y).abs() < 0.01), "rows are stacked without gaps");
    }

    #[test]
    fn lookups_by_height() {
        let days = [2, 2, 1];
        let l = build(&days, |d| format!("d{d}"), 2, 50.0, 0.0);
        // rows: H(0..40) C(40..90) H(90..130) C(130..180)
        assert_eq!(l.row_at(10.0), Some(0));
        assert_eq!(l.row_at(45.0), Some(1));
        assert_eq!(l.row_at(135.0), Some(3));
        assert_eq!(l.row_at(500.0), None);
        assert_eq!(l.visible(41.0, 100.0), (1, 3));
        assert_eq!(l.visible(0.0, 1000.0), (0, 4));
        assert_eq!(l.label_at(50.0), Some("d2"));
        assert_eq!(l.label_at(140.0), Some("d1"));
        assert_eq!(l.label_at(9999.0), Some("d1"), "past the end is the last section");
        assert!(build(&[], |_| String::new(), 3, 10.0, 1.0).rows.is_empty());
    }

    #[test]
    fn day_names() {
        let today = day_key(2026, 10, 21);
        let yesterday = day_key(2026, 10, 20);
        assert_eq!(day_label(today, today, yesterday), "Today");
        assert_eq!(day_label(yesterday, today, yesterday), "Yesterday");
        assert_eq!(day_label(day_key(2026, 3, 5), today, yesterday), "5 March");
        assert_eq!(day_label(day_key(2025, 12, 31), today, yesterday), "31 December 2025");
    }
}
