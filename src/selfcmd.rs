//! `vibox self`: manage the installed binary itself.
//!
//! `self update` shells out to `cargo install vibox --force`, and `self check`
//! asks the registry for the latest release through `cargo search`, so the
//! answer comes from the same registry `cargo install` would pull from.
//!
//! Output here is plain: vibox has no colour crate, and adding one for two
//! commands would not be worth a dependency.

use anyhow::{Result, anyhow, bail};
use std::io::{IsTerminal, Write};
use std::process::{Command, ExitCode};

const CRATE: &str = "vibox";

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Reinstall the latest release from crates.io
    ///   -y   skip the confirmation prompt
    #[command(verbatim_doc_comment)]
    Update {
        /// Skip the confirmation prompt, which is the only way to update from a script
        #[arg(short, long)]
        yes: bool,
    },
    /// Ask crates.io whether a newer release exists, without installing anything
    ///
    /// Exits 0 when this is the newest release, 1 when a newer one is out and 2
    /// when crates.io could not be asked, so a script gates on the code:
    /// `vibox self check; [ $? -eq 1 ] && vibox self update -y`
    Check,
}

pub fn run(cmd: Cmd) -> Result<ExitCode> {
    match cmd {
        Cmd::Update { yes } => update(yes).map(|()| ExitCode::SUCCESS),
        Cmd::Check => Ok(check()),
    }
}

fn update(yes: bool) -> Result<()> {
    if !yes {
        // A prompt with nobody to answer it reads end of file, and calling
        // that a no would let a script believe it had updated.
        if !std::io::stdin().is_terminal() {
            bail!(
                "`self update` asks before installing and there is no terminal to ask on, pass `-y`"
            );
        }
        if !confirm() {
            eprintln!("not updated");
            return Ok(());
        }
    }

    println!("updating via cargo install vibox --force\n");
    match Command::new("cargo")
        .args(["install", CRATE, "--force"])
        .status()
    {
        Ok(status) if status.success() => {
            println!("\nvibox is up to date");
            Ok(())
        }
        Ok(status) => bail!(
            "update failed (cargo exited {})",
            status.code().unwrap_or(1)
        ),
        Err(e) => Err(no_cargo(&e)),
    }
}

/// Compare the installed version with the newest one on crates.io. Nothing is
/// downloaded or written, so this is safe to run on a machine you do not want to
/// change.
fn check() -> ExitCode {
    let current = env!("CARGO_PKG_VERSION");
    match latest() {
        Ok(latest) if newer(&latest, current) => {
            println!("vibox {latest} is available, you have {current}");
            println!("run `vibox self update`");
            ExitCode::from(1)
        }
        Ok(_) => {
            println!("vibox {current} is the latest release");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("vibox: {}", crate::reason::chain(&e));
            ExitCode::from(2)
        }
    }
}

fn no_cargo(e: &std::io::Error) -> anyhow::Error {
    anyhow!(
        "cannot run `cargo` ({}), install Rust from https://rustup.rs",
        crate::reason::of(e)
    )
}

/// `cargo search` prints `vibox = "X.Y.Z"    # description` for an exact name
/// match.
fn latest() -> Result<String> {
    let out = Command::new("cargo")
        .args(["search", CRATE, "--limit", "1"])
        .output()
        .map_err(|e| no_cargo(&e))?;
    if !out.status.success() {
        // cargo's own words, the last few lines of them, which are the ones
        // that say what went wrong.
        let said = String::from_utf8_lossy(&out.stderr);
        let lines: Vec<&str> = said.trim().lines().collect();
        let tail = lines[lines.len().saturating_sub(5)..].join("\n");
        bail!("could not reach crates.io: {tail}");
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let prefix = format!("{CRATE} = \"");
    text.lines()
        .find_map(|l| l.strip_prefix(&prefix))
        .and_then(|rest| rest.split('"').next())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("the registry did not list `{CRATE}`"))
}

/// Compare dotted versions field by field, so `0.10.0` correctly beats `0.9.9`
/// where a plain string compare would not.
fn newer(a: &str, b: &str) -> bool {
    let fields = |v: &str| {
        v.split(['.', '-'])
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    fields(a) > fields(b)
}

/// Ask the user to confirm. Defaults to No, so a bare Enter cancels.
fn confirm() -> bool {
    print!("update vibox to the latest release via cargo? [y/N] ");
    std::io::stdout().flush().ok();

    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::newer;

    /// The check compares field by field, because a plain string compare puts
    /// `0.9.9` above `0.10.0` and would tell everyone they are up to date.
    #[test]
    fn a_newer_release_is_recognised_field_by_field() {
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("1.0.0", "0.9.9"));
        assert!(newer("0.4.2", "0.4.1"));
        assert!(!newer("0.4.1", "0.4.1"));
        assert!(!newer("0.4.0", "0.4.1"));
        assert!(!newer("0.9.9", "0.10.0"));
    }
}
