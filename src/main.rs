mod cargo;
mod cli;
mod log;
mod process;
mod watch;

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{ExitCode, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;

use crate::cli::Cli;
use crate::process::{Launch, describe, signal_group, spawn_group};

pub enum Event {
    Changed(Vec<PathBuf>),
    /// The user typed `rs`.
    Restart,
    BuildDone {
        id: u64,
        result: Result<PathBuf, String>,
    },
    Exited {
        id: u64,
        status: ExitStatus,
    },
    Quit,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error(format!("{e:#}"));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let cwd = std::env::current_dir()?.canonicalize()?;
    let watch_paths = if cli.watch.is_empty() {
        vec![cwd.clone()]
    } else {
        cli.watch.clone()
    };
    let watch_paths = watch_paths
        .iter()
        .map(|p| {
            p.canonicalize()
                .with_context(|| format!("cannot find {}", p.display()))
        })
        .collect::<Result<Vec<_>>>()?;

    let (tx, rx) = mpsc::channel();
    let filter = watch::Filter::new(&cwd, &cli.ext, &cli.ignore)?;
    let _watcher = watch::start(&watch_paths, filter, tx.clone())?;
    read_stdin(tx.clone());
    let quit_tx = tx.clone();
    ctrlc::set_handler(move || {
        let _ = quit_tx.send(Event::Quit);
    })?;

    let shown: Vec<_> = watch_paths.iter().map(|p| relative(p, &cwd)).collect();
    let exts = if cli.ext.iter().all(|e| e.is_empty()) {
        "*".into()
    } else {
        cli.ext.join(",")
    };
    log::warn(format!(
        "watch: {} (extensions: {exts}). Type `rs` to restart.",
        shown.join(", ")
    ));

    let mut sup = Supervisor::new(cli, cwd, tx);
    sup.trigger("start".into());
    while !sup.done {
        let event = match sup.next_deadline() {
            Some(deadline) => {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(event) => Some(event),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match rx.recv() {
                Ok(event) => Some(event),
                Err(_) => break,
            },
        };
        match event {
            Some(event) => sup.handle(event),
            None => sup.on_timer(),
        }
    }
    Ok(())
}

fn read_stdin(tx: Sender<Event>) {
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.trim() == "rs" && tx.send(Event::Restart).is_err() {
                break;
            }
        }
    });
}

fn relative(path: &std::path::Path, cwd: &std::path::Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(p) if p.as_os_str().is_empty() => ".".into(),
        Ok(p) => p.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// A process that rwatch started. The ID tells new events from events of old processes.
struct Running {
    id: u64,
    pid: u32,
    started: Instant,
}

struct Supervisor {
    cli: Cli,
    cargo_args: Vec<String>,
    cwd: PathBuf,
    tx: Sender<Event>,
    next_id: u64,
    build: Option<Running>,
    app: Option<Running>,
    /// The time to send SIGKILL to an app that does not stop.
    kill_at: Option<Instant>,
    /// The command to start when the old app stops.
    pending: Option<Launch>,
    changed: Vec<PathBuf>,
    /// The time to restart, if no more changes come before it.
    restart_at: Option<Instant>,
    /// The time when the last restart started, to show the full restart time.
    trigger_time: Instant,
    quitting: bool,
    done: bool,
}

impl Supervisor {
    fn new(cli: Cli, cwd: PathBuf, tx: Sender<Event>) -> Self {
        Self {
            cargo_args: cli.cargo_args(),
            cli,
            cwd,
            tx,
            next_id: 0,
            build: None,
            app: None,
            kill_at: None,
            pending: None,
            changed: Vec::new(),
            restart_at: None,
            trigger_time: Instant::now(),
            quitting: false,
            done: false,
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        [self.restart_at, self.kill_at].into_iter().flatten().min()
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Changed(paths) if !self.quitting => {
                for path in paths {
                    if !self.changed.contains(&path) {
                        self.changed.push(path);
                    }
                }
                self.restart_at = Some(Instant::now() + Duration::from_millis(self.cli.delay));
            }
            Event::Changed(_) => {}
            Event::Restart if !self.quitting => self.trigger("manual restart".into()),
            Event::Restart => {}
            Event::BuildDone { id, result } => self.build_done(id, result),
            Event::Exited { id, status } => self.exited(id, status),
            Event::Quit => self.quit(),
        }
    }

    fn on_timer(&mut self) {
        let now = Instant::now();
        if self.restart_at.is_some_and(|t| t <= now) {
            self.restart_at = None;
            let changed = std::mem::take(&mut self.changed);
            let mut reason = format!("{} changed", relative(&changed[0], &self.cwd));
            if changed.len() > 1 {
                reason += &format!(" (and {} more)", changed.len() - 1);
            }
            self.trigger(reason);
        }
        if self.kill_at.is_some_and(|t| t <= now) {
            self.kill_at = None;
            if let Some(app) = &self.app {
                log::warn(format!(
                    "the app did not stop in {} ms. Send SIGKILL.",
                    self.cli.grace
                ));
                signal_group(app.pid, libc::SIGKILL);
            }
            if self.quitting {
                self.done = true;
            }
        }
    }

    /// Starts a new build, or a new run of the `-x` command.
    fn trigger(&mut self, reason: String) {
        if self.cli.clear {
            log::clear_screen();
        }
        log::warn(format!("restart: {reason}"));
        self.trigger_time = Instant::now();
        match self.cli.exec.clone() {
            Some(command) => self.request_start(Launch {
                argv: vec!["sh".into(), "-c".into(), command.into()],
            }),
            None => self.start_build(),
        }
    }

    fn start_build(&mut self) {
        if let Some(old) = self.build.take() {
            // A new change makes the old build out of date. Stop it to start the new one sooner.
            signal_group(old.pid, libc::SIGTERM);
        }
        let id = self.new_id();
        match cargo::start(id, &self.cargo_args, self.tx.clone()) {
            Ok(pid) => {
                self.build = Some(Running {
                    id,
                    pid,
                    started: Instant::now(),
                })
            }
            Err(e) => log::error(format!("cannot start cargo: {e}")),
        }
    }

    fn build_done(&mut self, id: u64, result: Result<PathBuf, String>) {
        let Some(build) = self.build.take_if(|b| b.id == id) else {
            return;
        };
        match result {
            Ok(exe) => {
                log::info(format!(
                    "build done in {:.2} s",
                    build.started.elapsed().as_secs_f64()
                ));
                let mut argv = vec![exe.into_os_string()];
                argv.extend(self.cli.args.iter().map(Into::into));
                self.request_start(Launch { argv });
            }
            Err(e) if self.app.is_some() => {
                log::error(format!("{e}. The old app continues to run."))
            }
            Err(e) => log::error(format!("{e}. Wait for changes.")),
        }
    }

    /// Starts the command after the current app stops.
    fn request_start(&mut self, launch: Launch) {
        if self.quitting {
            return;
        }
        match &self.app {
            Some(app) => {
                if self.kill_at.is_none() {
                    signal_group(app.pid, libc::SIGTERM);
                    self.kill_at = Some(Instant::now() + Duration::from_millis(self.cli.grace));
                }
                self.pending = Some(launch);
            }
            None => self.start(launch),
        }
    }

    fn start(&mut self, launch: Launch) {
        let mut cmd = launch.command();
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        let mut child = match spawn_group(&mut cmd) {
            Ok(child) => child,
            Err(e) => {
                log::error(format!("cannot start {}: {e}", launch.name()));
                return;
            }
        };
        let id = self.new_id();
        let tx = self.tx.clone();
        let pid = child.id();
        thread::spawn(move || {
            if let Ok(status) = child.wait() {
                let _ = tx.send(Event::Exited { id, status });
            }
        });
        self.app = Some(Running {
            id,
            pid,
            started: Instant::now(),
        });
        let name = relative(launch.argv[0].as_ref(), &self.cwd);
        log::info(format!(
            "start: {name} (pid {pid}, {} ms after the change)",
            self.trigger_time.elapsed().as_millis()
        ));
    }

    fn exited(&mut self, id: u64, status: ExitStatus) {
        let Some(app) = self.app.take_if(|a| a.id == id) else {
            return;
        };
        self.kill_at = None;
        // Stop child processes that the app left behind. They can hold a port that the
        // new app needs.
        signal_group(app.pid, libc::SIGKILL);
        if self.quitting {
            self.done = true;
            return;
        }
        if let Some(launch) = self.pending.take() {
            self.start(launch);
            return;
        }
        let ran = app.started.elapsed().as_secs_f64();
        if status.success() {
            log::info(format!(
                "the app stopped ({}) after {ran:.1} s. Wait for changes.",
                describe(status)
            ));
        } else {
            log::error(format!(
                "the app stopped ({}) after {ran:.1} s. Wait for changes",
                describe(status)
            ));
        }
    }

    fn quit(&mut self) {
        if self.quitting {
            // A second Ctrl-C stops everything now.
            if let Some(app) = &self.app {
                signal_group(app.pid, libc::SIGKILL);
            }
            self.done = true;
            return;
        }
        self.quitting = true;
        self.restart_at = None;
        if let Some(build) = self.build.take() {
            signal_group(build.pid, libc::SIGTERM);
        }
        match &self.app {
            Some(app) => {
                signal_group(app.pid, libc::SIGTERM);
                self.kill_at = Some(Instant::now() + Duration::from_millis(self.cli.grace));
            }
            None => self.done = true,
        }
    }

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}
