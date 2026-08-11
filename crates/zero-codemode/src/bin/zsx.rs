#![forbid(unsafe_code)]

#[cfg(unix)]
type SessionStream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type SessionStream = zero_process::PipeConnection;

#[cfg(unix)]
type SessionEndpoint = std::path::PathBuf;
#[cfg(windows)]
type SessionEndpoint = String;

#[cfg(unix)]
struct SessionChild(Option<std::process::Child>);
#[cfg(windows)]
struct SessionChild(Option<zero_process::VerifiedChild>);

#[cfg(any(unix, windows))]
fn main() {
    let response = run().unwrap_or_else(
        |e| serde_json::json!({"protocol":"zerostack-session/v1","ok":false,"error":e.to_string()}),
    );
    println!("{}", serde_json::to_string(&response).unwrap());
    if response.get("ok") == Some(&serde_json::Value::Bool(false)) {
        std::process::exit(1)
    }
}

#[cfg(not(any(unix, windows)))]
fn main() {
    println!(
        "{\"protocol\":\"zerostack-session/v1\",\"ok\":false,\"error\":\"unsupported platform\"}"
    );
    std::process::exit(2)
}

#[cfg(any(unix, windows))]
fn run() -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    use std::{
        io::{BufReader, Read},
        path::PathBuf,
        process::{Command, Stdio},
    };
    use zero_codemode::session::{
        SESSION_PROTOCOL, SESSION_SHUTDOWN_TOKEN_ENV, SESSION_SOCKET_ENV, SESSION_TOKEN_ENV,
    };
    let mut a = std::env::args().skip(1);
    if a.next().as_deref() != Some("exec") {
        return Err("usage: zsx exec -C ROOT [--file PLAN]".into());
    }
    let (mut root, mut file) = (None, None);
    while let Some(k) = a.next() {
        match k.as_str() {
            "-C" => root = a.next().map(PathBuf::from),
            "--file" => file = a.next().map(PathBuf::from),
            _ => return Err(format!("unknown argument {k}").into()),
        }
    }
    let root = root.ok_or("missing -C ROOT")?.canonicalize()?;
    let source = if let Some(p) = file {
        std::fs::read_to_string(p)?
    } else {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    };
    let mut child = None;
    let mut shutdown_token = None;
    let (endpoint, token, ready_generation) = match (
        std::env::var(SESSION_SOCKET_ENV),
        std::env::var(SESSION_TOKEN_ENV),
    ) {
        (Ok(endpoint), Ok(token)) => (endpoint_from_env(endpoint)?, token, None),
        (Err(std::env::VarError::NotPresent), Err(std::env::VarError::NotPresent)) => {
            let token = random_capability()?;
            let stop_token = random_capability()?;
            let runtime_nonce = random_capability()?;
            let dir = session_runtime_dir(&runtime_nonce);
            let endpoint = endpoint_for_runtime(&dir)?;
            let owner = zero_process::ProcessIdentity::current()?.encode();
            let bin = std::env::current_exe()?.with_file_name("zerostack-session");
            let mut command = Command::new(bin);
            command
                .args([
                    "serve",
                    "--root",
                    root.to_str().ok_or("non UTF-8 root")?,
                    "--runtime-dir",
                    dir.to_str().ok_or("non UTF-8 runtime dir")?,
                    "--owner",
                    &owner,
                ])
                .env(SESSION_TOKEN_ENV, &token)
                .env(SESSION_SHUTDOWN_TOKEN_ENV, &stop_token)
                .env(SESSION_SOCKET_ENV, endpoint_as_env(&endpoint))
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit());
            let (mut launched, stdout) = spawn_session(command)?;
            let ready = read(&mut BufReader::new(stdout))?;
            if ready["type"] != "ready" || ready["protocol"] != SESSION_PROTOCOL {
                launched.terminate();
                return Err("invalid session ready handshake".into());
            }
            let generation = ready["generation"]
                .as_u64()
                .filter(|value| *value != 0)
                .ok_or("invalid session generation")?;
            shutdown_token = Some(stop_token);
            child = Some(launched);
            (endpoint, token, Some(generation))
        }
        _ => return Err("incomplete inherited session endpoint".into()),
    };
    let mut stream = connect_session(&endpoint)?;
    set_stream_timeouts(&stream)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    send(
        &mut stream,
        &serde_json::json!({"type":"hello","protocol":SESSION_PROTOCOL,"token":token}),
    )?;
    let hello = read(&mut reader)?;
    if hello["protocol"] != SESSION_PROTOCOL || hello["ok"] != true {
        return Err("session authentication failed".into());
    }
    let generation = hello["generation"]
        .as_u64()
        .filter(|value| *value != 0)
        .ok_or("invalid authenticated generation")?;
    if ready_generation.is_some_and(|ready| ready != generation) {
        return Err("session generation changed during startup".into());
    }
    let request_nonce = random_capability()?;
    let request_id = u64::from_str_radix(&request_nonce[..16], 16)?.max(1);
    send(
        &mut stream,
        &serde_json::json!({"type":"execute","id":request_id,"generation":generation,"root":root,"source":source,"timeout_ms":30000}),
    )?;
    let result = read(&mut reader)?;
    if result["protocol"] != SESSION_PROTOCOL
        || result["id"] != request_id
        || result["generation"] != generation
    {
        return Err("invalid execute response binding".into());
    }
    if let (Some(c), Some(stop_token)) = (child.as_mut(), shutdown_token) {
        let shutdown_nonce = random_capability()?;
        let shutdown_id = u64::from_str_radix(&shutdown_nonce[..16], 16)?.max(1);
        send(
            &mut stream,
            &serde_json::json!({"type":"shutdown","id":shutdown_id,"token":stop_token}),
        )?;
        let stopped = read(&mut reader)?;
        if stopped["protocol"] != SESSION_PROTOCOL
            || stopped["id"] != shutdown_id
            || stopped["generation"] != generation
            || stopped["ok"] != true
        {
            return Err("invalid shutdown response binding".into());
        }
        if !c.wait_success()? {
            return Err("session shutdown failed".into());
        }
    }
    Ok(result)
}
#[cfg(unix)]
fn session_runtime_dir(nonce: &str) -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp").join(format!("zsx-{}-{}", std::process::id(), &nonce[..32]))
}

#[cfg(windows)]
fn session_runtime_dir(nonce: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "zerostack-session-{}-{}",
        std::process::id(),
        &nonce[..32]
    ))
}

#[cfg(unix)]
fn endpoint_from_env(value: String) -> Result<SessionEndpoint, Box<dyn std::error::Error>> {
    let endpoint = std::path::PathBuf::from(value);
    if !endpoint.is_absolute() {
        return Err("inherited session endpoint must be absolute".into());
    }
    Ok(endpoint)
}

#[cfg(windows)]
fn endpoint_from_env(value: String) -> Result<SessionEndpoint, Box<dyn std::error::Error>> {
    if !value.starts_with(r"\\.\pipe\zerostack-session-") || value.contains('\0') {
        return Err("inherited session endpoint is not a ZeroStack named pipe".into());
    }
    Ok(value)
}

#[cfg(unix)]
fn endpoint_for_runtime(
    dir: &std::path::Path,
) -> Result<SessionEndpoint, Box<dyn std::error::Error>> {
    Ok(dir.join("session.sock"))
}

#[cfg(windows)]
fn endpoint_for_runtime(
    dir: &std::path::Path,
) -> Result<SessionEndpoint, Box<dyn std::error::Error>> {
    let stem = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("runtime dir has no UTF-8 name")?;
    if stem.is_empty()
        || !stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("runtime dir name is not pipe-safe".into());
    }
    Ok(format!(r"\\.\pipe\{stem}"))
}

#[cfg(unix)]
fn endpoint_as_env(endpoint: &SessionEndpoint) -> std::ffi::OsString {
    endpoint.as_os_str().to_owned()
}

#[cfg(windows)]
fn endpoint_as_env(endpoint: &SessionEndpoint) -> std::ffi::OsString {
    endpoint.into()
}

#[cfg(unix)]
fn connect_session(endpoint: &SessionEndpoint) -> std::io::Result<SessionStream> {
    std::os::unix::net::UnixStream::connect(endpoint)
}

#[cfg(windows)]
fn connect_session(endpoint: &SessionEndpoint) -> std::io::Result<SessionStream> {
    zero_process::PipeConnection::connect(endpoint)
}

#[cfg(unix)]
fn set_stream_timeouts(stream: &SessionStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(40)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(2)))
}

#[cfg(windows)]
fn set_stream_timeouts(stream: &SessionStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(40)));
    stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    Ok(())
}

#[cfg(unix)]
fn spawn_session(
    mut command: std::process::Command,
) -> std::io::Result<(SessionChild, std::process::ChildStdout)> {
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "missing session stdout")
    })?;
    Ok((SessionChild(Some(child)), stdout))
}

#[cfg(windows)]
fn spawn_session(
    command: std::process::Command,
) -> std::io::Result<(SessionChild, std::process::ChildStdout)> {
    let (child, pipes) =
        zero_process::VerifiedChild::spawn_tree_with_pipes(command, "zsx-sidecar", 0)?;
    let stdout = pipes.stdout.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "missing session stdout")
    })?;
    Ok((SessionChild(Some(child)), stdout))
}

#[cfg(unix)]
impl SessionChild {
    fn terminate(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn wait_success(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        let mut child = self.0.take().ok_or("session child already reaped")?;
        Ok(child.wait()?.success())
    }
}

#[cfg(windows)]
impl SessionChild {
    fn terminate(&mut self) {
        if let Some(child) = self.0.take() {
            let _ = child.signal_graceful_for("zsx-sidecar", 0, std::time::Duration::ZERO);
            let _ = child.revoke();
        }
    }

    fn wait_success(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        let child = self.0.take().ok_or("session child already reaped")?;
        if !child.wait_for_exit(std::time::Duration::from_secs(5)) {
            let _ = child.signal_graceful_for("zsx-sidecar", 0, std::time::Duration::ZERO);
        }
        child.revoke()?;
        Ok(child
            .terminal_status()
            .is_some_and(|status| status.success()))
    }
}

#[cfg(any(unix, windows))]
impl Drop for SessionChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(any(unix, windows))]
fn send(
    w: &mut impl std::io::Write,
    v: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = serde_json::to_vec(v)?;
    if bytes.len().saturating_add(1) > zero_codemode::session::MAX_SESSION_FRAME {
        return Err("request exceeds frame bound".into());
    }
    bytes.push(b'\n');
    w.write_all(&bytes)?;
    w.flush()?;
    Ok(())
}

#[cfg(any(unix, windows))]
fn read(r: &mut impl std::io::BufRead) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    let mut limited = std::io::Read::take(
        &mut *r,
        (zero_codemode::session::MAX_SESSION_FRAME + 1) as u64,
    );
    let read = std::io::BufRead::read_until(&mut limited, b'\n', &mut bytes)?;
    if read == 0
        || bytes.len() > zero_codemode::session::MAX_SESSION_FRAME
        || bytes.last() != Some(&b'\n')
    {
        return Err("invalid or oversized response frame".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(any(unix, windows))]
fn random_capability() -> Result<String, Box<dyn std::error::Error>> {
    let mut bytes = [0u8; 32];
    zero_process::fill_random(&mut bytes)?;
    let mut encoded = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(encoded)
}
