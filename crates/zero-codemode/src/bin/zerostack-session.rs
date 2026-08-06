#![forbid(unsafe_code)]
#[cfg(unix)]
fn main() {
    if let Err(e) = run() {
        eprintln!("zerostack-session: {e}");
        std::process::exit(1);
    }
}
#[cfg(not(unix))]
fn main() {
    eprintln!("zerostack-session: unsupported platform; Job Object gate unmet");
    std::process::exit(2);
}

#[cfg(unix)]
struct RuntimeCleanup {
    socket: std::path::PathBuf,
    dir: std::path::PathBuf,
}

#[cfg(unix)]
impl Drop for RuntimeCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

#[cfg(unix)]
const MAX_SESSION_CLIENTS: usize = 8;

#[cfg(unix)]
enum SessionEvent {
    Client(std::os::unix::net::UnixStream),
    Terminate(Option<String>),
}

#[cfg(unix)]
struct ActiveClientGuard(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[cfg(unix)]
struct ClientHandler {
    join: std::thread::JoinHandle<()>,
    control: std::os::unix::net::UnixStream,
}

#[cfg(unix)]
impl Drop for ActiveClientGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

#[cfg(unix)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use std::{
        fs,
        io::{BufReader, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };
    use zero_codemode::session::{
        AggregateSession, SESSION_PROTOCOL, SESSION_SHUTDOWN_TOKEN_ENV, SESSION_TOKEN_ENV,
        SessionRequest, SessionResponse,
    };
    use zerostack_machine_permit::session_owner::{
        OwnerWatcher, ProcessIdentity, current_euid, peer_euid,
    };
    let mut a = std::env::args().skip(1);
    if a.next().as_deref() != Some("serve") {
        return Err(
            "usage: zerostack-session serve --root ROOT --runtime-dir DIR --owner ID".into(),
        );
    }
    let (mut root, mut dir, mut owner) = (None, None, None);
    while let Some(k) = a.next() {
        match k.as_str() {
            "--root" => root = a.next().map(PathBuf::from),
            "--runtime-dir" => dir = a.next().map(PathBuf::from),
            "--owner" => owner = a.next(),
            _ => return Err(format!("unknown argument {k}").into()),
        }
    }
    let root = root.ok_or("missing --root")?.canonicalize()?;
    let dir = dir.ok_or("missing --runtime-dir")?;
    let owner = ProcessIdentity::decode(&owner.ok_or("missing --owner")?)?;
    let token = std::env::var(SESSION_TOKEN_ENV).map_err(|_| "missing session token")?;
    let shutdown_token =
        std::env::var(SESSION_SHUTDOWN_TOKEN_ENV).map_err(|_| "missing shutdown token")?;
    if token.len() < 32 || shutdown_token.len() < 32 {
        return Err("capabilities too short".into());
    }
    if constant_time_eq(token.as_bytes(), shutdown_token.as_bytes()) {
        return Err("session capabilities must be distinct".into());
    }
    prepare_runtime(&dir, current_euid())?;
    let socket = dir.join("session.sock");
    if socket.exists() {
        return Err("runtime socket already exists".into());
    };
    let listener = UnixListener::bind(&socket)?;
    let _cleanup = RuntimeCleanup {
        socket: socket.clone(),
        dir: dir.clone(),
    };
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let generation = entropy_u64()?;
    let exec = Arc::new(AggregateSession::new_authorized(generation, root.clone())?);
    let watcher = OwnerWatcher::new(owner)?;
    let (tx, rx) = mpsc::sync_channel(8);
    let terminating = Arc::new(AtomicBool::new(false));
    let admission_token = token.clone();
    let listener_terminating = terminating.clone();
    let listener_cancellation = exec.cancellation();
    let listener_exec = Arc::clone(&exec);
    let watcher_tx = tx.clone();
    let handler_tx = tx.clone();
    thread::spawn(move || {
        for incoming in listener.incoming() {
            match incoming {
                Ok(s) => match tx.try_send(SessionEvent::Client(s)) {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Full(SessionEvent::Client(mut stream))) => {
                        if peer_euid(&stream).ok() == Some(current_euid()) {
                            let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                            if let Ok(cloned) = stream.try_clone()
                                && let Ok(SessionRequest::Hello {
                                    protocol,
                                    token: provided,
                                }) = read_frame(&mut BufReader::new(cloned))
                                && protocol == SESSION_PROTOCOL
                                && constant_time_eq(provided.as_bytes(), admission_token.as_bytes())
                            {
                                let active_generation =
                                    listener_exec.generation().unwrap_or(generation);
                                let _ = write_frame(
                                    &mut stream,
                                    &SessionResponse::typed_error_with_retry(
                                        None,
                                        active_generation,
                                        "backpressure",
                                        "session client queue is full",
                                        Some(1),
                                    ),
                                );
                            }
                        }
                    }
                    Err(mpsc::TrySendError::Full(SessionEvent::Terminate(_))) => unreachable!(),
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                },
                Err(error) => {
                    eprintln!("fatal session listener: {error}");
                    listener_terminating.store(true, Ordering::Release);
                    listener_cancellation.cancel();
                    let _ = tx.send(SessionEvent::Terminate(Some(format!(
                        "session listener failed: {error}"
                    ))));
                    break;
                }
            }
        }
    });
    let watcher_terminating = terminating.clone();
    let watcher_cancellation = exec.cancellation();
    thread::spawn(move || {
        let error = watcher.wait().err().map(|error| {
            eprintln!("fatal owner watcher: {error}");
            format!("owner watcher failed: {error}")
        });
        watcher_terminating.store(true, Ordering::Release);
        watcher_cancellation.cancel();
        let _ = watcher_tx.send(SessionEvent::Terminate(error));
    });
    println!(
        "{}",
        serde_json::json!({"type":"ready","protocol":SESSION_PROTOCOL,"generation":generation})
    );
    std::io::stdout().flush()?;
    let active_clients = Arc::new(AtomicUsize::new(0));
    let mut handlers = Vec::new();
    let fatal_error = loop {
        match rx.recv()? {
            SessionEvent::Terminate(error) => {
                terminating.store(true, Ordering::Release);
                exec.cancellation().cancel();
                break error;
            }
            SessionEvent::Client(mut stream) => {
                if terminating.load(Ordering::Acquire) {
                    continue;
                }
                let admitted = active_clients
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                        (active < MAX_SESSION_CLIENTS).then_some(active + 1)
                    })
                    .is_ok();
                if !admitted {
                    let active_generation = exec.generation().unwrap_or(generation);
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                    let _ = write_frame(
                        &mut stream,
                        &SessionResponse::typed_error_with_retry(
                            None,
                            active_generation,
                            "backpressure",
                            "session client limit reached; retry after 1ms",
                            Some(1),
                        ),
                    );
                    continue;
                }
                let session = Arc::clone(&exec);
                let root = root.clone();
                let token = token.clone();
                let shutdown_token = shutdown_token.clone();
                let handler_terminating = Arc::clone(&terminating);
                let handler_events = handler_tx.clone();
                let active = Arc::clone(&active_clients);
                let control = stream.try_clone()?;
                let join = thread::spawn(move || {
                    let _guard = ActiveClientGuard(active);
                    if let Err(error) = handle_client(
                        stream,
                        session,
                        root,
                        token,
                        shutdown_token,
                        handler_terminating,
                        handler_events,
                    ) {
                        eprintln!("session client failed: {error}");
                    }
                });
                handlers.push(ClientHandler { join, control });
                let mut pending = Vec::with_capacity(handlers.len());
                for handler in handlers.drain(..) {
                    if handler.join.is_finished() {
                        let _ = handler.join.join();
                    } else {
                        pending.push(handler);
                    }
                }
                handlers = pending;
            }
        }
    };
    let shutdown_result = exec.shutdown();
    for handler in &handlers {
        let _ = handler.control.shutdown(std::net::Shutdown::Both);
    }
    for handler in handlers {
        let _ = handler.join.join();
    }
    if let Err(error) = shutdown_result
        && fatal_error.is_none()
    {
        return Err(error.into());
    }
    if let Some(error) = fatal_error {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(unix)]
fn handle_client(
    mut stream: std::os::unix::net::UnixStream,
    session: std::sync::Arc<zero_codemode::session::AggregateSession>,
    root: std::path::PathBuf,
    token: String,
    shutdown_token: String,
    terminating: std::sync::Arc<std::sync::atomic::AtomicBool>,
    events: std::sync::mpsc::SyncSender<SessionEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::Value;
    use std::{
        collections::BTreeSet, io::BufReader, path::PathBuf, sync::atomic::Ordering, time::Duration,
    };
    use zero_codemode::session::{SESSION_PROTOCOL, SessionRequest, SessionResponse};
    use zerostack_machine_permit::session_owner::{current_euid, peer_euid};

    let generation = session.generation()?;
    match peer_euid(&stream) {
        Ok(uid) if uid == current_euid() => {}
        Ok(uid) => {
            eprintln!("peer identity rejected: {uid} != {}", current_euid());
            let _ = write_frame(
                &mut stream,
                &SessionResponse::typed_error(
                    None,
                    generation,
                    "peer_identity_rejected",
                    "peer identity rejected",
                ),
            );
            return Ok(());
        }
        Err(error) => {
            eprintln!("peer credential failure: {error}");
            let _ = write_frame(
                &mut stream,
                &SessionResponse::typed_error(
                    None,
                    generation,
                    "peer_identity_unavailable",
                    "peer credential unavailable",
                ),
            );
            return Ok(());
        }
    }
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let hello = match read_frame(&mut reader) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let authed = match hello {
        SessionRequest::Hello {
            protocol,
            token: provided,
        } => {
            protocol == SESSION_PROTOCOL && constant_time_eq(provided.as_bytes(), token.as_bytes())
        }
        _ => false,
    };
    if !authed {
        let active_generation = session.generation().unwrap_or(generation);
        let _ = write_frame(
            &mut stream,
            &SessionResponse::typed_error(
                None,
                active_generation,
                "authentication_rejected",
                "authentication rejected",
            ),
        );
        return Ok(());
    }
    // The authenticated session is long-lived. Teardown interrupts this blocking
    // read through the supervisor-held stream clone instead of a polling timeout.
    stream.set_read_timeout(None)?;
    let active_generation = session.generation()?;
    write_frame(
        &mut stream,
        &SessionResponse::ok(
            None,
            active_generation,
            serde_json::json!({
                "authenticated": true,
                "generation": active_generation,
            }),
        ),
    )?;
    let mut ids = BTreeSet::new();
    loop {
        if terminating.load(Ordering::Acquire) {
            break;
        }
        let raw_request = match read_value_frame(&mut reader) {
            Ok(value) => value,
            Err(error) if error.connection_closed => break,
            Err(error) => {
                write_frame(
                    &mut stream,
                    &SessionResponse::typed_error(
                        None,
                        active_generation,
                        error.code,
                        error.to_string(),
                    ),
                )?;
                if error.recoverable && !terminating.load(Ordering::Acquire) {
                    continue;
                }
                break;
            }
        };
        let request_id = raw_request.get("id").and_then(Value::as_u64);
        let request = match serde_json::from_value::<SessionRequest>(raw_request.clone()) {
            Ok(request) => request,
            Err(error) => {
                let request_type = raw_request.get("type").and_then(Value::as_str);
                let code = if request_type.is_some_and(|kind| {
                    !matches!(kind, "hello" | "execute" | "replace" | "shutdown")
                }) {
                    "unknown_request_type"
                } else {
                    "invalid_request"
                };
                write_frame(
                    &mut stream,
                    &SessionResponse::typed_error(
                        request_id,
                        active_generation,
                        code,
                        format!("request rejected: {error}"),
                    ),
                )?;
                continue;
            }
        };
        if session.generation()? != active_generation {
            write_frame(
                &mut stream,
                &SessionResponse::typed_error(
                    request_id,
                    active_generation,
                    "reauthentication_required",
                    "session generation changed; open a fresh authenticated connection",
                ),
            )?;
            break;
        }
        match request {
            SessionRequest::Execute {
                id,
                generation,
                root: requested,
                source,
                timeout_ms,
            } => {
                if !ids.insert((generation, id)) {
                    write_frame(
                        &mut stream,
                        &SessionResponse::typed_error(
                            Some(id),
                            active_generation,
                            "duplicate_request_id",
                            "duplicate request id",
                        ),
                    )?;
                    continue;
                }
                if PathBuf::from(requested).canonicalize().ok().as_ref() != Some(&root) {
                    write_frame(
                        &mut stream,
                        &SessionResponse::typed_error(
                            Some(id),
                            active_generation,
                            "authorized_root_mismatch",
                            "authorized root mismatch",
                        ),
                    )?;
                    continue;
                }
                match session.execute(
                    generation,
                    id,
                    source,
                    Duration::from_millis(timeout_ms.unwrap_or(30_000).clamp(1, 3_600_000)),
                ) {
                    Ok(result) => write_frame(
                        &mut stream,
                        &SessionResponse::ok(Some(id), active_generation, result.value),
                    )?,
                    Err(error) => {
                        write_frame(
                            &mut stream,
                            &SessionResponse::typed_error_with_retry(
                                Some(id),
                                active_generation,
                                error.code.as_str(),
                                error.to_string(),
                                error.retry_after_ms,
                            ),
                        )?;
                        if session.generation()? != active_generation {
                            break;
                        }
                    }
                }
            }
            SessionRequest::Replace {
                id,
                generation,
                token: provided,
                reason,
            } => {
                if !ids.insert((generation, id)) {
                    write_frame(
                        &mut stream,
                        &SessionResponse::typed_error(
                            Some(id),
                            active_generation,
                            "duplicate_request_id",
                            "duplicate request id",
                        ),
                    )?;
                    continue;
                }
                if !constant_time_eq(provided.as_bytes(), shutdown_token.as_bytes()) {
                    write_frame(
                        &mut stream,
                        &SessionResponse::typed_error(
                            Some(id),
                            active_generation,
                            "replacement_capability_rejected",
                            "replacement capability rejected",
                        ),
                    )?;
                    continue;
                }
                match session.replace(generation, reason) {
                    Ok(receipt) => {
                        write_frame(
                            &mut stream,
                            &SessionResponse::ok(
                                Some(id),
                                active_generation,
                                serde_json::json!({
                                    "previous_generation": receipt.previous_generation,
                                    "generation": receipt.generation,
                                    "reason": receipt.reason.as_str(),
                                    "reauthentication_required": true,
                                }),
                            ),
                        )?;
                        break;
                    }
                    Err(error) => {
                        write_frame(
                            &mut stream,
                            &SessionResponse::typed_error_with_retry(
                                Some(id),
                                active_generation,
                                error.code.as_str(),
                                error.to_string(),
                                error.retry_after_ms,
                            ),
                        )?;
                        if session.generation()? != active_generation {
                            break;
                        }
                    }
                }
            }
            SessionRequest::Shutdown {
                id,
                token: provided,
            } => {
                let generation = session.generation()?;
                if !ids.insert((generation, id)) {
                    write_frame(
                        &mut stream,
                        &SessionResponse::typed_error(
                            Some(id),
                            generation,
                            "duplicate_request_id",
                            "duplicate request id",
                        ),
                    )?;
                    continue;
                }
                if !constant_time_eq(provided.as_bytes(), shutdown_token.as_bytes()) {
                    write_frame(
                        &mut stream,
                        &SessionResponse::typed_error(
                            Some(id),
                            generation,
                            "shutdown_capability_rejected",
                            "shutdown capability rejected",
                        ),
                    )?;
                    continue;
                }
                let shutdown_generation = session.shutdown()?;
                write_frame(
                    &mut stream,
                    &SessionResponse::ok(Some(id), shutdown_generation, Value::Null),
                )?;
                terminating.store(true, Ordering::Release);
                let _ = events.send(SessionEvent::Terminate(None));
                break;
            }
            SessionRequest::Hello { .. } => {
                let generation = session.generation()?;
                write_frame(
                    &mut stream,
                    &SessionResponse::typed_error(
                        Some(0),
                        generation,
                        "duplicate_hello",
                        "duplicate hello",
                    ),
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_runtime(dir: &std::path::Path, uid: u32) -> Result<(), Box<dyn std::error::Error>> {
    use std::{
        ffi::OsStr,
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt},
    };
    if dir.as_os_str() == OsStr::new("") {
        return Err("empty runtime dir".into());
    };
    match fs::symlink_metadata(dir) {
        Ok(m) => {
            if !m.is_dir() || m.uid() != uid || (m.mode() & 0o777) != 0o700 {
                return Err("runtime dir must be owned private directory".into());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(dir)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?
        }
        Err(e) => return Err(e.into()),
    };
    Ok(())
}
#[cfg(unix)]
fn entropy_u64() -> Result<u64, Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut b = [0u8; 8];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    let v = u64::from_ne_bytes(b);
    if v == 0 {
        Err("entropy generated zero generation".into())
    } else {
        Ok(v)
    }
}
#[cfg(unix)]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (&x, &y) in a.iter().zip(b) {
        d |= x ^ y
    }
    d == 0
}
#[cfg(unix)]
#[derive(Debug)]
struct SessionFrameError {
    code: &'static str,
    message: String,
    recoverable: bool,
    connection_closed: bool,
}

#[cfg(unix)]
impl std::fmt::Display for SessionFrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(unix)]
impl std::error::Error for SessionFrameError {}

#[cfg(unix)]
fn read_value_frame(r: &mut impl std::io::BufRead) -> Result<serde_json::Value, SessionFrameError> {
    let mut bytes = Vec::new();
    let mut limited = std::io::Read::take(
        &mut *r,
        (zero_codemode::session::MAX_SESSION_FRAME + 1) as u64,
    );
    let count = std::io::BufRead::read_until(&mut limited, b'\n', &mut bytes).map_err(|error| {
        SessionFrameError {
            code: "frame_io_error",
            message: format!("request frame read failed: {error}"),
            recoverable: false,
            connection_closed: false,
        }
    })?;
    if count == 0 {
        return Err(SessionFrameError {
            code: "connection_closed",
            message: "connection closed".into(),
            recoverable: false,
            connection_closed: true,
        });
    }
    if bytes.len() > zero_codemode::session::MAX_SESSION_FRAME || bytes.last() != Some(&b'\n') {
        return Err(SessionFrameError {
            code: "oversized_frame",
            message: "request frame exceeds the session limit".into(),
            recoverable: false,
            connection_closed: false,
        });
    }
    serde_json::from_slice(&bytes).map_err(|error| SessionFrameError {
        code: "invalid_frame",
        message: format!("request frame is not valid JSON: {error}"),
        recoverable: true,
        connection_closed: false,
    })
}

#[cfg(unix)]
fn read_frame(
    r: &mut impl std::io::BufRead,
) -> Result<zero_codemode::session::SessionRequest, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(read_value_frame(r)?)?)
}
#[cfg(unix)]
fn write_frame(
    w: &mut impl std::io::Write,
    v: &zero_codemode::session::SessionResponse,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut b = serde_json::to_vec(v)?;
    if b.len().saturating_add(1) > zero_codemode::session::MAX_SESSION_FRAME {
        return Err("response exceeds frame bound".into());
    }
    b.push(b'\n');
    w.write_all(&b)?;
    w.flush()?;
    Ok(())
}
