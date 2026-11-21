//! Export of all datasets: one CSV per series, a combined "long" CSV, a "wide"
//! CSV for XY plots, and a ZIP bundling them with the project JSON.

use std::io::Write;

use crate::axes::Calibration;
use crate::dataset::Dataset;
use crate::table::{csv_join, format_rows, headers, rows, SortBy, TableOptions};

/// A dataset with the calibration and panel it belongs to.
pub struct SeriesRef<'a> {
    pub panel: &'a str,
    pub cal: &'a Calibration,
    pub ds: &'a Dataset,
}

pub fn sanitize_filename(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    let t = t.trim_matches('_').to_string();
    if t.is_empty() {
        "series".into()
    } else {
        t
    }
}

/// CSV text for one series (with a header row).
pub fn series_csv(s: &SeriesRef, opts: &TableOptions) -> String {
    let o = TableOptions { header: true, ..opts.clone() };
    let r = rows(s.cal, s.ds, &o);
    format_rows(s.cal, &r, &o)
}

/// Long format: panel, series, then the data columns of each series.
pub fn long_csv(series: &[SeriesRef], opts: &TableOptions) -> String {
    let sep = opts.separator_str();
    let ncol = series.iter().map(|s| headers(s.cal).len()).max().unwrap_or(2);
    let mut head = vec!["panel".to_string(), "series".to_string()];
    match series.first() {
        Some(s) if series.iter().all(|t| headers(t.cal) == headers(s.cal)) => head.extend(headers(s.cal)),
        _ => head.extend((1..=ncol).map(|i| format!("v{i}"))),
    }
    let mut out = csv_join(&head, &sep) + "\n";
    for s in series {
        let o = TableOptions { header: false, ..opts.clone() };
        let r = rows(s.cal, s.ds, &o);
        for line in format_rows(s.cal, &r, &o).lines() {
            out += &csv_join(&[s.panel.to_string(), s.ds.name.clone()], &sep);
            out += &sep;
            out += line;
            out.push('\n');
        }
    }
    out
}

/// Wide format for XY series of one panel: the union of all x values, with each
/// series linearly interpolated inside its own x range (empty outside).
pub fn wide_csv(series: &[SeriesRef], opts: &TableOptions) -> Option<String> {
    if series.is_empty() || !series.iter().all(|s| matches!(s.cal, Calibration::Xy(_))) {
        return None;
    }
    let sep = opts.separator_str();
    let sorted: Vec<Vec<(f64, f64)>> = series
        .iter()
        .map(|s| {
            let o = TableOptions { sort_by: SortBy::Column(0), ascending: true, ..opts.clone() };
            rows(s.cal, s.ds, &o).into_iter().map(|r| (r.values[0], r.values[1])).collect()
        })
        .collect();
    let mut xs: Vec<f64> = sorted.iter().flatten().map(|p| p.0).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs.dedup_by(|a, b| (*a - *b).abs() <= 1e-12 * a.abs().max(1.0));
    let cal = series[0].cal;
    let mut head = vec![headers(cal)[0].clone()];
    head.extend(series.iter().map(|s| s.ds.name.clone()));
    let mut out = csv_join(&head, &sep) + "\n";
    for x in xs {
        let mut cells = vec![cal.format_value(0, x, opts.format, opts.digits)];
        for pts in &sorted {
            cells.push(interp(pts, x).map(|y| cal.format_value(1, y, opts.format, opts.digits)).unwrap_or_default());
        }
        out += &csv_join(&cells, &sep);
        out.push('\n');
    }
    Some(out)
}

fn interp(pts: &[(f64, f64)], x: f64) -> Option<f64> {
    let first = pts.first()?;
    let last = pts.last()?;
    if x < first.0 || x > last.0 {
        return None;
    }
    let i = pts.partition_point(|p| p.0 < x);
    if i < pts.len() && (pts[i].0 - x).abs() < 1e-12 {
        return Some(pts[i].1);
    }
    if i == 0 {
        return Some(first.1);
    }
    let (a, b) = (pts[i - 1], pts[i.min(pts.len() - 1)]);
    if (b.0 - a.0).abs() < 1e-15 {
        return Some(a.1);
    }
    Some(a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0))
}

/// Build a ZIP with per-series CSVs, the long CSV, wide CSVs per panel and the project.
pub fn export_zip(series: &[SeriesRef], opts: &TableOptions, project_json: Option<&str>) -> Result<Vec<u8>, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let fo = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut used = std::collections::HashSet::new();
        for s in series {
            let mut name = format!("{}_{}.csv", sanitize_filename(s.panel), sanitize_filename(&s.ds.name));
            let mut k = 2;
            while !used.insert(name.clone()) {
                name = format!("{}_{}_{k}.csv", sanitize_filename(s.panel), sanitize_filename(&s.ds.name));
                k += 1;
            }
            z.start_file(name, fo).map_err(|e| e.to_string())?;
            z.write_all(series_csv(s, opts).as_bytes()).map_err(|e| e.to_string())?;
        }
        z.start_file("all_series_long.csv", fo).map_err(|e| e.to_string())?;
        z.write_all(long_csv(series, opts).as_bytes()).map_err(|e| e.to_string())?;
        let mut panels: Vec<&str> = series.iter().map(|s| s.panel).collect();
        panels.dedup();
        for p in panels {
            let ps: Vec<SeriesRef> =
                series.iter().filter(|s| s.panel == p).map(|s| SeriesRef { panel: s.panel, cal: s.cal, ds: s.ds }).collect();
            if let Some(w) = wide_csv(&ps, opts) {
                z.start_file(format!("{}_wide.csv", sanitize_filename(p)), fo).map_err(|e| e.to_string())?;
                z.write_all(w.as_bytes()).map_err(|e| e.to_string())?;
            }
        }
        if let Some(j) = project_json {
            z.start_file("project.plot2csv.json", fo).map_err(|e| e.to_string())?;
            z.write_all(j.as_bytes()).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axes::{Axes, AxesKind};
    use crate::dataset::DataPoint;

    #[test]
    fn wide_interpolates() {
        let cal = Axes::new(AxesKind::Image, 10.0).calibrate().unwrap();
        // Image axes are not XY, so wide is not produced.
        let mut a = Dataset::new("a");
        a.points = vec![DataPoint::new(0.0, 0.0), DataPoint::new(2.0, 2.0)];
        assert!(wide_csv(&[SeriesRef { panel: "p", cal: &cal, ds: &a }], &TableOptions::default()).is_none());
        let mut axes = Axes::new(AxesKind::Xy, 10.0);
        axes.points = vec![
            crate::geom::Pt::new(0.0, 0.0),
            crate::geom::Pt::new(1.0, 0.0),
            crate::geom::Pt::new(0.0, 0.0),
            crate::geom::Pt::new(0.0, -1.0),
        ];
        let xy = axes.calibrate().unwrap();
        let mut b = Dataset::new("b");
        b.points = vec![DataPoint::new(1.0, 0.0)];
        let w = wide_csv(
            &[SeriesRef { panel: "p", cal: &xy, ds: &a }, SeriesRef { panel: "p", cal: &xy, ds: &b }],
            &TableOptions::default(),
        )
        .unwrap();
        assert_eq!(w, "X,a,b\n0,0,\n1,-1,0\n2,-2,\n");
    }

    #[test]
    fn zip_builds() {
        let cal = Axes::new(AxesKind::Image, 10.0).calibrate().unwrap();
        let mut a = Dataset::new("My series");
        a.points = vec![DataPoint::new(1.0, 2.0)];
        let z = export_zip(&[SeriesRef { panel: "Plot 1", cal: &cal, ds: &a }], &TableOptions::default(), Some("{}")).unwrap();
        assert!(z.len() > 100);
        assert_eq!(&z[..2], b"PK");
    }
}
