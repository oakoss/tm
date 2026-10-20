use std::io::Read;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use crate::support::tm_command;

/// Bare `tm` without a terminal prints help and exits 0, so an Agent never
/// waits on an interactive screen (design.md "Interface").
#[test]
fn bare_tm_without_a_terminal_prints_help_and_exits() {
    let mut child = tm_command()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("tm binary should start");
    // Read while waiting, so output larger than a pipe cannot pass for a hang.
    let drain = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut text = String::new();
            pipe.read_to_string(&mut text).map(|_| text)
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("bare tm without a terminal did not exit");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stderr) = (
        stdout.join().unwrap().unwrap(),
        stderr.join().unwrap().unwrap(),
    );
    assert!(status.success(), "{status:?}: {stderr}");
    assert!(stdout.contains("Usage: tm"), "{stdout}");
    assert_eq!(stderr, "");
}
