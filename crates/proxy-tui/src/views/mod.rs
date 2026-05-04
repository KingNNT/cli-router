//! Per-view renderers. Keeps `ui.rs` focused on shared chrome (tab bar,
//! status line). New views land here; existing views remain in `ui.rs` until
//! they're touched.

pub mod usage;
pub mod account;
