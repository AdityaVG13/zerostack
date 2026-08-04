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
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::Value;
    use std::{
        collections::BTreeSet,
        fs,
        io::{BufReader, Write},
        os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc,
        },
        thread,
        time::Duration,
    };
    use zero_codemode::session::{
        SessionExecutor, SessionRequest, SessionResponse, SESSION_PROTOCOL,
        SESSION_SHUTDOWN_TOKEN_ENV, SESSION_TOKEN_ENV,
    };
    use zerostack_machine_permit::session_owner::{
        current_euid, peer_euid, OwnerWatcher, ProcessIdentity,
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
    std::env::remove_var(SESSION_TOKEN_ENV);
    std::env::remove_var(SESSION_SHUTDOWN_TOKEN_ENV);
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
    std::env::set_var("ZEROSTACK_SESSION_ROOT", &root);
    let generation = entropy_u64()?;
    let session_id = format!("session-{generation:016x}");
    std::env::set_var("ZEROSTACK_SESSION_ID", &session_id);
    let exec = SessionExecutor::new()?;
    let watcher = OwnerWatcher::new(owner)?;
    enum Event {
        Client(UnixStream),
        Terminate(Option<String>),
    }
    let (tx, rx) = mpsc::sync_channel(8);
    let terminating = Arc::new(AtomicBool::new(false));
    let admission_token = token.clone();
    let listener_terminating = terminating.clone();
    let listener_cancellation = exec.cancellation();
    let watcher_tx = tx.clone();
    thread::spawn(move || {
        for incoming in listener.incoming() {
            match incoming {
                Ok(s) => match tx.try_send(Event::Client(s)) {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Full(Event::Client(mut stream))) => {
                        if peer_euid(&stream).ok() == Some(current_euid()) {
                            let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                            if let Ok(cloned) = stream.try_clone() {
                                if let Ok(SessionRequest::Hello {
                                    protocol,
                                    token: provided,
                                }) = read_frame(&mut BufReader::new(cloned))
                                {
                                    if protocol == SESSION_PROTOCOL
                                        && constant_time_eq(
                                            provided.as_bytes(),
                                            admission_token.as_bytes(),
                                        )
                                    {
                                        let _ =
                                            write_error(&mut stream, generation, "session busy");
                                    }
                                }
                            }
                        }
                    }
                    Err(mpsc::TrySendError::Full(Event::Terminate(_))) => unreachable!(),
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                },
                Err(error) => {
                    eprintln!("fatal session listener: {error}");
                    listener_terminating.store(true, Ordering::Release);
                    listener_cancellation.cancel();
                    let _ = tx.send(Event::Terminate(Some(format!(
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
        let _ = watcher_tx.send(Event::Terminate(error));
    });
    exec.execute("return null", Duration::from_secs(1))?;
    println!(
        "{}",
        serde_json::json!({"type":"ready","protocol":SESSION_PROTOCOL,"generation":generation})
    );
    std::io::stdout().flush()?;
    let mut stop = false;
    let mut fatal_error = None;
    while !stop {
        match rx.recv()? {
            Event::Terminate(error) => {
                fatal_error = error;
                break;
            }
            Event::Client(mut stream) => {
                if terminating.load(Ordering::Acquire) {
                    continue;
                }
                match peer_euid(&stream) {
                    Ok(uid) if uid == current_euid() => {}
                    Ok(uid) => {
                        eprintln!("peer identity rejected: {uid} != {}", current_euid());
                        let _ = write_error(&mut stream, generation, "peer identity rejected");
                        continue;
                    }
                    Err(error) => {
                        eprintln!("peer credential failure: {error}");
                        let _ = write_error(&mut stream, generation, "peer credential unavailable");
                        continue;
                    }
                }
                stream.set_read_timeout(Some(Duration::from_millis(250)))?;
                stream.set_write_timeout(Some(Duration::from_millis(250)))?;
                let mut reader = BufReader::new(stream.try_clone()?);
                let hello = match read_frame(&mut reader) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let authed = match hello {
                    SessionRequest::Hello {
                        protocol,
                        token: provided,
                    } => {
                        protocol == SESSION_PROTOCOL
                            && constant_time_eq(provided.as_bytes(), token.as_bytes())
                    }
                    _ => false,
                };
                if !authed {
                    let _ = write_error(&mut stream, generation, "authentication rejected");
                    continue;
                };
                write_frame(
                    &mut stream,
                    &SessionResponse::ok(
                        None,
                        generation,
                        serde_json::json!({"authenticated":true,"generation":generation}),
                    ),
                )?;
                let mut ids = BTreeSet::new();
                loop {
                    if terminating.load(Ordering::Acquire) {
                        break;
                    }
                    let req = match read_frame(&mut reader) {
                        Ok(v) => v,
                        Err(_) => break,
                    };
                    match req {
                        SessionRequest::Execute {
                            id,
                            generation: given,
                            root: requested,
                            source,
                            timeout_ms,
                        } => {
                            if !ids.insert(id) {
                                write_error_id(
                                    &mut stream,
                                    generation,
                                    id,
                                    "duplicate request id",
                                )?;
                                continue;
                            }
                            if given != generation {
                                write_error_id(&mut stream, generation, id, "stale generation")?;
                                continue;
                            }
                            if PathBuf::from(requested).canonicalize().ok().as_ref() != Some(&root)
                            {
                                write_error_id(
                                    &mut stream,
                                    generation,
                                    id,
                                    "authorized root mismatch",
                                )?;
                                continue;
                            }
                            let result = exec.execute(
                                &source,
                                Duration::from_millis(
                                    timeout_ms.unwrap_or(30000).clamp(1, 3_600_000),
                                ),
                            );
                            match result {
                                Ok(v) => write_frame(
                                    &mut stream,
                                    &SessionResponse::ok(Some(id), generation, v),
                                )?,
                                Err(e) => {
                                    write_error_id(&mut stream, generation, id, &e.to_string())?
                                }
                            }
                        }
                        SessionRequest::Shutdown {
                            id,
                            token: provided,
                        } => {
                            if !constant_time_eq(provided.as_bytes(), shutdown_token.as_bytes()) {
                                write_error_id(
                                    &mut stream,
                                    generation,
                                    id,
                                    "shutdown capability rejected",
                                )?;
                                continue;
                            }
                            write_frame(
                                &mut stream,
                                &SessionResponse::ok(Some(id), generation, Value::Null),
                            )?;
                            stop = true;
                            break;
                        }
                        SessionRequest::Hello { .. } => {
                            write_error_id(&mut stream, generation, 0, "duplicate hello")?
                        }
                    }
                }
            }
        }
    }
    drop(exec);
    if let Some(error) = fatal_error {
        return Err(error.into());
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
fn read_frame(
    r: &mut impl std::io::BufRead,
) -> Result<zero_codemode::session::SessionRequest, Box<dyn std::error::Error>> {
    let mut b = Vec::new();
    let mut limited = std::io::Read::take(
        &mut *r,
        (zero_codemode::session::MAX_SESSION_FRAME + 1) as u64,
    );
    let n = std::io::BufRead::read_until(&mut limited, b'\n', &mut b)?;
    if n == 0 || b.len() > zero_codemode::session::MAX_SESSION_FRAME || b.last() != Some(&b'\n') {
        return Err("invalid or oversized frame".into());
    }
    Ok(serde_json::from_slice(&b)?)
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
#[cfg(unix)]
fn write_error(
    w: &mut impl std::io::Write,
    g: u64,
    msg: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    write_frame(
        w,
        &zero_codemode::session::SessionResponse::error(None, g, msg),
    )
}
#[cfg(unix)]
fn write_error_id(
    w: &mut impl std::io::Write,
    g: u64,
    id: u64,
    msg: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    write_frame(
        w,
        &zero_codemode::session::SessionResponse::error(Some(id), g, msg),
    )
}
