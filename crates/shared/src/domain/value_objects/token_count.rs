use std::iter::Sum;
use std::ops::{Add, AddAssign};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TokenCount(u64);

impl TokenCount {
    /// ```
    /// use shared::domain::value_objects::TokenCount;
    /// assert_eq!(TokenCount::new(42).value(), 42);
    /// ```
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn zero() -> Self {
        Self(0)
    }

    pub fn value(&self) -> u64 {
        self.0
    }

    /// Clamps negative SQLite values to zero so stray `-1` rows don't underflow.
    ///
    /// ```
    /// use shared::domain::value_objects::TokenCount;
    /// assert_eq!(TokenCount::from_i64(-5).value(), 0);
    /// assert_eq!(TokenCount::from_i64(7).value(), 7);
    /// ```
    pub fn from_i64(value: i64) -> Self {
        Self(value.max(0) as u64)
    }
}

impl Add for TokenCount {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl AddAssign for TokenCount {
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.saturating_add(rhs.0);
    }
}

impl Sum for TokenCount {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::zero(), |a, b| a + b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_i64_clamps_negative_to_zero() {
        assert_eq!(TokenCount::from_i64(-5).value(), 0);
    }

    #[test]
    fn from_i64_passes_through_positive() {
        assert_eq!(TokenCount::from_i64(42).value(), 42);
    }

    #[test]
    fn addition_saturates_on_overflow() {
        let big = TokenCount::new(u64::MAX);
        let one = TokenCount::new(1);
        assert_eq!((big + one).value(), u64::MAX);
    }

    #[test]
    fn sum_totals_values() {
        let tc: TokenCount = [TokenCount::new(3), TokenCount::new(4)].into_iter().sum();
        assert_eq!(tc.value(), 7);
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn addition_never_overflows(a: u64, b: u64) {
            let result = (TokenCount::new(a) + TokenCount::new(b)).value();
            let expected = a.saturating_add(b);
            prop_assert_eq!(result, expected);
        }

        #[test]
        fn from_i64_is_never_negative(v: i64) {
            let tc = TokenCount::from_i64(v);
            if v < 0 {
                prop_assert_eq!(tc.value(), 0);
            } else {
                prop_assert_eq!(tc.value(), v as u64);
            }
        }

        #[test]
        fn addition_is_commutative(a: u64, b: u64) {
            let lhs = (TokenCount::new(a) + TokenCount::new(b)).value();
            let rhs = (TokenCount::new(b) + TokenCount::new(a)).value();
            prop_assert_eq!(lhs, rhs);
        }
    }
}
