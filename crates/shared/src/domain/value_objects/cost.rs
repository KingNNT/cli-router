use crate::domain::DomainError;
use std::iter::Sum;
use std::ops::{Add, AddAssign};

#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd)]
pub struct Cost(f64);

impl Cost {
    /// Rejects negative, NaN, and infinite values.
    ///
    /// ```
    /// use shared::domain::value_objects::Cost;
    /// assert_eq!(Cost::new(1.23).unwrap().value(), 1.23);
    /// assert!(Cost::new(-0.1).is_err());
    /// assert!(Cost::new(f64::NAN).is_err());
    /// ```
    pub fn new(value: f64) -> Result<Self, DomainError> {
        if value.is_finite() && value >= 0.0 {
            Ok(Self(value))
        } else {
            Err(DomainError::InvalidCost(value))
        }
    }

    pub fn zero() -> Self {
        Self(0.0)
    }

    pub fn value(&self) -> f64 {
        self.0
    }
}

impl Add for Cost {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Cost {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sum for Cost {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::zero(), |a, b| a + b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_zero() {
        assert_eq!(Cost::new(0.0).unwrap().value(), 0.0);
    }

    #[test]
    fn new_accepts_positive() {
        assert_eq!(Cost::new(1.23).unwrap().value(), 1.23);
    }

    #[test]
    fn new_rejects_negative() {
        assert_eq!(Cost::new(-1.0), Err(DomainError::InvalidCost(-1.0)));
    }

    #[test]
    fn new_rejects_nan() {
        assert!(matches!(
            Cost::new(f64::NAN),
            Err(DomainError::InvalidCost(_))
        ));
    }

    #[test]
    fn new_rejects_infinity() {
        assert_eq!(
            Cost::new(f64::INFINITY),
            Err(DomainError::InvalidCost(f64::INFINITY))
        );
    }

    #[test]
    fn addition_sums_values() {
        let sum = Cost::new(1.5).unwrap() + Cost::new(2.5).unwrap();
        assert_eq!(sum.value(), 4.0);
    }

    #[test]
    fn sum_iterator_empty_is_zero() {
        let total: Cost = [].into_iter().sum();
        assert_eq!(total, Cost::zero());
    }

    #[test]
    fn sum_iterator_totals() {
        let costs = vec![
            Cost::new(1.0).unwrap(),
            Cost::new(2.0).unwrap(),
            Cost::new(3.0).unwrap(),
        ];
        let total: Cost = costs.into_iter().sum();
        assert_eq!(total.value(), 6.0);
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn any_finite_nonneg_is_accepted(v in 0.0f64..1e12) {
            prop_assert!(Cost::new(v).is_ok());
        }

        #[test]
        fn any_negative_finite_is_rejected(v in -1e12f64..-1e-12) {
            prop_assert!(Cost::new(v).is_err());
        }

        #[test]
        fn nan_and_inf_always_rejected(_dummy in 0u8..1) {
            prop_assert!(Cost::new(f64::NAN).is_err());
            prop_assert!(Cost::new(f64::INFINITY).is_err());
            prop_assert!(Cost::new(f64::NEG_INFINITY).is_err());
        }
    }
}
