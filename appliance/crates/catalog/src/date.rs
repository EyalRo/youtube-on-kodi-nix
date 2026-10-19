use chrono::NaiveDate;

/// `1991`, `1991-6` or `1991-06-23` to a date; missing month/day become 1.
/// Anything else (including impossible dates) is `None`.
pub fn parse_release(value: &str) -> Option<NaiveDate> {
    let mut parts = value.trim().split('-');
    let year: i32 = parts.next()?.trim().parse().ok()?;
    let month: u32 = match parts.next() {
        Some(m) => m.trim().parse().ok()?,
        None => 1,
    };
    let day: u32 = match parts.next() {
        Some(d) => d.trim().parse().ok()?,
        None => 1,
    };
    if parts.next().is_some() || !(1000..=9999).contains(&year) {
        return None;
    }
    NaiveDate::from_ymd_opt(year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, day)
    }

    #[test]
    fn precision_defaults_to_first() {
        assert_eq!(parse_release("1991"), d(1991, 1, 1));
        assert_eq!(parse_release("1991-6"), d(1991, 6, 1));
        assert_eq!(parse_release("1991-06-23"), d(1991, 6, 23));
        assert_eq!(parse_release(" 1991-06-23 "), d(1991, 6, 23));
    }

    #[test]
    fn rejects_nonsense() {
        assert_eq!(parse_release("soon"), None);
        assert_eq!(parse_release("1991-13"), None);
        assert_eq!(parse_release("1991-02-30"), None);
        assert_eq!(parse_release("1991-1-1-1"), None);
        assert_eq!(parse_release(""), None);
        assert_eq!(parse_release("91"), None);
    }
}
