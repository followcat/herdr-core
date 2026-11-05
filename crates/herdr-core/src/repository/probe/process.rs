use super::Repository;
use std::io::{self, Read};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

const LIMIT: usize = 1024 * 1024;

pub(super) struct Budget<'a> {
    pub home: &'a Path,
    xdg: &'a Path,
    repo: &'a Repository,
    deadline: Instant,
    remaining: usize,
    pub processes: usize,
}

pub(super) struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
}

impl<'a> Budget<'a> {
    pub fn new(home: &'a Path, xdg: &'a Path, repo: &'a Repository) -> Self {
        Self { home, xdg, repo, deadline: Instant::now() + Duration::from_secs(2), remaining: LIMIT, processes: 0 }
    }

    pub fn run(&mut self, args: &[&str]) -> io::Result<Output> {
        if self.processes >= 8 || Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "repository probe budget exhausted"));
        }
        self.processes += 1;
        let mut command = crate::platform::repository_command();
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.home)
            .env("XDG_CONFIG_HOME", self.xdg)
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_ASKPASS", "false")
            .args([
                "--no-pager",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.sshCommand=false",
                "-c",
                "protocol.allow=never",
            ])
            .arg("-C")
            .arg(&self.repo.root)
            .arg("--git-dir")
            .arg(&self.repo.git_dir)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let result = (|| {
            let mut stdout = child.stdout.take().expect("piped stdout");
            let mut stderr = child.stderr.take().expect("piped stderr");
            crate::platform::repository_pipes(&stdout, &stderr)?;
            let until = self.deadline.min(Instant::now() + Duration::from_secs(1));
            let mut bytes = Vec::new();
            let (mut out_done, mut err_done) = (false, false);
            loop {
                if !out_done {
                    out_done = drain(&mut stdout, Some(&mut bytes), &mut self.remaining)?;
                }
                if !err_done {
                    err_done = drain(&mut stderr, None, &mut self.remaining)?;
                }
                if let Some(status) = child.try_wait()?
                    && out_done
                    && err_done
                {
                    return Ok(Output { status, stdout: bytes });
                }
                if Instant::now() >= until {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "repository subprocess timed out"));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        })();
        if result.is_err() {
            crate::platform::repository_kill(&mut child);
        }
        result
    }
}

fn drain(reader: &mut impl Read, mut output: Option<&mut Vec<u8>>, remaining: &mut usize) -> io::Result<bool> {
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                *remaining = remaining.checked_sub(n).ok_or_else(|| io::Error::other("repository output limit"))?;
                if let Some(out) = &mut output {
                    out.extend_from_slice(&buffer[..n]);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}
