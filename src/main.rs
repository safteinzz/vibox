//! vibox: a jukebox you exit with `:q`.
//!
//! The library is a directory, the keys are vi's, and the last line of the
//! screen is the command line.

mod app;
mod excmd;
mod keys;
mod library;
mod lyrics;
mod matrix;
mod mpris;
mod player;
mod ui;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;

use app::App;
use library::SortKey;

/// Redraw cadence: fast enough for a moving progress bar, slow enough to idle.
const TICK: Duration = Duration::from_millis(200);
/// Redraw cadence while the visualiser is running.
const FRAME: Duration = Duration::from_millis(50);

#[derive(Parser)]
#[command(
    name = "vibox",
    version,
    about = "a jukebox you exit with :q - a cli music player with vi motions, ex commands, and tmux manners",
    after_help = "The library is just a directory. `:e <dir>` opens another one, `c` edits the\n\
                  filenames in it, and `:help` lists every key."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,

    /// Library root to scan. Defaults to your music directory.
    path: Option<PathBuf>,

    /// Initial sort: path, title, artist, album, duration
    #[arg(short, long, default_value = "path")]
    sort: String,
}

#[derive(Subcommand)]
enum Cmd {
    /// Update vibox to the latest release
    ///   -y          skip the confirmation prompt
    #[command(verbatim_doc_comment)]
    Update {
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Cmd::Update { yes }) = cli.cmd {
        return cmd_update(yes);
    }

    let Some(sort) = SortKey::parse(&cli.sort) else {
        bail!(
            "bad sort key `{}` - use path, title, artist, album or duration",
            cli.sort
        );
    };

    let root = match cli.path {
        Some(p) => p,
        None => dirs::audio_dir().unwrap_or(std::env::current_dir()?),
    };

    // ratatui would panic with a raw os error if there is no terminal to take over.
    if !std::io::stdout().is_terminal() {
        bail!("vibox is a full screen player and needs a terminal; it has nothing to pipe");
    }

    let mut app = App::new(root, sort)?;
    excmd::load_state(&mut app);
    excmd::load_rc(&mut app);
    let terminal = ratatui::init();
    let result = run(terminal, &mut app);
    ratatui::restore();
    excmd::save_state(&app);
    result
}

fn run(mut terminal: DefaultTerminal, app: &mut App) -> Result<()> {
    let mut shape = None;
    while !app.quit {
        terminal.draw(|frame| ui::draw(frame, app))?;

        // A thin bar while inserting, a block everywhere else, and only on the
        // frames where it changes.
        let wanted = if app.mode == app::Mode::EditInsert {
            SetCursorStyle::SteadyBar
        } else {
            SetCursorStyle::SteadyBlock
        };
        if shape != Some(app.mode == app::Mode::EditInsert) {
            execute!(std::io::stdout(), wanted)?;
            shape = Some(app.mode == app::Mode::EditInsert);
        }

        // The visualiser needs frames; idle browsing does not.
        let tick = if app.matrix.on { FRAME } else { TICK };
        if event::poll(tick)?
            && let Event::Key(key) = event::read()?
        {
            keys::handle(app, key);
        }

        app.tick();
    }
    Ok(())
}


/// `vibox update`: reinstall the latest release with
/// `cargo install vibox --force`. Prompts first unless `-y`.
fn cmd_update(yes: bool) -> Result<()> {
    use std::io::Write;

    if !yes {
        print!("Update vibox to the latest release via cargo? [y/N] ");
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    }

    println!("updating via cargo install vibox --force\n");
    match std::process::Command::new("cargo")
        .args(["install", "vibox", "--force"])
        .status()
    {
        Ok(status) if status.success() => {
            println!("\nvibox is up to date");
            Ok(())
        }
        Ok(status) => bail!("update failed (cargo exited {})", status.code().unwrap_or(1)),
        Err(e) => {
            bail!("could not run cargo: {e} - is it installed and on PATH? (https://rustup.rs)")
        }
    }
}
