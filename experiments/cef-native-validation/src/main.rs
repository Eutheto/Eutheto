#![forbid(unsafe_code)]

#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, mpsc},
    time::{Instant, timeout},
};

const LIMIT: usize = 16 * 1024 * 1024;
const STAGES: [&str; 8] = [
    "startup", "context", "browser", "loaded", "ready", "printing", "printed", "closed",
];
const FAULTS: [&str; 16] = [
    "failed",
    "renderer-exit",
    "deny-popup",
    "deny-navigation",
    "deny-tab",
    "deny-favicon",
    "deny-resource",
    "deny-handler",
    "deny-protocol",
    "deny-download",
    "deny-print-dialog",
    "deny-print-job",
    "mac-teardown-watchdog",
    "mac-session-watchdog",
    "mac-sigterm-watchdog",
    "diagnostic-self-stack-resume-failed",
];
const DIAGNOSTICS: [&str; 17] = [
    "lifecycle-loop-returned",
    "lifecycle-shutdown-entered",
    "lifecycle-shutdown-returned",
    "lifecycle-probe-returned",
    "lifecycle-pool-drained",
    "lifecycle-unload-entered",
    "lifecycle-unload-returned",
    "diagnostic-self-stack-captured",
    "diagnostic-self-stack-unavailable",
    "diagnostic-nearest-mach-message",
    "diagnostic-nearest-pthread-join",
    "diagnostic-nearest-condition-wait",
    "diagnostic-nearest-semaphore-wait",
    "diagnostic-nearest-ulock-wait",
    "diagnostic-nearest-dispatch-wait",
    "diagnostic-nearest-audio-dispose",
    "diagnostic-nearest-unknown",
];
fn invalid() -> io::Error {
    io::Error::other("probe boundary failure")
}
fn emit(value: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{value}")?;
    out.flush()
}
fn termination() -> io::Result<impl std::future::Future<Output = ()>> {
    #[cfg(unix)]
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    #[cfg(windows)]
    let mut interrupt = tokio::signal::windows::ctrl_c()?;
    Ok(async move {
        #[cfg(unix)]
        tokio::select! { _ = term.recv() => (), _ = interrupt.recv() => () }
        #[cfg(windows)]
        {
            interrupt.recv().await;
        }
    })
}
fn group_empty(pid: u32) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use nix::{
            errno::Errno,
            sys::signal::{Signal, killpg},
            unistd::Pid,
        };
        match killpg(
            Pid::from_raw(i32::try_from(pid).map_err(|_| invalid())?),
            None::<Signal>,
        ) {
            Err(Errno::ESRCH) => Ok(true),
            Ok(()) => Ok(false),
            Err(error) => Err(io::Error::from_raw_os_error(error as i32)),
        }
    }
    #[cfg(windows)]
    {
        let _ = pid;
        Ok(true)
    } // Patched JobObject::try_wait requires ACTIVE_PROCESS_ZERO.
}
async fn settle(child: &mut dyn ChildWrapper, pid: u32) -> io::Result<ExitStatus> {
    if let Err(error) = child.start_kill() {
        if !group_empty(pid)? {
            return Err(error);
        }
    }
    timeout(Duration::from_secs(5), async {
        loop {
            // Unix root polling may already have reaped the root, bypassing
            // ProcessGroupChild's waitpid cache. Keep the same root observer.
            #[cfg(unix)]
            let status = child.inner_mut().try_wait()?;
            #[cfg(windows)]
            let status = child.try_wait()?;
            if let Some(status) = status {
                if group_empty(pid)? {
                    return Ok(status);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| invalid())?
}
async fn stages(
    mut pipe: impl AsyncRead + Unpin,
    sender: mpsc::Sender<&'static str>,
) -> io::Result<()> {
    let mut buffer = [0; 256];
    let mut line = [0; 128];
    let mut used = 0;
    let mut total = 0usize;
    loop {
        let count = pipe.read(&mut buffer).await?;
        if count == 0 {
            return if used == 0 { Ok(()) } else { Err(invalid()) };
        }
        total += count;
        if total > 4096 {
            return Err(invalid());
        }
        for byte in &buffer[..count] {
            if *byte == b'\n' {
                let text = std::str::from_utf8(&line[..used]).map_err(|_| invalid())?;
                let event = STAGES
                    .iter()
                    .chain(FAULTS.iter())
                    .chain(DIAGNOSTICS.iter())
                    .copied()
                    .find(|event| *event == text)
                    .ok_or_else(invalid)?;
                sender.send(event).await.map_err(|_| invalid())?;
                used = 0;
            } else {
                if used == line.len() {
                    return Err(invalid());
                }
                line[used] = *byte;
                used += 1;
            }
        }
    }
}
async fn native_stderr(
    mut pipe: impl AsyncRead + Unpin,
    sender: mpsc::Sender<&'static str>,
) -> io::Result<()> {
    let mut buffer = [0; 4096];
    let mut total = 0usize;
    let mut line = [0; 128];
    let mut used = 0;
    let mut overflow = false;
    let mut emitted = [false; 3];
    let watchdogs: [(&[u8], &str); 3] = [
        (
            b"Teardown watchdog expired; recording dump and terminating.",
            "mac-teardown-watchdog",
        ),
        (
            b"SessionEnding watchdog expired; recording dump and terminating.",
            "mac-session-watchdog",
        ),
        (
            b"SIGTERM shutdown watchdog expired; recording dump and re-raising.",
            "mac-sigterm-watchdog",
        ),
    ];
    loop {
        let count = pipe.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        total += count;
        if total > 65536 {
            return Err(invalid());
        }
        for byte in &buffer[..count] {
            if *byte == b'\n' {
                if !overflow {
                    for (index, (literal, event)) in watchdogs.iter().enumerate() {
                        if !emitted[index] && &line[..used] == *literal {
                            sender.send(*event).await.map_err(|_| invalid())?;
                            emitted[index] = true;
                        }
                    }
                }
                used = 0;
                overflow = false;
            } else if used < line.len() {
                line[used] = *byte;
                used += 1;
            } else {
                overflow = true;
            }
        }
    }
}
async fn log_pipe(
    mut pipe: impl AsyncRead + Unpin,
    log: Arc<Mutex<(tokio::fs::File, usize)>>,
) -> io::Result<()> {
    let mut buffer = [0; 8192];
    loop {
        let count = pipe.read(&mut buffer).await?;
        if count == 0 {
            return log.lock().await.0.flush().await;
        }
        let mut output = log.lock().await;
        output.1 += count;
        if output.1 > 2 * 1024 * 1024 {
            return Err(invalid());
        }
        output.0.write_all(&buffer[..count]).await?;
    }
}

async fn run() -> io::Result<bool> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() > 64 || args.iter().map(|arg| arg.len()).sum::<usize>() > 16384 {
        return Err(invalid());
    }
    let tool_mode = args
        .first()
        .is_some_and(|arg| arg == OsStr::new("--command"));
    let (host, cwd, mode, seconds, html, log) = if tool_mode {
        if args.len() < 4 {
            return Err(invalid());
        }
        let seconds: u64 = args[1]
            .to_str()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        if !(1..=1200).contains(&seconds) {
            return Err(invalid());
        }
        let log_path = PathBuf::from(&args[2]);
        if !log_path.is_absolute() {
            return Err(invalid());
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(log_path)?;
        let log = Arc::new(Mutex::new((tokio::fs::File::from_std(file), 0)));
        (
            PathBuf::from(&args[3]),
            std::env::current_dir()?,
            "command",
            seconds,
            Vec::new(),
            Some(log),
        )
    } else {
        if args.len() != 4 {
            return Err(invalid());
        }
        let mode = args[3].to_str().ok_or_else(invalid)?;
        if ![
            "normal",
            "oversized",
            "startup-cancel",
            "ready-cancel",
            "printing-cancel",
            "deadline",
            "observe",
            "shutdown-diagnostic",
        ]
        .contains(&mode)
        {
            return Err(invalid());
        }
        if mode == "shutdown-diagnostic" && !cfg!(target_os = "macos") {
            return Err(invalid());
        }
        let input = PathBuf::from(&args[2]);
        if !input.is_absolute() {
            return Err(invalid());
        }
        // Only the synthetic oversized case delivers the first forbidden byte.
        let bound = LIMIT + usize::from(mode == "oversized");
        let metadata = std::fs::symlink_metadata(&input)?;
        if !metadata.is_file() || metadata.len() > bound as u64 {
            return Err(invalid());
        }
        let mut html = Vec::with_capacity(metadata.len() as usize);
        std::fs::File::open(input)?
            .take((bound + 1) as u64)
            .read_to_end(&mut html)?;
        if html.is_empty() || html.len() > bound {
            return Err(invalid());
        }
        (
            PathBuf::from(&args[0]),
            PathBuf::from(&args[1]),
            mode,
            if mode == "deadline" { 5 } else { 60 },
            html,
            None,
        )
    };
    if !host.is_absolute() || !cwd.is_absolute() {
        return Err(invalid());
    }
    let mut command = CommandWrap::with_new(host.as_os_str(), |command| {
        command
            .env_clear()
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if tool_mode {
            // Python supplies its closed compiler environment, never ambient CI credentials.
            command
                .envs(std::env::vars_os())
                .args(&args[4..])
                .stdin(Stdio::null());
        } else {
            command.stdin(Stdio::piped());
            for key in ["SystemRoot", "WINDIR", "LANG", "LC_ALL"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            command
                .env("HOME", &cwd)
                .env("USERPROFILE", &cwd)
                .env("TEMP", &cwd)
                .env("TMP", &cwd)
                .env("TMPDIR", &cwd)
                .env("EUTHETO_PROBE_JOB_DIR", &cwd);
            if mode == "shutdown-diagnostic" {
                command.env("EUTHETO_PROBE_SHUTDOWN_DIAGNOSTIC", "1");
            }
            if ["ready-cancel", "deadline", "observe"].contains(&mode) {
                command.env("EUTHETO_PROBE_HOLD_READY", "1");
            }
            if std::env::var_os("EUTHETO_PROBE_POPUP_TEST").as_deref() == Some(OsStr::new("1")) {
                command.env("EUTHETO_PROBE_POPUP_TEST", "1");
            }
        }
    });
    command.wrap(KillOnDrop);
    #[cfg(unix)]
    command.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(JobObject);
    let interrupted = termination()?;
    tokio::pin!(interrupted);
    let mut child = command.spawn()?;
    let pid = child.id().ok_or_else(invalid)?;
    if emit(&format!("pid {pid}")).is_err() {
        let _ = settle(child.as_mut(), pid).await;
        return Err(invalid());
    }
    if !tool_mode {
        // Native stdin stays open and empty until the parent retains an identity.
        let mut ack = [0];
        let mut control = tokio::io::stdin();
        let admitted = tokio::select! {
            result = timeout(Duration::from_secs(10), control.read_exact(&mut ack)) => matches!(result, Ok(Ok(_))) && ack == [b'a'],
            _ = &mut interrupted => false,
        };
        if !admitted {
            let settled = settle(child.as_mut(), pid).await.is_ok();
            emit("reason initial-observation-failure")?;
            emit(&format!("settled {}", u8::from(settled)))?;
            return Ok(false);
        }
    }
    let stdout = child.stdout().take();
    let stderr = child.stderr().take();
    let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
        let _ = settle(child.as_mut(), pid).await;
        return Err(invalid());
    };
    let stdin = child.stdin().take();
    let withheld = mode == "startup-cancel";
    let mut writer = tokio::spawn(async move {
        if tool_mode {
            return Ok(());
        }
        let mut stdin = stdin.ok_or_else(invalid)?;
        if withheld {
            std::future::pending::<()>().await;
        }
        stdin.write_all(&html).await?;
        stdin.shutdown().await
    });
    let (sender, mut receiver) = mpsc::channel(32);
    let (mut reader, mut diagnostics) = if let Some(log) = log {
        drop(sender);
        (
            tokio::spawn(log_pipe(stdout, Arc::clone(&log))),
            tokio::spawn(log_pipe(stderr, log)),
        )
    } else {
        (
            tokio::spawn(stages(stdout, sender.clone())),
            tokio::spawn(native_stderr(stderr, sender)),
        )
    };
    let mut next_stage = 0;
    let mut fault = None;
    let mut reason = "complete";
    let mut writer_done = false;
    let mut reader_done = false;
    let mut diagnostics_done = false;
    let mut events_done = false;
    let mut status = None;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut poll = tokio::time::interval(Duration::from_millis(10));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = poll.tick() => {
                // Observe the direct root separately; only settle() establishes
                // the complete owned group/job terminal condition.
                match child.inner_mut().try_wait() {
                    Ok(value) => {
                        status = value;
                        if status.is_some_and(|value| !value.success()) {
                            fault.get_or_insert("native-failure");
                        }
                    }
                    Err(_) => { reason = "native-failure"; break; }
                }
                if Instant::now() >= deadline { reason = "deadline"; break; }
            }
            result = &mut writer, if !writer_done => {
                writer_done = true;
                if !matches!(result, Ok(Ok(()))) { reason = "pipe-failure"; break; }
            }
            result = &mut reader, if !reader_done => {
                reader_done = true;
                if !matches!(result, Ok(Ok(()))) { reason = "protocol-failure"; break; }
            }
            result = &mut diagnostics, if !diagnostics_done => {
                diagnostics_done = true;
                if !matches!(result, Ok(Ok(()))) { reason = "pipe-failure"; break; }
            }
            event = receiver.recv(), if !events_done => {
                if let Some(event) = event {
                    if event.starts_with("diagnostic-") && mode != "shutdown-diagnostic" {
                        reason = "protocol-failure"; break;
                    }
                    if FAULTS.contains(&event) { fault.get_or_insert("native-failure"); }
                    else if STAGES.get(next_stage).copied() == Some(event) { next_stage += 1; }
                    else if !DIAGNOSTICS.contains(&event) && !(fault.is_some() && event == "closed") { reason = "protocol-failure"; break; }
                    if emit(event).is_err() { reason = "pipe-failure"; break; }
                    if (mode == "startup-cancel" && event == "startup") || (mode == "ready-cancel" && event == "ready") || (mode == "printing-cancel" && event == "printing") {
                        reason = "stage-cancel"; break;
                    }
                } else { events_done = true; }
            }
            _ = &mut interrupted => { reason = "external-cancel"; break; }
        }
        if status.is_some() && writer_done && reader_done && diagnostics_done && events_done {
            break;
        }
    }
    if !matches!(reason, "protocol-failure" | "pipe-failure") {
        if let Some(first) = fault {
            reason = first;
        } else if reason == "complete" && status.is_some_and(|value| !value.success()) {
            reason = "native-failure";
        }
    }
    let settled = match settle(child.as_mut(), pid).await {
        Ok(value) => {
            status = Some(value);
            true
        }
        Err(_) => false,
    };
    writer.abort();
    reader.abort();
    diagnostics.abort();
    let _ = timeout(Duration::from_secs(1), async {
        if !writer_done {
            let _ = writer.await;
        }
        if !reader_done {
            let _ = reader.await;
        }
        if !diagnostics_done {
            let _ = diagnostics.await;
        }
    })
    .await;
    drop(child);
    emit(&format!("reason {reason}"))?;
    emit(&format!(
        "exit {}",
        status.and_then(|value| value.code()).unwrap_or(-1)
    ))?;
    emit(&format!("settled {}", u8::from(settled)))?;
    Ok(reason == "complete"
        && (tool_mode || (mode == "normal" && next_stage == STAGES.len()))
        && settled
        && status.is_some_and(|value| value.success()))
}
fn main() {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => std::process::exit(70),
    };
    let success = matches!(runtime.block_on(run()), Ok(true));
    runtime.shutdown_timeout(Duration::from_secs(1));
    std::process::exit(if success { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn stderr_publishes_only_complete_exact_watchdogs_once() {
        let (sender, mut receiver) = mpsc::channel(4);
        let literal = b"Teardown watchdog expired; recording dump and terminating.\n";
        let mut input = b"PRIVATE_SENTINEL ".to_vec();
        input.extend_from_slice(literal);
        input.extend_from_slice(&[b'x'; 129]);
        input.extend_from_slice(literal);
        input.extend_from_slice(literal);
        input.extend_from_slice(literal);
        input.extend_from_slice(b"SessionEnding watchdog expired; recording dump and terminating.");
        native_stderr(input.as_slice(), sender).await.unwrap();
        assert_eq!(receiver.recv().await, Some("mac-teardown-watchdog"));
        assert_eq!(receiver.recv().await, None);
        let (sender, _) = mpsc::channel(4);
        assert!(native_stderr(&[b'x'; 65537][..], sender).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn reaped_root_settles_with_empty_unix_group() {
        let mut command = CommandWrap::with_new("/bin/sh", |command| {
            command
                .args(["-c", "exit 0"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
        });
        command.wrap(KillOnDrop).wrap(ProcessGroup::leader());
        let mut child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(status) = child.inner_mut().try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(settle(child.as_mut(), pid).await.unwrap().success());
    }
}
