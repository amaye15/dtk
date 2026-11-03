//! `cargo run --release --example digest_view -- FILE...`: the log digest of each file that the
//! hook would digest: not a table, and at least `DIGEST_MIN_CHARS` (default 30000, the size at
//! which Claude Code saves an output to a file). A tool for tuning and for checking that code,
//! documents and lists are never digested.
fn main() {
    let min: usize = std::env::var("DIGEST_MIN_CHARS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30_000);
    for path in std::env::args().skip(1) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.len() < min
            || dtk::table::detect(&text).is_some()
            || dtk::table::detect_wrapped(&text).is_some()
        {
            continue;
        }
        match dtk::digest::digest(&text, Some(&path)) {
            Some(d) => println!(
                "=== {path}: {} tokens -> {}\n{d}",
                dtk::lean::est_tokens(&text),
                dtk::lean::est_tokens(&d)
            ),
            None => println!("--- {path}: not a log"),
        }
    }
}
