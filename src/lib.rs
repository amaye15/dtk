//! dtk: finds tables in command output and renders short, typed summaries of them.
//!
//! The `dtk` binary uses this library for its Claude Code hook. Other programs, such as an rtk
//! plugin, can call it directly:
//!
//! ```
//! let ctx = dtk::lean::Ctx { bytes: 0, csv_path: None, raw_path: None };
//! assert!(dtk::summarize("not a table", &ctx, 2500).is_none());
//! ```

pub mod lean;
mod logs;
pub mod table;
mod textcols;

/// Finds a table in `text` and renders its lean view. `None` means `text` is not a table.
pub fn summarize(
    text: &str,
    ctx: &lean::Ctx,
    budget: usize,
) -> Option<(table::Table, lean::Rendered)> {
    let t = table::detect(text)?;
    let r = lean::render(&t, ctx, budget);
    Some((t, r))
}
