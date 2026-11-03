# Install dtk

dtk is one file with no runtime dependencies. This page lists every way to get it, for macOS, Linux and Windows. Each way ends with the same check: `dtk verify`.

## Before you install

Check whether dtk is already installed:

```sh
dtk --version
which dtk          # Windows: where dtk
```

**Name collision.** Other programs use the name `dtk` (for example decomp-toolkit, a game-modding tool). If `dtk --version` prints something other than `dtk X.Y.Z`, or if `dtk --help` does not start with `dtk: data token killer`, you have another program. Remove it or put this one first on your `PATH`. `dtk verify` is the final test: it prints `N of N built-in checks passed` (all of them) only for this program.

## Quick install

### macOS and Linux

```sh
curl -fsSL https://raw.githubusercontent.com/amaye15/dtk/main/install.sh | sh
```

The script downloads the archive for your system, checks its SHA-256 sum, and puts `dtk` in `~/.local/bin`. It stops and installs nothing when the sum does not match.

| Variable | Use |
|---|---|
| `DTK_VERSION=v0.4.0` | Install one release. The default is the newest. |
| `DTK_INSTALL_DIR=/usr/local/bin` | Install in another folder. |
| `DTK_SKIP_CHECKSUM=1` | Skip the SHA-256 check. Not recommended. |
| `DTK_NO_INIT=1` | Install only. Do not register the hook. |

When Claude Code's folder (`~/.claude`) exists, the script also runs `dtk init --global`: it registers the hook, keeps a backup of `settings.json`, and chains to rtk when rtk is installed. If the folder does not exist, the script says so and registers nothing.

If `~/.local/bin` is not on your `PATH`, the script prints the line to add to your shell profile.

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/amaye15/dtk/main/install.ps1 | iex
```

The script installs `dtk.exe` in `%USERPROFILE%\.local\bin` after it checks the SHA-256 sum. It uses the same variables as `install.sh` (including `DTK_NO_INIT=1`), registers the hook when `%USERPROFILE%\.claude` exists, and `DTK_ADD_TO_PATH=1` also adds the folder to your user `PATH`.

## Homebrew (macOS and Linux)

```sh
brew install amaye15/tap/dtk
```

The tap follows each dtk release by itself, within 6 hours. Update with `brew upgrade dtk`.

## Scoop (Windows)

```powershell
scoop bucket add amaye15 https://github.com/amaye15/scoop-bucket
scoop install dtk
```

Update with `scoop update dtk`.

## winget (Windows)

The manifests are in [`packaging/winget`](../packaging/winget). Until the package is accepted into the public winget repository, install from the local files:

```powershell
winget settings --enable LocalManifestFiles
winget install --manifest packaging\winget\manifests\a\amaye15\dtk\<version>
```

When the package is in the public repository, the command is `winget install amaye15.dtk`.

## Cargo

```sh
cargo install dtk --locked                              # from crates.io
cargo install --git https://github.com/amaye15/dtk --locked   # the newest source on GitHub
```

You need Rust 1.80 or newer. `cargo install` puts `dtk` in `~/.cargo/bin`.

## Pre-built binaries

Download an archive from the [releases page](https://github.com/amaye15/dtk/releases). Each archive has a `.sha256` file next to it. `checksums.txt` lists all the sums.

| System | Archive |
|---|---|
| macOS, Apple silicon | `dtk-<version>-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `dtk-<version>-x86_64-apple-darwin.tar.gz` |
| Linux, x86_64 (static, any distribution) | `dtk-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux, ARM64 (static, any distribution) | `dtk-<version>-aarch64-unknown-linux-musl.tar.gz` |
| Windows, x86_64 | `dtk-<version>-x86_64-pc-windows-msvc.zip` |

Check the sum, unpack, and put `dtk` on your `PATH`:

```sh
shasum -a 256 -c dtk-<version>-<target>.tar.gz.sha256
tar -xzf dtk-<version>-<target>.tar.gz
install -m 755 dtk ~/.local/bin/
```

On Windows, check the sum with `Get-FileHash <file>.zip -Algorithm SHA256` and compare it to the `.sha256` file. Unzip the archive and put `dtk.exe` in a folder on your `PATH`.

## Debian, Ubuntu, Fedora and RHEL packages

```sh
# Debian, Ubuntu (amd64 or arm64)
curl -fsSLO https://github.com/amaye15/dtk/releases/latest/download/dtk_amd64.deb
sudo dpkg -i dtk_amd64.deb

# Fedora, RHEL (x86_64 or aarch64)
sudo dnf install https://github.com/amaye15/dtk/releases/latest/download/dtk.x86_64.rpm
```

For ARM, use `dtk_arm64.deb` and `dtk.aarch64.rpm`. The packages put `dtk` in `/usr/bin` and the README in `/usr/share/doc/dtk`. They have no dependencies.

## Windows: native and WSL

- **Native Windows.** Use Scoop, winget, `install.ps1`, Cargo or the zip. Run `dtk init --global` in PowerShell. dtk writes the hook into `%USERPROFILE%\.claude\settings.json`, which is the file that Claude Code for Windows reads.
- **WSL.** Use one of the Linux ways inside the WSL shell. dtk then edits the `~/.claude/settings.json` of your WSL home folder. That is the right file only when you run Claude Code inside WSL.
- A dtk in WSL and a Claude Code on native Windows do not see each other. Install dtk where Claude Code runs.

## Register the hook

`install.sh` and `install.ps1` do this for you. After Homebrew, Scoop, winget, Cargo or a package, register the hook once, then start a new Claude Code session:

```sh
dtk init --global       # edits ~/.claude/settings.json, writes a backup first
```

If rtk is installed, dtk chains to it, and `rtk init -g` keeps working. Use `dtk init --global --no-rtk` to skip the chain. See "Working with rtk" in the [README](../README.md).

## Check the install

```sh
dtk --version           # dtk X.Y.Z
dtk verify              # built-in checks, then the setup checks; prints "all good"
dtk doctor              # the setup checks only
dtk discover            # what dtk would have saved in your Claude Code history
```

dtk is **live** by default: it replaces big tables with summaries. To see first what it would do, run `dtk discover` (it reads your history and changes nothing) or `dtk mode shadow` (it logs its decisions and changes no output). Run `dtk mode live` to go back.

## Update

| Installed with | Update with |
|---|---|
| `install.sh`, `install.ps1` | Run the script again. |
| Homebrew | `brew upgrade dtk` |
| Scoop | `scoop update dtk` |
| winget | `winget upgrade amaye15.dtk` |
| Cargo | `cargo install dtk --locked --force` |
| `.deb`, `.rpm` | Install the new package. |

The hook names a path that stays the same through an update (a folder on your `PATH`, not a versioned Homebrew folder), so an update needs no other step. The two scripts run `dtk init --global` again by themselves; it is safe to repeat. Run it yourself if `dtk doctor` says the hook points at a missing file.

## Uninstall

First remove the hook and, if you want, the data. This step is the same for every install way:

```sh
dtk init --uninstall            # removes the hook; keeps the decision log and the saved data
dtk init --uninstall --purge    # also deletes ~/.local/share/dtk and ~/.config/dtk
```

If you skip this step, the hook stays in `settings.json` and points at a file that no longer exists. `dtk init --uninstall` also restores the rtk hook that dtk replaced.

Then remove the program:

| Installed with | Remove with |
|---|---|
| `install.sh` | `rm ~/.local/bin/dtk` |
| `install.ps1` | Delete `%USERPROFILE%\.local\bin\dtk.exe` |
| Homebrew | `brew uninstall dtk` |
| Scoop | `scoop uninstall dtk` |
| winget | `winget uninstall amaye15.dtk` |
| Cargo | `cargo uninstall dtk` |
| `.deb` | `sudo apt remove dtk` |
| `.rpm` | `sudo dnf remove dtk` |

`dtk init --uninstall` prints the right line for the way you installed.

## Build from source

```sh
git clone https://github.com/amaye15/dtk && cd dtk
cargo build --release --locked      # target/release/dtk
cargo test --locked                 # the full test suite
```

The drill-down commands that summaries suggest (`dtk top`, `dtk where`, ...) are part of dtk. `qsv` and `jaq` are optional extras (`brew install qsv jaq`).
