//! Turning OCR lines into chart text items: split merged tick rows, clean
//! tick-mark debris, parse values and assign roles (ticks, titles, legend).

use plot2csv_core::geom::RectF;
use plot2csv_core::project::{TextItem, TextRole};
use plot2csv_core::value::{normalize_number_text, parse_date, parse_number};

use crate::indra::TextLine;

/// Remove characters that come from tick marks or axis lines touching the text.
pub fn clean_label(s: &str) -> String {
    let t = normalize_number_text(s);
    let raw: Vec<&str> = t.split_whitespace().collect();
    let mut tokens: Vec<String> = Vec::new();
    let is_debris = |tok: &str| tok.chars().all(|c| matches!(c, '-' | '_' | '|' | '—' | '–' | '.' | ','));
    let mut i = 0;
    while i < raw.len() {
        let tok = raw[i];
        // A lone minus right before a number is its sign ("− 1.0" -> "-1.0").
        if tok == "-" && i + 1 < raw.len() && raw[i + 1].chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && tokens.is_empty() {
            tokens.push(format!("-{}", raw[i + 1]));
            i += 2;
            continue;
        }
        if !is_debris(tok) {
            tokens.push(tok.to_string());
        }
        i += 1;
    }
    let mut out = tokens.join(" ");
    // Trailing tick dash glued to a number: "0.5-" (but keep exponents like 1e-).
    while out.len() > 1 && (out.ends_with('-') || out.ends_with('|')) && !out.ends_with("e-") && !out.ends_with("E-") {
        out.pop();
        out = out.trim_end().to_string();
    }
    while out.starts_with('|') {
        out.remove(0);
        out = out.trim_start().to_string();
    }
    out
}

/// Numeric value of a label, if it is one.
pub fn label_value(s: &str) -> Option<f64> {
    let c = clean_label(s);
    // OCR of superscripts: "10°" (10⁰), "10'" (10¹) — read the base; the axis fit
    // decides whether these are powers of ten.
    let c2 = c.trim_end_matches(['°', '\'', '’', '`', '"', '*', '^']).to_string();
    parse_number(&c).or_else(|| parse_date(&c)).or_else(|| if c2 != c { parse_number(&c2) } else { None })
}

/// Split a line whose tokens are several numbers (a merged row of tick labels)
/// into one item per token, using the CTC character positions.
fn split_numeric_tokens(line: &TextLine) -> Vec<(RectF, String)> {
    let b = RectF::new(line.bbox[0] as f64, line.bbox[1] as f64, line.bbox[2] as f64, line.bbox[3] as f64);
    let tokens: Vec<&str> = line.text.split_whitespace().collect();
    let numeric = tokens.iter().filter(|t| label_value(t).is_some()).count();
    if tokens.len() < 2 || numeric < 2 || line.chars.is_empty() {
        return vec![(b, line.text.clone())];
    }
    // Group characters into tokens at spaces.
    let mut groups: Vec<Vec<(String, f32)>> = vec![Vec::new()];
    for (c, x) in &line.chars {
        if c.trim().is_empty() {
            if !groups.last().unwrap().is_empty() {
                groups.push(Vec::new());
            }
        } else {
            groups.last_mut().unwrap().push((c.clone(), *x));
        }
    }
    groups.retain(|g| !g.is_empty());
    let n_chars: usize = groups.iter().map(|g| g.len()).sum();
    let char_w = ((b.width()) / (n_chars.max(1) as f64 + groups.len() as f64)).max(2.0);
    groups
        .into_iter()
        .map(|g| {
            let text: String = g.iter().map(|(c, _)| c.as_str()).collect();
            let x0 = g.first().unwrap().1 as f64 - char_w * 0.6;
            let x1 = g.last().unwrap().1 as f64 + char_w * 0.6;
            (RectF::new(x0.max(b.x0), b.y0, x1.min(b.x1), b.y1), text)
        })
        .collect()
}

/// Convert OCR lines (corrected-image px) into text items and classify them.
pub fn items_from_lines(lines: &[(TextLine, bool)], inner: &RectF) -> Vec<TextItem> {
    let mut items = Vec::new();
    for (l, vertical) in lines {
        if l.text.trim().is_empty() {
            continue;
        }
        let parts = if *vertical { vec![(RectF::new(l.bbox[0] as f64, l.bbox[1] as f64, l.bbox[2] as f64, l.bbox[3] as f64), l.text.clone())] } else { split_numeric_tokens(l) };
        for (bbox, text) in parts {
            let cleaned = clean_label(&text);
            if cleaned.is_empty() {
                continue;
            }
            items.push(TextItem {
                bbox,
                value: label_value(&cleaned),
                text: cleaned,
                conf: l.conf,
                role: TextRole::Other,
                manual: false,
                vertical: *vertical,
            });
        }
    }
    classify(&mut items, inner);
    items
}

/// Assign roles from position relative to the data area and alignment.
pub fn classify(items: &mut [TextItem], inner: &RectF) {
    let iw = inner.width().max(1.0);
    let ih = inner.height().max(1.0);
    let mx = 0.04 * iw + 6.0;
    let my = 0.04 * ih + 6.0;
    for it in items.iter_mut() {
        if it.manual {
            continue;
        }
        let c = it.bbox.center();
        let below_amt = c.y - inner.y1;
        let left_amt = inner.x0 - c.x;
        // In the bottom-left corner, the label belongs to the axis it is further outside of.
        let below = c.y > inner.y1 - 2.0 && it.bbox.y0 > inner.y1 - 0.6 * it.bbox.height() && below_amt >= left_amt;
        let left = c.x < inner.x0 + 2.0 && it.bbox.x1 < inner.x0 + 0.6 * it.bbox.width().max(it.bbox.height()) && left_amt > below_amt;
        let above = c.y < inner.y0;
        let right = c.x > inner.x1;
        let in_x = c.x > inner.x0 - mx && c.x < inner.x1 + mx;
        let in_y = c.y > inner.y0 - my && c.y < inner.y1 + my;
        let numeric = it.value.is_some();
        let _ = numeric;
        it.role = if it.vertical {
            if left && !numeric {
                TextRole::YTitle
            } else {
                TextRole::Other
            }
        } else if below && in_x {
            if numeric {
                TextRole::XTick
            } else {
                TextRole::XTitle
            }
        } else if left && in_y {
            if numeric {
                TextRole::YTick
            } else {
                TextRole::YTitle
            }
        } else if above && in_x {
            if numeric && c.y > inner.y0 - 3.0 * it.bbox.height() {
                TextRole::Other
            } else {
                TextRole::Title
            }
        } else if inner.contains(c) {
            if numeric {
                TextRole::Other
            } else {
                TextRole::Legend
            }
        } else if right && in_y && !numeric {
            TextRole::Legend
        } else {
            TextRole::Other
        };
    }
    category_ticks(items, inner);
    refine_ticks(items, inner, true);
    refine_ticks(items, inner, false);
    // Keep only one title per axis: the closest non-tick text.
    pick_single(items, TextRole::XTitle, |it| it.bbox.y0);
    pick_single(items, TextRole::YTitle, |it| -it.bbox.x1);
    pick_single(items, TextRole::Title, |it| -it.bbox.y1);
}

/// Text items in a single row right under the X axis (no numeric ticks there)
/// are category labels (bar charts): give them the X-tick role.
fn category_ticks(items: &mut [TextItem], inner: &RectF) {
    let has_numeric_x = items.iter().any(|i| i.role == TextRole::XTick);
    if has_numeric_x {
        return;
    }
    let below: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| matches!(i.role, TextRole::XTitle) && i.bbox.y0 > inner.y1 - 2.0 && i.bbox.y0 < inner.y1 + 3.0 * i.bbox.height())
        .map(|(k, _)| k)
        .collect();
    if below.len() < 2 {
        return;
    }
    // Same row: centres within half a line height.
    let row_y = items[below[0]].bbox.center().y;
    let row: Vec<usize> = below.iter().copied().filter(|&k| (items[k].bbox.center().y - row_y).abs() < 0.6 * items[k].bbox.height()).collect();
    if row.len() >= 2 {
        for k in row {
            items[k].role = TextRole::XTick;
        }
    }
}

/// Ticks must form a row (X) or column (Y) close to the axis; numbers further
/// away become titles or are ignored.
fn refine_ticks(items: &mut [TextItem], inner: &RectF, x_axis: bool) {
    let role = if x_axis { TextRole::XTick } else { TextRole::YTick };
    let idx: Vec<usize> = items.iter().enumerate().filter(|(_, it)| it.role == role).map(|(i, _)| i).collect();
    if idx.is_empty() {
        return;
    }
    // Distance from the axis: for X, top of the box below inner.y1; for Y, right edge left of inner.x0.
    let key = |it: &TextItem| if x_axis { it.bbox.center().y } else { it.bbox.x1 };
    let size = |it: &TextItem| it.bbox.height();
    // Cluster by key; choose the cluster nearest to the axis with >= 2 members.
    let mut sorted = idx.clone();
    sorted.sort_by(|&a, &b| {
        let (ka, kb) = (key(&items[a]), key(&items[b]));
        if x_axis {
            ka.partial_cmp(&kb).unwrap()
        } else {
            kb.partial_cmp(&ka).unwrap()
        }
    });
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for i in sorted {
        let tol = 0.7 * size(&items[i]).max(6.0) + if x_axis { 0.0 } else { 6.0 };
        match clusters.last_mut() {
            Some(c) if (key(&items[c[0]]) - key(&items[i])).abs() < tol => c.push(i),
            _ => clusters.push(vec![i]),
        }
    }
    let best = clusters.iter().position(|c| c.len() >= 2).unwrap_or(0);
    for (ci, c) in clusters.iter().enumerate() {
        if ci == best {
            continue;
        }
        for &i in c {
            items[i].role = TextRole::Other;
        }
    }
    let _ = inner;
}

fn pick_single(items: &mut [TextItem], role: TextRole, order: impl Fn(&TextItem) -> f64) {
    let mut idx: Vec<usize> = items.iter().enumerate().filter(|(_, it)| it.role == role && !it.manual).map(|(i, _)| i).collect();
    if idx.len() <= 1 {
        return;
    }
    // Prefer the widest text, ties broken by `order`.
    idx.sort_by(|&a, &b| {
        let (wa, wb) = (items[a].text.len(), items[b].text.len());
        wb.cmp(&wa).then(order(&items[a]).partial_cmp(&order(&items[b])).unwrap())
    });
    for &i in &idx[1..] {
        items[i].role = if role == TextRole::Title { TextRole::Other } else { TextRole::Legend.min_role(items[i].role) };
    }
}

trait MinRole {
    fn min_role(self, cur: TextRole) -> TextRole;
}

impl MinRole for TextRole {
    fn min_role(self, _cur: TextRole) -> TextRole {
        TextRole::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(b: [f32; 4], text: &str) -> (TextLine, bool) {
        (TextLine { bbox: b, text: text.into(), conf: 0.9, chars: vec![] }, false)
    }

    #[test]
    fn cleans_tick_debris() {
        assert_eq!(clean_label("0.0 -"), "0.0");
        assert_eq!(clean_label("-0.5 -"), "-0.5");
        assert_eq!(clean_label("− 1.0"), "-1.0");
        assert_eq!(label_value("−1.0 -"), Some(-1.0));
        assert_eq!(clean_label("1e-"), "1e-");
    }

    #[test]
    fn classify_sine_chart() {
        // Lines from the OCR of tests/fixtures/sine_basic.png
        let inner = RectF::new(90.0, 50.0, 760.0, 480.0);
        let lines = vec![
            line([345., 19., 488., 35.], "Sine and cosine"),
            line([54., 45., 83., 57.], "1.0 "),
            line([52., 149., 77., 165.], "0.5"),
            line([53., 258., 83., 271.], "0.0 -"),
            line([47., 366., 86., 379.], "-0.5 -"),
            line([46., 472., 90., 489.], "-1.0 -"),
            line([86., 496., 95., 507.], "0"),
            line([198., 495., 205., 506.], "1"),
            line([311., 496., 317., 504.], "2"),
            line([422., 496., 430., 506.], "3"),
            line([397., 524., 469., 543.], "Time (s)"),
        ];
        let items = items_from_lines(&lines, &inner);
        let role = |t: &str| items.iter().find(|i| i.text == t).map(|i| i.role);
        assert_eq!(role("Sine and cosine"), Some(TextRole::Title));
        assert_eq!(role("0.5"), Some(TextRole::YTick));
        assert_eq!(role("-1.0"), Some(TextRole::YTick));
        assert_eq!(role("2"), Some(TextRole::XTick));
        assert_eq!(role("Time (s)"), Some(TextRole::XTitle));
        assert_eq!(items.iter().find(|i| i.text == "-0.5").unwrap().value, Some(-0.5));
    }

    #[test]
    fn splits_merged_row() {
        let l = TextLine {
            bbox: [100.0, 500.0, 300.0, 512.0],
            text: "0 10 20".into(),
            conf: 0.9,
            chars: vec![("0".into(), 105.0), (" ".into(), 150.0), ("1".into(), 196.0), ("0".into(), 204.0), (" ".into(), 250.0), ("2".into(), 286.0), ("0".into(), 294.0)],
        };
        let parts = split_numeric_tokens(&l);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].1, "10");
        assert!(parts[1].0.x0 > 180.0 && parts[1].0.x1 < 220.0, "{:?}", parts[1].0);
    }
}
