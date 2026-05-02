use crate::domain::value_objects::Cost;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BudgetStatus {
    Under { remaining: Cost },
    Exceeded { overage: Cost },
}

pub fn check(total: Cost, limit: Cost) -> BudgetStatus {
    if total <= limit {
        BudgetStatus::Under {
            remaining: Cost::new(limit.value() - total.value()).unwrap_or(Cost::zero()),
        }
    } else {
        BudgetStatus::Exceeded {
            overage: Cost::new(total.value() - limit.value()).unwrap_or(Cost::zero()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_budget_reports_remaining() {
        let status = check(Cost::new(3.0).unwrap(), Cost::new(10.0).unwrap());
        assert_eq!(
            status,
            BudgetStatus::Under {
                remaining: Cost::new(7.0).unwrap()
            }
        );
    }

    #[test]
    fn at_limit_reports_zero_remaining() {
        let status = check(Cost::new(10.0).unwrap(), Cost::new(10.0).unwrap());
        assert_eq!(
            status,
            BudgetStatus::Under {
                remaining: Cost::zero()
            }
        );
    }

    #[test]
    fn over_budget_reports_overage() {
        let status = check(Cost::new(12.0).unwrap(), Cost::new(10.0).unwrap());
        assert_eq!(
            status,
            BudgetStatus::Exceeded {
                overage: Cost::new(2.0).unwrap()
            }
        );
    }
}
