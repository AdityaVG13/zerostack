//! Session-owner identity and blocking owner-death notification.
use std::{fmt, io};

#[cfg(unix)]
pub fn current_euid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
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
    identity: ProcessIdentity,
}
impl OwnerWatcher {
    pub fn new(identity: ProcessIdentity) -> Result<Self, OwnerWatchError> {
        if !identity.is_live()? {
            return Err(OwnerWatchError::IdentityChanged);
        }
        Ok(Self { identity })
    }
    pub fn wait(self) -> Result<(), OwnerWatchError> {
        wait_for_exit(&self.identity)
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
#[cfg(not(unix))]
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
#[cfg(not(unix))]
fn wait_for_exit(_: &ProcessIdentity) -> Result<(), OwnerWatchError> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "blocking owner watcher unsupported",
    )
    .into())
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
}
