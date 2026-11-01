mod doctor;
mod hook;
mod init;

use std::process::ExitCode;

const HELP: &str = "dtk: data token killer

A Claude Code PostToolUse hook. It finds big tables, CSV, JSON arrays and JSONL in Bash output
and swaps them for a lean summary. The full data stays on disk. It works with rtk.

usage:
  dtk hook claude [--then CMD ARGS...]
                           run as a hook (reads the event on standard input); with --then,
                           events that dtk does not handle go to CMD, normally `rtk hook claude`
  dtk view [FILE|-]        show the lean view of a file or standard input
  dtk init --global        register the hook in ~/.claude/settings.json and chain to rtk
  dtk init --global --no-rtk
                           register without the rtk chain
  dtk init --uninstall     remove the hook again (and restore rtk's own PostToolUse hook)
  dtk doctor               check the hook, rtk and the helper tools
  dtk mode [shadow|live]   show or set the mode (shadow = log only, change nothing)
  dtk gain                 totals from the decision log, with rtk's totals beside them
  dtk --version

environment:
  DTK_DISABLED=1           dtk passes every output through (the rtk chain still runs)
  DTK_MODE=shadow|live     override the mode file
  DTK_CHAIN_TIMEOUT_MS     longest the chained command may run (default 10000)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("hook") => hook::run(&args[1..]),
        Some("view") => hook::view(args.get(1).map(String::as_str)),
        Some("mode") => hook::mode(args.get(1).map(String::as_str)),
        Some("gain") => hook::gain(),
        Some("doctor") => doctor::run(),
        Some("init") => init::run(&args[1..]),
        Some("--version") | Some("-V") => {
            println!("dtk {}", env!("CARGO_PKG_VERSION"));
            0
        }
        None | Some("--help") | Some("-h") => {
            println!("{HELP}");
            0
        }
        Some(other) => {
            eprintln!("dtk: unknown command {other}\n\n{HELP}");
            2
        }
    };
    ExitCode::from(code as u8)
}
