//! vibox: a jukebox you exit with `:q`.
//!
//! The library is a directory, the keys are vi's, and the last line of the
//! screen is the command line.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;

use vibox::app::{self, App};
use vibox::boot::Boot;
use vibox::library::SortKey;
use vibox::{excmd, keys, ui};

/// Redraw cadence: fast enough for a moving progress bar, slow enough to idle.
const TICK: Duration = Duration::from_millis(200);
/// Redraw cadence while the visualiser is running.
const FRAME: Duration = Duration::from_millis(50);

const AFTER: &str = concat!(
    "The library is just a directory. `:e <dir>` opens another one, `c` edits the\n\
     filenames in it, and `:help` lists every key.",
    "\n\n",
    env!("CARGO_PKG_REPOSITORY"),
    "\ncontributors: ",
    env!("CARGO_PKG_AUTHORS"),
);

/// `-V` stays a bare version string for scripts; `--version` spells out the
/// license, where it lives, and who's contributed. Every field comes from
/// Cargo.toml, so none of it can drift from the manifest.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n",
    env!("CARGO_PKG_LICENSE"),
    "  ",
    env!("CARGO_PKG_REPOSITORY"),
    "\ncontributors: ",
    env!("CARGO_PKG_AUTHORS"),
);

#[derive(Parser)]
#[command(
    name = "vibox",
    version,
    long_version = LONG_VERSION,
    about,
    after_help = AFTER
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
    /// Manage vibox itself: `self update` reinstalls, `self check` looks for a newer release
    #[command(name = "self", subcommand)]
    Selfie(vibox::selfcmd::Cmd),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Cmd::Selfie(cmd)) = cli.cmd {
        return vibox::selfcmd::run(cmd);
    }

    let Some(sort) = SortKey::parse(&cli.sort) else {
        bail!(
            "bad sort key `{}` - use path, title, artist, album or duration",
            cli.sort
        );
    };

    // A path on the command line is for this session only; with none, the
    // library is whatever `:set root=` put in the rc file.
    let argv_root = cli.path.is_some();
    let root = match cli.path {
        Some(p) => p,
        None => excmd::configured_music()
            .or_else(dirs::audio_dir)
            .unwrap_or(std::env::current_dir()?),
    };

    // ratatui would panic with a raw os error if there is no terminal to take over.
    if !std::io::stdout().is_terminal() {
        bail!("vibox is a full screen player and needs a terminal; it has nothing to pipe");
    }

    let boot = Boot::new(&root);
    let app = App::new(root, sort, &|scan| boot.report(scan));
    boot.finish();

    let mut app = app?;
    excmd::load_state(&mut app);
    excmd::load_rc(&mut app, argv_root);
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


