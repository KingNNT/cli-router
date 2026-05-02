use chrono::NaiveDate;

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum DomainError {
    #[error("invalid date range: from {from} must be on or before to {to}")]
    InvalidDateRange { from: NaiveDate, to: NaiveDate },

    #[error("invalid cost: {0} must be finite and non-negative")]
    InvalidCost(f64),

    #[error("invalid token count: value overflowed u64")]
    InvalidTokenCount,

    #[error("invalid model id: must not be empty")]
    InvalidModelId,

    #[error("invalid project path: must not be empty")]
    InvalidProjectPath,

    #[error("invalid price: {0} must be finite and non-negative")]
    InvalidPrice(f64),
}
