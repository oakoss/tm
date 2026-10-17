use std::process::Command;

#[test]
fn binary_runs() {
    let status = Command::new(env!("CARGO_BIN_EXE_tm"))
        .status()
        .expect("tm binary should start");
    assert!(status.success());
}
