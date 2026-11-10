//! The rows printed while the library is scanned, before the terminal is taken
//! over: a braille wordmark decoding above a progress line.
//!
//! Everything here writes into a `Write` rather than straight to stderr, so the
//! tests at the bottom can assert the escape codes and `examples/boot.rs` can
//! play the whole thing back without a library to scan.

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

/// `line` cut to `cols` cells, with `…` where it was cut.
fn fit(line: &str, cols: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if line.width() <= cols {
        return line.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in line.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w + 1 > cols {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
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
    /// How wide the terminal is. A row that wraps is one the next frame climbs
    /// over one row short, and every frame after that leaves a copy behind.
    cols: usize,
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
            cols,
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

        // One cell short of the edge: a line that fills the last column leaves
        // the cursor waiting to wrap, and some terminals wrap it.
        let line = fit(&line, self.cols.saturating_sub(1));

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

#[cfg(test)]
mod tests {
    //! The boot rows are three rows of escape codes rewritten in place, and the
    //! cursor arithmetic is the part that breaks silently: one row out and the
    //! progress line eats the wordmark, or the wipe leaves half of it behind.

    use std::sync::{Arc, Mutex};

    use super::Boot;
    use crate::library::Scan;

    /// A writer the test can read back.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Recorder {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    fn booted() -> (Boot<Recorder>, Recorder) {
        let out = Recorder::default();
        let mut boot = Boot::with_writer(std::path::Path::new("/music"), out.clone(), true);
        boot.draw_every_frame();
        (boot, out)
    }

    #[test]
    fn the_first_frame_does_not_move_the_cursor_up_into_the_prompt() {
        let (boot, out) = booted();
        boot.report(Scan::Reading(0, 100));

        let text = out.text();
        assert!(
            !text.starts_with("\x1b[2A"),
            "there is nothing above the first frame to go back to"
        );
        assert_eq!(
            text.matches('\n').count(),
            2,
            "two wordmark rows, then the progress line"
        );
    }

    #[test]
    fn every_later_frame_goes_back_over_the_wordmark_first() {
        let (boot, out) = booted();
        boot.report(Scan::Reading(0, 100));
        let first = out.text().len();
        boot.report(Scan::Reading(100, 100));

        let later = &out.text()[first..];
        assert!(
            later.starts_with("\x1b[2A"),
            "a redraw has to step back over the two wordmark rows, got `{}`",
            later
                .escape_debug()
                .to_string()
                .chars()
                .take(40)
                .collect::<String>()
        );
    }

    #[test]
    fn finishing_wipes_every_row_it_drew() {
        let (boot, out) = booted();
        boot.report(Scan::Reading(50, 100));
        let before = out.text().len();
        boot.finish();

        assert_eq!(
            &out.text()[before..],
            "\x1b[2A\r\x1b[J",
            "back to the top row, then clear to the bottom of the screen"
        );
    }

    #[test]
    fn nothing_is_drawn_at_all_when_there_is_no_terminal() {
        let out = Recorder::default();
        let mut boot = Boot::with_writer(std::path::Path::new("/music"), out.clone(), false);
        boot.draw_every_frame();

        boot.report(Scan::Reading(50, 100));
        boot.finish();
        assert!(
            out.text().is_empty(),
            "escape codes belong in a terminal, not in a redirected stderr"
        );
    }

    /// The wordmark is the point: it has to end up spelling the thing.
    #[test]
    fn the_wordmark_resolves_as_the_scan_does() {
        let (boot, _out) = booted();

        let done = boot.wordmark(1.0);
        for row in ["⢣⡜ ⡇⠀⣏⣹ ⡎⢱ ⢣⡜", "⠈⡇ ⡇ ⣇⣸ ⢇⡸ ⡜⢣"] {
            for ch in row.chars() {
                assert!(done.contains(ch), "a finished wordmark is missing `{ch}`");
            }
        }

        // and at the start only the gaps are settled
        let begun = boot.wordmark(0.0);
        assert!(
            !begun.contains('⣹'),
            "nothing but the gaps has locked in yet"
        );
    }
}
