//! Work that runs off the input thread: reading a library and writing a `:w`.
//!
//! The thread does the disk work and reports back over a channel; the input
//! thread lands each report in `poll_job`, once per tick, so the screen keeps
//! drawing and the keys keep working however long the work takes. Changes wait
//! until the job lands, since what it is about to apply or replace is exactly
//! what they would change.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::Result;

use crate::library::{self, Scan, Track};

use super::plan::Step;
use super::write::{Done, Event};
use super::*;

/// The work running now, if any. There is only ever one.
pub enum Job {
    Scan(ScanJob),
    Write(WriteJob),
}

/// What a scan is for, which decides what happens when it lands.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScanFor {
    /// `:reload`: the same library, read again.
    Reload,
    /// After `:e!`, which has already said what it did.
    Refresh,
    /// `:e <path>`: another library, for this session.
    Open,
    /// `:set root=`: another library, and the one to open by default.
    Root,
}

pub struct ScanJob {
    root: PathBuf,
    purpose: ScanFor,
    found: Arc<AtomicUsize>,
    read: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    rx: Receiver<Result<Vec<Track>>>,
    started: Instant,
}

pub struct WriteJob {
    pub(super) steps: Vec<Step>,
    pub(super) rx: Receiver<Event>,
    pub(super) done: Done,
    pub(super) problems: Vec<String>,
    /// What went wrong after the files themselves were written, such as a
    /// playlist that could not be repaired.
    pub(super) notes: Vec<String>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) handle: Option<JoinHandle<()>>,
    pub(super) landed: usize,
    pub(super) started: Instant,
}

/// One frame of the spinner every tenth of a second.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl Job {
    /// What the statusline says while it runs, a moving mark first:
    /// `⠹ reading 300/1200`.
    pub fn progress(&self) -> String {
        let started = match self {
            Job::Scan(scan) => scan.started,
            Job::Write(write) => write.started,
        };
        let frame = SPINNER[(started.elapsed().as_millis() / 100) as usize % SPINNER.len()];
        match self {
            Job::Scan(scan) => {
                let total = scan.total.load(Ordering::Relaxed);
                if total > 0 {
                    format!(
                        "{frame} reading {}/{total}",
                        scan.read.load(Ordering::Relaxed)
                    )
                } else {
                    format!("{frame} scanning {}", scan.found.load(Ordering::Relaxed))
                }
            }
            Job::Write(write) => format!("{frame} writing {}/{}", write.landed, write.steps.len()),
        }
    }
}

impl App {
    /// True, and says so, while a job is running: a change has to wait for it,
    /// because a scan is about to replace every row and a write is applying
    /// the changes already queued.
    pub fn busy(&mut self) -> bool {
        let what = match self.job {
            Some(Job::Scan(_)) => "the library is still being read",
            Some(Job::Write(_)) => "`:w` is still writing",
            None => return false,
        };
        self.error(format!("{what}, this waits until it is done"));
        true
    }

    /// Reads `root` on a thread of its own. What happens when it lands is
    /// `purpose`'s; nothing changes before then, so a library that cannot be
    /// read leaves the one open as it was.
    pub fn start_scan(&mut self, root: PathBuf, purpose: ScanFor) {
        if self.busy() {
            return;
        }
        let (found, read, total) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        );
        let (tx, rx) = channel();
        let (f, r, t) = (Arc::clone(&found), Arc::clone(&read), Arc::clone(&total));
        let at = root.clone();
        let spawned = std::thread::Builder::new()
            .name("vibox-scan".into())
            .spawn(move || {
                let report = |scan: Scan| match scan {
                    Scan::Walking(n) => f.store(n, Ordering::Relaxed),
                    Scan::Reading(done, of) => {
                        r.store(done, Ordering::Relaxed);
                        t.store(of, Ordering::Relaxed);
                    }
                };
                let _ = tx.send(library::scan_reporting(&at, &report));
            });
        if let Err(e) = spawned {
            self.error(format!(
                "cannot start reading the library: {}",
                crate::reason::of(&e)
            ));
            return;
        }
        self.job = Some(Job::Scan(ScanJob {
            root,
            purpose,
            found,
            read,
            total,
            rx,
            started: Instant::now(),
        }));
    }

    /// Lands whatever the job has finished. Called every tick.
    pub fn poll_job(&mut self) {
        self.drain_job(false);
    }

    /// Waits for the job and lands all of it, for a caller with no event loop
    /// to land it in: a test, or vibox on its way out.
    pub fn finish_job(&mut self) {
        self.drain_job(true);
    }

    /// Quitting does not wait for a scan, which changes nothing on disk, but a
    /// write is stopped between two steps or two chunks of a copy, so it never
    /// leaves half a file behind, and whatever it had done is landed.
    pub fn stop_jobs(&mut self) {
        match &self.job {
            Some(Job::Write(write)) => {
                write.cancel.store(true, Ordering::Relaxed);
                self.finish_job();
            }
            Some(Job::Scan(_)) => self.job = None,
            None => {}
        }
    }

    fn drain_job(&mut self, wait: bool) {
        let Some(job) = self.job.take() else {
            return;
        };
        match job {
            Job::Scan(scan) => {
                let got = if wait {
                    scan.rx.recv().map_err(|_| TryRecvError::Disconnected)
                } else {
                    scan.rx.try_recv()
                };
                match got {
                    Ok(result) => self.land_scan(&scan, result),
                    Err(TryRecvError::Empty) => self.job = Some(Job::Scan(scan)),
                    Err(TryRecvError::Disconnected) => {
                        self.error("reading the library stopped before it finished");
                    }
                }
            }
            Job::Write(mut write) => {
                loop {
                    let got = if wait {
                        write.rx.recv().map_err(|_| TryRecvError::Disconnected)
                    } else {
                        write.rx.try_recv()
                    };
                    match got {
                        Ok(event) => {
                            if self.write_landed(&mut write, event) {
                                return;
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        // The thread is gone without saying it was done: land
                        // what it did say, which is all that happened.
                        Err(TryRecvError::Disconnected) => {
                            write
                                .problems
                                .push("the write stopped before it finished".into());
                            self.write_finished(&mut write);
                            return;
                        }
                    }
                }
                self.job = Some(Job::Write(write));
            }
        }
    }

    /// A scan has landed: the library it read replaces the one in memory,
    /// with every row kept on its track (`adopt`).
    fn land_scan(&mut self, scan: &ScanJob, result: Result<Vec<Track>>) {
        let tracks = match result {
            Ok(tracks) => tracks,
            Err(e) => {
                let title = match scan.purpose {
                    ScanFor::Reload | ScanFor::Refresh => "cannot reread the library",
                    ScanFor::Open | ScanFor::Root => "library not opened",
                };
                self.fail(title, crate::reason::chain(&e));
                return;
            }
        };
        if matches!(scan.purpose, ScanFor::Open | ScanFor::Root) {
            self.root.clone_from(&scan.root);
            self.folder_cur = 0;
            self.folder_open = 0;
            self.cur = 0;
            self.top = 0;
        }
        self.adopt(tracks);

        let n = self.library_len();
        let tracks = format!("{n} track{}", plural(n));
        let root = self.root.display().to_string();
        match scan.purpose {
            ScanFor::Refresh => {}
            ScanFor::Reload => self.info(tracks),
            ScanFor::Open if n == 0 => self.error(format!("no audio files under `{root}`")),
            ScanFor::Open => self.info(format!("{root}: {tracks}")),
            ScanFor::Root => {
                self.music = Some(self.root.clone());
                self.info(format!("library is {root}, `:mkrc` keeps it"));
            }
        }
    }
}
