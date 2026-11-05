use windows_sys::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};

pub fn observed_hostname() -> Option<String> {
    let mut buffer = [0u16; 256];
    let mut size = buffer.len() as u32;
    let result = unsafe { GetComputerNameExW(ComputerNameDnsHostname, buffer.as_mut_ptr(), &mut size) };
    if result == 0 || size == 0 || size as usize >= buffer.len() {
        return None;
    }
    String::from_utf16(&buffer[..size as usize]).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn hostname_comes_from_the_os() {
        assert!(super::observed_hostname().is_some_and(|name| !name.is_empty()));
    }
}
