//! Plays the boot rows back, for looking at them.
//!
//! `cargo run --example boot -- 5`
//!
//! A real library almost never takes long enough to see these: the page cache
//! is warm after the first run and the scan beats the grace period, so there
//! is otherwise nowhere to watch the wordmark decode.

use std::time::Duration;

use vibox::boot::Boot;
use vibox::library::Scan;

/// What a middling library looks like, so the counter reads like a real one.
const TRACKS: usize = 2908;
const FRAME: Duration = Duration::from_millis(50);

fn main() {
    let secs: f64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(4.0);

    let root = dirs::audio_dir().unwrap_or_else(|| "/music".into());
    let mut boot = Boot::with_writer(&root, std::io::stdout(), true);
    boot.draw_every_frame();

    // A short walk and then the read, in roughly the proportions a real scan
    // has: the wordmark cannot start resolving until there is a total.
    let frames = ((secs / FRAME.as_secs_f64()).round() as usize).max(2);
    let walking = (frames / 5).max(1);
    for f in 0..=frames {
        if f < walking {
            boot.report(Scan::Walking(TRACKS * f / walking));
        } else {
            boot.report(Scan::Reading(
                TRACKS * (f - walking) / (frames - walking).max(1),
                TRACKS,
            ));
        }
        std::thread::sleep(FRAME);
    }
    boot.finish();
}
