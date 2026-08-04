#![forbid(unsafe_code)]
#[cfg(unix)]
fn main() {
    let response = run().unwrap_or_else(
        |e| serde_json::json!({"protocol":"zerostack-session/v1","ok":false,"error":e.to_string()}),
    );
    println!("{}", serde_json::to_string(&response).unwrap());
    if response.get("ok") == Some(&serde_json::Value::Bool(false)) {
        std::process::exit(1)
    }
}
#[cfg(not(unix))]
fn main() {
    println!(
        "{\"protocol\":\"zerostack-session/v1\",\"ok\":false,\"error\":\"unsupported platform\"}"
    );
    std::process::exit(2)
}
#[cfg(unix)]
fn run() -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    use std::{
        io::{BufReader, Read},
        os::unix::net::UnixStream,
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
    let (socket, token, ready_generation) = match (
        std::env::var(SESSION_SOCKET_ENV),
        std::env::var(SESSION_TOKEN_ENV),
    ) {
        (Ok(s), Ok(t)) => (PathBuf::from(s), t, None),
        (Err(std::env::VarError::NotPresent), Err(std::env::VarError::NotPresent)) => {
            let token = random_capability()?;
            let stop_token = random_capability()?;
            let runtime_nonce = random_capability()?;
            let dir = PathBuf::from("/tmp").join(format!(
                "zsx-{}-{}",
                std::process::id(),
                &runtime_nonce[..32]
            ));
            let socket = dir.join("session.sock");
            let owner =
                zerostack_machine_permit::session_owner::ProcessIdentity::current()?.encode();
            let bin = std::env::current_exe()?.with_file_name("zerostack-session");
            let mut c = Command::new(bin)
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
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()?;
            let ready = read(&mut BufReader::new(
                c.stdout.take().ok_or("missing session stdout")?,
            ))?;
            if ready["type"] != "ready" || ready["protocol"] != SESSION_PROTOCOL {
                let _ = c.kill();
                let _ = c.wait();
                return Err("invalid session ready handshake".into());
            }
            let generation = ready["generation"]
                .as_u64()
                .filter(|value| *value != 0)
                .ok_or("invalid session generation")?;
            shutdown_token = Some(stop_token);
            child = Some(c);
            (socket, token, Some(generation))
        }
        _ => return Err("incomplete inherited session endpoint".into()),
    };
    let mut stream = UnixStream::connect(&socket)?;
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
    send(
        &mut stream,
        &serde_json::json!({"type":"execute","id":1,"generation":generation,"root":root,"source":source,"timeout_ms":30000}),
    )?;
    let result = read(&mut reader)?;
    if result["protocol"] != SESSION_PROTOCOL
        || result["id"] != 1
        || result["generation"] != generation
    {
        return Err("invalid execute response binding".into());
    }
    if let (Some(c), Some(stop_token)) = (child.as_mut(), shutdown_token) {
        send(
            &mut stream,
            &serde_json::json!({"type":"shutdown","id":2,"token":stop_token}),
        )?;
        let stopped = read(&mut reader)?;
        if stopped["protocol"] != SESSION_PROTOCOL
            || stopped["id"] != 2
            || stopped["generation"] != generation
            || stopped["ok"] != true
        {
            return Err("invalid shutdown response binding".into());
        }
        let status = c.wait()?;
        if !status.success() {
            return Err("session shutdown failed".into());
        }
    }
    Ok(result)
}
#[cfg(unix)]
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
#[cfg(unix)]
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
#[cfg(unix)]
fn random_capability() -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut encoded = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(encoded)
}
