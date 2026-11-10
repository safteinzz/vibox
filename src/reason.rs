//! Why something failed, in words for the message row or for stderr.
//!
//! An `io::Error` prints as the C library's sentence with the errno after it,
//! `No such file or directory (os error 2)`, which is a diagnosis for a
//! programmer. These say the same thing to whoever is looking at the screen.

use std::io::{Error, ErrorKind};

/// One io failure as a lowercase clause, with no errno in it.
pub fn of(e: &Error) -> String {
    let plain = match e.kind() {
        ErrorKind::NotFound => "it is not there",
        ErrorKind::PermissionDenied => "permission denied",
        ErrorKind::AlreadyExists => "something by that name is already there",
        ErrorKind::IsADirectory => "it is a folder",
        ErrorKind::NotADirectory => "part of that path is a file, not a folder",
        ErrorKind::DirectoryNotEmpty => "the folder is not empty",
        ErrorKind::StorageFull => "the disk is full",
        ErrorKind::ReadOnlyFilesystem => "the disk is mounted read only",
        ErrorKind::CrossesDevices => "it is on a different disk",
        ErrorKind::InvalidFilename => "that name is not allowed there",
        ErrorKind::InvalidData => "it is not text vibox can read",
        ErrorKind::ConnectionRefused => "nothing is listening there",
        ErrorKind::TimedOut => "it took too long to answer",
        _ => return without_errno(&e.to_string()),
    };
    plain.to_string()
}

/// An error and everything under it, joined the way a message row reads:
/// `cannot read playlist `x`: it is a folder`.
pub fn chain(e: &anyhow::Error) -> String {
    e.chain()
        .map(|cause| match cause.downcast_ref::<Error>() {
            Some(io) => of(io),
            None => without_errno(&cause.to_string()),
        })
        .collect::<Vec<_>>()
        .join(": ")
}

/// `Broken pipe (os error 32)` as `broken pipe`.
fn without_errno(text: &str) -> String {
    let text = match text.rfind(" (os error ") {
        Some(at) if text.ends_with(')') => &text[..at],
        _ => text,
    };
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}
