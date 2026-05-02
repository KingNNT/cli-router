use chrono::NaiveDate;

use crate::domain::DomainError;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DateRange {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

impl DateRange {
    /// Errors if `from > to`. Either bound may be `None` (open-ended).
    ///
    /// ```
    /// use shared::domain::value_objects::DateRange;
    /// use chrono::NaiveDate;
    /// let a = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    /// let b = NaiveDate::from_ymd_opt(2026, 4, 30).unwrap();
    /// assert!(DateRange::new(Some(a), Some(b)).is_ok());
    /// assert!(DateRange::new(Some(b), Some(a)).is_err());
    /// ```
    pub fn new(from: Option<NaiveDate>, to: Option<NaiveDate>) -> Result<Self, DomainError> {
        if let (Some(f), Some(t)) = (from, to) {
            if f > t {
                return Err(DomainError::InvalidDateRange { from: f, to: t });
            }
        }
        Ok(Self { from, to })
    }

    pub fn unbounded() -> Self {
        Self {
            from: None,
            to: None,
        }
    }

    /// A range covering the last `n` days ending on `today` (inclusive).
    ///
    /// ```
    /// use shared::domain::value_objects::DateRange;
    /// use chrono::NaiveDate;
    /// let today = NaiveDate::from_ymd_opt(2026, 4, 25).unwrap();
    /// let r = DateRange::last_n_days(today, 7);
    /// assert_eq!(r.to, Some(today));
    /// assert_eq!(r.from, Some(NaiveDate::from_ymd_opt(2026, 4, 18).unwrap()));
    /// ```
    pub fn last_n_days(today: NaiveDate, n: i64) -> Self {
        Self {
            from: Some(today - chrono::Duration::days(n)),
            to: Some(today),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbounded_is_none_none() {
        let r = DateRange::unbounded();
        assert_eq!(r.from, None);
        assert_eq!(r.to, None);
    }

    #[test]
    fn new_accepts_matching_from_and_to() {
        let d = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let r = DateRange::new(Some(d), Some(d)).unwrap();
        assert_eq!(r.from, Some(d));
        assert_eq!(r.to, Some(d));
    }

    #[test]
    fn new_accepts_ascending() {
        let a = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        let b = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        assert!(DateRange::new(Some(a), Some(b)).is_ok());
    }

    #[test]
    fn new_rejects_reversed() {
        let a = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        let b = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let err = DateRange::new(Some(b), Some(a)).unwrap_err();
        assert_eq!(err, DomainError::InvalidDateRange { from: b, to: a });
    }

    #[test]
    fn new_accepts_one_sided() {
        let d = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        assert!(DateRange::new(Some(d), None).is_ok());
        assert!(DateRange::new(None, Some(d)).is_ok());
    }

    #[test]
    fn last_n_days_is_today_minus_n() {
        let today = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let r = DateRange::last_n_days(today, 30);
        assert_eq!(r.to, Some(today));
        assert_eq!(r.from, Some(NaiveDate::from_ymd_opt(2026, 3, 24).unwrap()));
    }

    use proptest::prelude::*;

    fn any_date() -> impl Strategy<Value = NaiveDate> {
        // Stay well inside NaiveDate's valid range.
        (2000i32..2100, 1u32..13, 1u32..28)
            .prop_map(|(y, m, d)| NaiveDate::from_ymd_opt(y, m, d).unwrap())
    }

    proptest! {
        #[test]
        fn new_accepts_iff_from_le_to(from in any_date(), to in any_date()) {
            let result = DateRange::new(Some(from), Some(to));
            if from <= to {
                prop_assert!(result.is_ok());
            } else {
                prop_assert!(result.is_err());
            }
        }

        #[test]
        fn one_sided_always_ok(d in any_date()) {
            prop_assert!(DateRange::new(Some(d), None).is_ok());
            prop_assert!(DateRange::new(None, Some(d)).is_ok());
            prop_assert!(DateRange::new(None, None).is_ok());
        }

        #[test]
        fn last_n_days_from_le_to(today in any_date(), n in 0i64..365) {
            let r = DateRange::last_n_days(today, n);
            prop_assert!(r.from.unwrap() <= r.to.unwrap());
        }
    }
}
