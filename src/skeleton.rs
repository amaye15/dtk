//! The layout of an output with every value taken out, to send with a bug report.
//!
//! When dtk leaves alone an output that it should read, the layout is what a maintainer needs:
//! the delimiters, the alignment, the header, the shape of each line. The values are the part that
//! can be private. `skeleton` keeps the first one and drops the second:
//!
//! - every digit becomes `9`, every lowercase letter `a`, every uppercase letter `A`;
//! - punctuation, spaces and line breaks stay as they are, so the columns still line up;
//! - only the first lines and the last few are kept, and a long line is cut.
//!
//! No character of the original that is a letter or a digit is in the result. That is not
//! encryption and not a hash: it is a loss of information on purpose. Read the result before you
//! send it. The first line (the column names) can be kept with `keep_header`; it is the one place
//! where words survive, and a column name can be private too.

use crate::table::clean;

/// What to keep.
pub struct Options {
    /// Keep the first non-blank line as it is.
    pub keep_header: bool,
    /// Lines to keep from the top.
    pub head: usize,
    /// Lines to keep from the bottom.
    pub tail: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            keep_header: false,
            head: 40,
            tail: 5,
        }
    }
}

/// A line is cut after this many characters.
const MAX_LINE: usize = 300;

fn mask(c: char) -> char {
    if c.is_numeric() {
        '9'
    } else if c.is_uppercase() {
        'A'
    } else if c.is_alphabetic() {
        'a'
    } else {
        c
    }
}

fn mask_line(line: &str) -> String {
    let mut out: String = line.chars().take(MAX_LINE).map(mask).collect();
    let rest = line.chars().count().saturating_sub(MAX_LINE);
    if rest > 0 {
        out.push_str(&format!(" ...(+{rest} chars)"));
    }
    out
}

/// The layout of `text`. Colour codes are taken out first, as dtk does before it reads a table.
pub fn skeleton(text: &str, opts: &Options) -> String {
    let cleaned = clean(text);
    let lines: Vec<&str> = cleaned.lines().collect();
    let header_at = if opts.keep_header {
        lines.iter().position(|l| !l.trim().is_empty())
    } else {
        None
    };
    let line = |i: usize| -> String {
        if Some(i) == header_at {
            lines[i].chars().take(MAX_LINE).collect()
        } else {
            mask_line(lines[i])
        }
    };
    let mut out = String::new();
    if lines.len() <= opts.head + opts.tail {
        for i in 0..lines.len() {
            out.push_str(&line(i));
            out.push('\n');
        }
        return out;
    }
    for i in 0..opts.head {
        out.push_str(&line(i));
        out.push('\n');
    }
    out.push_str(&format!(
        "... {} lines left out ...\n",
        lines.len() - opts.head - opts.tail
    ));
    for i in lines.len() - opts.tail..lines.len() {
        out.push_str(&line(i));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_and_letters_become_classes_and_the_layout_stays() {
        let s = skeleton("id,name,amount\n12,Ann Lee,3.50\n", &Options::default());
        assert_eq!(s, "aa,aaaa,aaaaaa\n99,Aaa Aaa,9.99\n");
    }

    #[test]
    fn no_letter_or_digit_of_the_original_survives() {
        let secret = "hunter2-SECRET 4111-1111-1111-1111 éàü 日本語 ²";
        let s = skeleton(secret, &Options::default());
        for c in s.chars() {
            assert!(
                !c.is_alphanumeric() || matches!(c, 'a' | 'A' | '9'),
                "`{c}` survived in {s}"
            );
        }
        assert!(!s.contains("hunter"), "{s}");
        // The punctuation is the layout, and it stays.
        assert!(s.contains("-"));
    }

    #[test]
    fn only_the_head_and_the_tail_are_kept_and_the_gap_is_counted() {
        let text: String = (0..100).map(|i| format!("row {i}\n")).collect();
        let s = skeleton(
            &text,
            &Options {
                keep_header: false,
                head: 3,
                tail: 2,
            },
        );
        assert_eq!(
            s,
            "aaa 9\naaa 9\naaa 9\n... 95 lines left out ...\naaa 99\naaa 99\n"
        );
    }

    #[test]
    fn the_header_can_be_kept_and_nothing_else() {
        let s = skeleton(
            "order_id,customer\n5042,Ann\n",
            &Options {
                keep_header: true,
                ..Options::default()
            },
        );
        assert_eq!(s, "order_id,customer\n9999,Aaa\n");
    }

    #[test]
    fn a_long_line_is_cut_and_says_by_how_much() {
        let s = skeleton(&format!("{}\n", "x".repeat(1000)), &Options::default());
        assert!(s.starts_with(&"a".repeat(300)));
        assert!(s.contains("(+700 chars)"), "{s}");
    }

    #[test]
    fn colour_codes_are_taken_out_first() {
        let s = skeleton(
            "\u{1b}[1m\u{1b}[31mERROR\u{1b}[0m 42\n",
            &Options::default(),
        );
        assert_eq!(s, "AAAAA 99\n");
    }
}
