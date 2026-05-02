use crate::adapters::presenters::formatting::{fmt_cost, fmt_num};
use crate::adapters::view_models::projects_vm::{ProjectRowVM, ProjectsViewModel};
use crate::application::dto::GetProjectsBreakdownOutput;

pub fn present_projects(out: &GetProjectsBreakdownOutput) -> ProjectsViewModel {
    let rows: Vec<ProjectRowVM> = out
        .projects
        .iter()
        .map(|p| ProjectRowVM {
            project: p.project.as_str().to_string(),
            messages: fmt_num(p.message_count),
            input: fmt_num(p.tokens.input.value()),
            output: fmt_num(p.tokens.output.value()),
            cost: fmt_cost(p.cost.value()),
        })
        .collect();

    ProjectsViewModel {
        empty: rows.is_empty(),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::dto::Filter;
    use shared::domain::entities::ProjectUsage;
    use shared::domain::value_objects::{Cost, ProjectPath, TokenBreakdown, TokenCount};

    #[test]
    fn empty_input_produces_empty_vm() {
        let out = GetProjectsBreakdownOutput {
            filter_applied: Filter::default(),
            projects: vec![],
        };
        let vm = present_projects(&out);
        assert!(vm.empty);
    }

    #[test]
    fn rows_are_formatted() {
        let out = GetProjectsBreakdownOutput {
            filter_applied: Filter::default(),
            projects: vec![ProjectUsage {
                project: ProjectPath::new("/work/alpha").unwrap(),
                message_count: 42,
                tokens: TokenBreakdown {
                    input: TokenCount::new(1500),
                    output: TokenCount::new(800),
                    ..Default::default()
                },
                cost: Cost::new(3.15).unwrap(),
            }],
        };
        let vm = present_projects(&out);
        assert_eq!(vm.rows.len(), 1);
        assert_eq!(vm.rows[0].project, "/work/alpha");
        assert_eq!(vm.rows[0].cost, "$3.15");
    }
}
