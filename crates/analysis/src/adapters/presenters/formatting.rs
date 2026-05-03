//! Formatting helpers — relocated to `shared::adapters::presenters::formatting`.
//! Re-exported here so existing call sites in `analysis` keep their imports.

pub use shared::adapters::presenters::formatting::{
    fmt_cost, fmt_num, fmt_num_compact, fmt_num_trunc, format_date_md, format_date_year,
    trunc_model,
};
