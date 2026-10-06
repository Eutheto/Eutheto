#![cfg(windows)]
#![forbid(unsafe_code)]

use process_wrap::tokio::{CommandWrap, JobObject, KillOnDrop};
use std::{
    process::{Command, Stdio},
    time::Duration,
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
