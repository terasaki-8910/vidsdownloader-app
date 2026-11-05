//! `vids_core::runner` against real processes: output lines and exit codes, and ending
//! whole process trees (SPEC Q2).
//!
//! Tree tests run this test binary again as a helper process: `helper_process` turns into a
//! parent that starts a child copy of itself when `VIDS_RUNNER_TEST_ROLE` is set.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{Instant, sleep, timeout};
use vids_core::runner::{self, ExitInfo, RunSpec, RunnerError, RunningProcess};
use vids_core::types::{OutputLine, OutputStream};

/// Environment variable that turns this binary into a helper process.
const ROLE: &str = "VIDS_RUNNER_TEST_ROLE";
/// The test that is the helper's entry point.
const HELPER: &str = "helper_process";
/// Upper bound for a helper to start and report, guarding against hangs only.
const HANG: Duration = Duration::from_secs(30);
/// Q2: no process of an ended tree may be alive this long after it was ended.
const GONE_WITHIN: Duration = Duration::from_secs(3);
/// Grace period passed to `cancel`.
const GRACE: Duration = Duration::from_secs(2);

/// Entry point of helper processes, not a test of its own: without the role variable it
/// returns at once.
#[test]
fn helper_process() {
    let Some(role) = std::env::var_os(ROLE) else {
        return;
    };
    match role.to_str() {
        Some("tree-parent") => tree_parent(),
        Some("orphan-parent") => orphan_parent(),
        Some("tree-child") => tree_child(),
        Some("streams") => streams(),
        Some("many-lines") => many_lines(),
        _ => panic!("unknown helper role {role:?}"),
    }
}

/// Arguments that make this binary run only `helper_process`, printing straight to its
/// pipes, with nothing of the harness's own on the same lines.
fn helper_args() -> Vec<OsString> {
    [
        "--exact",
        HELPER,
        "--nocapture",
        "--quiet",
        "--test-threads=1",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

fn helper_spec(role: &str) -> RunSpec {
    RunSpec {
        program: std::env::current_exe().expect("path of this test binary"),
        args: helper_args(),
        cwd: None,
        env: vec![(ROLE.into(), role.into())],
    }
}

/// Starts a child copy of itself and prints both process ids. Then, like the PyInstaller
/// bootloader of yt-dlp.exe, it waits for its child, which sleeps for 60 s.
fn tree_parent() {
    let mut child = std::process::Command::new(std::env::current_exe().expect("own path"))
        .args(helper_args())
        .env(ROLE, "tree-child")
        .stdin(Stdio::null())
        .spawn()
        .expect("start the child copy");
    let mut out = std::io::stdout().lock();
    writeln!(out, "VDL-PARENT {}", std::process::id()).expect("write");
    writeln!(out, "VDL-CHILD {}", child.id()).expect("write");
    out.flush().expect("flush");
    drop(out);
    child.wait().expect("wait for the child copy");
}

/// Starts a child copy of itself that inherits its stdout and stderr, prints both ids, and
/// exits at once, leaving the child (sleeping 60 s) with the pipes still open.
#[allow(
    clippy::zombie_processes,
    reason = "the point is a process that outlives its parent; the runner ends it"
)]
fn orphan_parent() {
    let child = std::process::Command::new(std::env::current_exe().expect("own path"))
        .args(helper_args())
        .env(ROLE, "tree-child")
        .stdin(Stdio::null())
        .spawn()
        .expect("start the child copy");
    let mut out = std::io::stdout().lock();
    writeln!(out, "VDL-PARENT {}", std::process::id()).expect("write");
    writeln!(out, "VDL-CHILD {}", child.id()).expect("write");
    out.flush().expect("flush");
}

/// Confirms it runs, then sleeps.
fn tree_child() {
    let mut out = std::io::stdout().lock();
    writeln!(out, "VDL-READY {}", std::process::id()).expect("write");
    out.flush().expect("flush");
    drop(out);
    std::thread::sleep(Duration::from_secs(60));
}

/// Writes exact bytes to both pipes, then exits with code 7.
fn streams() {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    out.write_all(b"VDL-BEGIN\n").expect("write");
    err.write_all(b"VDL-BEGIN\n").expect("write");
    out.write_all(b"out one\r\n").expect("write");
    err.write_all(b"err one\r\n").expect("write");
    out.write_all(b"bad \xff\xfe bytes\n").expect("write");
    out.write_all(b"\r\n").expect("write");
    out.write_all(b"last without newline").expect("write");
    out.flush().expect("flush");
    err.flush().expect("flush");
    std::process::exit(7);
}

/// Writes far more lines than the runner buffers, then exits with code 0.
fn many_lines() {
    let mut out = std::io::stdout().lock();
    for n in 0..5000 {
        writeln!(out, "VDL-LINE {n}").expect("write");
    }
    out.flush().expect("flush");
    std::process::exit(0);
}

/// Starts the helper tree and reads its ids: (direct child, grandchild).
async fn start_tree() -> (RunningProcess, mpsc::Receiver<OutputLine>, u32, u32) {
    let mut process = runner::spawn(helper_spec("tree-parent"))
        .await
        .expect("start the helper");
    let mut lines = process.take_lines().expect("lines are taken once");
    let (parent, child) = read_tree_ids(&mut lines).await;
    assert_eq!(
        process.id(),
        parent,
        "the helper parent is the direct child"
    );
    (process, lines, parent, child)
}

/// Reads lines until the parent has printed both ids and the child has confirmed it runs.
async fn read_tree_ids(lines: &mut mpsc::Receiver<OutputLine>) -> (u32, u32) {
    let (mut parent, mut child, mut ready) = (None, None, None);
    let mut other = Vec::new();
    let read = async {
        while parent.is_none() || child.is_none() || ready.is_none() {
            let Some(line) = lines.recv().await else {
                return false;
            };
            let id = line
                .text
                .split_once(' ')
                .and_then(|(tag, id)| Some((tag, id.trim().parse::<u32>().ok()?)));
            match id {
                Some(("VDL-PARENT", id)) => parent = Some(id),
                Some(("VDL-CHILD", id)) => child = Some(id),
                Some(("VDL-READY", id)) => ready = Some(id),
                _ => other.push(line.text),
            }
        }
        true
    };
    let complete = timeout(HANG, read).await.unwrap_or(false);
    assert!(
        complete,
        "the helper tree did not report its ids; other output: {other:?}"
    );
    assert_eq!(child, ready, "the child reports the id its parent printed");
    (parent.expect("parent id"), child.expect("child id"))
}

/// Polls until none of `pids` is alive; fails if one still is at `deadline`.
async fn assert_gone_by(pids: &[u32], deadline: Instant) {
    loop {
        let alive: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|&pid| os::process_alive(pid))
            .collect();
        if alive.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "still alive at the deadline: {alive:?}"
        );
        sleep(Duration::from_millis(25)).await;
    }
}

/// Every line until both pipes reach end of file.
async fn collect(mut lines: mpsc::Receiver<OutputLine>) -> Vec<OutputLine> {
    let all = async {
        let mut all = Vec::new();
        while let Some(line) = lines.recv().await {
            all.push(line);
        }
        all
    };
    timeout(HANG, all).await.expect("output ends")
}

fn texts(lines: &[OutputLine], stream: OutputStream) -> Vec<&str> {
    lines
        .iter()
        .filter(|line| line.stream == stream)
        .map(|line| line.text.as_str())
        .collect()
}

/// The lines after the helper's `VDL-BEGIN` marker; the test harness prints its own lines
/// before it.
fn after_begin(texts: Vec<&str>) -> Vec<&str> {
    let begin = texts
        .iter()
        .position(|text| *text == "VDL-BEGIN")
        .expect("the helper prints VDL-BEGIN");
    texts[begin + 1..].to_vec()
}

#[tokio::test]
async fn wait_ends_a_leftover_that_keeps_the_pipes_open() {
    let mut process = runner::spawn(helper_spec("orphan-parent"))
        .await
        .expect("start the helper");
    let mut lines = process.take_lines().expect("lines are taken once");
    let (parent, child) = read_tree_ids(&mut lines).await;
    let _pins = os::pin(&[parent, child]);
    let reader = tokio::spawn(collect(lines));

    // The direct child exits at once; its own child keeps the inherited pipes open.
    let started = Instant::now();
    let exit = timeout(HANG, process.wait())
        .await
        .expect("wait returns although a leftover holds the pipes");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "wait took {:?}",
        started.elapsed()
    );
    assert!(!exit.canceled);
    assert_eq!(exit.code, Some(0));
    assert_gone_by(&[child], Instant::now() + GONE_WITHIN).await;
    reader.await.expect("the output reader finishes");
}

#[tokio::test]
async fn cancel_ends_the_whole_tree() {
    let (process, lines, parent, child) = start_tree().await;
    let _pins = os::pin(&[parent, child]);
    assert!(os::process_alive(parent), "the parent runs before cancel");
    assert!(os::process_alive(child), "the child runs before cancel");

    let canceled_at = Instant::now();
    process.cancel(GRACE).await;
    assert_gone_by(&[parent, child], canceled_at + GONE_WITHIN).await;

    drop(lines);
    let exit = timeout(HANG, process.wait())
        .await
        .expect("wait returns once the tree is gone");
    assert!(exit.canceled);
    assert_ne!(exit.code, Some(0));
}

#[tokio::test]
async fn ending_only_the_direct_child_leaves_the_grandchild_but_cancel_ends_it() {
    let (process, lines, parent, child) = start_tree().await;
    let _pins = os::pin(&[parent, child]);

    // What cancel must not be: ending only the direct child, as M1 saw with yt-dlp.exe.
    os::kill_only(parent);
    assert_gone_by(&[parent], Instant::now() + GONE_WITHIN).await;
    assert!(
        os::process_alive(child),
        "the grandchild outlives its parent unless the whole tree is ended"
    );

    let canceled_at = Instant::now();
    process.cancel(GRACE).await;
    assert_gone_by(&[child], canceled_at + GONE_WITHIN).await;

    drop(lines);
    timeout(HANG, process.wait())
        .await
        .expect("wait returns once the tree is gone");
}

#[tokio::test]
async fn dropping_the_process_ends_the_whole_tree() {
    let (process, lines, parent, child) = start_tree().await;
    let _pins = os::pin(&[parent, child]);
    assert!(os::process_alive(parent) && os::process_alive(child));

    let dropped_at = Instant::now();
    drop(process);
    assert_gone_by(&[parent, child], dropped_at + GONE_WITHIN).await;
    drop(lines);
}

#[tokio::test]
async fn lines_are_tagged_with_their_stream_and_the_exit_code_is_reported() {
    let mut process = runner::spawn(shell_spec()).await.expect("start the shell");
    let lines = collect(process.take_lines().expect("lines")).await;
    let exit = process.wait().await;

    assert_eq!(texts(&lines, OutputStream::Stdout), ["out1", "out2"]);
    assert_eq!(texts(&lines, OutputStream::Stderr), ["err1"]);
    assert_eq!(
        exit,
        ExitInfo {
            code: Some(3),
            canceled: false
        }
    );
}

/// A shell that prints two lines to stdout and one to stderr, all ending in "\r\n" or
/// "\n", and exits with code 3.
#[cfg(windows)]
fn shell_spec() -> RunSpec {
    let program = std::env::var_os("ComSpec").map_or_else(
        || PathBuf::from(r"C:\Windows\System32\cmd.exe"),
        PathBuf::from,
    );
    RunSpec {
        program,
        args: ["/d", "/c", "echo out1&(echo err1)1>&2&echo out2&exit /b 3"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        ..RunSpec::default()
    }
}

#[cfg(unix)]
fn shell_spec() -> RunSpec {
    RunSpec {
        program: PathBuf::from("/bin/sh"),
        args: [
            "-c",
            r"printf 'out1\r\n'; printf 'err1\r\n' >&2; printf 'out2\n'; exit 3",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
        ..RunSpec::default()
    }
}

#[tokio::test]
async fn crlf_invalid_utf8_and_an_unterminated_last_line_are_delivered() {
    let mut process = runner::spawn(helper_spec("streams"))
        .await
        .expect("start the helper");
    let lines = collect(process.take_lines().expect("lines")).await;
    let exit = process.wait().await;

    assert_eq!(
        after_begin(texts(&lines, OutputStream::Stdout)),
        [
            "out one",
            "bad \u{fffd}\u{fffd} bytes",
            "",
            "last without newline"
        ]
    );
    assert_eq!(
        after_begin(texts(&lines, OutputStream::Stderr)),
        ["err one"]
    );
    assert_eq!(
        exit,
        ExitInfo {
            code: Some(7),
            canceled: false
        }
    );
}

#[tokio::test]
async fn wait_without_taking_the_lines_does_not_stall() {
    let process = runner::spawn(helper_spec("many-lines"))
        .await
        .expect("start the helper");
    let exit = timeout(HANG, process.wait())
        .await
        .expect("wait finishes although nobody read the lines");
    assert_eq!(
        exit,
        ExitInfo {
            code: Some(0),
            canceled: false
        }
    );
}

#[tokio::test]
async fn a_missing_program_is_a_spawn_error() {
    let spec = RunSpec {
        program: std::env::temp_dir().join("vids-runner-test-no-such-program"),
        ..RunSpec::default()
    };
    let error = runner::spawn(spec).await.expect_err("nothing to start");
    assert!(matches!(error, RunnerError::Spawn { .. }), "{error:?}");
}

#[cfg(windows)]
mod os {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        TerminateProcess,
    };

    /// Handles that keep the processes' ids from being reused while a test checks them.
    pub struct Pins {
        _handles: Vec<OwnedHandle>,
    }

    fn open(pid: u32, access: u32) -> Option<OwnedHandle> {
        // SAFETY: takes no pointers; a null result means failure.
        let raw = unsafe { OpenProcess(access, 0, pid) };
        if raw.is_null() {
            return None;
        }
        // SAFETY: a non-null result is a new handle owned by nothing else.
        Some(unsafe { OwnedHandle::from_raw_handle(raw) })
    }

    pub fn pin(pids: &[u32]) -> Pins {
        Pins {
            _handles: pids
                .iter()
                .map(|&pid| open(pid, PROCESS_QUERY_LIMITED_INFORMATION).expect("open a helper"))
                .collect(),
        }
    }

    /// OpenProcess + GetExitCodeProcess == STILL_ACTIVE.
    pub fn process_alive(pid: u32) -> bool {
        let Some(process) = open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
            // SAFETY: reads the calling thread's last error, set by OpenProcess.
            return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
        };
        let mut code = 0_u32;
        // SAFETY: the handle is valid and `code` is a writable u32.
        let ok = unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) };
        ok != 0 && code == STILL_ACTIVE as u32
    }

    /// Ends one process, without its children.
    pub fn kill_only(pid: u32) {
        let process = open(pid, PROCESS_TERMINATE).expect("open the helper parent");
        // SAFETY: the handle is valid and has PROCESS_TERMINATE access.
        let ok = unsafe { TerminateProcess(process.as_raw_handle(), 9) };
        assert_ne!(
            ok,
            0,
            "TerminateProcess: {}",
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(unix)]
mod os {
    /// Nothing to hold on Unix: process ids are not reused this quickly.
    pub struct Pins;

    pub fn pin(_pids: &[u32]) -> Pins {
        Pins
    }

    fn pid_t(pid: u32) -> libc::pid_t {
        libc::pid_t::try_from(pid).expect("a process id fits pid_t")
    }

    /// kill(pid, 0) succeeds (or fails with EPERM) while the process exists. On Linux a
    /// zombie that has not been reaped yet counts as gone.
    pub fn process_alive(pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        if is_zombie(pid) {
            return false;
        }
        // SAFETY: signal 0 only checks that the process exists; no memory is shared.
        let rc = unsafe { libc::kill(pid_t(pid), 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(target_os = "linux")]
    fn is_zombie(pid: u32) -> bool {
        // "/proc/<pid>/stat" is "<pid> (<comm>) <state> ..."; the state follows the last ')'.
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                let (_, rest) = stat.rsplit_once(')')?;
                Some(rest.trim_start().starts_with('Z'))
            })
            .unwrap_or(false)
    }

    /// Ends one process, without its children.
    pub fn kill_only(pid: u32) {
        // SAFETY: delivers a signal to one process id; no memory is shared.
        let rc = unsafe { libc::kill(pid_t(pid), libc::SIGKILL) };
        assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
    }
}
