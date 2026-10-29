//! Limits and lifetime of child processes: the document worker
//! (`docs::worker`) and llama-server (`ai::llama`).
//!
//! - Windows: a job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (the
//!   child dies when the app's last handle to the job closes, also when the
//!   app crashes or is killed in Task Manager) and optionally
//!   `JOB_OBJECT_LIMIT_PROCESS_MEMORY` (allocations beyond the limit fail).
//! - macOS/Linux: `setrlimit` in the child itself (see
//!   [`limit_current_process`]); RLIMIT_AS/RLIMIT_DATA are not reliably
//!   enforced on macOS, so the parent additionally watches the child's
//!   resident memory ([`resident_bytes`]).

use std::time::Duration;

/// Resident memory of process `pid` in bytes, `None` if it is gone.
pub fn resident_bytes(system: &mut sysinfo::System, pid: u32) -> Option<u64> {
    let pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_memory(),
    );
    system.process(pid).map(|p| p.memory())
}

/// How often the parent samples the child's memory.
pub const MEMORY_POLL: Duration = Duration::from_millis(100);

/// Limits for the current process (called by a child before it touches
/// untrusted data): address space / data segment, CPU time, no core dumps.
/// Best effort: a limit the system refuses is skipped.
#[cfg(unix)]
pub fn limit_current_process(memory_bytes: u64, cpu_seconds: u64) {
    fn set(resource: libc::c_int, value: u64) {
        let limit = libc::rlimit {
            rlim_cur: value as libc::rlim_t,
            rlim_max: value as libc::rlim_t,
        };
        // SAFETY: plain syscall with a valid pointer to an initialized struct.
        unsafe {
            libc::setrlimit(resource as _, &limit);
        }
    }
    // Linux counts private writable mappings (heap, anonymous mmap, stacks)
    // in RLIMIT_DATA. The address space also contains the shared libraries
    // the app links (WebKitGTK etc.), hence the generous extra room.
    set(libc::RLIMIT_DATA as libc::c_int, memory_bytes);
    #[cfg(target_os = "linux")]
    set(
        libc::RLIMIT_AS as libc::c_int,
        memory_bytes.saturating_add(ADDRESS_SPACE_SLACK),
    );
    set(libc::RLIMIT_CPU as libc::c_int, cpu_seconds.max(1));
    set(libc::RLIMIT_CORE as libc::c_int, 0);
}

/// Address space beyond the memory limit for mapped libraries and thread
/// stacks (Linux, see [`limit_current_process`]).
#[cfg(target_os = "linux")]
pub const ADDRESS_SPACE_SLACK: u64 = 2 * 1024 * 1024 * 1024;

#[cfg(windows)]
pub use job::KillJob;

#[cfg(windows)]
mod job {
    use std::os::windows::io::RawHandle;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };

    /// A job object whose processes are killed when it is dropped (or when
    /// the app exits in any way, because the system closes the handle).
    pub struct KillJob(HANDLE);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for KillJob {}
    unsafe impl Sync for KillJob {}

    impl KillJob {
        /// `memory_limit`: committed memory per process in bytes. Unhandled
        /// exceptions end the process without the error reporting dialog.
        pub fn new(memory_limit: Option<u64>) -> windows::core::Result<Self> {
            // SAFETY: no security attributes, no name; the handle is owned
            // by the returned value.
            let job = KillJob(unsafe { CreateJobObjectW(None, PCWSTR::null())? });
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            if let Some(limit) = memory_limit {
                info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.ProcessMemoryLimit = usize::try_from(limit).unwrap_or(usize::MAX);
            }
            // SAFETY: `info` is a valid JOBOBJECT_EXTENDED_LIMIT_INFORMATION
            // of the given size.
            unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )?;
            }
            Ok(job)
        }

        /// Puts a (just started) process into the job.
        pub fn assign(&self, process: RawHandle) -> windows::core::Result<()> {
            // SAFETY: `process` is a live process handle owned by the
            // caller's `Child`.
            unsafe { AssignProcessToJobObject(self.0, HANDLE(process)) }
        }
    }

    impl Drop for KillJob {
        fn drop(&mut self) {
            // SAFETY: the handle is owned and closed exactly once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_memory_of_this_process() {
        let mut system = sysinfo::System::new();
        let rss = resident_bytes(&mut system, std::process::id()).unwrap();
        assert!(rss > 1024 * 1024, "{rss}");
        // A pid that does not exist (beyond the usual pid range).
        assert!(resident_bytes(&mut system, u32::MAX - 7).is_none());
    }
}
