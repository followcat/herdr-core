pub fn observed_hostname() -> Option<String> {
    let mut buffer = [0u8; 256];
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    hostname_result(result, &buffer)
}

fn hostname_result(result: i32, buffer: &[u8]) -> Option<String> {
    if result != 0 {
        return None;
    }
    let end = buffer.iter().position(|&b| b == 0)?;
    if end == 0 {
        return None;
    }
    std::str::from_utf8(&buffer[..end]).ok().map(String::from)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hostname_errors_empty_truncated_and_non_utf8_are_unknown() {
        for (result, bytes) in [(-1, b"host-a\0".as_slice()), (0, b""), (0, b"\0"), (0, b"host-a"), (0, b"\xff\0")] {
            assert_eq!(super::hostname_result(result, bytes), None);
        }
        assert_eq!(super::hostname_result(0, b"Host-A.example\0"), Some("Host-A.example".into()));
        assert!(super::observed_hostname().is_some(), "native OS observation");
    }
}
