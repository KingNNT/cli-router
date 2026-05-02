use crate::domain::DomainError;

#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd)]
pub struct PricePerToken(f64);

impl PricePerToken {
    /// Rejects negative, NaN, and infinite values.
    ///
    /// ```
    /// use shared::domain::value_objects::PricePerToken;
    /// assert_eq!(PricePerToken::new(0.000_015).unwrap().value(), 0.000_015);
    /// assert!(PricePerToken::new(-1.0).is_err());
    /// assert!(PricePerToken::new(f64::INFINITY).is_err());
    /// ```
    pub fn new(value: f64) -> Result<Self, DomainError> {
        if value.is_finite() && value >= 0.0 {
            Ok(Self(value))
        } else {
            Err(DomainError::InvalidPrice(value))
        }
    }

    pub fn zero() -> Self {
        Self(0.0)
    }

    pub fn value(&self) -> f64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_zero() {
        assert_eq!(PricePerToken::new(0.0).unwrap().value(), 0.0);
    }

    #[test]
    fn new_accepts_positive() {
        assert_eq!(PricePerToken::new(0.000015).unwrap().value(), 0.000015);
    }

    #[test]
    fn new_rejects_negative() {
        assert!(matches!(
            PricePerToken::new(-0.1),
            Err(DomainError::InvalidPrice(_))
        ));
    }

    #[test]
    fn new_rejects_nan() {
        assert!(matches!(
            PricePerToken::new(f64::NAN),
            Err(DomainError::InvalidPrice(_))
        ));
    }

    #[test]
    fn new_rejects_infinity() {
        assert!(matches!(
            PricePerToken::new(f64::INFINITY),
            Err(DomainError::InvalidPrice(_))
        ));
    }
}
