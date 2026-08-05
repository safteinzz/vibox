//! `:matrix`. Falling glyphs over the interface, driven by how loud the audio
//! going out right now is.
//!
//! The rain is painted last, so it covers whatever it lands on. Quiet passages
//! barely drip, a drop hits and the whole screen falls.

use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};

/// Latin and digits only: no glyph here needs a font the terminal might lack.
const GLYPHS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789=+*<>[]{}/\\|-_$#@%&";

struct Drop {
    /// Row of the leading glyph, fractional so it can fall slowly.
    head: f32,
    speed: f32,
    len: usize,
}

pub struct Matrix {
    pub on: bool,
    drops: Vec<Drop>,
    rng: u64,
    frame: u64,
}

impl Default for Matrix {
    fn default() -> Self {
        Matrix {
            on: false,
            drops: Vec::new(),
            rng: 0x9E37_79B9_7F4A_7C15,
            frame: 0,
        }
    }
}

impl Matrix {
    fn next_random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn spawn(&mut self, height: usize) -> Drop {
        let a = self.next_random();
        let b = self.next_random();
        Drop {
            // Start above the top so columns do not all appear in a row.
            head: -((a % (height as u64 * 2).max(1)) as f32),
            speed: 0.3 + (b % 100) as f32 / 100.0,
            len: 4 + (a % 14) as usize,
        }
    }

    /// Advances every column. `level` is the current loudness, 0.0 to 1.0.
    fn step(&mut self, width: usize, height: usize, level: f32) {
        while self.drops.len() < width {
            let drop = self.spawn(height);
            self.drops.push(drop);
        }
        self.drops.truncate(width);

        // Nothing playing, nothing falling: it is a visualiser, not a screensaver.
        let push = level * 6.0;
        if push <= f32::EPSILON {
            // The frame counter drives the glyph shuffle too, so leaving it to
            // tick would keep the letters churning on a frozen screen.
            return;
        }

        self.frame = self.frame.wrapping_add(1);
        for i in 0..self.drops.len() {
            self.drops[i].head += self.drops[i].speed * push;
            if self.drops[i].head - self.drops[i].len as f32 > height as f32 {
                self.drops[i] = self.spawn(height);
            }
        }
    }

    /// Glyph for a cell, stable for a while so the rain does not seethe.
    fn glyph(&self, col: usize, row: usize) -> char {
        let era = self.frame / 6;
        let mut h = (col as u64)
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add((row as u64).wrapping_mul(0x85EB_CA6B))
            .wrapping_add(era.wrapping_mul(0xC2B2_AE35));
        h ^= h >> 15;
        GLYPHS[(h % GLYPHS.len() as u64) as usize] as char
    }
}

/// Paints the rain over the interface. It is a secret command, so it gets to
/// be the whole screen while it is on.
pub fn overlay(frame: &mut Frame, matrix: &mut Matrix, level: f32) {
    let area = frame.area();
    let (width, height) = (area.width as usize, area.height as usize);
    if width == 0 || height == 0 {
        return;
    }
    matrix.step(width, height, level);

    // Collect first: painting borrows the frame, and glyph() borrows matrix.
    let mut cells: Vec<(u16, u16, char, Style)> = Vec::new();
    for col in 0..width {
        let drop = &matrix.drops[col];
        for back in 0..drop.len {
            let row = drop.head - back as f32;
            if row < 0.0 || row >= height as f32 {
                continue;
            }
            let row = row as usize;
            // Bright at the head, fading into the dark down the tail.
            let style = match back {
                0 => Style::default()
                    .fg(Color::Indexed(120))
                    .add_modifier(Modifier::BOLD),
                1..=2 => Style::default().fg(Color::Indexed(35)),
                3..=6 => Style::default().fg(Color::Indexed(28)),
                _ => Style::default().fg(Color::Indexed(22)),
            };
            cells.push((col as u16, row as u16, matrix.glyph(col, row), style));
        }
    }

    let buffer = frame.buffer_mut();
    for (x, y, glyph, style) in cells {
        let cell = &mut buffer[(x, y)];
        cell.set_char(glyph);
        cell.set_style(style);
    }
}
