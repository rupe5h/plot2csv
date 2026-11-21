//! Parsing and formatting of axis values: plain numbers, exponents and dates.

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

/// Days since 1970-01-01 are used as the numeric representation of dates.
const SECS_PER_DAY: f64 = 86_400.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Parsed {
    Number(f64),
    /// days since the Unix epoch
    Date(f64),
}

impl Parsed {
    pub fn value(self) -> f64 {
        match self {
            Parsed::Number(v) | Parsed::Date(v) => v,
        }
    }
    pub fn is_date(self) -> bool {
        matches!(self, Parsed::Date(_))
    }
}

/// Normalise typographic variants seen in plots and OCR output.
pub fn normalize_number_text(s: &str) -> String {
    s.trim()
        .replace(['\u{2212}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{fe63}', '\u{ff0d}'], "-")
        .replace(['\u{00d7}', '\u{2715}', '\u{22c5}'], "x")
        .replace('\u{00a0}', " ")
}

const SUPERSCRIPTS: [(char, char); 12] = [
    ('⁰', '0'),
    ('¹', '1'),
    ('²', '2'),
    ('³', '3'),
    ('⁴', '4'),
    ('⁵', '5'),
    ('⁶', '6'),
    ('⁷', '7'),
    ('⁸', '8'),
    ('⁹', '9'),
    ('⁻', '-'),
    ('⁺', '+'),
];

fn sup_to_ascii(c: char) -> Option<char> {
    SUPERSCRIPTS.iter().find(|(s, _)| *s == c).map(|(_, a)| *a)
}

/// Parse a number as typed by a user or read by OCR.
///
/// Accepts `1.5`, `-0.5`, `−0.5`, `1e-3`, `1E+03`, `10^3`, `10^-3`, `10³`,
/// `2x10^3`, `2×10³`, `1,000`, `50%` (as 50), `+3`.
pub fn parse_number(s: &str) -> Option<f64> {
    let s = normalize_number_text(s);
    let s = s.trim().trim_end_matches('%').trim();
    if s.is_empty() {
        return None;
    }
    // Superscript exponent: 10³, 10⁻², 2x10⁵
    if let Some(pos) = s.char_indices().find(|(_, c)| sup_to_ascii(*c).is_some()).map(|(i, _)| i) {
        let (base, exp) = s.split_at(pos);
        let exp: String = exp.chars().filter_map(sup_to_ascii).collect();
        return parse_power(base, &exp);
    }
    if let Some((base, exp)) = s.split_once('^') {
        return parse_power(base, exp.trim_start_matches('{').trim_end_matches('}'));
    }
    let plain: String = s.chars().filter(|c| *c != ' ').collect();
    // thousands separators: 1,000 or 12,345.6 (only when groups of 3)
    let plain = if is_thousands_grouped(&plain) { plain.replace(',', "") } else { plain };
    plain.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn is_thousands_grouped(s: &str) -> bool {
    let body = s.trim_start_matches(['-', '+']);
    let int = body.split('.').next().unwrap_or("");
    if !int.contains(',') {
        return false;
    }
    let groups: Vec<&str> = int.split(',').collect();
    !groups[0].is_empty()
        && groups[0].len() <= 3
        && groups[1..].iter().all(|g| g.len() == 3 && g.chars().all(|c| c.is_ascii_digit()))
        && groups[0].chars().all(|c| c.is_ascii_digit())
}

/// `base` is like "10", "2x10", "2 x 10", "-10"; exp is the exponent text.
fn parse_power(base: &str, exp: &str) -> Option<f64> {
    let e: f64 = exp.trim().parse().ok()?;
    let base = base.trim();
    let (mult, b) = match base.rsplit_once(['x', '*']) {
        Some((m, b)) => (parse_number(m)?, b.trim()),
        None => (1.0, base),
    };
    let (sign, b) = match b.strip_prefix('-') {
        Some(r) => (-1.0, r),
        None => (1.0, b),
    };
    let b: f64 = b.parse().ok()?;
    let v = sign * mult * b.powf(e);
    v.is_finite().then_some(v)
}

/// Parse a date in WebPlotDigitizer's formats: yyyy/mm/dd, yyyy/mm, yyyy-mm-dd,
/// optionally followed by HH:MM[:SS]. Returns days since the Unix epoch.
pub fn parse_date(s: &str) -> Option<f64> {
    let s = s.trim();
    let (date_part, time_part) = match s.split_once([' ', 'T']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let parts: Vec<&str> = date_part.split(['/', '-', '.']).collect();
    if parts.len() < 2 || parts.len() > 3 || parts[0].len() != 4 {
        return None;
    }
    let y: i32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = if parts.len() == 3 { parts[2].parse().ok()? } else { 1 };
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    let mut secs = 0.0;
    if let Some(t) = time_part {
        let tp: Vec<&str> = t.split(':').collect();
        let h: f64 = tp.first()?.parse().ok()?;
        let mi: f64 = tp.get(1).map(|v| v.parse().ok()).unwrap_or(Some(0.0))?;
        let se: f64 = tp.get(2).map(|v| v.parse().ok()).unwrap_or(Some(0.0))?;
        secs = h * 3600.0 + mi * 60.0 + se;
    }
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
    Some((date - epoch).num_days() as f64 + secs / SECS_PER_DAY)
}

/// Parse a user-entered calibration value (number or date).
pub fn parse_value(s: &str) -> Option<Parsed> {
    if let Some(v) = parse_number(s) {
        return Some(Parsed::Number(v));
    }
    parse_date(s).map(Parsed::Date)
}

fn days_to_datetime(days: f64) -> Option<NaiveDateTime> {
    let secs = (days * SECS_PER_DAY).round() as i64;
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.naive_utc())
}

/// Format days-since-epoch with WebPlotDigitizer-style tokens:
/// `yyyy`, `yy`, `mm` (month number), `mmm` (Jan), `dd`, `HH`, `ii` (minutes), `ss`.
pub fn format_date(days: f64, fmt: &str) -> String {
    let Some(dt) = days_to_datetime(days) else {
        return format!("{days}");
    };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut out = String::new();
    let mut rest = fmt;
    while !rest.is_empty() {
        let (tok, n) = if rest.starts_with("yyyy") {
            (format!("{:04}", dt.year()), 4)
        } else if rest.starts_with("yy") {
            (format!("{:02}", dt.year() % 100), 2)
        } else if rest.starts_with("mmm") {
            (MONTHS[dt.month0() as usize].to_string(), 3)
        } else if rest.starts_with("mm") {
            (format!("{:02}", dt.month()), 2)
        } else if rest.starts_with("dd") {
            (format!("{:02}", dt.day()), 2)
        } else if rest.starts_with("HH") {
            (format!("{:02}", dt.hour()), 2)
        } else if rest.starts_with("ii") {
            (format!("{:02}", dt.minute()), 2)
        } else if rest.starts_with("ss") {
            (format!("{:02}", dt.second()), 2)
        } else {
            let c = rest.chars().next().unwrap();
            (c.to_string(), c.len_utf8())
        };
        out += &tok;
        rest = &rest[n..];
    }
    out
}

/// Number display modes from the Acquired Data dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum NumFormat {
    /// Full precision.
    #[default]
    Ignore,
    /// Fixed number of decimals.
    Fixed,
    /// Significant digits.
    Precision,
    /// Scientific notation with `digits` decimals.
    Exponential,
}

impl NumFormat {
    pub const ALL: [NumFormat; 4] = [NumFormat::Ignore, NumFormat::Fixed, NumFormat::Precision, NumFormat::Exponential];
    pub fn label(self) -> &'static str {
        match self {
            NumFormat::Ignore => "Ignore",
            NumFormat::Fixed => "Fixed",
            NumFormat::Precision => "Precision",
            NumFormat::Exponential => "Exponential",
        }
    }
}

/// Format like JavaScript's `toFixed` / `toPrecision` / `toExponential`.
pub fn format_number(v: f64, fmt: NumFormat, digits: usize) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    match fmt {
        NumFormat::Ignore => {
            let s = format!("{v}");
            if s == "-0" {
                "0".into()
            } else {
                s
            }
        }
        NumFormat::Fixed => {
            let s = format!("{:.*}", digits, v);
            fix_negative_zero(s)
        }
        NumFormat::Precision => {
            let p = digits.max(1);
            if v == 0.0 {
                return format!("{:.*}", p - 1, 0.0);
            }
            let e = v.abs().log10().floor() as i32;
            if e < -6 || e >= p as i32 {
                exp_string(v, p - 1)
            } else {
                let dec = (p as i32 - 1 - e).max(0) as usize;
                fix_negative_zero(format!("{:.*}", dec, v))
            }
        }
        NumFormat::Exponential => exp_string(v, digits),
    }
}

fn fix_negative_zero(s: String) -> String {
    if s.starts_with('-') && s[1..].chars().all(|c| c == '0' || c == '.') {
        s[1..].to_string()
    } else {
        s
    }
}

fn exp_string(v: f64, decimals: usize) -> String {
    let s = format!("{:.*e}", decimals, v);
    match s.split_once('e') {
        Some((m, e)) => {
            let e: i32 = e.parse().unwrap_or(0);
            format!("{m}e{}{}", if e < 0 { "-" } else { "+" }, e.abs())
        }
        None => s,
    }
}

/// Short display for live readouts: like the WPD hover readout, `6.7919e+0`.
pub fn format_readout(v: f64) -> String {
    format_number(v, NumFormat::Exponential, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(parse_number("1.5"), Some(1.5));
        assert_eq!(parse_number("−0.5"), Some(-0.5));
        assert_eq!(parse_number("1e-3"), Some(1e-3));
        assert_eq!(parse_number("10^3"), Some(1000.0));
        assert_eq!(parse_number("10^-2"), Some(0.01));
        assert_eq!(parse_number("10³"), Some(1000.0));
        assert_eq!(parse_number("10⁻²"), Some(0.01));
        assert_eq!(parse_number("2×10³"), Some(2000.0));
        assert_eq!(parse_number("1,000"), Some(1000.0));
        assert_eq!(parse_number("12,345.5"), Some(12345.5));
        assert_eq!(parse_number("50%"), Some(50.0));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("1,5"), None);
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("1970/01/02"), Some(1.0));
        assert_eq!(parse_date("2013/10"), parse_date("2013/10/01"));
        let d = parse_date("2013/10/23").unwrap();
        assert_eq!(format_date(d, "yyyy/mm/dd"), "2013/10/23");
        assert_eq!(format_date(d, "dd mmm yy"), "23 Oct 13");
        assert!(parse_value("2013/10/23").unwrap().is_date());
        assert!(!parse_value("2013").unwrap().is_date());
    }

    #[test]
    fn formatting() {
        assert_eq!(format_number(0.98840643, NumFormat::Fixed, 3), "0.988");
        assert_eq!(format_number(-0.0001, NumFormat::Fixed, 3), "0.000");
        assert_eq!(format_number(123.456, NumFormat::Precision, 4), "123.5");
        assert_eq!(format_number(0.000123456, NumFormat::Precision, 3), "0.000123");
        assert_eq!(format_number(6.7919, NumFormat::Exponential, 4), "6.7919e+0");
        assert_eq!(format_number(-0.16176, NumFormat::Exponential, 4), "-1.6176e-1");
        assert_eq!(format_number(0.1, NumFormat::Ignore, 3), "0.1");
    }
}
