use chrono::NaiveDate;

pub fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::new();
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

pub fn fmt_num_trunc(n: u64, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let formatted = fmt_num(n);
    if formatted.len() <= max_width {
        return formatted;
    }
    for (i, &b) in formatted.as_bytes().iter().enumerate() {
        if b == b',' {
            let remaining = &formatted[i + 1..];
            if remaining.len() < max_width {
                return format!("…{}", remaining);
            }
        }
    }
    format!("…{}", &formatted[formatted.len() - max_width + 1..])
}

pub fn fmt_cost(c: f64) -> String {
    format!("${:.2}", c)
}

pub fn fmt_num_compact(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else if n < 1_000_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else {
        format!("{:.2}B", n as f64 / 1_000_000_000.0)
    }
}

pub fn format_date_year(date: NaiveDate) -> String {
    date.format("%Y").to_string()
}

pub fn format_date_md(date: NaiveDate) -> String {
    date.format("%m-%d").to_string()
}

pub fn trunc_model(model: &str, max_width: usize) -> String {
    if model.chars().count() <= max_width {
        model.to_string()
    } else {
        let truncated: String = model.chars().take(max_width.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_num_adds_thousands_separators() {
        assert_eq!(fmt_num(1_234_567), "1,234,567");
        assert_eq!(fmt_num(42), "42");
        assert_eq!(fmt_num(0), "0");
    }

    #[test]
    fn fmt_num_trunc_truncates_from_the_left() {
        // 9-char input "1,234,567" with width 9 → passes through unchanged.
        assert_eq!(fmt_num_trunc(1_234_567, 9), "1,234,567");
        // Width 6 → "…234,567" (8 chars) doesn't fit, then "…567" (4 chars) does.
        assert_eq!(fmt_num_trunc(1_234_567, 6), "…567");
        assert_eq!(fmt_num_trunc(12, 0), "");
    }

    #[test]
    fn fmt_num_compact_switches_units_by_magnitude() {
        assert_eq!(fmt_num_compact(0), "0");
        assert_eq!(fmt_num_compact(999), "999");
        assert_eq!(fmt_num_compact(1_000), "1.0K");
        assert_eq!(fmt_num_compact(12_500), "12.5K");
        assert_eq!(fmt_num_compact(1_234_567), "1.23M");
        assert_eq!(fmt_num_compact(2_500_000_000), "2.50B");
    }

    #[test]
    fn fmt_cost_formats_two_decimals() {
        assert_eq!(fmt_cost(1.234), "$1.23");
        assert_eq!(fmt_cost(0.0), "$0.00");
    }

    #[test]
    fn format_date_year_extracts_year() {
        let d = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        assert_eq!(format_date_year(d), "2026");
    }

    #[test]
    fn format_date_md_extracts_month_day() {
        let d = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        assert_eq!(format_date_md(d), "04-23");
    }

    #[test]
    fn trunc_model_truncates_with_ellipsis() {
        assert_eq!(trunc_model("anthropic/claude-opus-4-7", 10), "anthropic…");
        assert_eq!(trunc_model("short", 10), "short");
    }
}
