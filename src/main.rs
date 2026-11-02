mod config;
mod discover;
mod doctor;
mod explain;
mod hook;
mod init;
mod rtk;

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
  dtk doctor               check the hook, rtk, the helper tools and the config
  dtk verify               built-in self-test of detection and decisions, then doctor
  dtk explain [FILE|-] [--command CMD] [--read] [--saved]
                           say whether dtk would summarize this output, and why
  dtk discover [--all | --since DAYS] [--limit N] [--json]
                           scan your Claude Code history for tables dtk would have summarized
  dtk config [--create]    show the settings in use, or write a default config file
  dtk mode [shadow|live]   show or set the mode (shadow = log only, change nothing)
  dtk gain [--json]        totals from the decision log, with rtk's totals beside them
  dtk --version

environment:
  DTK_DISABLED=1           dtk passes every output through (the rtk chain still runs)
  DTK_MODE=shadow|live     override the mode file
  DTK_CHAIN_TIMEOUT_MS     longest the chained command may run (default 10000)";

/// `dtk view big.csv | head` closes the pipe early. That is not an error: stop quietly.
fn quiet_on_broken_pipe() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = info
            .payload()
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| info.payload().downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        // Unix says "Broken pipe"; Windows says "The pipe is being closed" (os error 232) or
        // "The pipe has been ended" (os error 109).
        if [
            "Broken pipe",
            "pipe is being closed",
            "pipe has been ended",
            "os error 232",
            "os error 109",
        ]
        .iter()
        .any(|m| text.contains(m))
        {
            std::process::exit(0);
        }
        default(info);
    }));
}

/// `dtk config [--create]`: show the settings in use, or write the default file.
fn config_command(args: &[String]) -> i32 {
    let Some(path) = config::path() else { return 1 };
    if args.iter().any(|a| a == "--create") {
        if path.exists() {
            eprintln!("dtk: {} already exists; not touching it", path.display());
            return 1;
        }
        if let Some(dir) = path.parent() {
            let _ = hook::create_private_dir(dir);
        }
        return match std::fs::write(&path, config::default_file()) {
            Ok(()) => {
                println!("dtk: wrote {}", path.display());
                0
            }
            Err(e) => {
                eprintln!("dtk: cannot write {}: {e}", path.display());
                1
            }
        };
    }
    let (cfg, error) = config::Config::load();
    println!(
        "file:               {}{}",
        path.display(),
        if path.exists() {
            ""
        } else {
            " (does not exist; defaults)"
        }
    );
    if let Some(e) = error {
        println!("problem:            {e} (defaults are used)");
    }
    println!("exclude_commands:   {:?}", cfg.exclude_commands);
    println!("min_tokens:         {}", cfg.min_tokens);
    println!("budget:             {}", cfg.budget);
    println!("min_persisted_rows: {}", cfg.min_persisted_rows);
    println!("min_outline_tokens: {}", cfg.min_outline_tokens);
    0
}

/// dtk started under the name `rtk` is the placeholder that `dtk init` leaves for `rtk init` to
/// find (see `init.rs`). As `rtk hook claude` it does nothing, silently.
fn run_as_rtk_placeholder(args: &[String]) -> Option<ExitCode> {
    let name = std::env::args_os().next().map(std::path::PathBuf::from)?;
    let stem = name.file_stem()?.to_string_lossy().to_ascii_lowercase();
    if stem != "rtk" {
        return None;
    }
    if args.first().map(String::as_str) == Some("hook")
        && args.get(1).map(String::as_str) == Some("claude")
    {
        return Some(ExitCode::SUCCESS);
    }
    eprintln!(
        "this is dtk's placeholder for rtk, not rtk. It only answers `hook claude`, with nothing."
    );
    Some(ExitCode::from(2))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = run_as_rtk_placeholder(&args) {
        return code;
    }
    quiet_on_broken_pipe();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("hook") => hook::run(&args[1..]),
        Some("view") => hook::view(args.get(1).map(String::as_str)),
        Some("mode") => hook::mode(args.get(1).map(String::as_str)),
        Some("gain") => hook::gain(args.iter().any(|a| a == "--json")),
        Some("doctor") => i32::from(doctor::run() > 0),
        Some("explain") => explain::explain(&args[1..]),
        Some("verify") => explain::verify(),
        Some("discover") => discover::run(&args[1..]),
        Some("config") => config_command(&args[1..]),
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
