//! OS process metadata: readable facts, positively gone, or an unknown read.

use crate::foreground::Foreground;
use std::io;

/// Positive Unix observations distinguish a missing process from unavailable
/// metadata. Windows retains its established metadata/absence behaviour.
#[cfg(target_os = "linux")]
pub fn observe_foreground(pid: i32) -> io::Result<Option<Foreground>> {
    let root = std::path::PathBuf::from(format!("/proc/{pid}"));
    let read = || -> io::Result<Foreground> {
        let name = std::fs::read_to_string(root.join("comm"))?;
        let cmdline = std::fs::read(root.join("cmdline"))?;
        // A partial proc read is not evidence that the previous owner left.
        if !name.ends_with('\n') || name.trim_end().is_empty() || cmdline.last() != Some(&0) {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "incomplete process metadata"));
        }
        let argv = cmdline
            .split(|b| *b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        let cwd = std::fs::read_link(root.join("cwd"))?.display().to_string();
        let mut fg = Foreground { pid, name: name.trim_end().into(), argv, cwd, started: None, agent: None };
        fg.agent = crate::detect::identify(&fg);
        Ok(fg)
    };
    match read_linux_foreground(pid, || std::fs::read_to_string(root.join("stat")), read) {
        Ok(fg) => Ok(fg),
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(None),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && std::fs::metadata(&root).is_err_and(|e| e.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// The validation seam keeps proc reads separate from incarnation decisions.
#[cfg(target_os = "linux")]
pub(crate) fn read_linux_foreground(
    pid: i32,
    mut read_stat: impl FnMut() -> io::Result<String>,
    read_metadata: impl FnOnce() -> io::Result<Foreground>,
) -> io::Result<Option<Foreground>> {
    let (state, group, started) = linux_stat(pid, &read_stat()?)?;
    if group != pid {
        return Err(io::Error::other("sampled foreground pid is not its group leader"));
    }
    if matches!(state, b'Z' | b'X') {
        return Ok(None);
    }
    let metadata = read_metadata();
    let (state, group, after) = linux_stat(pid, &read_stat()?)?;
    if group != pid || after != started {
        return Err(io::Error::other("foreground incarnation changed during metadata read"));
    }
    // A process can die while comm/cmdline/cwd are being read. Validate its
    // incarnation first, then let positive death evidence override a partial
    // metadata error. An opaque live process must still stay unknown.
    if matches!(state, b'Z' | b'X') {
        return Ok(None);
    }
    let mut fg = metadata?;
    fg.started = Some(started);
    Ok(Some(fg))
}

#[cfg(target_os = "linux")]
fn linux_stat(pid: i32, stat: &str) -> io::Result<(u8, i32, u64)> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid foreground process stat");
    // comm can contain spaces and parentheses; only its final ') ' ends it.
    let (head, rest) = stat.rsplit_once(") ").ok_or_else(invalid)?;
    let recorded: i32 = head.split_once(" (").ok_or_else(invalid)?.0.parse().map_err(|_| invalid())?;
    if recorded != pid {
        return Err(invalid());
    }
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let state = fields.first().filter(|s| s.len() == 1).ok_or_else(invalid)?.as_bytes()[0];
    let group = fields.get(2).ok_or_else(invalid)?.parse().map_err(|_| invalid())?; // field 5
    let started = fields.get(19).ok_or_else(invalid)?.parse().map_err(|_| invalid())?; // field 22
    Ok((state, group, started))
}

#[cfg(windows)]
pub fn observe_foreground(pid: i32) -> io::Result<Option<Foreground>> {
    Ok(windows_about(pid))
}

#[cfg(target_os = "macos")]
pub fn observe_foreground(pid: i32) -> io::Result<Option<Foreground>> {
    if let Some(fg) = macos_about(pid) {
        return Ok(Some(fg));
    }
    // A missing libproc record also means permissions/partial metadata.
    // Only a separate positive ESRCH observation proves the process gone.
    if unsafe { libc::kill(pid, 0) } < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(io::Error::other("process metadata unavailable"))
    }
}

/// A pseudo console has no foreground group: this is the terminal's first
/// process, with the folder and command line Windows keeps in its memory.
/// None for a 32-bit process, whose own copy of them lives elsewhere.
#[cfg(windows)]
fn windows_about(pid: i32) -> Option<Foreground> {
    use std::ffi::c_void;
    use std::mem::{MaybeUninit, size_of};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows_sys::Win32::Foundation::{HANDLE, LocalFree, UNICODE_STRING};
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Threading::*;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid as u32) };
    if raw.is_null() {
        return None;
    }
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let h = process.as_raw_handle() as HANDLE;
    let mut wow64 = 0;
    if unsafe { IsWow64Process(h, &mut wow64) } == 0 || wow64 != 0 {
        return None;
    }
    let read = |at: *const c_void, into: *mut c_void, len: usize| {
        let mut n = 0;
        unsafe { ReadProcessMemory(h, at, into, len, &mut n) != 0 && n == len }
    };
    fn value<T>(read: impl Fn(*const c_void, *mut c_void, usize) -> bool, at: *const c_void) -> Option<T> {
        let mut v = MaybeUninit::<T>::uninit();
        read(at, v.as_mut_ptr().cast(), size_of::<T>()).then(|| unsafe { v.assume_init() })
    }
    let text = |s: UNICODE_STRING| -> Option<Vec<u16>> {
        let mut buf = vec![0u16; s.Length as usize / 2];
        read(s.Buffer.cast(), buf.as_mut_ptr().cast(), buf.len() * 2).then_some(buf)
    };

    let mut basic: PROCESS_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let size = size_of::<PROCESS_BASIC_INFORMATION>() as u32;
    let status =
        unsafe { NtQueryInformationProcess(h, ProcessBasicInformation, (&raw mut basic).cast(), size, &mut 0) };
    if status != 0 || basic.PebBaseAddress.is_null() {
        return None;
    }
    let peb: PEB = value(read, basic.PebBaseAddress.cast())?;
    let params: RTL_USER_PROCESS_PARAMETERS = value(read, peb.ProcessParameters.cast())?;
    // CurrentDirectory.DosPath: right after the standard handles, in what the SDK calls Reserved2.
    let dir: UNICODE_STRING = value(read, peb.ProcessParameters.cast::<u8>().wrapping_add(0x38).cast())?;
    let mut cwd = String::from_utf16(&text(dir)?).ok()?;
    if cwd.len() > 3 && cwd.ends_with('\\') {
        cwd.pop(); // Windows keeps C:\dir\; only a drive's root has one
    }
    let image = String::from_utf16(&text(params.ImagePathName)?).ok()?;
    let name = image.rsplit('\\').next().unwrap_or(&image).to_string();

    let mut line = text(params.CommandLine)?;
    line.push(0);
    let mut argc = 0;
    let list = unsafe { CommandLineToArgvW(line.as_ptr(), &mut argc) };
    if list.is_null() {
        return None;
    }
    let argv = (0..argc as usize)
        .map(|i| unsafe {
            let arg = *list.add(i);
            String::from_utf16_lossy(std::slice::from_raw_parts(arg, (0..).take_while(|&n| *arg.add(n) != 0).count()))
        })
        .collect();
    unsafe { LocalFree(list.cast()) };
    let mut fg = Foreground { pid, name, argv, cwd, started: None, agent: None };
    fg.agent = crate::detect::identify(&fg);
    Some(fg)
}

#[cfg(target_os = "macos")]
fn macos_about(pid: i32) -> Option<Foreground> {
    let mut name = [0u8; 256];
    let n = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
    if n <= 0 {
        return None;
    }
    let name = String::from_utf8_lossy(&name[..n as usize]).into_owned();
    let mut vnode: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    if unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&raw mut vnode).cast(), size) } != size {
        return None;
    }
    let path = unsafe { std::ffi::CStr::from_ptr(vnode.pvi_cdir.vip_path.as_ptr().cast()) };
    let (argv, _) = procargs(pid)?;
    let mut fg = Foreground { pid, name, argv, cwd: path.to_string_lossy().into_owned(), started: None, agent: None };
    fg.agent = crate::detect::identify(&fg);
    Some(fg)
}

/// KERN_PROCARGS2: argc, the executable path, the arguments, then the
/// environment, each NUL-terminated. Returns (arguments, environment).
#[cfg(target_os = "macos")]
pub fn procargs(pid: i32) -> Option<(Vec<String>, Vec<u8>)> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    let call = |buf: *mut libc::c_void, size: &mut libc::size_t, mib: &mut [i32; 3]| unsafe {
        libc::sysctl(mib.as_mut_ptr(), 3, buf, size, std::ptr::null_mut(), 0)
    };
    if call(std::ptr::null_mut(), &mut size, &mut mib) != 0 || size < 4 {
        return None;
    }
    let mut buf = vec![0u8; size];
    if call(buf.as_mut_ptr().cast(), &mut size, &mut mib) != 0 {
        return None;
    }
    buf.truncate(size);
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?) as usize;
    let mut rest = &buf[4..];
    let one = |r: &[u8]| -> Option<usize> { r.iter().position(|b| *b == 0) };
    rest = &rest[one(rest)? + 1..]; // the executable path
    while rest.first() == Some(&0) {
        rest = &rest[1..]; // its padding
    }
    let mut argv = Vec::with_capacity(argc);
    for _ in 0..argc {
        let end = one(rest)?;
        argv.push(String::from_utf8_lossy(&rest[..end]).into_owned());
        rest = &rest[end + 1..];
    }
    Some((argv, rest.to_vec()))
}
