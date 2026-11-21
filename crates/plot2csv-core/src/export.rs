//! Export of all datasets: one CSV per series, a combined "long" CSV, a "wide"
//! CSV with every series of a panel side by side (X-Y and bar charts), and a
//! ZIP bundling them with the project JSON.

use std::io::Write;

use crate::axes::Calibration;
use crate::dataset::Dataset;
use crate::table::{csv_join, format_rows, headers, rows, AxisTitles, SortBy, TableOptions};

/// A dataset with the calibration, panel and axis titles it belongs to.
pub struct SeriesRef<'a> {
    pub panel: &'a str,
    pub cal: &'a Calibration,
    pub ds: &'a Dataset,
    pub titles: &'a AxisTitles,
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
    format_rows(s.cal, &r, &o, s.titles)
}

/// Long format: panel, series, then the data columns of each series (headed
/// by the axis titles when every series shares them).
pub fn long_csv(series: &[SeriesRef], opts: &TableOptions) -> String {
    let sep = opts.separator_str();
    let ncol = series.iter().map(|s| s.cal.variables().len()).max().unwrap_or(2);
    let mut head = vec!["panel".to_string(), "series".to_string()];
    let titled = |s: &SeriesRef| headers(s.cal, s.titles);
    match series.first() {
        Some(s) if series.iter().all(|t| titled(t) == titled(s)) => head.extend(titled(s)),
        Some(s) if series.iter().all(|t| t.cal.variables() == s.cal.variables()) => head.extend(s.cal.variables()),
        _ => head.extend((1..=ncol).map(|i| format!("v{i}"))),
    }
    let mut out = csv_join(&head, &sep) + "\n";
    for s in series {
        let o = TableOptions { header: false, ..opts.clone() };
        let r = rows(s.cal, s.ds, &o);
        for line in format_rows(s.cal, &r, &o, s.titles).lines() {
            out += &csv_join(&[s.panel.to_string(), s.ds.name.clone()], &sep);
            out += &sep;
            out += line;
            out.push('\n');
        }
    }
    out
}

/// Wide format: every series of one panel side by side, one column each.
///
/// X-Y plots: the rows are the union of all the series' X values, so no
/// sampled point is lost; each series is linearly interpolated at the X values
/// of the others inside its own X range (empty outside). Bar charts: one row
/// per category, in the order of the bars. `None` for other plot types.
/// The header row (axis titles, series names) follows `opts.header`.
pub fn wide_csv(series: &[SeriesRef], opts: &TableOptions) -> Option<String> {
    let first = series.first()?;
    let (head, body) = match first.cal {
        Calibration::Xy(_) if series.iter().all(|s| matches!(s.cal, Calibration::Xy(_))) => wide_xy(series, opts),
        Calibration::Bar(_) if series.iter().all(|s| matches!(s.cal, Calibration::Bar(_))) => wide_bars(series, opts),
        _ => return None,
    };
    let sep = opts.separator_str();
    let mut out = String::new();
    if opts.header {
        out += &csv_join(&head, &sep);
        out.push('\n');
    }
    for cells in body {
        out += &csv_join(&cells, &sep);
        out.push('\n');
    }
    Some(out)
}

/// Series column header: the series name (the Y title is the same for all).
fn series_heads<'a>(series: &'a [SeriesRef<'a>]) -> impl Iterator<Item = String> + 'a {
    series.iter().map(|s| s.ds.name.clone())
}

fn wide_xy(series: &[SeriesRef], opts: &TableOptions) -> (Vec<String>, Vec<Vec<String>>) {
    let sorted: Vec<Vec<(f64, f64)>> = series
        .iter()
        .map(|s| {
            let o = TableOptions { sort_by: SortBy::Column(0), ascending: true, ..opts.clone() };
            rows(s.cal, s.ds, &o).into_iter().map(|r| (r.values[0], r.values[1])).filter(|p| p.0.is_finite()).collect()
        })
        .collect();
    let mut xs: Vec<f64> = sorted.iter().flatten().map(|p| p.0).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs.dedup_by(|a, b| (*a - *b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1e-300));
    if opts.sort_by == SortBy::Column(0) && !opts.ascending {
        xs.reverse();
    }
    let cal = series[0].cal;
    let mut head = vec![headers(cal, series[0].titles)[0].clone()];
    head.extend(series_heads(series));
    let body = xs
        .into_iter()
        .map(|x| {
            let mut cells = vec![cal.format_value(0, x, opts.format, opts.digits)];
            for pts in &sorted {
                cells.push(interp(pts, x).map(|y| cal.format_value(1, y, opts.format, opts.digits)).unwrap_or_default());
            }
            cells
        })
        .collect();
    (head, body)
}

fn wide_bars(series: &[SeriesRef], opts: &TableOptions) -> (Vec<String>, Vec<Vec<String>>) {
    // Category key: the label (and its occurrence, for a label repeated in a
    // series), or the bar's rank for unlabelled bars.
    struct Category {
        key: (String, usize),
        x_sum: f64,
        n: usize,
        values: Vec<Option<f64>>,
    }
    let mut cats: Vec<Category> = Vec::new();
    for (si, s) in series.iter().enumerate() {
        let mut bars: Vec<(f64, Option<String>, f64)> = rows(s.cal, s.ds, &TableOptions { sort_by: SortBy::Raw, ..opts.clone() })
            .into_iter()
            .filter_map(|r| Some((s.ds.points.get(r.index)?.x, r.label.clone(), *r.values.first()?)))
            .collect();
        bars.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (rank, (x, label, v)) in bars.into_iter().enumerate() {
            let key = match label.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()) {
                Some(l) => {
                    let k = seen.entry(l.clone()).or_insert(0);
                    *k += 1;
                    (l, *k)
                }
                None => (format!("Bar{}", rank + 1), 1),
            };
            let ci = match cats.iter().position(|c| c.key == key) {
                Some(ci) => ci,
                None => {
                    cats.push(Category { key, x_sum: 0.0, n: 0, values: vec![None; series.len()] });
                    cats.len() - 1
                }
            };
            let c = &mut cats[ci];
            c.x_sum += x;
            c.n += 1;
            c.values[si] = Some(v);
        }
    }
    // Categories in the order of their bars along the axis.
    cats.sort_by(|a, b| (a.x_sum / a.n as f64).total_cmp(&(b.x_sum / b.n as f64)));
    let cal = series[0].cal;
    let mut head = vec![headers(cal, series[0].titles)[0].clone()];
    head.extend(series_heads(series));
    let body = cats
        .into_iter()
        .map(|c| {
            let name = if c.key.1 > 1 { format!("{} ({})", c.key.0, c.key.1) } else { c.key.0 };
            let mut cells = vec![name];
            // Bar values are column 1 for display purposes (as in `format_rows`).
            cells.extend(c.values.iter().map(|v| v.map(|v| cal.format_value(1, v, opts.format, opts.digits)).unwrap_or_default()));
            cells
        })
        .collect();
    (head, body)
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
            let ps: Vec<SeriesRef> = series
                .iter()
                .filter(|s| s.panel == p)
                .map(|s| SeriesRef { panel: s.panel, cal: s.cal, ds: s.ds, titles: s.titles })
                .collect();
            let o = TableOptions { header: true, ..opts.clone() };
            if let Some(w) = wide_csv(&ps, &o) {
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

    const NO_TITLES: &AxisTitles = &AxisTitles { x: None, y: None };

    #[test]
    fn wide_interpolates() {
        let cal = Axes::new(AxesKind::Image, 10.0).calibrate().unwrap();
        // Image axes are not XY, so wide is not produced.
        let mut a = Dataset::new("a");
        a.points = vec![DataPoint::new(0.0, 0.0), DataPoint::new(2.0, 2.0)];
        assert!(wide_csv(&[SeriesRef { panel: "p", cal: &cal, ds: &a, titles: NO_TITLES }], &TableOptions::default()).is_none());
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
        let header = TableOptions { header: true, ..Default::default() };
        let pair = [
            SeriesRef { panel: "p", cal: &xy, ds: &a, titles: NO_TITLES },
            SeriesRef { panel: "p", cal: &xy, ds: &b, titles: NO_TITLES },
        ];
        assert_eq!(wide_csv(&pair, &header).unwrap(), "X,a,b\n0,0,\n1,-1,0\n2,-2,\n");
        // Without the header row; with the X axis title as its header.
        assert_eq!(wide_csv(&pair, &TableOptions::default()).unwrap(), "0,0,\n1,-1,0\n2,-2,\n");
        let titles = AxisTitles { x: Some("Time [s]".into()), y: Some("Speed".into()) };
        let titled = [SeriesRef { panel: "p", cal: &xy, ds: &a, titles: &titles }, SeriesRef { panel: "p", cal: &xy, ds: &b, titles: &titles }];
        assert!(wide_csv(&titled, &header).unwrap().starts_with("Time [s],a,b\n"));
        assert!(series_csv(&titled[0], &TableOptions::default()).starts_with("Time [s],Speed\n"));
        assert!(long_csv(&titled, &TableOptions::default()).starts_with("panel,series,Time [s],Speed\n"));
    }

    #[test]
    fn wide_bars_one_column_per_series() {
        // Bar values: y pixel 100 -> 0, 0 -> 10.
        let mut axes = Axes::new(AxesKind::Bar, 10.0);
        axes.points = vec![crate::geom::Pt::new(0.0, 100.0), crate::geom::Pt::new(0.0, 0.0)];
        axes.params = crate::axes::AxesParams::Bar(crate::axes::BarParams { p1: "0".into(), p2: "10".into(), log: false });
        let cal = axes.calibrate().unwrap();
        let bar = |x: f64, y: f64, l: &str| DataPoint { x, y, label: Some(l.into()) };
        let mut od = Dataset::new("OD");
        od.points = vec![bar(110.0, 50.0, "DS2"), bar(10.0, 80.0, "DS1")];
        let mut eda = Dataset::new("EDA");
        eda.points = vec![bar(20.0, 70.0, "DS1"), bar(120.0, 40.0, "DS2"), bar(220.0, 30.0, "DS3")];
        let titles = AxisTitles { x: Some("Datasets".into()), y: Some("F1 (%)".into()) };
        let s = [SeriesRef { panel: "p", cal: &cal, ds: &od, titles: &titles }, SeriesRef { panel: "p", cal: &cal, ds: &eda, titles: &titles }];
        let opts = TableOptions { header: true, format: crate::NumFormat::Fixed, digits: 1, ..Default::default() };
        assert_eq!(wide_csv(&s, &opts).unwrap(), "Datasets,OD,EDA\nDS1,2.0,3.0\nDS2,5.0,6.0\nDS3,,7.0\n");
    }

    #[test]
    fn zip_builds() {
        let cal = Axes::new(AxesKind::Image, 10.0).calibrate().unwrap();
        let mut a = Dataset::new("My series");
        a.points = vec![DataPoint::new(1.0, 2.0)];
        let z = export_zip(&[SeriesRef { panel: "Plot 1", cal: &cal, ds: &a, titles: NO_TITLES }], &TableOptions::default(), Some("{}")).unwrap();
        assert!(z.len() > 100);
        assert_eq!(&z[..2], b"PK");
    }
}
