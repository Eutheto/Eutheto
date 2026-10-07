#![cfg(windows)]
#![forbid(unsafe_code)]

use process_wrap::tokio::{CommandWrap, JobObject, KillOnDrop};
use std::{
    process::{Command, Output, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[test]
#[ignore = "subprocess fixture, launched only inside the owned job"]
fn job_leaf() {
    std::thread::sleep(Duration::from_secs(30));
}

#[test]
#[ignore = "subprocess fixture, launched only inside the owned job"]
fn job_root() {
    let _child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "job_leaf"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Deliberately exit while the descendant remains alive in the same job.
}

#[test]
#[ignore = "subprocess fixture, launched only inside the owned job"]
fn job_exit_zero() {}

#[test]
#[ignore = "subprocess fixture, launched only inside the owned job"]
fn job_exit_nonzero() {
    std::process::exit(23);
}

fn run_owner_tool(seconds: u64, args: &[&str]) -> Output {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let log = std::env::temp_dir().join(format!(
        "cef-job-lifetime-{}-{nonce}.log",
        std::process::id()
    ));
    let mut command = Command::new(env!("CARGO_BIN_EXE_cef-native-validation-owner"));
    command
        .arg("--command")
        .arg(seconds.to_string())
        .arg(&log)
        .arg(std::env::current_exe().unwrap())
        .args(args);
    let output = command.output().unwrap();
    let _ = std::fs::remove_file(log);
    output
}

fn assert_owner_receipt(output: &Output, success: bool, reason: &str, exit: Option<&str>) {
    let receipt = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.success(), success, "{receipt}");
    let expected = format!("reason {reason}");
    assert!(
        receipt.lines().any(|line| line == expected.as_str()),
        "{receipt}"
    );
    assert!(receipt.lines().any(|line| line == "settled 1"), "{receipt}");
    if let Some(exit) = exit {
        let expected = format!("exit {exit}");
        assert!(
            receipt.lines().any(|line| line == expected.as_str()),
            "{receipt}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn root_exit_and_new_process_packets_do_not_establish_empty_job() {
    let mut command = CommandWrap::with_new(std::env::current_exe().unwrap(), |command| {
        command
            .args(["--ignored", "--exact", "job_root"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    });
    command.wrap(KillOnDrop).wrap(JobObject);
    let mut child = command.spawn().unwrap();
    let root = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = child.inner_mut().try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(root.success());
    assert!(
        child.try_wait().unwrap().is_none(),
        "a live descendant still owns job membership"
    );
    child.start_kill().unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(
        completed.success(),
        "retain the original successful root exit"
    );
    assert!(
        child.try_wait().unwrap().is_some(),
        "job-empty state persists after its packet is consumed"
    );
}

#[test]
fn plain_command_complete_receipt_follows_job_settlement() {
    let output = run_owner_tool(10, &["--ignored", "--exact", "job_exit_zero"]);
    assert_owner_receipt(&output, true, "complete", Some("0"));
}

#[test]
fn plain_command_nonzero_preserves_exit_and_settlement_receipt() {
    let output = run_owner_tool(10, &["--ignored", "--exact", "job_exit_nonzero"]);
    assert_owner_receipt(&output, false, "native-failure", Some("23"));
}

#[test]
fn plain_command_deadline_kills_and_settles_the_job() {
    let output = run_owner_tool(1, &["--ignored", "--exact", "job_leaf"]);
    assert_owner_receipt(&output, false, "deadline", None);
}

#[test]
fn plain_command_descendant_is_included_in_settlement() {
    let output = run_owner_tool(10, &["--ignored", "--exact", "job_root"]);
    assert_owner_receipt(&output, true, "complete", Some("0"));
}
