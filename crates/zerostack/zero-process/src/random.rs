//! Cryptographically random bytes for session capabilities and generations.
use std::io;

/// Fill `buffer` with bytes from the operating system CSPRNG. Unix reads `/dev/urandom`;
/// Windows calls `BCryptGenRandom` with the system-preferred RNG (never the ambient `rand` seed).
pub fn fill_random(buffer: &mut [u8]) -> io::Result<()> {
    if buffer.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "random request exceeds platform limit",
        ));
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")?.read_exact(buffer)
    }
    #[cfg(windows)]
    {
        // SAFETY: BCryptGenRandom with a null algorithm handle and the
        // system-preferred flag fills the caller buffer; NTSTATUS 0 is success.
        let rc = unsafe {
            windows_sys::Win32::Security::Cryptography::BCryptGenRandom(
                std::ptr::null_mut(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                windows_sys::Win32::Security::Cryptography::BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(rc as i32))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = buffer;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "operating-system entropy unsupported",
        ))
    }
}
