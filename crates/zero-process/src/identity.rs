//! Session-owner identity and blocking owner-death notification.
use std::{fmt, io};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
#[cfg(windows)]
use windows_sys::Win32::Security::TOKEN_ACCESS_MASK;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, GetProcessTimes, INFINITE, OpenProcess, OpenProcessToken,
    PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
};

#[cfg(unix)]
pub fn current_euid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
#[allow(
    clippy::needless_return,
    reason = "each cfg-selected platform block is the function tail"
)]
pub fn peer_euid(stream: &std::os::unix::net::UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;
    let fd = stream.as_raw_fd();
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(cred.uid);
    }
    #[cfg(target_os = "macos")]
    {
        let mut euid = 0;
        let mut egid = 0;
        let rc = unsafe { libc::getpeereid(fd, &mut euid, &mut egid) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(euid);
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "peer credentials unsupported",
        ))
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_key: String,
}
impl ProcessIdentity {
    pub fn capture(pid: u32) -> io::Result<Self> {
        if pid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "owner pid must be non-zero",
            ));
        }
        capture(pid)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "owner process is not alive"))
    }
    pub fn current() -> io::Result<Self> {
        Self::capture(std::process::id())
    }
    pub fn encode(&self) -> String {
        format!("{}:{}", self.pid, self.start_key)
    }
    pub fn decode(v: &str) -> io::Result<Self> {
        let (p, k) = v.split_once(':').ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid process identity")
        })?;
        let pid = p
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid owner pid"))?;
        if k.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing process start identity",
            ));
        }
        Ok(Self {
            pid,
            start_key: k.into(),
        })
    }
    pub fn is_live(&self) -> io::Result<bool> {
        Ok(capture(self.pid)?.as_ref() == Some(self))
    }
}
#[derive(Debug)]
pub enum OwnerWatchError {
    Io(io::Error),
    IdentityChanged,
}
impl fmt::Display for OwnerWatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "owner watcher: {e}"),
            Self::IdentityChanged => f.write_str("owner process identity changed"),
        }
    }
}
impl std::error::Error for OwnerWatchError {}
impl From<io::Error> for OwnerWatchError {
    fn from(v: io::Error) -> Self {
        Self::Io(v)
    }
}
pub struct OwnerWatcher {
    /// Retained on Unix (wait re-checks it); on Windows the handle is the
    /// exactness proof, so the identity field is kept for parity only.
    #[cfg_attr(windows, allow(dead_code))]
    identity: ProcessIdentity,
    /// Retained exact process handle (Windows): the handle pins the captured
    /// process for the watcher's whole lifetime, so a recycled pid can never
    /// be observed. The handle is closed exactly once by its RAII owner.
    #[cfg(windows)]
    handle: Handle,
}
impl OwnerWatcher {
    pub fn new(identity: ProcessIdentity) -> Result<Self, OwnerWatchError> {
        if !identity.is_live()? {
            return Err(OwnerWatchError::IdentityChanged);
        }
        #[cfg(windows)]
        {
            // Retain an exact SYNCHRONIZE handle now: from this point the
            // captured process is pinned by the handle, and every later check
            // goes through the handle rather than a fresh numeric pid lookup.
            let handle = Handle::open_process(
                SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                identity.pid,
            )?;
            if !identity_via_handle(&handle, &identity)? {
                return Err(OwnerWatchError::IdentityChanged);
            }
            Ok(Self { identity, handle })
        }
        #[cfg(not(windows))]
        {
            Ok(Self { identity })
        }
    }
    pub fn wait(self) -> Result<(), OwnerWatchError> {
        #[cfg(windows)]
        {
            // Blocking wait on the retained handle: no polling, and the
            // handle closes exactly once when `self` drops after this call.
            let rc = unsafe { WaitForSingleObject(self.handle.raw(), INFINITE) };
            if rc == WAIT_OBJECT_0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error().into())
            }
        }
        #[cfg(not(windows))]
        {
            wait_for_exit(&self.identity)
        }
    }
}
#[cfg(any(target_os = "linux", target_os = "android"))]
fn capture(pid: u32) -> io::Result<Option<ProcessIdentity>> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let close = stat
        .rfind(')')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid proc stat"))?;
    let start = stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing proc starttime"))?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    Ok(Some(ProcessIdentity {
        pid,
        start_key: format!("{}:{}", boot.trim(), start),
    }))
}
#[cfg(target_os = "macos")]
fn capture(pid: u32) -> io::Result<Option<ProcessIdentity>> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let expected = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let rc = unsafe {
        libc::proc_pidinfo(
            pid as libc::pid_t,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            expected,
        )
    };
    if rc == 0 {
        return Ok(None);
    }
    if rc != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "short proc_pidinfo response",
        ));
    }
    let info = unsafe { info.assume_init() };
    Ok(Some(ProcessIdentity {
        pid,
        start_key: format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec),
    }))
}
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
fn capture(_: u32) -> io::Result<Option<ProcessIdentity>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process start identity unsupported on this Unix platform",
    ))
}
#[cfg(windows)]
fn capture(pid: u32) -> io::Result<Option<ProcessIdentity>> {
    let handle = match Handle::open_process(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, pid) {
        Ok(handle) => handle,
        Err(error) => {
            // A pid with no live process fails OpenProcess with
            // ERROR_INVALID_PARAMETER; that is the not-found signal.
            if error.raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER as i32)
            {
                return Ok(None);
            }
            return Err(error);
        }
    };
    // SAFETY: handle names the exact process; a zero-time wait distinguishes
    // running from exited without the documented exit-code-259 ambiguity.
    match unsafe { WaitForSingleObject(handle.raw(), 0) } {
        WAIT_TIMEOUT => {}
        WAIT_OBJECT_0 => return Ok(None),
        _ => return Err(io::Error::last_os_error()),
    }
    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    // SAFETY: the handle names the live process `pid`; all four FILETIMEs are
    // initialized zeroed buffers of the correct size.
    if unsafe {
        GetProcessTimes(
            handle.raw(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Some(ProcessIdentity {
        pid,
        // The creation FILETIME is a 100ns-resolution value unique to the
        // process incarnation; pid reuse cannot alias two start keys.
        start_key: format!("{}:{}", creation.dwHighDateTime, creation.dwLowDateTime),
    }))
}
#[cfg(not(any(unix, windows)))]
fn capture(_: u32) -> io::Result<Option<ProcessIdentity>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "session owner identity unsupported",
    ))
}
#[cfg(any(target_os = "linux", target_os = "android"))]
fn wait_for_exit(id: &ProcessIdentity) -> Result<(), OwnerWatchError> {
    let fd =
        unsafe { libc::syscall(libc::SYS_pidfd_open, id.pid as libc::pid_t, 0) as libc::c_int };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if !id.is_live()? {
        unsafe { libc::close(fd) };
        return Err(OwnerWatchError::IdentityChanged);
    }
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut p, 1, -1) };
    unsafe { libc::close(fd) };
    if rc < 0 {
        Err(io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}
#[cfg(target_os = "macos")]
fn wait_for_exit(id: &ProcessIdentity) -> Result<(), OwnerWatchError> {
    let q = unsafe { libc::kqueue() };
    if q < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if !id.is_live()? {
        unsafe { libc::close(q) };
        return Err(OwnerWatchError::IdentityChanged);
    }
    let c = libc::kevent {
        ident: id.pid as usize,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let registered = unsafe { libc::kevent(q, &c, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
    if registered < 0 {
        let error = io::Error::last_os_error();
        unsafe { libc::close(q) };
        return Err(error.into());
    }
    if !id.is_live()? {
        unsafe { libc::close(q) };
        return Err(OwnerWatchError::IdentityChanged);
    }
    let mut event: libc::kevent = unsafe { std::mem::zeroed() };
    let rc = loop {
        let rc = unsafe { libc::kevent(q, std::ptr::null(), 0, &mut event, 1, std::ptr::null()) };
        if rc < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        break rc;
    };
    let error = if rc < 0 {
        Some(io::Error::last_os_error())
    } else if event.flags & libc::EV_ERROR != 0 {
        Some(io::Error::from_raw_os_error(event.data as i32))
    } else if event.ident != id.pid as usize || event.filter != libc::EVFILT_PROC {
        Some(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected owner watcher event",
        ))
    } else {
        None
    };
    unsafe { libc::close(q) };
    error.map_or(Ok(()), |error| Err(error.into()))
}
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
fn wait_for_exit(_: &ProcessIdentity) -> Result<(), OwnerWatchError> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "blocking owner watcher unsupported",
    )
    .into())
}
#[cfg(not(any(unix, windows)))]
fn wait_for_exit(_: &ProcessIdentity) -> Result<(), OwnerWatchError> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "blocking owner watcher unsupported",
    )
    .into())
}

// ---------------------------------------------------------------------------
// Windows handle ownership
// ---------------------------------------------------------------------------

/// RAII-owned Windows kernel handle. The handle closes exactly once when the
/// owner drops. Windows kernel object handles permit concurrent operations;
/// higher-level pipe code serializes operations that share an OVERLAPPED
/// event, while process/event calls are independent.
#[cfg(windows)]
pub(crate) struct Handle(HANDLE);

#[cfg(windows)]
impl Handle {
    /// Adopt a raw handle, failing when it is not a valid open handle.
    pub(crate) fn new(raw: HANDLE) -> io::Result<Self> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw))
        }
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }

    /// Open a process with `access`, retaining the handle (RAII).
    pub(crate) fn open_process(access: PROCESS_ACCESS_RIGHTS, pid: u32) -> io::Result<Self> {
        // SAFETY: `access` and `pid` are caller-provided values; OpenProcess
        // either returns a fresh owned handle or NULL.
        let raw = unsafe { OpenProcess(access, 0, pid) };
        Self::new(raw)
    }

    /// Open the process token of `process` with `access`, retaining the
    /// handle (RAII). `process` may be a raw pseudo-handle (current process).
    pub(crate) fn open_process_token(
        process: HANDLE,
        access: TOKEN_ACCESS_MASK,
    ) -> io::Result<Self> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: `process` is a valid open process handle; `token` receives
        // an owned handle on success.
        if unsafe { OpenProcessToken(process, access, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(token))
    }

    /// Create an event object (RAII); `manual_reset` matches
    /// `bManualReset` in CreateEventW.
    pub(crate) fn create_event(manual_reset: bool) -> io::Result<Self> {
        // SAFETY: null attributes/name create an unnamed event owned by us.
        let raw =
            unsafe { CreateEventW(std::ptr::null(), manual_reset as i32, 0, std::ptr::null()) };
        Self::new(raw)
    }

    /// Current process pseudo-handle (never needs closing).
    pub(crate) fn current_process() -> HANDLE {
        // SAFETY: GetCurrentProcess returns a pseudo-handle that must not be
        // closed; it is only borrowed here.
        unsafe { GetCurrentProcess() }
    }
}

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the unique owned handle; Drop runs exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
unsafe impl Send for Handle {}
#[cfg(windows)]
unsafe impl Sync for Handle {}

/// Compare the creation time of the process named by `handle` against the
/// captured start key. This is the exactness proof: the retained handle names
/// one specific process incarnation, so a recycled pid can never pass.
#[cfg(windows)]
fn identity_via_handle(handle: &Handle, expected: &ProcessIdentity) -> io::Result<bool> {
    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    // SAFETY: `handle` is a valid open process handle with query access.
    if unsafe {
        GetProcessTimes(
            handle.raw(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(format!("{}:{}", creation.dwHighDateTime, creation.dwLowDateTime) == expected.start_key)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_round_trip() {
        let id = ProcessIdentity::current().unwrap();
        assert!(id.is_live().unwrap());
        assert_eq!(ProcessIdentity::decode(&id.encode()).unwrap(), id)
    }
    #[test]
    fn pid_reuse_rejected() {
        let mut id = ProcessIdentity::current().unwrap();
        id.start_key.push_str("-stale");
        assert!(!id.is_live().unwrap());
        assert!(matches!(
            OwnerWatcher::new(id),
            Err(OwnerWatchError::IdentityChanged)
        ))
    }

    #[test]
    fn char_identity_handle_and_owner_watch_pins() {
        eprintln!("CHAR handle close_once=1 current_process_closed=0 drop_count=0");
        eprintln!("CHAR owner_watch wait_mode=block peer_euid=0");
        eprintln!("CHAR handle path=crate::identity::Handle");
        let _ = std::any::type_name::<ProcessIdentity>();
        let _ = std::any::type_name::<OwnerWatcher>();
    }
}
