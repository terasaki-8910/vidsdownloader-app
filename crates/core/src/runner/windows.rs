//! Windows: every run gets a Job Object of its own, with kill-on-close.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::time::Duration;

use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

use super::StartFailure;

/// Exit code of every process in a canceled job.
const CANCELED_EXIT_CODE: u32 = 1;

/// The Job Object holding one run's process tree. Its handle is the only one; closing it
/// when this is dropped ends every process still in the job
/// (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`).
pub(super) struct Group {
    job: OwnedHandle,
}

/// Starts `command` inside a new Job Object, so that the child and everything it starts
/// later can be ended together. Returns the child, its job and its process id.
///
/// The child is created with `CREATE_SUSPENDED`, assigned to the job, and only then
/// resumed, so it cannot run a single instruction, let alone start a process of its own,
/// outside the job. Assigning a normally started child would race with its first
/// `CreateProcess` (yt-dlp.exe starts its PyInstaller child at once), and a grandchild
/// started before the assignment would stay outside the job. Creating the process inside
/// the job directly (`PROC_THREAD_ATTRIBUTE_JOB_LIST`) needs std's unstable
/// `spawn_with_attributes` or a hand-written `CreateProcessW` that redoes std's argument
/// quoting and pipe setup, so the suspended start is used instead. std closes the primary
/// thread's handle after `CreateProcessW`, so the thread is found again by process id in a
/// ToolHelp snapshot; that id cannot be reused meanwhile, because the child is suspended
/// and its process handle is held.
///
/// `CREATE_NO_WINDOW` keeps a console window from appearing.
pub(super) fn spawn(command: &mut Command) -> Result<(Child, Group, u32), StartFailure> {
    let group = Group::new().map_err(StartFailure::Group)?;
    command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    let mut child = command.spawn().map_err(StartFailure::Spawn)?;
    match group.adopt(&child) {
        Ok(id) => Ok((child, group, id)),
        Err(error) => {
            // The child is still suspended and has run nothing. It may not have made it into
            // the job, so it is terminated directly; dropping `group` ends it otherwise.
            let _ = child.start_kill();
            Err(StartFailure::Group(error))
        }
    }
}

impl Group {
    /// Creates an unnamed job that ends its processes when its last handle is closed.
    fn new() -> io::Result<Self> {
        // SAFETY: null security attributes and a null name are valid arguments; they create
        // an unnamed job whose handle is not inherited by child processes.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is a valid handle that was just created and is owned by nothing
        // else; `OwnedHandle` closes it exactly once.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };

        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the job handle is valid, and the pointer and length describe `limits`, a
        // JOBOBJECT_EXTENDED_LIMIT_INFORMATION that lives until the call returns.
        let ok = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limits).cast::<c_void>(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { job })
    }

    /// Puts the suspended `child` into the job, then lets it run. Returns its process id.
    fn adopt(&self, child: &Child) -> io::Result<u32> {
        let (Some(process), Some(id)) = (child.raw_handle(), child.id()) else {
            return Err(io::Error::other("the new process has already been reaped"));
        };
        // SAFETY: both handles are valid for the whole call: the job handle is owned by
        // `self`, the process handle by `child`, which outlives this call.
        if unsafe { AssignProcessToJobObject(self.job.as_raw_handle(), process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        resume_threads(id)?;
        Ok(id)
    }

    /// Ends every process in the job at once; `_grace` is not used on Windows.
    pub(super) async fn cancel(&self, _grace: Duration) {
        // SAFETY: the job handle is valid and owned by `self`.
        let ok = unsafe { TerminateJobObject(self.job.as_raw_handle(), CANCELED_EXIT_CODE) };
        // Not expected to fail: this process created the job with full access. If it did,
        // dropping the group (closing the handle) still ends the job.
        debug_assert_ne!(ok, 0, "TerminateJobObject: {}", io::Error::last_os_error());
    }
}

/// Resumes every thread of process `id`: the single, suspended primary thread of a process
/// created with `CREATE_SUSPENDED`.
fn resume_threads(id: u32) -> io::Result<()> {
    // SAFETY: takes no pointers; the result is checked before use.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a valid snapshot handle that was just created and is owned by nothing
    // else; `OwnedHandle` closes it exactly once.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };

    let entry_size = size_of::<THREADENTRY32>() as u32;
    let mut entry = THREADENTRY32 {
        dwSize: entry_size,
        ..Default::default()
    };
    let mut resumed = 0_usize;
    // SAFETY: the snapshot handle is valid, and `entry` is a writable THREADENTRY32 whose
    // `dwSize` holds its size, as Thread32First requires.
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
    while found {
        if entry.th32OwnerProcessID == id {
            resume_thread(entry.th32ThreadID)?;
            resumed += 1;
        }
        entry.dwSize = entry_size;
        // SAFETY: as for Thread32First above.
        found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    if resumed == 0 {
        return Err(io::Error::other(
            "the new process's thread was not found, so it was not started",
        ));
    }
    Ok(())
}

/// Resumes one thread by id.
fn resume_thread(thread_id: u32) -> io::Result<()> {
    // SAFETY: takes no pointers; the result is checked before use.
    let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` is a valid thread handle that was just opened and is owned by nothing
    // else; `OwnedHandle` closes it exactly once.
    let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: the thread handle is valid and was opened with THREAD_SUSPEND_RESUME access.
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
