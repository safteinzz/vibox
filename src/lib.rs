//! vibox: a jukebox you exit with `:q`.
//!
//! The library is a directory, the keys are vi's, and the last line of the
//! screen is the command line.
//!
//! Everything lives here rather than in `main.rs` so that `tests/` and
//! `examples/` can reach it: a binary target cannot be imported, and reaching
//! into one through an environment variable is not a way to test it.

pub mod app;
pub mod boot;
pub mod excmd;
pub mod keys;
pub mod library;
pub mod lyrics;
pub mod matrix;
pub mod mpris;
pub mod name;
pub mod player;
pub mod selfcmd;
pub mod ui;
