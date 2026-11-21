//! Child-process spawning that is safe under Android's seccomp filter.
//!
//! `std::process::Command` (and therefore `tokio::process::Command`) calls
//! `pidfd_open()` after `posix_spawn()` on Linux. Android's seccomp-bpf filter
//! does not allow `pidfd_open` for ordinary apps, so the whole API process is
//! killed with `SIGSYS` (`si_code=SYS_SECCOMP`) the moment `/api/core/start`
//! tries to launch sing-box. Observed on Termux with strace:
//!
//! ```text
//! clone(CLONE_VM|CLONE_VFORK|SIGCHLD) = <child>
//! --- SIGSYS { si_syscall=__NR_pidfd_open, si_code=SYS_SECCOMP } ---
//! ```
//!
//! We therefore call musl/bionic's `posix_spawn()` directly and track the
//! child with `waitpid`/`kill`. No `pidfd_open`, no C toolchain, no extra
//! crate — the binary stays a static aarch64-musl ELF.

use std::io;

#[cfg(unix)]
mod imp {
    use super::*;
    use std::ffi::{CString, c_void};
    use std::os::raw::c_char;
    use std::ptr;

    const WNOHANG: i32 = 1;
    const SIGKILL: i32 = 9;

    unsafe extern "C" {
        fn posix_spawn(
            pid: *mut i32,
            path: *const c_char,
            file_actions: *const c_void,
            attrp: *const c_void,
            argv: *const *mut c_char,
            envp: *const *mut c_char,
        ) -> i32;
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
        fn kill(pid: i32, sig: i32) -> i32;
        static mut environ: *const *mut c_char;
    }

    /// A child created through raw `posix_spawn`. Only the PID is tracked; the
    /// process is reaped explicitly with `try_wait`/`kill`.
    pub struct SpawnedChild {
        pid: i32,
    }

    impl SpawnedChild {
        pub fn id(&self) -> Option<u32> {
            Some(self.pid as u32)
        }

        /// Non-blocking reaping. `Ok(None)` means the child is still alive.
        pub fn try_wait(&mut self) -> io::Result<Option<()>> {
            let mut status = 0;
            let rc = unsafe { waitpid(self.pid, &mut status, WNOHANG) };
            if rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    return self.try_wait();
                }
                return Err(err);
            }
            Ok(if rc == 0 { None } else { Some(()) })
        }

        /// Send SIGKILL and reap. Safe to call on an already-exited child.
        pub fn kill(&mut self) -> io::Result<()> {
            unsafe {
                kill(self.pid, SIGKILL);
            }
            let mut status = 0;
            loop {
                let rc = unsafe { waitpid(self.pid, &mut status, 0) };
                if rc >= 0 {
                    return Ok(());
                }
                if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    return Err(io::Error::last_os_error());
                }
            }
        }
    }

    /// Spawn `path` with `args` (excluding argv[0]) and inherit the environment.
    pub fn spawn(path: &str, args: &[&str]) -> io::Result<SpawnedChild> {
        let c_path = CString::new(path)?;

        // Build every CString *before* posix_spawn so no allocation or Rust
        // panic can happen in the vfork child.
        let mut owned: Vec<CString> = Vec::with_capacity(args.len() + 1);
        owned.push(c_path.clone()); // argv[0]
        for arg in args {
            owned.push(CString::new(*arg)?);
        }
        let mut argv: Vec<*mut c_char> = owned.iter().map(|s| s.as_ptr() as *mut c_char).collect();
        argv.push(ptr::null_mut());

        let envp = unsafe { environ };
        let mut pid: i32 = 0;
        let rc = unsafe {
            posix_spawn(
                &mut pid,
                c_path.as_ptr(),
                ptr::null(),
                ptr::null(),
                argv.as_ptr(),
                envp,
            )
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc));
        }
        Ok(SpawnedChild { pid })
    }
}

/// Windows developer builds keep using the standard library; release builds
/// are Linux-only, so this path never ships to Termux.
#[cfg(not(unix))]
mod imp {
    use super::*;
    use std::process::{Child, Command, Stdio};

    pub struct SpawnedChild(Child);

    impl SpawnedChild {
        pub fn id(&self) -> Option<u32> {
            Some(self.0.id())
        }
        pub fn try_wait(&mut self) -> io::Result<Option<()>> {
            Ok(self.0.try_wait()?.map(|_| ()))
        }
        pub fn kill(&mut self) -> io::Result<()> {
            self.0.kill()
        }
    }

    pub fn spawn(path: &str, args: &[&str]) -> io::Result<SpawnedChild> {
        Command::new(path)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(SpawnedChild)
    }
}

pub use imp::{SpawnedChild, spawn};
