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
mod name;
mod mpris;
mod player;
mod ui;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
    about,
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

    // A path on the command line is for this session only; with none, the
    // library is whatever `:set root=` put in the rc file.
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
    excmd::load_rc(&mut app);
    let terminal = ratatui::init();
    let result = run(terminal, &mut app);
    ratatui::restore();
    excmd::save_state(&app);
    result
}

/// How long the scan is allowed to take before it starts explaining itself.
const GRACE: Duration = Duration::from_millis(150);
/// How often the progress line is rewritten once it is up.
const REDRAW: Duration = Duration::from_millis(60);
/// Width of the progress bar, in cells.
const BAR: usize = 20;
/// What the rest of the longest progress line costs, so the root can be cut to
/// whatever is left. A line that wraps is a line `\r` can no longer rewrite.
const AROUND: usize = 52;

/// A root as the progress line shows it: `~` for the home directory, and the
/// head dropped once it is still wider than `budget`, since the tail is the
/// part that says which library this is.
fn short(root: &std::path::Path, budget: usize) -> String {
    let full = match dirs::home_dir() {
        Some(home) => match root.strip_prefix(&home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => root.display().to_string(),
        },
        None => root.display().to_string(),
    };

    let len = full.chars().count();
    if len <= budget {
        return full;
    }
    let tail: String = full.chars().skip(len - budget + 1).collect();
    format!("…{tail}")
}

/// The progress line printed while the library is scanned, before the terminal
/// is taken over.
///
/// A big or cold library takes seconds to walk and tag, and a player that
/// prints nothing until it is ready looks hung. Anything that finishes inside
/// `GRACE` prints nothing at all, so a small library still opens in silence.
struct Boot {
    start: Instant,
    /// The root being read, short enough to sit beside the bar on one line.
    root: String,
    /// Locked across the write, so the worker threads cannot interleave halves
    /// of two different lines.
    last: Mutex<Instant>,
    printed: AtomicBool,
    /// Nothing to draw on: the progress line is escape codes, and those belong
    /// in a terminal and not in whatever file stderr was redirected to.
    tty: bool,
}

impl Boot {
    fn new(root: &std::path::Path) -> Boot {
        let now = Instant::now();
        let cols = ratatui::crossterm::terminal::size().map_or(80, |(c, _)| usize::from(c));
        Boot {
            start: now,
            root: short(root, cols.saturating_sub(AROUND).max(12)),
            last: Mutex::new(now),
            printed: AtomicBool::new(false),
            tty: std::io::stderr().is_terminal(),
        }
    }

    fn report(&self, scan: library::Scan) {
        use std::io::Write;

        if !self.tty || self.start.elapsed() < GRACE {
            return;
        }
        // The last file is worth a frame of its own; everything else waits its turn.
        let final_frame = matches!(scan, library::Scan::Reading(done, total) if done == total);
        let Ok(mut last) = self.last.lock() else {
            return;
        };
        if !final_frame && last.elapsed() < REDRAW {
            return;
        }
        *last = Instant::now();

        let root = &self.root;
        let line = match scan {
            library::Scan::Walking(found) => format!("scanning `{root}` ... {found} files"),
            library::Scan::Reading(_, 0) => return,
            library::Scan::Reading(done, total) => {
                let pct = done * 100 / total;
                let full = done * BAR / total;
                let bar: String = (0..BAR).map(|i| if i < full { '#' } else { '-' }).collect();
                format!("reading `{root}` [{bar}] {pct:>3}% ({done}/{total})")
            }
        };
        // \r to the start of the line and \x1b[K to wipe what the longer
        // previous line left behind.
        eprint!("\r\x1b[K{line}");
        std::io::stderr().flush().ok();
        self.printed.store(true, Ordering::Relaxed);
    }

    /// Wipes the progress line so the terminal is as it was found.
    fn finish(&self) {
        use std::io::Write;

        if self.printed.load(Ordering::Relaxed) {
            eprint!("\r\x1b[K");
            std::io::stderr().flush().ok();
        }
    }
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
