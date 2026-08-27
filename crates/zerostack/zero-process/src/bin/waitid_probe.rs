fn main() {
    unsafe {
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("3");
        let mut child = command.spawn().unwrap();
        let pid = child.id();
        for attempt in 0..5 {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            std::thread::sleep(std::time::Duration::from_millis(50));
            let probe = libc::waitid(libc::P_PID, pid as libc::id_t, &mut info,
                libc::WEXITED | libc::WNOWAIT | libc::WNOHANG);
            println!("attempt {attempt}: rc={probe} si_pid={} err={}", info.si_pid,
                std::io::Error::last_os_error());
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}
