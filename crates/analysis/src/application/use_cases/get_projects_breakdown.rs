use std::sync::Arc;

use crate::application::dto::{Filter, GetProjectsBreakdownInput, GetProjectsBreakdownOutput};
use shared::application::errors::ApplicationError;
use crate::application::ports::UsageRepository;
use shared::application::ports::Clock;
use shared::domain::value_objects::DateRange;

pub struct GetProjectsBreakdown {
    repo: Arc<dyn UsageRepository>,
    clock: Arc<dyn Clock>,
}

impl GetProjectsBreakdown {
    pub fn new(repo: Arc<dyn UsageRepository>, clock: Arc<dyn Clock>) -> Self {
        Self { repo, clock }
    }

    pub fn execute(
        &self,
        input: GetProjectsBreakdownInput,
    ) -> Result<GetProjectsBreakdownOutput, ApplicationError> {
        let filter = input.filter.unwrap_or_else(|| Filter {
            date_range: Some(DateRange::last_n_days(self.clock.today(), 30)),
            ..Filter::default()
        });
        let projects = self.repo.by_project(&filter)?;
        Ok(GetProjectsBreakdownOutput {
            filter_applied: filter,
            projects,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::test_support::FakeUsageRepository;
    use shared::application::test_support::FixedClock;
    use shared::domain::entities::ProjectUsage;
    use shared::domain::value_objects::{Cost, ProjectPath, TokenBreakdown};

    #[test]
    fn returns_repo_projects_with_resolved_filter() {
        let repo = Arc::new(FakeUsageRepository {
            by_project: vec![ProjectUsage {
                project: ProjectPath::new("/tmp").unwrap(),
                message_count: 1,
                tokens: TokenBreakdown::default(),
                cost: Cost::new(1.0).unwrap(),
            }],
            ..Default::default()
        });
        let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(2026, 4, 23));
        let repo_dyn: Arc<dyn UsageRepository> = repo.clone();
        let uc = GetProjectsBreakdown::new(repo_dyn, clock);
        let out = uc.execute(GetProjectsBreakdownInput::default()).unwrap();
        assert_eq!(out.projects.len(), 1);
        assert_eq!(out.projects[0].project.as_str(), "/tmp");
    }
}
