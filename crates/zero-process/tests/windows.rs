//! cfg(windows) native coverage: identity capture/mismatch/dead owner, owner
//! crash via the retained-handle watcher, private current-user-only pipe ACL,
//! peer SID verification, normal close, endpoint disappearance, nested
//! descendants inside the Job Object tree, and <=1s bounded cleanup.
#![cfg(all(windows, feature = "test-fixture"))]

use std::io::{Read, Write};
use std::process::Command;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, LocalFree};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl,
    SE_DACL_PROTECTED,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
use zero_process::{
    OwnerWatchError, OwnerWatcher, PipeConnection, PipeListener, ProcessIdentity, VerifiedChild,
};

fn fixture() -> Command {
    Command::new(env!("CARGO_BIN_EXE_process_fixture"))
}

fn fixture_pid_file(tag: &str) -> std::path::PathBuf {
    let nonce = format!(
        "{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::env::temp_dir().join(nonce)
}

fn read_pid_file(path: &std::path::Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse() {
                return pid;
            }
        }
        assert!(Instant::now() < deadline, "fixture pid file never appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn process_exists(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: probe-only open; the handle is closed immediately.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    // SAFETY: our own probe handle.
    unsafe { CloseHandle(handle) };
    true
}

#[test]
fn identity_capture_mismatch_and_dead_owner() {
    let mut child = fixture().arg("sleep").spawn().unwrap();
    let identity = ProcessIdentity::capture(child.id()).unwrap();
    assert!(identity.is_live().unwrap());

    // Tampered start key: never live, watcher construction rejects it.
    let mut stale = identity.clone();
    stale.start_key.push_str("-stale");
    assert!(!stale.is_live().unwrap());
    assert!(matches!(
        OwnerWatcher::new(stale),
        Err(OwnerWatchError::IdentityChanged)
    ));

    // Dead owner: capture goes not-found, liveness is false.
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(ProcessIdentity::capture(child.id()).is_err());
    assert!(!identity.is_live().unwrap());
    assert!(matches!(
        OwnerWatcher::new(identity),
        Err(OwnerWatchError::IdentityChanged)
    ));
}

#[test]
fn owner_watcher_wait_returns_on_owner_crash_within_one_second() {
    let mut child = fixture().arg("sleep").spawn().unwrap();
    let identity = ProcessIdentity::capture(child.id()).unwrap();
    let watcher = OwnerWatcher::new(identity).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    let started = Instant::now();
    watcher
        .wait()
        .expect("blocking wait returns after owner crash");
    assert!(started.elapsed() < Duration::from_secs(1));
}

fn unique_pipe_name(tag: &str) -> String {
    format!(
        r"\\.\pipe\zerostack-7n05-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[test]
fn private_acl_grants_only_current_user() {
    let name = unique_pipe_name("acl");
    let listener = PipeListener::new(&name).unwrap();
    let handle = listener.instance_handle();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: handle is our pipe instance; outputs receive pointers inside the
    // LocalAlloc'd descriptor returned below.
    let rc = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(rc, ERROR_SUCCESS, "GetSecurityInfo failed");
    let mut control = 0u16;
    let mut revision = 0u32;
    // SAFETY: descriptor is the live GetSecurityInfo allocation.
    assert_ne!(
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
        0,
        "security descriptor control unavailable"
    );
    assert_ne!(control & SE_DACL_PROTECTED, 0, "DACL must be protected");
    // SAFETY: the DACL is live and reports exactly one ACE.
    let acl = unsafe { &*dacl };
    assert_eq!(acl.AceCount, 1, "DACL must contain exactly one ACE");
    let mut raw_ace: *mut core::ffi::c_void = std::ptr::null_mut();
    assert_ne!(unsafe { GetAce(dacl, 0, &mut raw_ace) }, 0);
    let ace = unsafe { &*raw_ace.cast::<ACCESS_ALLOWED_ACE>() };
    assert_eq!(u32::from(ace.Header.AceType), ACCESS_ALLOWED_ACE_TYPE);
    assert_eq!(ace.Mask, FILE_ALL_ACCESS);
    // SAFETY: descriptor is the owned allocation and every borrowed pointer
    // above is dead before this exact single free.
    unsafe { LocalFree(descriptor) };
}

#[test]
fn peer_verification_and_normal_close() {
    let name = unique_pipe_name("peer");
    let mut listener = PipeListener::new(&name).unwrap();
    let server = std::thread::spawn(move || {
        let mut connection = listener.accept().unwrap();
        let same_user = connection.peer_is_current_user().unwrap();
        let mut buffer = [0u8; 16];
        // Normal close: client drops -> read reaches EOF (0).
        let count = connection.read(&mut buffer).unwrap();
        (same_user, count)
    });
    let client = PipeConnection::connect(&name).unwrap();
    assert!(client.peer_is_current_user().unwrap());
    drop(client);
    let (same_user, count) = server.join().unwrap();
    assert!(same_user, "connected client SID must be the current user");
    assert_eq!(count, 0, "read after client close must report EOF");
}

#[test]
fn endpoint_disappearance() {
    let name = unique_pipe_name("missing");
    match PipeConnection::connect(&name) {
        Ok(_) => panic!("connect to a missing endpoint must fail"),
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
    }
}

#[test]
fn pending_accept_is_cancelled_without_polling() {
    let name = unique_pipe_name("cancel-accept");
    let mut listener = PipeListener::new(&name).unwrap();
    let canceller = listener.canceller().unwrap();
    let waiter = std::thread::spawn(move || listener.accept());
    std::thread::sleep(Duration::from_millis(50));
    canceller.cancel().unwrap();
    let error = match waiter.join().unwrap() {
        Ok(_) => panic!("accept must be cancelled"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
}

#[test]
fn late_accept_cancel_does_not_cancel_connected_io() {
    let name = unique_pipe_name("late-cancel");
    let mut listener = PipeListener::new(&name).unwrap();
    let canceller = listener.canceller().unwrap();
    let (accepted_tx, accepted_rx) = std::sync::mpsc::sync_channel(0);
    let (bytes_tx, bytes_rx) = std::sync::mpsc::sync_channel(0);
    let server = std::thread::spawn(move || {
        let mut connection = listener.accept().unwrap();
        accepted_tx.send(()).unwrap();
        let mut bytes = [0u8; 2];
        connection.read_exact(&mut bytes).unwrap();
        bytes_tx.send(bytes).unwrap();
    });
    let mut client = PipeConnection::connect(&name).unwrap();
    accepted_rx.recv().unwrap();
    canceller.cancel().unwrap();
    client.write_all(b"ok").unwrap();
    assert_eq!(
        bytes_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        *b"ok"
    );
    server.join().unwrap();
}

#[test]
fn nested_descendants_die_with_job_within_one_second() {
    let pid_file = fixture_pid_file("nested");
    let mut command = fixture();
    command
        .arg("spawn-sleeper")
        .arg("--pid-file")
        .arg(&pid_file);
    let (tree, _, _) =
        VerifiedChild::spawn_tree(command, "windows-tree", 0).expect("spawn isolated tree");
    let descendant = read_pid_file(&pid_file);
    let root = tree.child_id();
    assert!(process_exists(root), "root must be alive before teardown");
    assert!(
        process_exists(descendant),
        "descendant must be alive before teardown"
    );

    let started = Instant::now();
    tree.signal_graceful_for("windows-tree", 0, Duration::from_millis(500))
        .expect("bounded tree teardown");
    tree.revoke().expect("reap root");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "bounded cleanup exceeded 1s: {:?}",
        started.elapsed()
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    while (process_exists(root) || process_exists(descendant)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!process_exists(root), "root must be gone");
    assert!(
        !process_exists(descendant),
        "nested descendant must be gone"
    );
}
