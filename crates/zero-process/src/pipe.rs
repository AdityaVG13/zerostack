//! Safe Windows named-pipe server/client primitives with a protected
//! current-user-only ACL and connected-client SID verification.
//!
//! Every HANDLE and every security-descriptor allocation has exactly one RAII
//! owner ([`super::identity::Handle`] and [`LocalBuffer`]); no default or
//! ambient ACL is ever used, and no polling is performed: reads and writes
//! block on kernel events (overlapped I/O) and are interruptible only through
//! the connection's cancel event.
#![cfg(windows)]

use std::io::{self, Read, Write};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER,
    ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_NOT_FOUND, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_ALL, GENERIC_READ, GENERIC_WRITE,
    HANDLE, INVALID_HANDLE_VALUE, LocalFree, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::{
    ACL, AddAccessAllowedAce, GetTokenInformation, InitializeAcl, InitializeSecurityDescriptor,
    SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SID, SetSecurityDescriptorControl,
    SetSecurityDescriptorDacl, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, OVERLAPPED, OVERLAPPED_0};
use windows_sys::Win32::System::Memory::{LPTR, LocalAlloc};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    NMPWAIT_USE_DEFAULT_WAIT, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;
use windows_sys::Win32::System::Threading::{
    INFINITE, PROCESS_QUERY_LIMITED_INFORMATION, ResetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};

use crate::identity::Handle;

/// Bytes of one Windows SID (header + identifier authority + subauthorities),
/// copied out of a token so the caller never depends on another allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sid {
    bytes: Vec<u8>,
}

impl Sid {
    /// SID of the current user (from this process's primary token).
    pub fn current_user() -> io::Result<Self> {
        let process = Handle::current_process();
        let token = Handle::open_process_token(process, windows_sys::Win32::Security::TOKEN_QUERY)?;
        Self::from_token(token.raw())
    }

    fn from_token(token: HANDLE) -> io::Result<Self> {
        let mut needed = 0u32;
        // First probe with no buffer: ERROR_INSUFFICIENT_BUFFER reports size.
        // SAFETY: token is a valid token handle; tokeninformation null with
        // zero length is a legal size probe.
        unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(error);
        }
        let buffer = LocalBuffer::alloc(needed as usize)?;
        // SAFETY: buffer is a zeroed LocalAlloc block of exactly `needed`
        // bytes; GetTokenInformation writes a TOKEN_USER into it.
        if unsafe {
            GetTokenInformation(token, TokenUser, buffer.ptr().cast(), needed, &mut needed)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the buffer holds a valid TOKEN_USER after the successful
        // call above; the SID pointer is inside the same buffer.
        let user = unsafe { &*(buffer.ptr().cast::<TOKEN_USER>()) };
        Self::copy_sid(user.User.Sid)
    }

    fn copy_sid(raw: *mut core::ffi::c_void) -> io::Result<Self> {
        // SAFETY: `raw` names a valid SID; the copy length is derived from its
        // own SubAuthorityCount, so the copy never overruns the source.
        let sid = unsafe { &*raw.cast::<SID>() };
        let len = 8usize
            .checked_add(4 * sid.SubAuthorityCount as usize)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "SID subauthority count overflow",
                )
            })?;
        let mut bytes = vec![0u8; len];
        // SAFETY: `len` is the exact byte size of the SID structure.
        unsafe {
            ptr::copy_nonoverlapping(raw.cast::<u8>(), bytes.as_mut_ptr(), len);
        }
        Ok(Self { bytes })
    }

    fn as_ptr(&self) -> *mut core::ffi::c_void {
        self.bytes.as_ptr().cast_mut().cast()
    }
}

/// Zeroed LocalAlloc block; the single RAII owner of the allocation.
struct LocalBuffer(*mut core::ffi::c_void);

impl LocalBuffer {
    fn alloc(bytes: usize) -> io::Result<Self> {
        // SAFETY: LPTR zero-initializes the fresh allocation; null means OOM.
        let ptr = unsafe { LocalAlloc(LPTR, bytes) };
        if ptr.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(ptr))
        }
    }

    fn ptr(&self) -> *mut core::ffi::c_void {
        self.0
    }
}

impl Drop for LocalBuffer {
    fn drop(&mut self) {
        // SAFETY: `self.0` is our own LocalAlloc block; Drop runs exactly once.
        unsafe {
            LocalFree(self.0);
        }
    }
}

// SAFETY: LocalBuffer uniquely owns a LocalAlloc pointer; it is !Clone and
// Drop calls LocalFree exactly once. `ptr()` is crate-private.
unsafe impl Send for LocalBuffer {} // ubs:ignore — FFI wrapper, invariants: unique LocalAlloc owner
unsafe impl Sync for LocalBuffer {} // ubs:ignore — FFI wrapper, invariants: unique LocalAlloc owner

/// Zeroed OVERLAPPED with our completion event. All fields are integer/handle
/// POD; the anonymous union is the documented Offset/Pointer overlay.
fn overlapped_with_event(event: HANDLE) -> OVERLAPPED {
    OVERLAPPED {
        Internal: 0,
        InternalHigh: 0,
        Anonymous: OVERLAPPED_0 {
            Pointer: ptr::null_mut(),
        },
        hEvent: event,
    }
}

/// Explicit security descriptor with a DACL granting only the current user.
/// The descriptor and ACL allocations are both owned by this RAII struct, so
/// every pipe instance created from it carries the same protected ACL and no
/// default ACL is ever applied.
pub struct PipeSecurity {
    descriptor: LocalBuffer,
    /// Held only to keep the ACL allocation alive for the descriptor's
    /// lifetime; the DACL is read through the descriptor.
    #[allow(dead_code)]
    acl: LocalBuffer,
}

impl PipeSecurity {
    /// Build a security descriptor whose DACL contains exactly one ACE:
    /// GENERIC_ALL for the current user SID.
    pub fn current_user_only() -> io::Result<Self> {
        let sid = Sid::current_user()?;
        let descriptor = LocalBuffer::alloc(std::mem::size_of::<SECURITY_DESCRIPTOR>())?;
        let acl = LocalBuffer::alloc(512)?;
        // SAFETY: descriptor is a zeroed block sized for SECURITY_DESCRIPTOR.
        if unsafe { InitializeSecurityDescriptor(descriptor.ptr(), SECURITY_DESCRIPTOR_REVISION) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: acl is a zeroed 512-byte block sized for an ACL with one ACE.
        if unsafe { InitializeAcl(acl.ptr().cast::<ACL>(), 512, 2) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: acl is initialized; AddAccessAllowedAce copies the SID into
        // the ACL buffer.
        if unsafe { AddAccessAllowedAce(acl.ptr().cast::<ACL>(), 2, GENERIC_ALL, sid.as_ptr()) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: descriptor is initialized; the ACL outlives the descriptor
        // inside the same RAII owner. SE_DACL_PROTECTED forbids inherited ACEs.
        if unsafe { SetSecurityDescriptorDacl(descriptor.ptr(), 1, acl.ptr().cast::<ACL>(), 0) }
            == 0
            || unsafe {
                SetSecurityDescriptorControl(descriptor.ptr(), SE_DACL_PROTECTED, SE_DACL_PROTECTED)
            } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { descriptor, acl })
    }

    fn security_attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.ptr(),
            bInheritHandle: 0,
        }
    }
}

/// Server side of a named pipe. `new` creates the first instance with the
/// protected current-user-only ACL; each [`accept`](Self::accept) connects the
/// current instance and immediately creates the next one, so clients can
/// connect while a handler runs.
pub struct PipeListener {
    security: Arc<PipeSecurity>,
    name: Vec<u16>,
    next: Option<Arc<Handle>>,
    cancel: Arc<Handle>,
}

impl PipeListener {
    /// Bind `name` (`\\.\pipe\...`) with the current-user-only ACL.
    pub fn new(name: &str) -> io::Result<Self> {
        validate_pipe_name(name)?;
        let security = Arc::new(PipeSecurity::current_user_only()?);
        let cancel = Arc::new(Handle::create_event(true)?);
        let wide = encode_wide(name);
        let listener = Self {
            security,
            name: wide.clone(),
            cancel,
            next: None,
        };
        let mut listener = listener;
        listener.spawn_instance(true)?;
        Ok(listener)
    }

    /// Borrowed instance handle for introspection (tests, DACL verification).
    /// The handle remains owned by the listener; never close it.
    pub fn instance_handle(&self) -> HANDLE {
        self.next
            .as_ref()
            .map(|handle| handle.raw())
            .unwrap_or(INVALID_HANDLE_VALUE)
    }

    /// Return a cancellation handle for the currently pending accept.
    pub fn canceller(&self) -> io::Result<PipeListenerCancel> {
        Ok(PipeListenerCancel(Arc::clone(&self.cancel)))
    }

    /// Block until a client connects to the current instance, then return the
    /// connection and create the next instance.
    pub fn accept(&mut self) -> io::Result<PipeConnection> {
        let Some(instance) = &self.next else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "named pipe listener has no instance",
            ));
        };
        let raw = instance.raw();
        let event = Handle::create_event(false)?;
        let mut overlapped = overlapped_with_event(event.raw());
        // SAFETY: raw is our own pipe instance; overlapped is zeroed with a
        // fresh event; the instance was created with FILE_FLAG_OVERLAPPED.
        let rc = unsafe { ConnectNamedPipe(raw, &mut overlapped) };
        if rc == 0 {
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(code) if code == ERROR_IO_PENDING as i32 => {
                    let handles = [event.raw(), self.cancel.raw()];
                    // SAFETY: both handles are our live kernel events.
                    let wait = unsafe {
                        WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, INFINITE)
                    };
                    if wait == WAIT_OBJECT_0 + 1 {
                        cancel_and_settle(raw, &overlapped, event.raw())?;
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "named pipe accept cancelled",
                        ));
                    }
                    if wait != WAIT_OBJECT_0 {
                        let error = io::Error::last_os_error();
                        let _ = cancel_and_settle(raw, &overlapped, event.raw());
                        return Err(error);
                    }
                    let mut transferred = 0u32;
                    // SAFETY: the wait settled this exact overlapped connect.
                    if unsafe {
                        windows_sys::Win32::System::IO::GetOverlappedResult(
                            raw,
                            &overlapped,
                            &mut transferred,
                            0,
                        )
                    } == 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                }
                Some(code) if code == ERROR_PIPE_CONNECTED as i32 => {}
                // A client may close immediately after CreateFile succeeds.
                // Return that connected instance; its first read reports EOF.
                Some(code) if code == ERROR_NO_DATA as i32 => {}
                _ => return Err(error),
            }
        }
        let handle = self.next.take().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Other,
                "named pipe listener has no pending instance",
            )
        })?;
        let connection = PipeConnection::from_handle(handle, true)?;
        self.spawn_instance(false)?;
        Ok(connection)
    }

    fn spawn_instance(&mut self, first: bool) -> io::Result<()> {
        let open_mode = PIPE_ACCESS_DUPLEX
            | FILE_FLAG_OVERLAPPED
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            };
        let attributes = self.security.security_attributes();
        // SAFETY: name is a valid pipe path; attributes carries our explicit
        // protected DACL; the returned handle is owned by this listener.
        let raw = unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                65_536,
                65_536,
                0,
                &attributes,
            )
        };
        let handle = Arc::new(Handle::new(raw)?);
        self.next = Some(handle);
        Ok(())
    }
}
/// Cloneable terminal cancellation capability for one listener. Cancellation
/// wakes its pending accept and keeps later accepts interrupted. It never
/// aliases a pipe instance, so a late cancel cannot affect a raced connection.
#[derive(Clone)]
pub struct PipeListenerCancel(Arc<Handle>);

impl PipeListenerCancel {
    pub fn cancel(&self) -> io::Result<()> {
        // SAFETY: this is our own manual-reset cancellation event.
        if unsafe { windows_sys::Win32::System::Threading::SetEvent(self.0.raw()) } != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

fn cancel_and_settle(handle: HANDLE, overlapped: &OVERLAPPED, event: HANDLE) -> io::Result<()> {
    // SAFETY: all values belong to this exact pending accept. Waiting before
    // return keeps the stack-owned OVERLAPPED alive through cancellation.
    let cancelled = unsafe { CancelIoEx(handle, overlapped) };
    let cancel_error = (cancelled == 0).then(io::Error::last_os_error);
    if unsafe { WaitForSingleObject(event, INFINITE) } == WAIT_FAILED {
        return Err(io::Error::last_os_error());
    }
    if let Some(error) = cancel_error
        && error.raw_os_error() != Some(ERROR_NOT_FOUND as i32)
    {
        return Err(error);
    }
    Ok(())
}

/// A connected named-pipe byte stream. Reads are interruptible through the
/// shared cancel event; both directions are bounded by per-call timeouts and
/// block on kernel events (no polling).
pub struct PipeConnection {
    handle: Arc<Handle>,
    state: Arc<ConnState>,
    server_end: bool,
}

struct ConnState {
    cancel: Handle,
    read_event: Handle,
    write_event: Handle,
    read_gate: Mutex<()>,
    write_gate: Mutex<()>,
    read_timeout: Mutex<Option<Duration>>,
    write_timeout: Mutex<Option<Duration>>,
}

impl PipeConnection {
    fn from_handle(handle: Arc<Handle>, server_end: bool) -> io::Result<Self> {
        let cancel = Handle::create_event(true)?;
        let read_event = Handle::create_event(false)?;
        let write_event = Handle::create_event(false)?;
        Ok(Self {
            handle,
            state: Arc::new(ConnState {
                cancel,
                read_event,
                write_event,
                read_gate: Mutex::new(()),
                write_gate: Mutex::new(()),
                read_timeout: Mutex::new(Some(Duration::from_millis(250))),
                write_timeout: Mutex::new(Some(Duration::from_millis(250))),
            }),
            server_end,
        })
    }

    /// Client side: connect to an existing pipe, retrying while the server
    /// has no free instance (ERROR_PIPE_BUSY), bounded to ~5s.
    pub fn connect(name: &str) -> io::Result<Self> {
        validate_pipe_name(name)?;
        let wide = encode_wide(name);
        let mut last_error = io::Error::new(io::ErrorKind::NotFound, "named pipe not found");
        for _ in 0..50 {
            // SAFETY: wide is a null-terminated pipe path; OPEN_EXISTING on a
            // pipe never creates a file.
            let raw = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    ptr::null_mut(),
                )
            };
            if raw != INVALID_HANDLE_VALUE {
                return Self::from_handle(Arc::new(Handle::new(raw)?), false);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) {
                last_error = error;
                // SAFETY: waits for an instance to become available.
                unsafe { WaitNamedPipeW(wide.as_ptr(), NMPWAIT_USE_DEFAULT_WAIT) };
                continue;
            }
            return Err(map_connect_error(error));
        }
        Err(last_error)
    }

    /// Clone the connection for an independent reader/writer on the same
    /// kernel pipe handle. Each direction owns a distinct OVERLAPPED event.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            handle: Arc::clone(&self.handle),
            state: Arc::clone(&self.state),
            server_end: self.server_end,
        })
    }

    /// Cancel the connection: any blocked read returns EOF and the connection
    /// is unusable afterwards. Used for teardown of handler threads.
    pub fn cancel(&self) {
        // SAFETY: our own manual-reset event handle.
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(self.state.cancel.raw());
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) {
        *self
            .state
            .read_timeout
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = timeout;
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) {
        *self
            .state
            .write_timeout
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = timeout;
    }

    /// Verify the connected peer's SID is the current user's SID. The server
    /// queries the client pid; the client queries the server pid.
    pub fn peer_is_current_user(&self) -> io::Result<bool> {
        let mut pid = 0u32;
        // SAFETY: our pipe handle and endpoint role select the matching peer.
        let ok = unsafe {
            if self.server_end {
                GetNamedPipeClientProcessId(self.handle.raw(), &mut pid)
            } else {
                GetNamedPipeServerProcessId(self.handle.raw(), &mut pid)
            }
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pipe client process id unavailable",
            ));
        }
        let process = Handle::open_process(PROCESS_QUERY_LIMITED_INFORMATION, pid)?;
        let token =
            Handle::open_process_token(process.raw(), windows_sys::Win32::Security::TOKEN_QUERY)?;
        let peer = Sid::from_token(token.raw())?;
        Ok(peer == Sid::current_user()?)
    }
    fn read_inner(&mut self, buffer: &mut [u8], timeout: Option<Duration>) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let _gate = self
            .state
            .read_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // SAFETY: clear any completion left by a synchronous prior read.
        unsafe { ResetEvent(self.state.read_event.raw()) };
        let mut overlapped = overlapped_with_event(self.state.read_event.raw());
        let mut read = 0u32;
        // SAFETY: our pipe handle, a caller buffer, and a zeroed OVERLAPPED
        // with our event; the instance was created with FILE_FLAG_OVERLAPPED.
        let rc = unsafe {
            ReadFile(
                self.handle.raw(),
                buffer.as_mut_ptr(),
                buffer.len().min(u32::MAX as usize) as u32,
                &mut read,
                &mut overlapped,
            )
        };
        if rc != 0 {
            return Ok(read as usize);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return map_read_error(error);
        }
        let wait = self.wait_io(&overlapped, timeout)?;
        if wait == WAIT_OBJECT_0 + 1 {
            // SAFETY: cancel and settle our own pending read before return.
            unsafe { CancelIoEx(self.handle.raw(), &overlapped) };
            unsafe { WaitForSingleObject(overlapped.hEvent, INFINITE) };
            return Ok(0);
        }
        if wait == WAIT_TIMEOUT {
            // SAFETY: cancel and settle our own pending read before return.
            unsafe { CancelIoEx(self.handle.raw(), &overlapped) };
            unsafe { WaitForSingleObject(overlapped.hEvent, INFINITE) };
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "pipe read timed out",
            ));
        }
        let mut transferred = 0u32;
        // SAFETY: overlapped completed; bWait=0 reads the result directly.
        if unsafe {
            windows_sys::Win32::System::IO::GetOverlappedResult(
                self.handle.raw(),
                &overlapped,
                &mut transferred,
                0,
            )
        } != 0
        {
            Ok(transferred as usize)
        } else {
            map_read_error(io::Error::last_os_error())
        }
    }

    fn write_inner(&mut self, buffer: &[u8], timeout: Option<Duration>) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let _gate = self
            .state
            .write_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // SAFETY: clear any completion left by a synchronous prior write.
        unsafe { ResetEvent(self.state.write_event.raw()) };
        let mut overlapped = overlapped_with_event(self.state.write_event.raw());
        let mut written = 0u32;
        // SAFETY: our pipe handle, a caller buffer, and a zeroed OVERLAPPED.
        let rc = unsafe {
            WriteFile(
                self.handle.raw(),
                buffer.as_ptr(),
                buffer.len().min(u32::MAX as usize) as u32,
                &mut written,
                &mut overlapped,
            )
        };
        if rc != 0 {
            return Ok(written as usize);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(map_write_error(error));
        }
        let wait = self.wait_io(&overlapped, timeout)?;
        if wait == WAIT_OBJECT_0 + 1 {
            // SAFETY: cancel and settle our own pending write before return.
            unsafe { CancelIoEx(self.handle.raw(), &overlapped) };
            unsafe { WaitForSingleObject(overlapped.hEvent, INFINITE) };
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "pipe write cancelled",
            ));
        }
        if wait == WAIT_TIMEOUT {
            // SAFETY: cancel and settle our own pending write before return.
            unsafe { CancelIoEx(self.handle.raw(), &overlapped) };
            unsafe { WaitForSingleObject(overlapped.hEvent, INFINITE) };
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "pipe write timed out",
            ));
        }
        let mut transferred = 0u32;
        // SAFETY: overlapped completed; bWait=0 reads the result directly.
        if unsafe {
            windows_sys::Win32::System::IO::GetOverlappedResult(
                self.handle.raw(),
                &overlapped,
                &mut transferred,
                0,
            )
        } != 0
        {
            Ok(transferred as usize)
        } else {
            Err(map_write_error(io::Error::last_os_error()))
        }
    }

    /// Wait on {io event, cancel event}; returns the WAIT_* code.
    fn wait_io(&self, overlapped: &OVERLAPPED, timeout: Option<Duration>) -> io::Result<u32> {
        let handles = [overlapped.hEvent, self.state.cancel.raw()];
        let ms = timeout.map_or(INFINITE, |t| t.as_millis().min(u32::MAX as u128) as u32);
        // SAFETY: both handles are our own events; array is exactly two items.
        let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, ms) };
        if wait == WAIT_FAILED {
            Err(io::Error::last_os_error())
        } else {
            Ok(wait)
        }
    }
}

impl Read for PipeConnection {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let timeout = *self
            .state
            .read_timeout
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.read_inner(buffer, timeout)
    }
}

impl Write for PipeConnection {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let timeout = *self
            .state
            .write_timeout
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.write_inner(buffer, timeout)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn validate_pipe_name(value: &str) -> io::Result<()> {
    if !value.starts_with(r"\\.\pipe\") || value.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "named pipe must use a non-NUL \\\\.\\pipe\\ endpoint",
        ));
    }
    Ok(())
}

fn encode_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn map_read_error(error: io::Error) -> io::Result<usize> {
    match error.raw_os_error() {
        Some(code)
            if code == ERROR_BROKEN_PIPE as i32
                || code == ERROR_PIPE_NOT_CONNECTED as i32
                || code == ERROR_NO_DATA as i32
                || code == ERROR_OPERATION_ABORTED as i32 =>
        {
            Ok(0)
        }
        _ => Err(error),
    }
}

fn map_write_error(error: io::Error) -> io::Error {
    match error.raw_os_error() {
        Some(code)
            if code == ERROR_BROKEN_PIPE as i32
                || code == ERROR_PIPE_NOT_CONNECTED as i32
                || code == ERROR_NO_DATA as i32 =>
        {
            io::Error::new(io::ErrorKind::BrokenPipe, "named pipe peer closed")
        }
        _ => error,
    }
}

fn map_connect_error(error: io::Error) -> io::Error {
    match error.raw_os_error() {
        Some(code)
            if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_INVALID_PARAMETER as i32 =>
        {
            io::Error::new(
                io::ErrorKind::NotFound,
                "named pipe endpoint does not exist",
            )
        }
        _ => error,
    }
}
