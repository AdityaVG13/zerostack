use std::process::Command;
use std::time::{Duration, Instant};
use zero_process::VerifiedChild;

fn main() {
    let mut command = Command::new("/bin/sleep");
    command.arg("2");
    let started = Instant::now();
    let (child, _pipes) =
        VerifiedChild::spawn_tree_with_pipes(command, "probe-session", 0).unwrap();
    println!("spawn returned after {:?}", started.elapsed());
    let mut polls = 0;
    let deadline = started + Duration::from_secs(10);
    loop {
        polls += 1;
        if child.wait_for_exit(Duration::from_millis(10)) {
            println!("exit observed at {:?} after {polls} polls", started.elapsed());
            break;
        }
        if Instant::now() >= deadline {
            println!("no exit by deadline after {polls} polls");
            break;
        }
    }
}
