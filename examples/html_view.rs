//! `cargo run --release --example html_view -- PAGE.html...`: the text view of each page, and
//! how many tokens it takes. A tool for checking the reader on real pages.
fn main() {
    for path in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !dtk::html::looks_like_html(&raw) {
            println!("=== {path}: not an HTML page");
            continue;
        }
        let page = dtk::html::parse(&raw);
        let s = dtk::html::summary(&page, &raw, "0123456789ab", None, 2500);
        println!(
            "=== {path}: {} tokens -> {} (full text {} lines)\n{s}\n",
            dtk::lean::est_tokens(&raw),
            dtk::lean::est_tokens(&s),
            page.text.len()
        );
    }
}
