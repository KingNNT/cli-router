# Remove Projects View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the Projects sidebar view end-to-end from the `analysis` crate, leaving a three-view sidebar (Dashboard, Models, Pricing).

**Architecture:** Pure deletion, bottom-up through the clean-architecture rings (domain → application → adapters → tui framework → composition root → integration tests). No new behavior. Each task ends with a green `cargo check -p analysis` and a commit.

**Tech Stack:** Rust workspace, axum/ratatui app, clean architecture (domain/application/adapters/frameworks rings).

**Critical scope clarification (verified during planning):**
- **DELETE:** `ProjectUsage` entity — it is the aggregation row backing *only* the Projects view pipeline (DTO → use case → port method → 3 gateway impls → presenter → view model → renderer → controller field → main wiring → integration test).
- **KEEP:** `ProjectPath` value object, `Filter.project` field, and the query-builder's `filter.project` handling (`query_builder.rs:65-68`). These are *filtering dimensions* used by `UsageRecord`, by the `SqliteUsageRepository` WHERE-clause builder, and by `ClaudeCodeUsageRepository` JSONL parsing. They serve every other view and must remain.
- **KEEP:** `default_projects_root()` — locates `~/.claude/projects` for session JSONL discovery; unrelated to the view.

**Reference spec:** `docs/superpowers/specs/2026-07-03-remove-projects-view-design.md`

**Recurring verification commands** (run after each task unless noted):
```bash
cargo check -p analysis          # type-check
cargo clippy -p analysis -- -D warnings   # lint
```

---

## File Map

Files **deleted** (7):
- `crates/shared/src/domain/entities/project_usage.rs`
- `crates/analysis/src/application/dto/projects_breakdown.rs`
- `crates/analysis/src/application/use_cases/get_projects_breakdown.rs`
- `crates/analysis/src/adapters/view_models/projects_vm.rs`
- `crates/analysis/src/adapters/presenters/projects_presenter.rs`
- `crates/analysis/src/tui/renderer/projects.rs`

Files **modified** (15):
- `crates/shared/src/domain/entities/mod.rs`
- `crates/analysis/src/application/dto/mod.rs`
- `crates/analysis/src/application/use_cases/mod.rs`
- `crates/analysis/src/application/ports/usage_repository.rs`
- `crates/analysis/src/application/test_support.rs`
- `crates/analysis/src/adapters/view_models/mod.rs`
- `crates/analysis/src/adapters/presenters/mod.rs`
- `crates/analysis/src/adapters/gateways/sqlite/repository.rs`
- `crates/analysis/src/adapters/gateways/claudecode/repository.rs`
- `crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs`
- `crates/analysis/src/tui/renderer/mod.rs`
- `crates/analysis/src/tui/app_state.rs`
- `crates/analysis/src/tui/controllers/tui_controller.rs`
- `crates/analysis/src/main.rs`
- `crates/analysis/tests/integration.rs`

---

## Task 1: Delete shared `ProjectUsage` entity + re-export

**Files:**
- Delete: `crates/shared/src/domain/entities/project_usage.rs`
- Modify: `crates/shared/src/domain/entities/mod.rs`

- [ ] **Step 1: Delete the entity file**

```bash
rm crates/shared/src/domain/entities/project_usage.rs
```

- [ ] **Step 2: Remove its module declaration and re-export from `mod.rs`**

In `crates/shared/src/domain/entities/mod.rs`, remove these two lines:

```rust
pub mod project_usage;
```
and
```rust
pub use project_usage::ProjectUsage;
```

The file should go from 7 `pub mod` lines + 7 `pub use` lines to 6 of each. Final content:

```rust
pub mod daily_usage;
pub mod day_model_row;
pub mod model_pricing;
pub mod model_usage;
pub mod overview;
pub mod usage_record;

pub use daily_usage::DailyUsage;
pub use day_model_row::DayModelRow;
pub use model_pricing::ModelPricing;
pub use model_usage::ModelUsage;
pub use overview::Overview;
pub use usage_record::UsageRecord;
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo check -p shared
```

Expected: PASS (no consumers of `ProjectUsage` inside `shared` itself — it was only re-exported).

- [ ] **Step 4: Commit**

```bash
git add crates/shared/src/domain/entities/project_usage.rs crates/shared/src/domain/entities/mod.rs
git commit -m "refactor(shared): remove ProjectUsage entity

No longer needed — the analysis Projects view that aggregated usage
by project is being removed. ProjectPath stays; it is a filter
dimension used by UsageRecord and the query builder, independent of
the Projects view."
```

---

## Task 2: Delete `projects_breakdown` DTO + re-export

**Files:**
- Delete: `crates/analysis/src/application/dto/projects_breakdown.rs`
- Modify: `crates/analysis/src/application/dto/mod.rs`

- [ ] **Step 1: Delete the DTO file**

```bash
rm crates/analysis/src/application/dto/projects_breakdown.rs
```

- [ ] **Step 2: Remove its module declaration and re-export**

In `crates/analysis/src/application/dto/mod.rs`, remove these two lines:

```rust
pub mod projects_breakdown;
```
and
```rust
pub use projects_breakdown::{GetProjectsBreakdownInput, GetProjectsBreakdownOutput};
```

- [ ] **Step 3: Verify the crate still compiles**

It will NOT yet — the use case (Task 3), controller, and integration test still reference these types. Deletion proceeds bottom-up; the crate goes green again after Task 9 (controller) and Task 11 (integration test). Run a check anyway to confirm the *only* errors are the expected downstream references:

```bash
cargo check -p analysis 2>&1 | grep "projects_breakdown"
```

Expected: errors in `get_projects_breakdown.rs`, `tui_controller.rs`, `integration.rs` only.

- [ ] **Step 4: Do not commit yet** — continue to Task 3. These two deletions are part of one logical "remove the DTO + use case" change.

---

## Task 3: Delete `GetProjectsBreakdown` use case + re-export

**Files:**
- Delete: `crates/analysis/src/application/use_cases/get_projects_breakdown.rs`
- Modify: `crates/analysis/src/application/use_cases/mod.rs`

- [ ] **Step 1: Delete the use case file**

```bash
rm crates/analysis/src/application/use_cases/get_projects_breakdown.rs
```

- [ ] **Step 2: Remove its module declaration and re-export**

In `crates/analysis/src/application/use_cases/mod.rs`, remove:

```rust
pub mod get_projects_breakdown;
```
and
```rust
pub use get_projects_breakdown::GetProjectsBreakdown;
```

Final content:

```rust
pub mod get_dashboard;
pub mod get_models_breakdown;
pub mod get_pricing;
pub mod sync_pricing;

pub use get_dashboard::GetDashboard;
pub use get_models_breakdown::GetModelsBreakdown;
pub use get_pricing::GetPricing;
pub use sync_pricing::SyncPricing;
```

- [ ] **Step 3: Commit Tasks 2+3 together (DTO + use case removal)**

```bash
git add crates/analysis/src/application/dto/projects_breakdown.rs crates/analysis/src/application/dto/mod.rs crates/analysis/src/application/use_cases/get_projects_breakdown.rs crates/analysis/src/application/use_cases/mod.rs
git commit -m "refactor(analysis): remove Projects DTO and GetProjectsBreakdown use case

Part of removing the Projects view. Downstream references (controller,
main, integration test) are cleaned up in subsequent commits; the crate
will not compile end-to-end until then."
```

---

## Task 4: Remove `by_project` from the `UsageRepository` port

**Files:**
- Modify: `crates/analysis/src/application/ports/usage_repository.rs`

- [ ] **Step 1: Drop the trait method and the `ProjectUsage` import**

Current content of `crates/analysis/src/application/ports/usage_repository.rs`:

```rust
use crate::application::dto::Filter;
use shared::application::errors::ApplicationError;
use shared::domain::entities::{DayModelRow, ModelUsage, Overview, ProjectUsage};

pub trait UsageRepository: Send + Sync {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError>;
    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError>;
    fn by_model(&self, filter: &Filter) -> Result<Vec<ModelUsage>, ApplicationError>;
    fn by_project(&self, filter: &Filter) -> Result<Vec<ProjectUsage>, ApplicationError>;
}
```

New content:

```rust
use crate::application::dto::Filter;
use shared::application::errors::ApplicationError;
use shared::domain::entities::{DayModelRow, ModelUsage, Overview};

pub trait UsageRepository: Send + Sync {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError>;
    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError>;
    fn by_model(&self, filter: &Filter) -> Result<Vec<ModelUsage>, ApplicationError>;
}
```

- [ ] **Step 2: Do not compile-check yet** — the three gateway impls and `FakeUsageRepository` still have `by_project` methods that now fail to satisfy a trait they no longer need to. Compile after Task 5 (gateways) and Task 6 (test_support). Proceed.

---

## Task 5: Remove `by_project` impls from the three gateway repositories

**Files:**
- Modify: `crates/analysis/src/adapters/gateways/sqlite/repository.rs`
- Modify: `crates/analysis/src/adapters/gateways/claudecode/repository.rs`
- Modify: `crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs`

- [ ] **Step 1: SqliteUsageRepository — remove `by_project` impl + `ProjectUsage` import**

In `crates/analysis/src/adapters/gateways/sqlite/repository.rs`:

a) Remove the `ProjectUsage` token from the entities import (line 10):

Before:
```rust
use shared::domain::entities::{DayModelRow, ModelUsage, Overview, ProjectUsage};
```
After:
```rust
use shared::domain::entities::{DayModelRow, ModelUsage, Overview};
```

b) Delete the entire `by_project` method body (lines 221-276 inclusive):

```rust
    fn by_project(&self, filter: &Filter) -> Result<Vec<ProjectUsage>, ApplicationError> {
        let WhereClause {
            sql: where_sql,
            params,
        } = query_builder::build(filter);
        let p_refs = query_builder::param_refs(&params);

        let sql = format!(
            "SELECT \
                s.directory, \
                COUNT(*), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.input')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.output')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.reasoning')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.read')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.write')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.cost')), 0.0) \
            FROM message m JOIN session s ON m.session_id = s.id \
            {} \
            GROUP BY s.directory \
            ORDER BY COALESCE(SUM(json_extract(m.data, '$.cost')), 0.0) DESC",
            where_sql
        );

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql).map_err(AdapterError::from)?;
        let iter = stmt
            .query_map(p_refs.as_slice(), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, f64>(7)?,
                ))
            })
            .map_err(AdapterError::from)?;

        let mut out = Vec::new();
        for row in iter {
            let (project_str, count, input, output, reasoning, cache_read, cache_write, cost) =
                row.map_err(AdapterError::from)?;
            let project = ProjectPath::new(project_str)
                .map_err(|e| ApplicationError::from(AdapterError::from(e)))?;
            out.push(ProjectUsage {
                project,
                message_count: count.max(0) as u64,
                tokens: parse_tokens(input, output, reasoning, cache_read, cache_write),
                cost: parse_cost(cost).map_err(ApplicationError::from)?,
            });
        }
        Ok(out)
    }
```

Also remove `ProjectPath` from the value_objects import on line 12 **only if `ProjectPath` is not used elsewhere in this file**. It IS still used by `query_builder.rs` (separate file) but check `repository.rs` itself: `grep -n "ProjectPath" crates/analysis/src/adapters/gateways/sqlite/repository.rs`. The only usage was inside `by_project` (line 266), so remove `ProjectPath` from the import on line 12:

Before:
```rust
use shared::domain::value_objects::{
    Cost, DateRange, ModelId, ProjectPath, TokenBreakdown, TokenCount,
};
```
After:
```rust
use shared::domain::value_objects::{Cost, DateRange, ModelId, TokenBreakdown, TokenCount};
```

**Note:** `Cost`, `TokenBreakdown`, `TokenCount` are still used by `by_model` / `daily_by_model` impls — keep them. If `DateRange` becomes unused after removal (it is used in tests or other methods — verify with grep), keep it; only remove verifiably-dead imports.

- [ ] **Step 2: ClaudeCodeUsageRepository — remove `by_project` impl + `ProjectUsage` import**

In `crates/analysis/src/adapters/gateways/claudecode/repository.rs`:

a) Remove `ProjectUsage` from the entities import (line 13):

Before:
```rust
use shared::domain::entities::{DayModelRow, ModelUsage, Overview, ProjectUsage, UsageRecord};
```
After:
```rust
use shared::domain::entities::{DayModelRow, ModelUsage, Overview, UsageRecord};
```

b) Delete the entire `by_project` method (lines 260-293 inclusive):

```rust
    fn by_project(&self, filter: &Filter) -> Result<Vec<ProjectUsage>, ApplicationError> {
        let records = self.records().map_err(ApplicationError::from)?;
        let mut map: HashMap<String, (ProjectPath, u64, TokenBreakdown, Cost)> = HashMap::new();
        for r in records.iter().filter(|r| matches_filter(r, filter)) {
            let key = r.project.as_str().to_string();
            let entry = map.entry(key).or_insert_with(|| {
                (
                    r.project.clone(),
                    0,
                    TokenBreakdown::default(),
                    Cost::zero(),
                )
            });
            entry.1 += 1;
            entry.2 += r.tokens;
            entry.3 += r.cost;
        }
        let mut out: Vec<ProjectUsage> = map
            .into_values()
            .map(|(project, count, tokens, cost)| ProjectUsage {
                project,
                message_count: count,
                tokens,
                cost,
            })
            .collect();
        out.sort_by(|a, b| {
            b.cost
                .partial_cmp(&a.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.project.as_str().cmp(b.project.as_str()))
        });
        Ok(out)
    }
```

**Do NOT remove `ProjectPath` from the value_objects import** (line 15) — it is still used at line 110 (`parse_jsonl_into`: `let Ok(project) = ProjectPath::new(&cwd) else {`). Leave that import as-is.

c) Check if `HashMap` is still used after removal:

```bash
rg "HashMap" crates/analysis/src/adapters/gateways/claudecode/repository.rs
```
If `HashMap` was only used inside `by_project`, remove its `use std::collections::HashMap;` import too. If other methods use it, leave it.

- [ ] **Step 3: DispatchingUsageRepository — remove `by_project` impl + `ProjectUsage` import**

In `crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs`:

a) Remove `ProjectUsage` from the entities import (line 6):

Before:
```rust
use shared::domain::entities::{DayModelRow, ModelUsage, Overview, ProjectUsage};
```
After:
```rust
use shared::domain::entities::{DayModelRow, ModelUsage, Overview};
```

b) Delete the `by_project` method (lines 104-106 inclusive):

```rust
    fn by_project(&self, filter: &Filter) -> Result<Vec<ProjectUsage>, ApplicationError> {
        self.active().by_project(filter)
    }
```

- [ ] **Step 4: Do not commit yet** — `FakeUsageRepository` in `test_support.rs` still has `by_project`. Handle in Task 6, then commit both together.

---

## Task 6: Remove `by_project` from `FakeUsageRepository`

**Files:**
- Modify: `crates/analysis/src/application/test_support.rs`

- [ ] **Step 1: Remove the `by_project` field, method, and `ProjectUsage` import**

Current relevant content of `crates/analysis/src/application/test_support.rs`:

```rust
use shared::application::errors::ApplicationError;
use shared::domain::entities::ModelPricing;
use shared::domain::entities::{DayModelRow, ModelUsage, Overview, ProjectUsage};

use crate::application::dto::Filter;
use crate::application::ports::{PricingSource, UsageRepository};

#[derive(Default)]
pub struct FakeUsageRepository {
    pub overview: Overview,
    pub daily: Vec<DayModelRow>,
    pub by_model: Vec<ModelUsage>,
    pub by_project: Vec<ProjectUsage>,
    pub last_filter: Mutex<Option<Filter>>,
}

impl UsageRepository for FakeUsageRepository {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.overview.clone())
    }

    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.daily.clone())
    }

    fn by_model(&self, filter: &Filter) -> Result<Vec<ModelUsage>, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.by_model.clone())
    }

    fn by_project(&self, filter: &Filter) -> Result<Vec<ProjectUsage>, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.by_project.clone())
    }
}
```

Changes:
1. Remove `ProjectUsage` from the line `use shared::domain::entities::{DayModelRow, ModelUsage, Overview, ProjectUsage};` → `use shared::domain::entities::{DayModelRow, ModelUsage, Overview};`
2. Remove the field `pub by_project: Vec<ProjectUsage>,` from the struct.
3. Remove the entire `by_project` method from the impl block.

New full file content:

```rust
//! Test-only fakes for the analysis-specific application ports
//! (UsageRepository + PricingSource).

use std::sync::Mutex;

use shared::application::errors::ApplicationError;
use shared::domain::entities::ModelPricing;
use shared::domain::entities::{DayModelRow, ModelUsage, Overview};

use crate::application::dto::Filter;
use crate::application::ports::{PricingSource, UsageRepository};

#[derive(Default)]
pub struct FakeUsageRepository {
    pub overview: Overview,
    pub daily: Vec<DayModelRow>,
    pub by_model: Vec<ModelUsage>,
    pub last_filter: Mutex<Option<Filter>>,
}

impl UsageRepository for FakeUsageRepository {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.overview.clone())
    }

    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.daily.clone())
    }

    fn by_model(&self, filter: &Filter) -> Result<Vec<ModelUsage>, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.by_model.clone())
    }
}

#[derive(Default)]
pub struct FakePricingSource {
    pub rows: Vec<ModelPricing>,
    pub err: Option<String>,
}

impl PricingSource for FakePricingSource {
    fn fetch_all(&self) -> Result<Vec<ModelPricing>, ApplicationError> {
        if let Some(e) = &self.err {
            return Err(ApplicationError::InvalidInput(e.clone()));
        }
        Ok(self.rows.clone())
    }
}
```

- [ ] **Step 2: Verify the application + adapters layers now compile**

```bash
cargo check -p analysis 2>&1 | grep -E "projects_breakdown|by_project|ProjectUsage"
```

Expected: remaining errors should be confined to `tui_controller.rs` (the `get_projects_breakdown` field, `ensure_vm_for_current_view` arm, imports) and `integration.rs`. No errors should mention `by_project` or `ProjectUsage` in the gateway or test_support files.

- [ ] **Step 3: Commit Tasks 4+5+6 together (port + gateways + fake)**

```bash
git add crates/analysis/src/application/ports/usage_repository.rs crates/analysis/src/adapters/gateways/sqlite/repository.rs crates/analysis/src/adapters/gateways/claudecode/repository.rs crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs crates/analysis/src/application/test_support.rs
git commit -m "refactor(analysis): drop by_project from UsageRepository and all impls

The trait method, the three gateway implementations (sqlite,
claudecode, dispatching), and the FakeUsageRepository test double no
longer aggregate usage by project. ProjectPath stays as a Filter
dimension used by the remaining query builders."
```

---

## Task 7: Delete Projects view model + presenter + re-exports

**Files:**
- Delete: `crates/analysis/src/adapters/view_models/projects_vm.rs`
- Delete: `crates/analysis/src/adapters/presenters/projects_presenter.rs`
- Modify: `crates/analysis/src/adapters/view_models/mod.rs`
- Modify: `crates/analysis/src/adapters/presenters/mod.rs`

- [ ] **Step 1: Delete both files**

```bash
rm crates/analysis/src/adapters/view_models/projects_vm.rs
rm crates/analysis/src/adapters/presenters/projects_presenter.rs
```

- [ ] **Step 2: Remove view-model module declaration and re-export**

In `crates/analysis/src/adapters/view_models/mod.rs`, remove:

```rust
pub mod projects_vm;
```
and
```rust
pub use projects_vm::{ProjectRowVM, ProjectsViewModel};
```

Final content:

```rust
pub mod dashboard_vm;
pub use dashboard_vm::{DashboardViewModel, DayPivotRowVM, ModelBreakdownVM, ModelColumnVM};
pub mod models_vm;
pub use models_vm::{ModelRowVM, ModelsViewModel};
pub mod pricing_vm;
pub use pricing_vm::{PricingRowVM, PricingViewModel};
```

- [ ] **Step 3: Remove presenter module declaration**

In `crates/analysis/src/adapters/presenters/mod.rs`, remove the line declaring the `projects` submodule (and any `pub use projects_presenter::present_projects;` re-export if present). Consult the file's exact contents:

```bash
cat crates/analysis/src/adapters/presenters/mod.rs
```

Remove the `projects` line. The presenter module typically declares submodules like `pub mod projects;` — remove that one line.

- [ ] **Step 4: Commit**

```bash
git add crates/analysis/src/adapters/view_models/projects_vm.rs crates/analysis/src/adapters/view_models/mod.rs crates/analysis/src/adapters/presenters/projects_presenter.rs crates/analysis/src/adapters/presenters/mod.rs
git commit -m "refactor(analysis): remove Projects view model and presenter

Drops the ProjectsViewModel/ProjectRowVM view model and the
present_projects function that mapped ProjectUsage aggregation rows
into display rows."
```

---

## Task 8: Delete Projects renderer + remove dispatch arm + mod declaration

**Files:**
- Delete: `crates/analysis/src/tui/renderer/projects.rs`
- Modify: `crates/analysis/src/tui/renderer/mod.rs`

- [ ] **Step 1: Delete the renderer file**

```bash
rm crates/analysis/src/tui/renderer/projects.rs
```

- [ ] **Step 2: Remove the `pub mod projects;` declaration**

In `crates/analysis/src/tui/renderer/mod.rs` line 6, remove:

```rust
pub mod projects;
```

- [ ] **Step 3: Remove the `View::Projects` arm from `dispatch_view`**

In `crates/analysis/src/tui/renderer/mod.rs`, the `dispatch_view` function (lines 67-98) currently has four match arms. Remove the Projects arm:

```rust
        View::Projects => {
            if let Some(vm) = &state.projects_vm {
                projects::draw(f, vm, area, state.projects_offset);
            }
        }
```

The new `dispatch_view` body should have three arms (Dashboard, Models, Pricing):

```rust
fn dispatch_view(f: &mut Frame, state: &AppState, area: Rect, regions: &mut HitRegions) {
    match state.view {
        View::Dashboard => {
            if let Some(vm) = &state.dashboard_vm {
                let tabs = dashboard::draw(
                    f,
                    vm,
                    area,
                    state.dashboard_offset,
                    state.dashboard_col_offset,
                    state.data_source.get().label(),
                );
                regions.window_tabs = tabs;
            }
        }
        View::Models => {
            if let Some(vm) = &state.models_vm {
                models::draw(f, vm, area, state.models_offset);
            }
        }
        View::Pricing => {
            if let Some(vm) = &state.pricing_vm {
                pricing::draw(f, vm, area, state.pricing_offset, state.is_searching);
            }
        }
    }
}
```

- [ ] **Step 4: Do not commit yet** — `AppState` still has `View::Projects` and `projects_vm`/`projects_offset`. Compile will fail on the non-exhaustive match. Continue to Task 9.

---

## Task 9: Remove `View::Projects` variant + state fields from `AppState`

**Files:**
- Modify: `crates/analysis/src/tui/app_state.rs`

- [ ] **Step 1: Remove the `Projects` variant from `View` enum**

Before (lines 84-90):
```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Dashboard,
    Models,
    Projects,
    Pricing,
}
```
After:
```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Dashboard,
    Models,
    Pricing,
}
```

- [ ] **Step 2: Shrink `View::ALL` from 4 to 3 and drop the `Projects` label arm**

Before (lines 92-102):
```rust
impl View {
    pub const ALL: [View; 4] = [View::Dashboard, View::Models, View::Projects, View::Pricing];

    pub fn label(self) -> &'static str {
        match self {
            View::Dashboard => "Dashboard",
            View::Models => "Models",
            View::Projects => "Projects",
            View::Pricing => "Pricing",
        }
    }
}
```
After:
```rust
impl View {
    pub const ALL: [View; 3] = [View::Dashboard, View::Models, View::Pricing];

    pub fn label(self) -> &'static str {
        match self {
            View::Dashboard => "Dashboard",
            View::Models => "Models",
            View::Pricing => "Pricing",
        }
    }
}
```

- [ ] **Step 3: Remove `projects_vm` and `projects_offset` fields from `AppState`**

Before (lines 105-127):
```rust
pub struct AppState {
    pub view: View,
    pub sidebar_selected: usize,
    pub scroll_offset: u16,
    pub filter_window: FilterWindow,
    pub data_source: DataSourceCell,
    pub dashboard_vm: Option<DashboardViewModel>,
    pub models_vm: Option<ModelsViewModel>,
    pub projects_vm: Option<ProjectsViewModel>,
    pub pricing_vm: Option<PricingViewModel>,
    pub dashboard_offset: usize,
    pub dashboard_col_offset: usize,
    pub models_offset: usize,
    pub projects_offset: usize,
    pub pricing_offset: usize,
    pub focus: Focus,
    pub status_message: Option<String>,
    pub should_quit: bool,
    pub pricing_query: String,
    pub is_searching: bool,
    pub help_open: bool,
    pub hit_regions: HitRegions,
}
```
After (remove the two `projects_*` lines):
```rust
pub struct AppState {
    pub view: View,
    pub sidebar_selected: usize,
    pub scroll_offset: u16,
    pub filter_window: FilterWindow,
    pub data_source: DataSourceCell,
    pub dashboard_vm: Option<DashboardViewModel>,
    pub models_vm: Option<ModelsViewModel>,
    pub pricing_vm: Option<PricingViewModel>,
    pub dashboard_offset: usize,
    pub dashboard_col_offset: usize,
    pub models_offset: usize,
    pub pricing_offset: usize,
    pub focus: Focus,
    pub status_message: Option<String>,
    pub should_quit: bool,
    pub pricing_query: String,
    pub is_searching: bool,
    pub help_open: bool,
    pub hit_regions: HitRegions,
}
```

- [ ] **Step 4: Drop the two `projects_*` initializers in `with_data_source`**

In the `with_data_source` initializer (lines 134-158), remove:
```rust
            projects_vm: None,
```
and
```rust
            projects_offset: 0,
```

- [ ] **Step 5: Drop the two `projects_offset = 0` resets in `sidebar_up`**

In `sidebar_up` (lines 160-173), remove:
```rust
            self.projects_offset = 0;
```

- [ ] **Step 6: Drop the two `projects_offset = 0` resets in `sidebar_down`**

In `sidebar_down` (lines 175-188), remove:
```rust
            self.projects_offset = 0;
```

- [ ] **Step 7: Drop `projects_vm = None` and `projects_offset = 0` from `invalidate_all_vms`**

In `invalidate_all_vms` (lines 190-200), remove:
```rust
        self.projects_vm = None;
```
and
```rust
        self.projects_offset = 0;
```

- [ ] **Step 8: Drop the `View::Projects` arm in `current_offset`**

Before (lines 202-209):
```rust
    pub fn current_offset(&self) -> usize {
        match self.view {
            View::Dashboard => self.dashboard_offset,
            View::Models => self.models_offset,
            View::Projects => self.projects_offset,
            View::Pricing => self.pricing_offset,
        }
    }
```
After:
```rust
    pub fn current_offset(&self) -> usize {
        match self.view {
            View::Dashboard => self.dashboard_offset,
            View::Models => self.models_offset,
            View::Pricing => self.pricing_offset,
        }
    }
```

- [ ] **Step 9: Drop the `View::Projects` arm in `set_current_offset`**

Before (lines 211-218):
```rust
    pub fn set_current_offset(&mut self, value: usize) {
        match self.view {
            View::Dashboard => self.dashboard_offset = value,
            View::Models => self.models_offset = value,
            View::Projects => self.projects_offset = value,
            View::Pricing => self.pricing_offset = value,
        }
    }
```
After:
```rust
    pub fn set_current_offset(&mut self, value: usize) {
        match self.view {
            View::Dashboard => self.dashboard_offset = value,
            View::Models => self.models_offset = value,
            View::Pricing => self.pricing_offset = value,
        }
    }
```

- [ ] **Step 10: Remove the now-unused `ProjectsViewModel` import**

In the `use` block at the top of `app_state.rs` (lines 3-5):
```rust
use crate::adapters::view_models::{
    DashboardViewModel, ModelsViewModel, PricingViewModel, ProjectsViewModel,
};
```
Remove `ProjectsViewModel`:
```rust
use crate::adapters::view_models::{DashboardViewModel, ModelsViewModel, PricingViewModel};
```

- [ ] **Step 11: Verify the renderer + app_state layers compile**

```bash
cargo check -p analysis 2>&1 | grep -E "projects|Projects"
```

Expected: remaining errors are confined to `tui_controller.rs` (controller field, ctor, `ensure_vm_for_current_view`, imports, and test index references) and `integration.rs`. The renderer/mod.rs and app_state.rs should be clean.

- [ ] **Step 12: Commit Tasks 8+9 together (renderer + app state)**

```bash
git add crates/analysis/src/tui/renderer/projects.rs crates/analysis/src/tui/renderer/mod.rs crates/analysis/src/tui/app_state.rs
git commit -m "refactor(analysis): remove Projects renderer, View::Projects, and state

Drops the projects renderer module, the View::Projects enum variant
(sidebar shrinks from 4 to 3 items), and the projects_vm/projects_offset
state fields. Pricing is now sidebar index 2."
```

---

## Task 10: Remove Projects wiring + fix tests in `TuiController`

**Files:**
- Modify: `crates/analysis/src/tui/controllers/tui_controller.rs`

- [ ] **Step 1: Remove presenter import**

Before (lines 5-7):
```rust
use crate::adapters::presenters::{
    present_dashboard, present_models, present_pricing, present_projects,
};
```
After:
```rust
use crate::adapters::presenters::{present_dashboard, present_models, present_pricing};
```

- [ ] **Step 2: Remove DTO imports**

Before (lines 8-11):
```rust
use crate::application::dto::{
    Filter, GetDashboardInput, GetModelsBreakdownInput, GetPricingInput, GetProjectsBreakdownInput,
    SyncPricingInput,
};
```
After:
```rust
use crate::application::dto::{
    Filter, GetDashboardInput, GetModelsBreakdownInput, GetPricingInput, SyncPricingInput,
};
```

- [ ] **Step 3: Remove use case import**

Before (lines 12-14):
```rust
use crate::application::use_cases::{
    GetDashboard, GetModelsBreakdown, GetPricing, GetProjectsBreakdown, SyncPricing,
};
```
After:
```rust
use crate::application::use_cases::{GetDashboard, GetModelsBreakdown, GetPricing, SyncPricing};
```

- [ ] **Step 4: Remove the `get_projects_breakdown` field from `TuiController`**

Before (lines 21-28):
```rust
pub struct TuiController {
    pub get_dashboard: Arc<GetDashboard>,
    pub get_models_breakdown: Arc<GetModelsBreakdown>,
    pub get_projects_breakdown: Arc<GetProjectsBreakdown>,
    pub get_pricing: Arc<GetPricing>,
    pub sync_pricing: Arc<SyncPricing>,
    clock: Arc<dyn Clock>,
}
```
After:
```rust
pub struct TuiController {
    pub get_dashboard: Arc<GetDashboard>,
    pub get_models_breakdown: Arc<GetModelsBreakdown>,
    pub get_pricing: Arc<GetPricing>,
    pub sync_pricing: Arc<SyncPricing>,
    clock: Arc<dyn Clock>,
}
```

- [ ] **Step 5: Remove the ctor parameter and field init**

Before (lines 31-47):
```rust
impl TuiController {
    pub fn new(
        get_dashboard: Arc<GetDashboard>,
        get_models_breakdown: Arc<GetModelsBreakdown>,
        get_projects_breakdown: Arc<GetProjectsBreakdown>,
        get_pricing: Arc<GetPricing>,
        sync_pricing: Arc<SyncPricing>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            get_dashboard,
            get_models_breakdown,
            get_projects_breakdown,
            get_pricing,
            sync_pricing,
            clock,
        }
    }
```
After:
```rust
impl TuiController {
    pub fn new(
        get_dashboard: Arc<GetDashboard>,
        get_models_breakdown: Arc<GetModelsBreakdown>,
        get_pricing: Arc<GetPricing>,
        sync_pricing: Arc<SyncPricing>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            get_dashboard,
            get_models_breakdown,
            get_pricing,
            sync_pricing,
            clock,
        }
    }
```

- [ ] **Step 6: Remove the `projects_offset = 0;` reset in `handle_mouse`**

In `handle_mouse` (around line 159), inside the sidebar-item click handler, remove:
```rust
                        state.projects_offset = 0;
```

- [ ] **Step 7: Remove the `View::Projects` arm in `ensure_vm_for_current_view`**

In `ensure_vm_for_current_view` (lines 379-424), remove the whole Projects arm (lines 405-413):
```rust
            View::Projects if state.projects_vm.is_none() => {
                let out = self
                    .get_projects_breakdown
                    .execute(GetProjectsBreakdownInput {
                        filter: Some(filter),
                    })
                    .map_err(|e| AdapterError::DataMapping(e.to_string()))?;
                state.projects_vm = Some(present_projects(&out));
            }
```

- [ ] **Step 8: Update `ctl_with_source_rows` test helper**

In the test module (around lines 439-469), the `ctl_with_source_rows` helper builds a `TuiController`. Remove the `gp` construction and the `gp` argument:

Before (line 461):
```rust
        let gp = Arc::new(GetProjectsBreakdown::new(usage_dyn, clock.clone()));
        let get_pricing = Arc::new(GetPricing::new(pricing_repo_dyn.clone()));
        let controller_clock = clock.clone();
        let sync = Arc::new(SyncPricing::new(source_dyn, pricing_repo_dyn, clock));
        (
            TuiController::new(gd, gm, gp, get_pricing, sync, controller_clock),
            pricing_repo,
        )
```
After:
```rust
        let get_pricing = Arc::new(GetPricing::new(pricing_repo_dyn.clone()));
        let controller_clock = clock.clone();
        let sync = Arc::new(SyncPricing::new(source_dyn, pricing_repo_dyn, clock));
        (
            TuiController::new(gd, gm, get_pricing, sync, controller_clock),
            pricing_repo,
        )
```

- [ ] **Step 9: Update the `esc_backspace_left_h_backtab_return_to_sidebar` test**

Around line 685-690, the test maps each `View` to a `sidebar_selected` index. Remove the `Projects` arm and shift Pricing to 2:

Before:
```rust
            state.sidebar_selected = match view {
                View::Dashboard => 0,
                View::Models => 1,
                View::Projects => 2,
                View::Pricing => 3,
            };
```
After:
```rust
            state.sidebar_selected = match view {
                View::Dashboard => 0,
                View::Models => 1,
                View::Pricing => 2,
            };
```

Also check the `cases` array (lines 675-681) — it lists `(KeyCode, View)` pairs. Currently:
```rust
        let cases: &[(KeyCode, View)] = &[
            (KeyCode::Esc, View::Dashboard),
            (KeyCode::Backspace, View::Dashboard),
            (KeyCode::Left, View::Models),
            (KeyCode::Char('h'), View::Dashboard),
            (KeyCode::BackTab, View::Dashboard),
        ];
```
None reference `View::Projects`, so leave the array unchanged. The only change is the match in step 9.

- [ ] **Step 10: Rewrite the `mouse_click_on_sidebar_item_selects_view_and_enters_content` test**

This test (lines 887-916) clicks row 2 and asserts `state.view == View::Projects`. After removal, row 2 is Pricing. Update the assertion and the sidebar-items count from 4 to 3:

Before:
```rust
    #[test]
    fn mouse_click_on_sidebar_item_selects_view_and_enters_content() {
        use crate::tui::app_state::Hit;
        use ratatui::layout::Rect;
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        // Simulate a previously-rendered sidebar: 4 rows starting at (0, 0).
        state.hit_regions.sidebar_items = (0..4)
            .map(|i| {
                Hit(Rect {
                    x: 0,
                    y: i as u16,
                    width: 16,
                    height: 1,
                })
            })
            .collect();

        // Click on row 2 → Projects.
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        controller.handle_mouse(click, &mut state).unwrap();
        assert_eq!(state.sidebar_selected, 2);
        assert_eq!(state.view, View::Projects);
        assert_eq!(state.focus, Focus::Content);
    }
```
After:
```rust
    #[test]
    fn mouse_click_on_sidebar_item_selects_view_and_enters_content() {
        use crate::tui::app_state::Hit;
        use ratatui::layout::Rect;
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        // Simulate a previously-rendered sidebar: 3 rows starting at (0, 0).
        state.hit_regions.sidebar_items = (0..3)
            .map(|i| {
                Hit(Rect {
                    x: 0,
                    y: i as u16,
                    width: 16,
                    height: 1,
                })
            })
            .collect();

        // Click on row 2 → Pricing.
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        controller.handle_mouse(click, &mut state).unwrap();
        assert_eq!(state.sidebar_selected, 2);
        assert_eq!(state.view, View::Pricing);
        assert_eq!(state.focus, Focus::Content);
    }
```

- [ ] **Step 11: Shift Pricing tests from `sidebar_selected = 3` to `sidebar_selected = 2`**

Multiple unit tests set `state.sidebar_selected = 3` and `state.view = View::Pricing` to target the Pricing view. With the sidebar now 3-wide, Pricing is index 2. Find every occurrence:

```bash
rg -n "sidebar_selected = 3" crates/analysis/src/tui/controllers/tui_controller.rs
```

For each match, change `state.sidebar_selected = 3;` to `state.sidebar_selected = 2;`. Based on the references found during planning, the affected tests are approximately:
- `navigating_to_pricing_loads_pricing_vm` (~line 521)
- `pgdn_advances_pricing_offset_when_content_focused` (~line 542)
- `pgdn_does_nothing_when_sidebar_focused` (~line 555)
- `pgup_decreases_offset_saturating_at_zero` (~line 569)
- `home_resets_offset_to_zero` (~line 586)
- `end_sets_offset_to_usize_max` (~line 599)
- `sidebar_navigation_resets_all_offsets` (~line 612) — also asserts on `pricing_offset`; the assertion value is unaffected, only the setup `sidebar_selected = 3` becomes `2`.
- `esc_backspace_left_h_backtab_return_to_sidebar` — already handled in step 9 (this is the `match view` mapping).
- `pricing_state_content` (~line 1063)
- `sidebar_navigation_resets_pricing_search` (~line 1209)

Use a blanket replacement since the meaning is identical in every case:

```bash
# Verify each match is for Pricing before replacing; pricing_view grep confirms.
rg -n "sidebar_selected = 3" crates/analysis/src/tui/controllers/tui_controller.rs
```

Then for each line, change `3` to `2`. If your editor supports a literal replace of `sidebar_selected = 3` → `sidebar_selected = 2` across the file, use it (every occurrence in this file targets Pricing — verified by the `state.view = View::Pricing;` on the immediately following line in every case).

- [ ] **Step 12: Verify the controller compiles and tests pass**

```bash
cargo test -p analysis --lib
```

Expected: all unit tests in `tui_controller.rs` pass.

- [ ] **Step 13: Commit Tasks 7+8+9+10 are now logically grouped — but renderer/app_state were committed in Task 9 step 12, so commit only the controller changes here**

```bash
git add crates/analysis/src/tui/controllers/tui_controller.rs
git commit -m "refactor(analysis): drop Projects wiring from TuiController

Removes the get_projects_breakdown field, ctor parameter,
ensure_vm_for_current_view arm, and Projects-related imports. Updates
unit tests: Pricing is now sidebar index 2 (was 3), and the row-2
mouse-click test now asserts View::Pricing (was View::Projects)."
```

---

## Task 11: Remove Projects wiring from `main.rs`

**Files:**
- Modify: `crates/analysis/src/main.rs`

- [ ] **Step 1: Remove `GetProjectsBreakdown` from the use_cases import**

Before (lines 8-10):
```rust
use analysis::application::use_cases::{
    GetDashboard, GetModelsBreakdown, GetPricing, GetProjectsBreakdown, SyncPricing,
};
```
After:
```rust
use analysis::application::use_cases::{GetDashboard, GetModelsBreakdown, GetPricing, SyncPricing};
```

- [ ] **Step 2: Remove the `get_projects` construction**

Before (line 69):
```rust
    let get_projects = Arc::new(GetProjectsBreakdown::new(usage_repo, clock.clone()));
    let get_pricing = Arc::new(GetPricing::new(pricing_repo.clone()));
```
After:
```rust
    let get_pricing = Arc::new(GetPricing::new(pricing_repo.clone()));
```

- [ ] **Step 3: Remove `get_projects` from the `TuiController::new` call**

Before (lines 74-81):
```rust
    let controller = TuiController::new(
        get_dashboard,
        get_models,
        get_projects,
        get_pricing,
        sync_pricing,
        controller_clock,
    );
```
After:
```rust
    let controller = TuiController::new(
        get_dashboard,
        get_models,
        get_pricing,
        sync_pricing,
        controller_clock,
    );
```

- [ ] **Step 4: Confirm `default_projects_root` and `ClaudeCodeUsageRepository::new(default_projects_root())` are untouched**

Visually verify lines 3, 34-35 of `main.rs` still read:
```rust
use analysis::adapters::gateways::claudecode::{ClaudeCodeUsageRepository, default_projects_root};
```
and
```rust
    let claudecode_repo: Arc<dyn UsageRepository> =
        Arc::new(ClaudeCodeUsageRepository::new(default_projects_root()));
```

These are unrelated to the Projects *view* — `default_projects_root()` locates `~/.claude/projects` for session JSONL discovery and must remain.

- [ ] **Step 5: Verify the binary builds**

```bash
cargo build -p analysis
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/analysis/src/main.rs
git commit -m "refactor(analysis): stop constructing GetProjectsBreakdown in main

The composition root no longer wires the Projects use case into
TuiController. default_projects_root() is retained because it locates
Claude Code session JSONL files for the repository, not the view."
```

---

## Task 12: Remove Projects integration test

**Files:**
- Modify: `crates/analysis/tests/integration.rs`

- [ ] **Step 1: Remove the DTO import**

Before (lines 15-17):
```rust
use analysis::application::dto::{
    Filter, GetDashboardInput, GetModelsBreakdownInput, GetPricingInput, GetProjectsBreakdownInput,
};
```
After:
```rust
use analysis::application::dto::{Filter, GetDashboardInput, GetModelsBreakdownInput, GetPricingInput};
```

- [ ] **Step 2: Remove the use case import**

Before (lines 19-21):
```rust
use analysis::application::use_cases::{
    GetDashboard, GetModelsBreakdown, GetPricing, GetProjectsBreakdown,
};
```
After:
```rust
use analysis::application::use_cases::{GetDashboard, GetModelsBreakdown, GetPricing};
```

- [ ] **Step 3: Delete the `projects_breakdown_groups_by_session_directory` test**

Delete lines 177-201 inclusive — the entire test function:

```rust
#[test]
fn projects_breakdown_groups_by_session_directory() {
    let (usage_repo, _pricing, clock) = wire();
    let uc = GetProjectsBreakdown::new(usage_repo, clock);

    let out = uc
        .execute(GetProjectsBreakdownInput {
            filter: Some(unfiltered()),
        })
        .unwrap();

    assert_eq!(
        out.projects.len(),
        2,
        "two distinct directories in seed.sql"
    );

    let paths: Vec<_> = out
        .projects
        .iter()
        .map(|r| r.project.as_str().to_string())
        .collect();
    assert!(paths.iter().any(|p| p == "/work/alpha"));
    assert!(paths.iter().any(|p| p == "/work/beta"));
}
```

- [ ] **Step 4: Run the full analysis test suite**

```bash
cargo test -p analysis
```

Expected: all remaining tests pass (dashboard, models, pricing integration tests + all unit tests).

- [ ] **Step 5: Commit**

```bash
git add crates/analysis/tests/integration.rs
git commit -m "test(analysis): remove projects_breakdown integration test

The GetProjectsBreakdown use case no longer exists; the corresponding
end-to-end test and its imports are removed. The seed.sql fixture still
contains two distinct session directories, which the remaining
dashboard/models/pricing tests rely on unchanged."
```

---

## Task 13: Final workspace verification + formatting

- [ ] **Step 1: Format the whole workspace**

```bash
cargo fmt
```

- [ ] **Step 2: Workspace check + clippy with -D warnings**

```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
```

Expected: both PASS with zero warnings.

- [ ] **Step 3: Full workspace test suite**

```bash
cargo test --workspace
```

Expected: PASS.

- [ ] **Step 4: Final residual-reference grep**

Sanity-check that no straggler reference to the deleted view remains in the `analysis` or `shared` crates:

```bash
rg -n "ProjectsView|ProjectUsage|GetProjectsBreakdown|by_project|View::Projects|projects_vm|projects_offset|projects_presenter|projects_vm\.rs|projects\.rs|projects_breakdown" crates/analysis crates/shared
```

Expected: zero matches. (If `ProjectPath` or `filter.project` show up, those are expected and correct — they stay. The grep above intentionally excludes those terms.)

- [ ] **Step 5: If formatting or lint produced any changes, commit them**

```bash
git status --short
```
If anything is modified:
```bash
git add -A && git commit -m "chore(analysis): fmt after Projects view removal"
```
Otherwise skip.

---

## Self-Review Notes

**Spec coverage:**
- Spec section "1. Domain / DTO layer" → Task 2.
- Spec section "2. Port layer" → Task 4.
- Spec section "3. Use case layer" → Task 3.
- Spec section "4. Adapter layer — view models & presenters" → Task 7.
- Spec section "5. Adapter layer — gateways" → Task 5 (+ Task 6 for `FakeUsageRepository`, which the spec mentioned under step 5 but which actually lives in `application/test_support.rs`).
- Spec section "6. TUI framework — renderers" → Task 8.
- Spec section "7. TUI framework — app state" → Task 9.
- Spec section "8. TUI framework — controller" → Task 10.
- Spec section "9. Composition root" → Task 11.
- Spec section "10. Integration tests" → Task 12.
- Spec "Verification" → Task 13.
- Spec "Non-Goal: keep `default_projects_root`" → Task 11 step 4 (explicit verification).

**Placeholder scan:** None — every step contains exact file paths, exact code, and exact commands.

**Type consistency:** `ProjectUsage` is deleted in Task 1 and never referenced in later tasks. `ProjectPath` is explicitly preserved in Task 5 step 1 (sqlite import), Task 5 step 2 (claudecode import — "do NOT remove"), and verified in Task 13 step 4. `View::Pricing` index transitions from 3 → 2 consistently in Task 9 (`View::ALL`), Task 10 steps 9-11 (test index shifts). `TuiController::new` arity changes from 6 args → 5 args consistently in Task 10 step 5 (definition) and Task 11 step 3 (call site).
