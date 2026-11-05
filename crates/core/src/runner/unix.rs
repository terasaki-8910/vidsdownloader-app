//! Unix: every run gets a process group of its own.

use std::ffi::c_int;
use std::io;
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::time::{Instant, sleep};

use super::StartFailure;

/// How often `cancel` checks during the grace period whether the group has ended.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The process group holding one run's process tree. Dropping it sends SIGKILL to the
/// group.
pub(super) struct Group {
    /// The group's id: the direct child's pid, since the child leads the group.
    pgid: libc::pid_t,
}

/// Starts `command` as the leader of a new process group, so that the child and everything
/// it starts later can be ended together. Returns the child, its group and its process id.
///
/// The child joins its group (`setpgid(0, 0)`) before `exec`, so the group exists before
/// the child runs any code and its own children are born into it.
pub(super) fn spawn(command: &mut Command) -> Result<(Child, Group, u32), StartFailure> {
    command.process_group(0);
    let mut child = command.spawn().map_err(StartFailure::Spawn)?;
    // A group id of 0 or 1 would make killpg address this process's own group or init's;
    // a real child's pid is never either, but refuse rather than risk it.
    let ids = child
        .id()
        .and_then(|id| Some((id, libc::pid_t::try_from(id).ok()?)))
        .filter(|&(_, pgid)| pgid > 1);
    match ids {
        Some((id, pgid)) => Ok((child, Group { pgid }, id)),
        None => {
            let _ = child.start_kill();
            Err(StartFailure::Group(io::Error::other(
                "the new process has no usable process id",
            )))
        }
    }
}

impl Group {
    /// Sends `signal` to every process in the group; `Ok(false)` when the group is gone.
    fn signal(&self, signal: c_int) -> io::Result<bool> {
        // SAFETY: killpg takes two integers and touches no memory of this process. `pgid` is
        // greater than 1, so it never addresses this process's own group or init's.
        if unsafe { libc::killpg(self.pgid, signal) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(false)
        } else {
            Err(error)
        }
    }

    /// Whether any process, an unreaped zombie included, is left in the group. A member
    /// that may not be signalled (EPERM) still counts.
    fn is_alive(&self) -> bool {
        !matches!(self.signal(0), Ok(false))
    }

    /// SIGTERM to the group, then SIGKILL to whatever is left of it after `grace`.
    pub(super) async fn cancel(&self, grace: Duration) {
        if matches!(self.signal(libc::SIGTERM), Ok(false)) {
            return;
        }
        let deadline = Instant::now() + grace;
        while self.is_alive() {
            let now = Instant::now();
            if now >= deadline {
                let _ = self.signal(libc::SIGKILL);
                return;
            }
            sleep(POLL_INTERVAL.min(deadline - now)).await;
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // ESRCH (the group is already gone) is the normal case after a finished run.
        let _ = self.signal(libc::SIGKILL);
    }
}
