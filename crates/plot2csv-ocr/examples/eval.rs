//! Headless benchmark of the automatic pipeline against synthetic ground truth.
//!
//!   cargo run -p plot2csv-ocr --release --features native-ocr --example eval -- vendor/models tests/fixtures [name-filter]
//!
//! Runs correction → panels → OCR → axis inference → series detection on every
//! fixture (NAME.png/jpg + NAME.json) and reports per-series errors in data units,
//! normalised by the axis range (log10 for log axes).

use std::path::Path;

use plot2csv_core::axes::Calibration;
use plot2csv_core::geom::Pt;
use plot2csv_core::project::Panel;
use plot2csv_ocr::backend::native::NativeOrt;
use plot2csv_ocr::indra::{ocr_page, IndraConfig};
use plot2csv_ocr::panel::{map_lines, merge_items, panel_crops};
use serde_json::Value;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VT)
    }
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) };
    let mut cx = Context::from_waker(&waker);
    let mut f = Box::pin(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

struct AxisTruth {
    min: f64,
    max: f64,
    log: bool,
}

fn axis(v: &Value) -> Option<AxisTruth> {
    Some(AxisTruth { min: v["min"].as_f64()?, max: v["max"].as_f64()?, log: v["log"].as_bool().unwrap_or(false) })
}

fn norm(v: f64, a: &AxisTruth) -> f64 {
    if a.log {
        (v.max(1e-300).log10() - a.min.log10()) / (a.max.log10() - a.min.log10())
    } else {
        (v - a.min) / (a.max - a.min)
    }
}

fn interp(pts: &[(f64, f64)], x: f64) -> Option<f64> {
    if pts.is_empty() || x < pts[0].0 || x > pts[pts.len() - 1].0 {
        return None;
    }
    let i = pts.partition_point(|p| p.0 < x).max(1).min(pts.len() - 1);
    let (a, b) = (pts[i - 1], pts[i]);
    Some(if (b.0 - a.0).abs() < 1e-12 { a.1 } else { a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0) })
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let models = Path::new(&args[1]);
    let dir = Path::new(&args[2]);
    let filter = args.get(3).cloned().unwrap_or_default();
    let verbose = std::env::var("EVAL_VERBOSE").is_ok();
    let dump = std::env::var("EVAL_DUMP").ok();
    let cfg = IndraConfig::from_meta_json(&std::fs::read_to_string(models.join("meta.json"))?)?;
    let be = NativeOrt::new(&std::fs::read(models.join("det.onnx"))?, &std::fs::read(models.join("rec.onnx"))?)?;
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json") && n.contains(&filter))
        .map(|n| n.trim_end_matches(".json").to_string())
        .collect();
    names.sort();
    let mut all_ok = true;
    for name in names {
        let truth: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{name}.json")))?)?;
        let img_path = ["png", "jpg"].iter().map(|e| dir.join(format!("{name}.{e}"))).find(|p| p.exists()).unwrap();
        let t0 = std::time::Instant::now();
        let rgba = image::open(&img_path)?.to_rgba8();
        let gray = plot2csv_vision::raster::rgba_to_gray(&rgba);
        let corr = plot2csv_vision::correct::detect_correction(&gray);
        let c = &corr.correction;
        let corrected =
            if c.enabled { plot2csv_vision::warp::warp_perspective(&rgba, &c.homography(), c.out_w, c.out_h) } else { rgba.clone() };
        let cgray = plot2csv_vision::raster::rgba_to_gray(&corrected);
        let regs = plot2csv_vision::regions::detect_panels(&cgray);
        if verbose {
            println!("   correction note: {}", corr.note);
        }
        println!("\n== {name}  [{}]  correction: {}  panels: {} — {}", img_path.display(), c.method, regs.panels.len(), regs.note);
        let tpanels = truth["panels"].as_array().cloned().unwrap_or_default();
        if regs.panels.len() != tpanels.len() {
            println!("   ✗ expected {} panels", tpanels.len());
            all_ok = false;
        }
        for (pi, (pg, tp)) in regs.panels.iter().zip(&tpanels).enumerate() {
            let mut kind = pg.kind;
            if tp["kind"].as_str() == Some("bar") {
                kind = plot2csv_core::AxesKind::Bar;
            }
            let panel = Panel { id: pi as u32 + 1, name: format!("p{pi}"), kind, inner: pg.inner, outer: pg.outer, enabled: true };
            if verbose {
                let (i, o) = (&panel.inner, &panel.outer);
                println!(
                    "   panel {pi} inner=({:.0},{:.0},{:.0},{:.0}) outer=({:.0},{:.0},{:.0},{:.0})",
                    i.x0, i.y0, i.x1, i.y1, o.x0, o.y0, o.x1, o.y1
                );
            }
            let mut collected = Vec::new();
            for cr in panel_crops(&cgray, &panel) {
                let lines = block_on(ocr_page(&be, &cfg, &cr.img))?;
                collected.extend(map_lines(lines, &cr));
            }
            let mut items = merge_items(collected, &panel.inner);
            let extra = plot2csv_ocr::panel::missing_tick_label_boxes(&cgray, &panel, &items);
            if !extra.is_empty() {
                let boxes: Vec<[f32; 4]> = extra.iter().map(|e| e.0).collect();
                let lines = block_on(plot2csv_ocr::indra::recognize(&be, &cfg, &cgray, &boxes))?;
                let tagged: Vec<(plot2csv_ocr::TextLine, bool)> = lines.into_iter().zip(extra.iter().map(|e| e.1)).collect();
                let new = plot2csv_ocr::panel::tick_items(&tagged);
                if verbose {
                    println!(
                        "      tick-guided OCR: {} boxes -> {:?}",
                        boxes.len(),
                        new.iter().map(|i| i.text.clone()).collect::<Vec<_>>()
                    );
                }
                items.extend(new);
            }
            if let Some(scan) = plot2csv_ocr::powers::find_power_labels(&cgray, &panel.inner, &items) {
                let lines = block_on(plot2csv_ocr::indra::recognize(&be, &cfg, &scan.canvas, &scan.boxes))?;
                let n = plot2csv_ocr::powers::apply_power_labels(&mut items, &scan, &lines, &panel.inner);
                if verbose {
                    let read: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
                    println!("      exponent OCR: {} labels, {n} rewritten; base/exp reads {read:?}", scan.labels.len());
                }
            }
            if verbose {
                for it in &items {
                    println!(
                        "      text {:?} {:?} value={:?} conf={:.2} box=({:.0},{:.0},{:.0},{:.0})",
                        it.text, it.role, it.value, it.conf, it.bbox.x0, it.bbox.y0, it.bbox.x1, it.bbox.y1
                    );
                }
            }
            let ax = plot2csv_vision::axes_infer::infer_axes(&cgray, &panel, &items);
            let cal = ax.axes.calibrate();
            println!("   panel {pi} ({:?}): {} text items; axes: {}", panel.kind, items.len(), ax.note);
            let cal = match cal {
                Ok(c) => c,
                Err(e) => {
                    println!("   ✗ calibration failed: {e}");
                    all_ok = false;
                    continue;
                }
            };
            let res = plot2csv_vision::series::detect_series(&corrected, &panel, &items, Some(&cal));
            println!("   series: {}", res.note);
            if std::env::var("EVAL_POINTS").is_ok() {
                println!("   cal points {:?}", ax.axes.points);
                for ds in &res.datasets {
                    let first: Vec<String> = ds
                        .points
                        .iter()
                        .take(4)
                        .map(|p| {
                            let d = cal.to_data(Pt::new(p.x, p.y));
                            format!("({:.4},{:.4})@({:.1},{:.1})", d[0], d[1], p.x, p.y)
                        })
                        .collect();
                    println!("   {}: {}", ds.name, first.join(" "));
                }
            }
            if let Some(d) = &dump {
                let mut ov = corrected.clone();
                let cols = [[255u8, 0, 0], [0, 180, 0], [0, 0, 255], [255, 0, 255], [0, 200, 200], [200, 120, 0]];
                for (k, ds) in res.datasets.iter().enumerate() {
                    let c = cols[k % cols.len()];
                    for p in &ds.points {
                        for dy in -2i64..=2 {
                            for dx in -2i64..=2 {
                                let (x, y) = (p.x as i64 + dx, p.y as i64 + dy);
                                if x >= 0 && y >= 0 && (x as u32) < ov.width() && (y as u32) < ov.height() {
                                    ov.put_pixel(x as u32, y as u32, image::Rgba([c[0], c[1], c[2], 255]));
                                }
                            }
                        }
                    }
                }
                std::fs::create_dir_all(d)?;
                ov.save(Path::new(d).join(format!("{name}_p{pi}.png")))?;
            }
            let (tx, ty) = (axis(&tp["x"]), axis(&tp["y"]).unwrap());
            // Extra series are junk: grid fragments, edge shades, split curves.
            let n_truth = tp["series"].as_array().map(|a| a.len()).unwrap_or(0);
            if res.datasets.len() != n_truth {
                println!("   ✗ expected {n_truth} series, found {}", res.datasets.len());
                all_ok = false;
            }
            for ts in tp["series"].as_array().unwrap() {
                let tname = ts["name"].as_str().unwrap_or("?");
                let tkind = ts["kind"].as_str().unwrap_or("line");
                let tpts: Vec<(f64, f64)> =
                    ts["points"].as_array().unwrap().iter().map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap())).collect();
                // Score every detected dataset; keep the best.
                let mut best: Option<(f64, f64, String, usize)> = None;
                for ds in &res.datasets {
                    let data: Vec<Vec<f64>> = ds.points.iter().map(|p| cal.to_data(Pt::new(p.x, p.y))).collect();
                    let (err, cov) = match tkind {
                        "bars" => {
                            let mut vals: Vec<f64> = data.iter().map(|d| d[0]).collect();
                            vals.truncate(tpts.len());
                            if vals.len() != tpts.len() {
                                (f64::INFINITY, vals.len() as f64 / tpts.len() as f64)
                            } else {
                                let e = median(vals.iter().zip(&tpts).map(|(v, t)| (norm(*v, &ty) - norm(t.1, &ty)).abs()).collect());
                                (e, 1.0)
                            }
                        }
                        "markers" => {
                            let tx = tx.as_ref().unwrap();
                            let errs: Vec<f64> = tpts
                                .iter()
                                .map(|t| {
                                    data.iter()
                                        .map(|d| {
                                            ((norm(d[0], tx) - norm(t.0, tx)).powi(2) + (norm(d[1], &ty) - norm(t.1, &ty)).powi(2)).sqrt()
                                        })
                                        .fold(f64::INFINITY, f64::min)
                                })
                                .collect();
                            let recall = errs.iter().filter(|e| **e < 0.02).count() as f64 / tpts.len() as f64;
                            let prec = if data.is_empty() { 0.0 } else { tpts.len() as f64 / data.len().max(tpts.len()) as f64 };
                            (median(errs), recall * prec)
                        }
                        _ => {
                            let tx = tx.as_ref().unwrap();
                            let errs: Vec<f64> = data
                                .iter()
                                .filter_map(|d| interp(&tpts, d[0]).map(|ty_| (norm(d[1], &ty) - norm(ty_, &ty)).abs()))
                                .collect();
                            let xs: Vec<f64> = data.iter().map(|d| d[0]).collect();
                            let lo = xs.iter().cloned().fold(f64::INFINITY, f64::min);
                            let hi = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                            let tr = tpts.last().unwrap().0 - tpts[0].0;
                            let good = errs.iter().filter(|e| **e < 0.02).count() as f64 / errs.len().max(1) as f64;
                            let cov = ((hi.min(tpts.last().unwrap().0) - lo.max(tpts[0].0)) / tr).clamp(0.0, 1.0) * good;
                            let _ = tx;
                            (median(errs), cov)
                        }
                    };
                    let score = err - cov;
                    if best.as_ref().map(|b| score < b.0 - b.1).unwrap_or(true) {
                        best = Some((err, cov, ds.name.clone(), ds.points.len()));
                    }
                }
                match best {
                    Some((err, cov, dname, n)) => {
                        // Names read from the legend, when the fixture has one.
                        let legend = ts["legend"].as_str();
                        let named = legend.map(|l| l == dname).unwrap_or(true);
                        let ok = err < 0.01 && cov > 0.8 && named;
                        all_ok &= ok;
                        println!(
                            "   {} {tname:<8} ← {dname:<22} n={n:<4} median err {:.3}%  coverage {:.0}%{}",
                            if ok { "✓" } else { "✗" },
                            err * 100.0,
                            cov * 100.0,
                            if named { String::new() } else { format!("  (legend says {:?})", legend.unwrap_or_default()) }
                        );
                    }
                    None => {
                        all_ok = false;
                        println!("   ✗ {tname}: no dataset detected");
                    }
                }
            }
            if let Calibration::Xy(_) = cal {}
        }
        println!("   time {:?}", t0.elapsed());
    }
    println!("\n{}", if all_ok { "ALL OK" } else { "SOME CHECKS FAILED" });
    Ok(())
}
