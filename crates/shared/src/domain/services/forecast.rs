use crate::domain::entities::DailyUsage;
use crate::domain::value_objects::Cost;

pub fn linear_projection(daily: &[DailyUsage], days_ahead: u32) -> Cost {
    if daily.is_empty() {
        return Cost::zero();
    }
    let total: f64 = daily.iter().map(|d| d.cost.value()).sum();
    let avg = total / daily.len() as f64;
    Cost::new(avg * days_ahead as f64).unwrap_or(Cost::zero())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::value_objects::TokenBreakdown;
    use chrono::NaiveDate;

    fn day(n: u32, cost: f64) -> DailyUsage {
        DailyUsage {
            date: NaiveDate::from_ymd_opt(2026, 4, n).unwrap(),
            message_count: 1,
            tokens: TokenBreakdown::default(),
            cost: Cost::new(cost).unwrap(),
        }
    }

    #[test]
    fn empty_history_projects_zero() {
        assert_eq!(linear_projection(&[], 30).value(), 0.0);
    }

    #[test]
    fn projects_average_over_days_ahead() {
        let hist = vec![day(21, 2.0), day(22, 4.0), day(23, 6.0)];
        // avg = 4.0, project 10 days = 40.0
        assert_eq!(linear_projection(&hist, 10).value(), 40.0);
    }
}
