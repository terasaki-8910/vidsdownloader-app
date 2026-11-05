//! Running a child process tree with line-by-line output and cancel that ends the whole tree.
//!
//! [`spawn`] starts a program so that it and every process it starts later belong to one
//! group that is ended together: a Job Object with kill-on-close on Windows, a process
//! group of its own on Unix. yt-dlp's standalone builds are PyInstaller one-file
//! executables that run as a parent and a child process, and ending only the parent leaves
//! the child downloading (SPEC, "Process runner"), so [`RunningProcess::cancel`] and
//! dropping a [`RunningProcess`] always end the whole group.
//!
//! Lines from stdout and stderr arrive on one channel as [`OutputLine`]s tagged with their
//! pipe, in order within each pipe.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::types::{OutputLine, OutputStream};

#[cfg(unix)]
#[allow(unsafe_code)]
mod unix;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

/// Lines buffered between the pipe readers and the consumer. When the buffer is full the
/// pipes are not read, so a child that writes faster than its output is consumed waits on
/// its pipe instead of growing memory.
const LINE_BUFFER: usize = 1024;

/// How long [`RunningProcess::wait`] lets the pipes reach end of file after the direct child
/// exited before it ends whatever is left in the tree. A leftover process that inherited
/// the pipes would otherwise keep `wait`, and the queue slot behind it, from ever returning.
const LEFTOVER_GRACE: Duration = Duration::from_secs(2);

/// A program and its arguments, run directly, never through a shell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunSpec {
    /// The executable to start.
    pub program: PathBuf,
    /// Arguments, each passed as one argv entry.
    pub args: Vec<OsString>,
    /// Working directory; the parent's when `None`.
    pub cwd: Option<PathBuf>,
    /// Variables set on top of the inherited environment.
    pub env: Vec<(OsString, OsString)>,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    /// Exit code of the direct child; `None` when it was ended by a signal (Unix) or its
    /// status could not be read.
    pub code: Option<i32>,
    /// Whether [`RunningProcess::cancel`] was called before the runner saw the direct child
    /// exit.
    pub canceled: bool,
}

/// Why [`spawn`] failed. No process is left running in either case.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// The program could not be started.
    #[error("could not start {}: {source}", .program.display())]
    Spawn {
        program: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The process group could not be set up, so the new process was ended.
    #[error("could not set up the process group for {}: {source}", .program.display())]
    Group {
        program: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Where a platform `spawn` failed; mapped to a [`RunnerError`] by [`spawn`].
enum StartFailure {
    Spawn(io::Error),
    Group(io::Error),
}

/// The direct child's exit, published by the task that waits for it.
#[derive(Debug, Clone, Copy)]
struct ChildExit {
    code: Option<i32>,
}

/// A started process tree. Dropping it ends every process still in the tree.
pub struct RunningProcess {
    id: u32,
    lines: Option<mpsc::Receiver<OutputLine>>,
    exit: watch::Receiver<Option<ChildExit>>,
    readers: Vec<JoinHandle<()>>,
    cancel_requested: AtomicBool,
    group: platform::Group,
}

/// Starts `spec` with stdin null and stdout and stderr piped, in a new process group that
/// also holds every process the child starts later.
///
/// Must be called within a Tokio runtime: the pipes are read, and the child is waited for,
/// by tasks on it.
pub async fn spawn(spec: RunSpec) -> Result<RunningProcess, RunnerError> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .envs(spec.env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }

    let (mut child, group, id) =
        platform::spawn(&mut command).map_err(|failure| match failure {
            StartFailure::Spawn(source) => RunnerError::Spawn {
                program: spec.program.clone(),
                source,
            },
            StartFailure::Group(source) => RunnerError::Group {
                program: spec.program.clone(),
                source,
            },
        })?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        // Not expected: both pipes were requested. Returning drops `group`, which ends the
        // child.
        return Err(RunnerError::Spawn {
            program: spec.program,
            source: io::Error::other("the child's output pipes are missing"),
        });
    };

    let (sender, lines) = mpsc::channel(LINE_BUFFER);
    let readers = vec![
        tokio::spawn(read_lines(stdout, OutputStream::Stdout, sender.clone())),
        tokio::spawn(read_lines(stderr, OutputStream::Stderr, sender)),
    ];
    let (exit_sender, exit) = watch::channel(None);
    tokio::spawn(async move {
        let code = child.wait().await.ok().and_then(|status| status.code());
        exit_sender.send_replace(Some(ChildExit { code }));
    });

    Ok(RunningProcess {
        id,
        lines: Some(lines),
        exit,
        readers,
        cancel_requested: AtomicBool::new(false),
        group,
    })
}

impl RunningProcess {
    /// Process id of the direct child.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Takes the channel of output lines; `None` after the first call.
    ///
    /// The channel closes once both pipes have reached end of file. Keep reading it, or drop
    /// it, while the process runs: when it is full the pipes are not read and the child
    /// waits.
    pub fn take_lines(&mut self) -> Option<mpsc::Receiver<OutputLine>> {
        self.lines.take()
    }

    /// Ends the whole tree and returns once every process in it has been told to end.
    ///
    /// Unix: SIGTERM to the process group, then SIGKILL to whatever is left of it after
    /// `grace`. Windows: the Job Object is terminated at once; `grace` is not used, since the
    /// child runs on a hidden console of its own and there is no Ctrl+C to send it first.
    pub async fn cancel(&self, grace: Duration) {
        if self.exit.borrow().is_none() {
            self.cancel_requested.store(true, Ordering::SeqCst);
        }
        self.group.cancel(grace).await;
    }

    /// Waits until the direct child has exited and both pipes have reached end of file, then
    /// ends anything still left in the tree. If a leftover process still holds a pipe open
    /// [`LEFTOVER_GRACE`] after the direct child exited, the tree is ended first so that the
    /// pipes close.
    ///
    /// Lines that were never taken with [`take_lines`](Self::take_lines) are discarded. If
    /// they were taken, keep reading them meanwhile: a full channel stops the pipe readers
    /// and this never returns.
    pub async fn wait(self) -> ExitInfo {
        let Self {
            lines,
            mut exit,
            mut readers,
            cancel_requested,
            group,
            ..
        } = self;
        // Lines nobody took would fill the channel and stop the readers before end of file.
        drop(lines);
        let code = match exit.wait_for(Option::is_some).await {
            Ok(status) => {
                let exited = *status;
                exited.and_then(|exited| exited.code)
            }
            // The waiting task ended without a status: its runtime is shutting down.
            Err(_) => None,
        };
        let mut group = Some(group);
        let drained = async {
            for reader in &mut readers {
                // A reader that panicked or was cancelled has stopped; there is nothing to add.
                let _ = reader.await;
            }
        };
        tokio::pin!(drained);
        if tokio::time::timeout(LEFTOVER_GRACE, drained.as_mut())
            .await
            .is_err()
        {
            // Something left in the tree still holds a pipe open: end the tree so it closes.
            drop(group.take());
            drained.await;
        }
        let info = ExitInfo {
            code,
            canceled: cancel_requested.load(Ordering::SeqCst),
        };
        drop(group);
        info
    }
}

impl fmt::Debug for RunningProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunningProcess")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Reads `pipe` to end of file and sends each line, decoded by [`decode_line`]; a last line
/// without a terminator is sent too. Once the receiver is gone the pipe is still drained,
/// so the child never blocks on a full pipe.
async fn read_lines<R>(pipe: R, stream: OutputStream, sender: mpsc::Sender<OutputLine>)
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(pipe);
    let mut sender = Some(sender);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let read = reader.read_until(b'\n', &mut buffer).await;
        if !buffer.is_empty()
            && let Some(open) = &sender
        {
            let line = OutputLine {
                stream,
                text: decode_line(&buffer),
            };
            if open.send(line).await.is_err() {
                sender = None;
            }
        }
        match read {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

/// One raw line as text: without its `\n` and a `\r` before it, invalid UTF-8 replaced.
fn decode_line(raw: &[u8]) -> String {
    let line = raw.strip_suffix(b"\n").unwrap_or(raw);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8_lossy(line).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_line_strips_terminators_and_replaces_invalid_utf8() {
        assert_eq!(decode_line(b"plain\n"), "plain");
        assert_eq!(decode_line(b"windows\r\n"), "windows");
        assert_eq!(decode_line(b"no terminator"), "no terminator");
        assert_eq!(decode_line(b"\r\n"), "");
        assert_eq!(decode_line(b"inner\rcarriage\n"), "inner\rcarriage");
        assert_eq!(decode_line(b"bad \xff\xfe\n"), "bad \u{fffd}\u{fffd}");
        assert_eq!(decode_line("日本語\r\n".as_bytes()), "日本語");
    }

    #[tokio::test]
    async fn read_lines_splits_on_newline_and_keeps_an_unterminated_last_line() {
        let (sender, mut receiver) = mpsc::channel(8);
        read_lines(&b"one\r\ntwo\n\nthree"[..], OutputStream::Stderr, sender).await;
        let mut texts = Vec::new();
        while let Some(line) = receiver.recv().await {
            assert_eq!(line.stream, OutputStream::Stderr);
            texts.push(line.text);
        }
        assert_eq!(texts, ["one", "two", "", "three"]);
    }

    #[tokio::test]
    async fn read_lines_drains_the_pipe_after_the_receiver_is_gone() {
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let input = "line\n".repeat(10_000);
        // Returns only once the whole input has been read.
        read_lines(input.as_bytes(), OutputStream::Stdout, sender).await;
    }
}
