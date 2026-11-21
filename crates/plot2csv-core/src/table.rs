//! Turning a dataset into a formatted table: sorting and number formatting as in
//! WebPlotDigitizer's "Acquired Data" dialog, plus CSV output.

use serde::{Deserialize, Serialize};

use crate::axes::Calibration;
use crate::dataset::Dataset;
use crate::value::NumFormat;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SortBy {
    #[default]
    Raw,
    /// Sort by data column i.
    Column(usize),
    NearestNeighbor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TableOptions {
    pub sort_by: SortBy,
    pub ascending: bool,
    pub format: NumFormat,
    pub digits: usize,
    /// As typed by the user; `\t` means tab.
    pub separator: String,
    pub header: bool,
}

impl Default for TableOptions {
    fn default() -> Self {
        Self { sort_by: SortBy::Raw, ascending: true, format: NumFormat::Ignore, digits: 5, separator: ",".into(), header: false }
    }
}

impl TableOptions {
    pub fn separator_str(&self) -> String {
        if self.separator.is_empty() {
            ",".into()
        } else {
            self.separator.replace("\\t", "\t")
        }
    }
}

/// One table row: optional label then numeric values.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: Option<String>,
    pub values: Vec<f64>,
    /// Index of the point in the dataset.
    pub index: usize,
}

/// Compute rows (data values for every point) in the requested order.
pub fn rows(cal: &Calibration, ds: &Dataset, opts: &TableOptions) -> Vec<Row> {
    let mut rows: Vec<Row> = ds
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| Row { label: p.label.clone(), values: cal.to_data(p.pt()), index: i })
        .collect();
    match opts.sort_by {
        SortBy::Raw => {}
        SortBy::Column(c) => {
            rows.sort_by(|a, b| {
                let (x, y) = (a.values.get(c).copied().unwrap_or(0.0), b.values.get(c).copied().unwrap_or(0.0));
                x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
            });
            if !opts.ascending {
                rows.reverse();
            }
        }
        SortBy::NearestNeighbor => {
            // Greedy chain in pixel space starting from the first point (WPD behaviour).
            let pts: Vec<_> = ds.points.iter().map(|p| p.pt()).collect();
            let order = nearest_neighbor_order(&pts);
            let mut by_index: Vec<Option<Row>> = rows.into_iter().map(Some).collect();
            rows = order.into_iter().filter_map(|i| by_index[i].take()).collect();
        }
    }
    rows
}

pub fn nearest_neighbor_order(pts: &[crate::geom::Pt]) -> Vec<usize> {
    let n = pts.len();
    if n == 0 {
        return Vec::new();
    }
    let mut used = vec![false; n];
    let mut order = vec![0];
    used[0] = true;
    for _ in 1..n {
        let last = pts[*order.last().unwrap()];
        let mut best = usize::MAX;
        let mut bd = f64::INFINITY;
        for (i, p) in pts.iter().enumerate() {
            if !used[i] {
                let d = p.dist(last);
                if d < bd {
                    bd = d;
                    best = i;
                }
            }
        }
        used[best] = true;
        order.push(best);
    }
    order
}

/// Column headers for a dataset table.
pub fn headers(cal: &Calibration) -> Vec<String> {
    cal.variables()
}

/// Format rows as delimited text.
pub fn format_rows(cal: &Calibration, rows: &[Row], opts: &TableOptions) -> String {
    let sep = opts.separator_str();
    let is_bar = matches!(cal, Calibration::Bar(_));
    let mut out = String::new();
    if opts.header {
        out += &csv_join(&headers(cal), &sep);
        out.push('\n');
    }
    for r in rows {
        let mut cells: Vec<String> = Vec::new();
        if is_bar {
            cells.push(r.label.clone().unwrap_or_else(|| format!("Bar{}", r.index)));
            // Bar values are column 1 for display purposes but data index 0.
            for v in &r.values {
                cells.push(cal.format_value(1, *v, opts.format, opts.digits));
            }
        } else {
            for (i, v) in r.values.iter().enumerate() {
                cells.push(cal.format_value(i, *v, opts.format, opts.digits));
            }
        }
        out += &csv_join(&cells, &sep);
        out.push('\n');
    }
    out
}

/// Join cells, quoting any cell that contains the separator, a quote or a newline.
pub fn csv_join(cells: &[String], sep: &str) -> String {
    cells
        .iter()
        .map(|c| {
            if c.contains(sep) || c.contains('"') || c.contains('\n') {
                format!("\"{}\"", c.replace('"', "\"\""))
            } else {
                c.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axes::{Axes, AxesKind};
    use crate::dataset::DataPoint;
    use crate::geom::Pt;

    fn image_cal() -> Calibration {
        Axes::new(AxesKind::Image, 100.0).calibrate().unwrap()
    }

    #[test]
    fn sort_and_format() {
        let mut ds = Dataset::new("d");
        ds.points = vec![DataPoint::new(3.0, 1.0), DataPoint::new(1.0, 2.0), DataPoint::new(2.0, 3.0)];
        let cal = image_cal();
        let opts = TableOptions { sort_by: SortBy::Column(0), format: NumFormat::Fixed, digits: 1, ..Default::default() };
        let r = rows(&cal, &ds, &opts);
        assert_eq!(format_rows(&cal, &r, &opts), "1.0,2.0\n2.0,3.0\n3.0,1.0\n");
        let opts2 = TableOptions { separator: "\\t".into(), header: true, ..opts.clone() };
        assert_eq!(format_rows(&cal, &r, &opts2).lines().next().unwrap(), "X\tY");
    }

    #[test]
    fn nearest_neighbor_follows_curve() {
        let pts = [Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(1.0, 0.0), Pt::new(5.0, 0.0)];
        assert_eq!(nearest_neighbor_order(&pts), vec![0, 2, 3, 1]);
    }

    #[test]
    fn csv_quotes() {
        assert_eq!(csv_join(&["a,b".into(), "c".into()], ","), "\"a,b\",c");
    }
}
