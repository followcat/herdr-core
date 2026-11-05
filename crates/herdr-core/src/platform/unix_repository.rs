//! Resource containment for the repository service's allowlisted Git commands.

use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStderr, ChildStdout, Command};

pub fn repository_subprocesses() -> bool {
    true
}

pub fn repository_command() -> Command {
    let mut command = Command::new("git");
    command.process_group(0);
    unsafe {
        command.pre_exec(|| {
            for (resource, value) in
                [(libc::RLIMIT_AS, 512 * 1024 * 1024), (libc::RLIMIT_CPU, 2), (libc::RLIMIT_CORE, 0)]
            {
                let limit = libc::rlimit { rlim_cur: value, rlim_max: value };
                if libc::setrlimit(resource, &limit) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    command
}

pub fn repository_pipes(stdout: &ChildStdout, stderr: &ChildStderr) -> io::Result<()> {
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

pub fn repository_kill(child: &mut Child) {
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}
