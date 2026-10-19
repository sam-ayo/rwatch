use std::ffi::OsString;
use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus, Stdio};

/// A command to start: the program and its arguments.
#[derive(Clone, Debug)]
pub struct Launch {
    pub argv: Vec<OsString>,
}

impl Launch {
    pub fn name(&self) -> String {
        self.argv.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ")
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.argv[0]);
        cmd.args(&self.argv[1..]);
        cmd
    }
}

/// Starts the command in a new session, which also makes a new process group. Then one
/// signal stops the process and all its children. The terminal also does not send Ctrl-C
/// to the group, so rwatch controls how the process stops.
///
/// A new process group only would stay on the terminal as a background group. A child
/// that takes the terminal, for example `zsh -i`, then stops forever. A new session has
/// no controlling terminal, so this cannot occur.
pub fn spawn_group(cmd: &mut Command) -> io::Result<Child> {
    // SAFETY: setsid is async-signal-safe, so it is safe to call between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.stdin(Stdio::null()).spawn()
}

pub fn signal_group(pid: u32, signal: i32) {
    // SAFETY: killpg has no memory effects. An error only means that the group is gone.
    unsafe {
        libc::killpg(pid as libc::pid_t, signal);
    }
}

pub fn describe(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "unknown status".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_the_child_in_a_new_session_and_group() {
        let mut child = spawn_group(Command::new("sleep").arg("5")).unwrap();
        let pid = child.id() as libc::pid_t;
        // SAFETY: getsid and getpgid only read the IDs of the process.
        let (sid, pgid) = unsafe { (libc::getsid(pid), libc::getpgid(pid)) };
        signal_group(child.id(), libc::SIGKILL);
        let _ = child.wait();
        assert_eq!((sid, pgid), (pid, pid));
    }
}
