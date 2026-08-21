//! The rows printed while the library is scanned, before the terminal is taken
//! over: a braille wordmark decoding above a progress line.
//!
//! Everything here writes into a `Write` rather than straight to stderr, so
//! `tests/boot.rs` can assert the escape codes and `examples/boot.rs` can play
//! the whole thing back without a library to scan.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::library;

/// How long the scan is allowed to take before it starts explaining itself.
const GRACE: Duration = Duration::from_millis(150);
/// How often the progress line is rewritten once it is up.
const REDRAW: Duration = Duration::from_millis(60);
/// Width of the progress bar, in cells.
const BAR: usize = 20;
/// What the rest of the longest progress line costs, so the root can be cut to
/// whatever is left. A line that wraps is a line `\r` can no longer rewrite.
const AROUND: usize = 52;

/// The wordmark, in braille, two rows tall.
///
/// Braille because it is the one block of glyphs a terminal font is expected
/// to have at a fixed width, so this needs no figlet font, no image protocol
/// and no guess about what the user has installed.
const MARK: [&str; 2] = ["⢣⡜ ⡇⠀⣏⣹ ⡎⢱ ⢣⡜", "⠈⡇ ⡇ ⣇⣸ ⢇⡸ ⡜⢣"];
/// Unresolved cells, and the ones that have locked in.
const SCRAMBLED: &str = "\x1b[2;90m";
const LOCKED: &str = "\x1b[1;96m";
const OFF: &str = "\x1b[0m";

/// A root as the progress line and the sidebar show it: `~` for the home
/// directory, and the head dropped once it is still wider than `budget`, since
/// the tail is the part that says which library this is.
pub fn short(root: &std::path::Path, budget: usize) -> String {
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
pub struct Boot<W: Write + Send = std::io::Stderr> {
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
    /// State for the noise in the unresolved cells of the wordmark.
    noise: AtomicU64,
    /// Whether frames are rate limited. Off for a test or an example, which
    /// want every frame they ask for.
    throttle: bool,
    /// Where the rows go. Locked with `last`, so two worker threads cannot
    /// interleave halves of a frame.
    out: Mutex<W>,
}

impl Boot<std::io::Stderr> {
    /// The real one: draws on stderr, and only when that is a terminal.
    pub fn new(root: &std::path::Path) -> Boot<std::io::Stderr> {
        use std::io::IsTerminal;
        let tty = std::io::stderr().is_terminal();
        Boot::with_writer(root, std::io::stderr(), tty)
    }
}

impl<W: Write + Send> Boot<W> {
    /// Draws somewhere else, for a test that wants to read the codes back or
    /// an example that wants them on stdout.
    pub fn with_writer(root: &std::path::Path, out: W, tty: bool) -> Boot<W> {
        let now = Instant::now();
        let cols = ratatui::crossterm::terminal::size().map_or(80, |(c, _)| usize::from(c));
        Boot {
            start: now,
            root: short(root, cols.saturating_sub(AROUND).max(12)),
            last: Mutex::new(now),
            printed: AtomicBool::new(false),
            tty,
            noise: AtomicU64::new(0x2545_F491_4F6C_DD1D),
            throttle: true,
            out: Mutex::new(out),
        }
    }

    /// Draws every frame it is handed, immediately.
    ///
    /// The real thing waits out `GRACE` so a fast library opens in silence,
    /// and then rate limits to `REDRAW`. A test wants neither, and an example
    /// playing back at its own pace wants the frames it asks for.
    pub fn draw_every_frame(&mut self) {
        self.start = Instant::now() - GRACE;
        self.throttle = false;
    }

    /// One braille cell of noise.
    ///
    /// Decorative, so a counter run through a mixer is plenty and beats taking
    /// a dependency to shuffle two rows of glyphs.
    fn scramble(&self) -> char {
        let mut x = self
            .noise
            .fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
        x ^= x >> 33;
        x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        x ^= x >> 33;
        // The low 64 patterns, which is the six dot range the wordmark is
        // drawn in, so the noise looks like the thing it resolves into.
        char::from_u32(0x2800 + u32::try_from(x % 64).unwrap_or(0)).unwrap_or('⠿')
    }

    /// The wordmark with the leftmost `done / total` of its columns locked in.
    ///
    /// Tied to the scan rather than to a clock, so it finishes resolving
    /// exactly when the library finishes loading however long that takes.
    pub fn wordmark(&self, progress: f64) -> String {
        let cols = MARK[0].chars().count();
        let locked = (progress.clamp(0.0, 1.0) * cols as f64).round() as usize;

        let mut out = String::new();
        for row in MARK {
            for (i, ch) in row.chars().enumerate() {
                // A gap is a gap at every stage; scrambling it would make the
                // wordmark wider than it ends up being.
                if ch == ' ' || i < locked {
                    out.push_str(LOCKED);
                    out.push(ch);
                } else {
                    out.push_str(SCRAMBLED);
                    out.push(self.scramble());
                }
                out.push_str(OFF);
            }
            out.push_str("\x1b[K\n");
        }
        out
    }

    pub fn report(&self, scan: library::Scan) {
        if !self.tty || self.start.elapsed() < GRACE {
            return;
        }
        // The last file is worth a frame of its own; everything else waits its turn.
        let final_frame = matches!(scan, library::Scan::Reading(done, total) if done == total);
        let Ok(mut last) = self.last.lock() else {
            return;
        };
        if self.throttle && !final_frame && last.elapsed() < REDRAW {
            return;
        }
        *last = Instant::now();

        let root = &self.root;
        // Walking has no total to count against, so the wordmark sits fully
        // scrambled until the reading phase can say how far along it is.
        let (line, progress) = match scan {
            library::Scan::Walking(found) => (format!("scanning `{root}` ... {found} files"), 0.0),
            library::Scan::Reading(_, 0) => return,
            library::Scan::Reading(done, total) => {
                let pct = done * 100 / total;
                let full = done * BAR / total;
                let bar: String = (0..BAR).map(|i| if i < full { '#' } else { '-' }).collect();
                (
                    format!("reading `{root}` [{bar}] {pct:>3}% ({done}/{total})"),
                    done as f64 / total as f64,
                )
            }
        };

        // Three rows: two of wordmark and the progress line under them. Back up
        // over the wordmark first, so every frame overwrites the last one in
        // place. The progress line stays last because `\r` has to be able to
        // rewrite it.
        let mark = self.wordmark(progress);
        let up = if self.printed.swap(true, Ordering::Relaxed) {
            "\x1b[2A"
        } else {
            ""
        };
        if let Ok(mut out) = self.out.lock() {
            write!(out, "{up}\r{mark}\r\x1b[K{line}").ok();
            out.flush().ok();
        }
    }

    /// Wipes the wordmark and the progress line so the terminal is as it was
    /// found.
    pub fn finish(&self) {
        if self.printed.load(Ordering::Relaxed)
            && let Ok(mut out) = self.out.lock()
        {
            // Up to the first row of the wordmark, then clear everything from
            // there to the bottom of the screen in one go.
            write!(out, "\x1b[2A\r\x1b[J").ok();
            out.flush().ok();
        }
    }
}
