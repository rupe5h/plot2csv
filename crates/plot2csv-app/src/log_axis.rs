//! Logarithmic axes for `egui_plot`, which only draws linear ones: plot `log10(value)` and use
//! [`grid_marks`] / [`tick_label`] so the grid is log-spaced and the labels show real values.

use std::ops::RangeInclusive;

use egui_plot::{GridInput, GridMark};

/// Exponents beyond this are outside `f64` anyway; keeps the loops bounded when zoomed far out.
const MAX_EXP: f64 = 400.0;
/// Deepest sub-decade subdivision (1.1, 1.01, …); `f64` precision runs out beyond it.
const MAX_DEPTH: u32 = 12;

/// Grid marks for an axis plotted as `log10(value)`.
///
/// Decades (every 2, 5, 10, … decades when they crowd), then 2× and 5×, the other integer
/// multiples, then finer steps within a decade (1.1, 1.2, …) as you zoom in. Each mark's
/// `step_size` is its distance to the nearest mark at least as coarse, so egui fades the lines
/// and hides the labels exactly where they would crowd together.
pub fn grid_marks(input: GridInput) -> Vec<GridMark> {
    let ((lo, hi), min_step) = (input.bounds, input.base_step_size);
    if lo.is_nan() || hi.is_nan() || min_step.is_nan() || min_step <= 0.0 || hi <= lo {
        return Vec::new();
    }
    let (lo, hi) = (lo.max(-MAX_EXP), hi.min(MAX_EXP));
    if hi <= lo {
        return Vec::new();
    }
    // (value, level); a lower level is a coarser mark.
    let mut marks: Vec<(f64, u32)> = Vec::new();

    // Decades: every `steps[0]` of them; multiples of the bigger steps are coarser.
    let steps: Vec<i64> = (0..12).flat_map(|e| [1, 2, 5].map(|f| f * 10i64.pow(e))).skip_while(|&s| (s as f64) < min_step).take(6).collect();
    let Some(&every) = steps.first() else { return Vec::new() };
    let finest = steps.len() as u32 - 1;
    let mut n = (lo / every as f64).ceil() as i64 * every;
    while n as f64 <= hi {
        let coarsest = steps.iter().rposition(|&s| n % s == 0).unwrap_or(0) as u32;
        marks.push((n as f64, finest - coarsest));
        n += every;
    }

    // Within each visible decade n: m·10^(n−j) for m in 10^j..10^(j+1), skipping the coarser ones.
    for j in 0..=MAX_DEPTH {
        let base = 10i64.pow(j);
        // Even the widest gap at this depth (between 10^j and 10^j + 1) would be faded out.
        if (1.0 + 1.0 / base as f64).log10() < min_step {
            break;
        }
        for n in lo.floor() as i64..=hi.floor() as i64 {
            let m_lo = ((base as f64 * 10f64.powf(lo - n as f64)).ceil() as i64).max(base);
            let m_hi = ((base as f64 * 10f64.powf(hi - n as f64)).floor() as i64).min(10 * base - 1);
            for m in m_lo..=m_hi {
                let level = match (j, m) {
                    (0, 1) => continue, // the decade itself
                    (0, 2 | 5) => finest + 1,
                    (0, _) => finest + 2,
                    _ if m % 10 == 0 => continue, // already a mark at depth j − 1
                    _ if m % 5 == 0 => finest + 1 + 2 * j,
                    _ => finest + 2 + 2 * j,
                };
                marks.push(((m as f64).log10() + (n - j as i64) as f64, level));
            }
        }
    }

    marks.sort_by(|a, b| a.0.total_cmp(&b.0));
    let gaps = gaps_to_coarser(&marks);
    marks
        .iter()
        .zip(gaps)
        .filter(|&(_, gap)| gap >= min_step)
        .map(|(&(value, _), step_size)| GridMark { value, step_size })
        .collect()
}

/// For each (sorted) mark, the distance to the nearest mark on either side whose level is not
/// finer than its own (infinite when it is the only one in view).
fn gaps_to_coarser(marks: &[(f64, u32)]) -> Vec<f64> {
    let levels = marks.iter().map(|m| m.1 as usize + 1).max().unwrap_or(0);
    let mut gaps = vec![f64::INFINITY; marks.len()];
    let mut last = vec![f64::NEG_INFINITY; levels];
    for (i, &(v, l)) in marks.iter().enumerate() {
        let prev = last[..=l as usize].iter().copied().fold(f64::NEG_INFINITY, f64::max);
        gaps[i] = v - prev;
        last[l as usize] = v;
    }
    let mut next = vec![f64::INFINITY; levels];
    for (i, &(v, l)) in marks.iter().enumerate().rev() {
        let after = next[..=l as usize].iter().copied().fold(f64::INFINITY, f64::min);
        gaps[i] = gaps[i].min(after - v);
        next[l as usize] = v;
    }
    gaps
}

/// Axis tick label for a mark on a log axis: the real value.
pub fn tick_label(mark: GridMark, _range: &RangeInclusive<f64>) -> String {
    format_value(10f64.powf(mark.value), 12)
}

/// `x` with at most `sig` significant digits and no trailing zeros; scientific notation
/// (`1e6`, `2.5e-7`) outside 0.001 ≤ |x| < 100000.
pub fn format_value(x: f64, sig: usize) -> String {
    if x == 0.0 || !x.is_finite() {
        return x.to_string();
    }
    let sig = sig.max(1);
    let exp = x.abs().log10().floor() as i32;
    if (-3..5).contains(&exp) {
        let decimals = (sig as i32 - 1 - exp).max(0) as usize;
        trim_zeros(&format!("{x:.decimals$}")).to_string()
    } else {
        let s = format!("{x:.*e}", sig - 1);
        match s.split_once('e') {
            Some((mantissa, e)) => format!("{}e{e}", trim_zeros(mantissa)),
            None => s,
        }
    }
}

fn trim_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(lo: f64, hi: f64, base_step_size: f64) -> Vec<GridMark> {
        grid_marks(GridInput { bounds: (lo, hi), base_step_size })
    }

    fn labels(m: &[GridMark]) -> Vec<String> {
        m.iter().map(|m| tick_label(*m, &(0.0..=1.0))).collect()
    }

    fn step_at(m: &[GridMark], value: f64) -> f64 {
        m.iter().find(|m| (m.value - value).abs() < 1e-9).map(|m| m.step_size).unwrap_or_else(|| panic!("no mark at {value}"))
    }

    #[test]
    fn decades_and_multiples() {
        let m = marks(0.0, 3.0, 0.01);
        let l = labels(&m);
        for want in ["1", "2", "5", "9", "10", "20", "100", "1000"] {
            assert!(l.iter().any(|s| s == want), "{want} missing from {l:?}");
        }
        assert_eq!(step_at(&m, 1.0), 1.0);
        assert!((step_at(&m, 2f64.log10()) - 2f64.log10()).abs() < 1e-12, "2× is spaced to 1 and 5");
        assert!((step_at(&m, 9f64.log10()) - (10f64 / 9.0).log10()).abs() < 1e-12);
        assert!(m.windows(2).all(|w| w[0].value < w[1].value));
    }

    #[test]
    fn crowded_decades_get_coarser_tiers() {
        let m = marks(-20.0, 20.0, 0.5);
        assert_eq!(m.len(), 41, "one mark per decade, nothing in between");
        assert_eq!(step_at(&m, 1.0), 1.0);
        assert_eq!(step_at(&m, 2.0), 2.0);
        assert_eq!(step_at(&m, 10.0), 10.0);
        let m = marks(-100.0, 100.0, 3.0);
        assert!(m.iter().all(|m| m.value % 5.0 == 0.0));
    }

    #[test]
    fn zoomed_into_a_decade() {
        let m = marks(0.0, 1.5f64.log10(), 1e-4);
        let l = labels(&m);
        for want in ["1", "1.1", "1.25", "1.5"] {
            assert!(l.iter().any(|s| s == want), "{want} missing from {l:?}");
        }
        assert!(m.len() < 2000);
    }

    #[test]
    fn degenerate_inputs() {
        assert!(marks(1.0, 1.0, 0.1).is_empty());
        assert!(marks(2.0, 1.0, 0.1).is_empty());
        assert!(marks(0.0, 1.0, 0.0).is_empty());
        assert!(marks(f64::NAN, 1.0, 0.1).is_empty());
        assert!(marks(-1e300, 1e300, 1e297).len() <= 1);
        assert!(!marks(-1e300, 1e300, 50.0).is_empty());
    }

    #[test]
    fn value_formatting() {
        assert_eq!(format_value(10f64.powf(3f64.log10()), 12), "3");
        assert_eq!(format_value(1e6, 12), "1e6");
        assert_eq!(format_value(12345.0, 12), "12345");
        assert_eq!(format_value(0.001, 12), "0.001");
        assert_eq!(format_value(1e-4, 12), "1e-4");
        assert_eq!(format_value(2.5e-7, 12), "2.5e-7");
        assert_eq!(format_value(-0.25, 12), "-0.25");
        assert_eq!(format_value(1234.5678, 4), "1235");
        assert_eq!(format_value(9.99999999999999e5, 12), "1e6");
    }
}
