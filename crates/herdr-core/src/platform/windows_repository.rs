//! Until a contained Windows probe exists, repository facts never spawn Git.

use std::io;
use std::process::{Child, ChildStderr, ChildStdout, Command};

pub fn repository_subprocesses() -> bool {
    false
}

pub fn repository_command() -> Command {
    Command::new("git")
}

pub fn repository_pipes(_: &ChildStdout, _: &ChildStderr) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "repository subprocesses unavailable"))
}

pub fn repository_kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    #[test]
    fn uncontained_git_is_never_started() {
        assert!(!super::repository_subprocesses());
    }
}
