//! Windows: who serves the engine's named pipe? (audit 2, I2)
//!
//! Named pipes live in one namespace for all users of a machine. If another
//! logged-in user creates `\\.\pipe\docker_engine` (or Podman's pipe)
//! before the engine does, the app would send code to that user's process
//! and trust its answers. Before connecting, the app therefore asks the
//! system for the process serving the pipe (`GetNamedPipeServerProcessId`)
//! and accepts it only if it
//!
//! - runs as the same user as the app (Docker Desktop's backend, Podman's
//!   gvproxy/win-sshproxy and WSL helpers run in the user's session), or
//! - was started from a folder only administrators can write to
//!   (`%ProgramFiles%`, `%ProgramFiles(x86)%`, `%SystemRoot%`; engines
//!   running as a service).
//!
//! A process of another user can usually not even be queried; that counts
//! as "not ours" as well. The decision ([`judge`]) is pure and unit-tested;
//! the system calls exist only on Windows.

use std::path::{Path, PathBuf};

/// What the system says about the process serving a pipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeServer {
    pub pid: u32,
    /// Full image path, `None` if the process could not be queried.
    pub image: Option<PathBuf>,
    /// Same user as this app; `None` if the token could not be read.
    pub same_user: Option<bool>,
}

/// `\\.\pipe\name` for a bollard host `npipe:////./pipe/name`; `None` for
/// other hosts.
pub fn pipe_path(host: &str) -> Option<String> {
    let rest = host.strip_prefix("npipe://")?;
    let path = rest.replace('/', "\\");
    path.to_ascii_lowercase()
        .starts_with("\\\\.\\pipe\\")
        .then_some(path)
}

/// Whether `path` lies inside one of `dirs` (case-insensitive, at a path
/// boundary; Windows paths).
pub fn inside(path: &Path, dirs: &[PathBuf]) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    let path = norm(path);
    dirs.iter().map(|d| norm(d)).any(|d| {
        !d.is_empty()
            && path.len() > d.len()
            && path.starts_with(&d)
            && path.as_bytes()[d.len()] == b'\\'
    })
}

/// Accepts the pipe server or explains in German why not.
pub fn judge(pipe: &str, server: &PipeServer, protected_dirs: &[PathBuf]) -> Result<(), String> {
    if server.same_user == Some(true) {
        return Ok(());
    }
    if let Some(image) = &server.image {
        if inside(image, protected_dirs) {
            return Ok(());
        }
    }
    let what = server.image.as_ref().map_or(
        "ein Prozess, der sich nicht prüfen lässt".to_string(),
        |p| format!("„{}“", p.display()),
    );
    Err(format!(
        "Die Verbindung zur Container-Engine wurde aus Sicherheitsgründen abgelehnt: Die Named \
         Pipe {pipe} gehört weder zu deinem Benutzerkonto noch zu einem installierten Programm \
         ({what}, PID {}). Möglicherweise hat ein anderer angemeldeter Benutzer sie angelegt. \
         Bitte Docker Desktop bzw. Podman neu starten oder den Rechner neu starten.",
        server.pid
    ))
}

/// The folders only administrators can write to (from the environment).
pub fn protected_dirs() -> Vec<PathBuf> {
    [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "SystemRoot",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .map(PathBuf::from)
    .collect()
}

/// Checks the server of the pipe behind `host` (bollard `npipe://` hosts
/// only). `Ok` if it is acceptable or the pipe does not exist (the
/// connection then simply fails).
#[cfg(windows)]
pub fn verify(host: &str) -> Result<(), String> {
    let Some(pipe) = pipe_path(host) else {
        return Ok(());
    };
    match sys::server_of(&pipe)? {
        None => Ok(()),
        Some(server) => judge(&pipe, &server, &protected_dirs()),
    }
}

#[cfg(windows)]
mod sys {
    use std::path::PathBuf;

    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE,
    };
    use windows::Win32::Security::{
        EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE, OPEN_EXISTING,
    };
    use windows::Win32::System::Pipes::{GetNamedPipeServerProcessId, WaitNamedPipeW};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::PipeServer;

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: owned handle, closed once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// The TOKEN_USER of a process token (buffer aligned for the struct).
    fn token_user(process: HANDLE) -> Option<Vec<u64>> {
        let mut token = HANDLE::default();
        // SAFETY: valid process handle, out pointer to a local.
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
        let token = Handle(token);
        let mut len = 0u32;
        // SAFETY: size query with a null buffer.
        let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut len) };
        if len == 0 || len > 4096 {
            return None;
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: the buffer holds `len` bytes.
        unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(buf.as_mut_ptr().cast()),
                len,
                &mut len,
            )
        }
        .ok()?;
        Some(buf)
    }

    fn same_user(process: HANDLE) -> Option<bool> {
        let theirs = token_user(process)?;
        // SAFETY: pseudo handle of the current process.
        let ours = token_user(unsafe { GetCurrentProcess() })?;
        let sid = |buf: &Vec<u64>| {
            // SAFETY: the buffer was filled with a TOKEN_USER by the system.
            unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid }
        };
        // SAFETY: both SIDs point into live buffers.
        Some(unsafe { EqualSid(sid(&theirs), sid(&ours)) }.is_ok())
    }

    fn image(process: HANDLE) -> Option<PathBuf> {
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        // SAFETY: the buffer holds `len` UTF-16 units.
        unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        }
        .ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..len as usize],
        )))
    }

    /// The process serving `pipe`; `None` if the pipe does not exist.
    pub fn server_of(pipe: &str) -> Result<Option<PipeServer>, String> {
        let name = HSTRING::from(pipe);
        let mut busy_rounds = 0;
        let handle = loop {
            // SAFETY: plain client open of a named pipe.
            let opened = unsafe {
                CreateFileW(
                    &name,
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            };
            match opened {
                Ok(h) => break Handle(h),
                Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => return Ok(None),
                Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() && busy_rounds < 3 => {
                    busy_rounds += 1;
                    // SAFETY: waits for a free instance of the named pipe.
                    let _ = unsafe { WaitNamedPipeW(&name, 2_000) };
                }
                Err(e) => {
                    return Err(format!(
                        "Die Named Pipe {pipe} lässt sich nicht prüfen ({}).",
                        e.message()
                    ))
                }
            }
        };
        let mut pid = 0u32;
        // SAFETY: client handle of the pipe, out pointer to a local.
        unsafe { GetNamedPipeServerProcessId(handle.0, &mut pid) }.map_err(|e| {
            format!(
                "Der Prozess hinter der Named Pipe {pipe} lässt sich nicht ermitteln ({}).",
                e.message()
            )
        })?;
        drop(handle);
        // SAFETY: query-only access to the process.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
            .ok()
            .map(Handle);
        Ok(Some(PipeServer {
            pid,
            image: process.as_ref().and_then(|p| image(p.0)),
            same_user: process.as_ref().and_then(|p| same_user(p.0)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> Vec<PathBuf> {
        vec![
            PathBuf::from(r"C:\Program Files"),
            PathBuf::from(r"C:\Program Files (x86)"),
            PathBuf::from(r"C:\WINDOWS"),
        ]
    }

    #[test]
    fn pipe_paths() {
        assert_eq!(
            pipe_path("npipe:////./pipe/docker_engine").as_deref(),
            Some(r"\\.\pipe\docker_engine")
        );
        assert_eq!(
            pipe_path("npipe:////./pipe/podman-machine-default").as_deref(),
            Some(r"\\.\pipe\podman-machine-default")
        );
        assert_eq!(pipe_path("unix:///var/run/docker.sock"), None);
        assert_eq!(pipe_path("npipe:////server/share"), None);
    }

    #[test]
    fn protected_locations() {
        let d = dirs();
        assert!(inside(
            Path::new(r"C:\Program Files\Docker\Docker\resources\com.docker.backend.exe"),
            &d
        ));
        assert!(inside(
            Path::new(r"c:\program files\RedHat\Podman\gvproxy.exe"),
            &d
        ));
        assert!(inside(Path::new(r"C:\Windows\System32\wsl.exe"), &d));
        // Prefix without path boundary, user folders, the folder itself.
        assert!(!inside(Path::new(r"C:\Program FilesX\evil.exe"), &d));
        assert!(!inside(
            Path::new(r"C:\Users\mallory\AppData\Local\pipe.exe"),
            &d
        ));
        assert!(!inside(Path::new(r"C:\Program Files"), &d));
        assert!(!inside(Path::new(r"C:\x.exe"), &[PathBuf::new()]));
    }

    #[test]
    fn who_may_serve_the_pipe() {
        let pipe = r"\\.\pipe\docker_engine";
        let server = |image: Option<&str>, same_user: Option<bool>| PipeServer {
            pid: 4711,
            image: image.map(PathBuf::from),
            same_user,
        };
        // Our own user (Docker Desktop backend, Podman helpers).
        assert!(judge(
            pipe,
            &server(Some(r"C:\Users\me\x.exe"), Some(true)),
            &dirs()
        )
        .is_ok());
        // Another account, but installed by an administrator (service).
        assert!(judge(
            pipe,
            &server(Some(r"C:\Program Files\Docker\dockerd.exe"), Some(false)),
            &dirs()
        )
        .is_ok());
        // Another user's program in their profile, or not inspectable.
        let e = judge(
            pipe,
            &server(Some(r"C:\Users\mallory\squat.exe"), Some(false)),
            &dirs(),
        )
        .unwrap_err();
        assert!(e.contains("aus Sicherheitsgründen abgelehnt"), "{e}");
        assert!(e.contains("squat.exe") && e.contains("4711"), "{e}");
        let e = judge(pipe, &server(None, None), &dirs()).unwrap_err();
        assert!(e.contains("nicht prüfen"), "{e}");
    }
}
