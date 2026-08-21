//! The boot rows are three rows of escape codes rewritten in place, and the
//! cursor arithmetic is the part that breaks silently: one row out and the
//! progress line eats the wordmark, or the wipe leaves half of it behind.

use std::sync::{Arc, Mutex};

use vibox::boot::Boot;
use vibox::library::Scan;

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
    assert_eq!(text.matches('\n').count(), 2, "two wordmark rows, then the progress line");
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
        later.escape_debug().to_string().chars().take(40).collect::<String>()
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
