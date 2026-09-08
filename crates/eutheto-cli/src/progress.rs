//! Presentation-only progress; terminal authority never travels through this lossy channel.

use super::{Cli, Command, Outcome, ProgressArg, SafeCliError, SafeCliWarning, execute_cli};
use eutheto_solver_api::{OutputError, ProgressSink, SolveProgressEvent};
use eutheto_types::CancellationToken;
use std::io::Write;
use tokio::sync::mpsc;

struct PresentationProgress(Option<mpsc::Sender<SolveProgressEvent>>);

impl ProgressSink for PresentationProgress {
    fn emit(&mut self, event: SolveProgressEvent) -> Result<(), OutputError> {
        if let Some(sender) = &self.0 {
            // Dropping a full/closed presentation channel cannot alter solver or commit authority.
            let _ = sender.try_send(event);
        }
        Ok(())
    }
}

pub(super) async fn run(
    cli: Cli,
    cancellation: CancellationToken,
    stderr: &mut impl Write,
) -> Result<Outcome, (&'static str, SafeCliError)> {
    let mode = match &cli.command {
        Command::Solve(args) => args.progress,
        _ => ProgressArg::None,
    };
    if mode == ProgressArg::None {
        return Box::pin(execute_cli(
            cli,
            cancellation,
            &mut PresentationProgress(None),
        ))
        .await;
    }
    let (sender, mut receiver) = mpsc::channel(32);
    let mut sink = PresentationProgress(Some(sender));
    let mut execution = Box::pin(execute_cli(cli, cancellation, &mut sink));
    let mut failed_output = false;
    let result = loop {
        tokio::select! {
            result = &mut execution => break result,
            Some(event) = receiver.recv() => {
                if !failed_output {
                    failed_output = render(stderr, mode, &event).is_err();
                }
            }
        }
    };
    while let Ok(event) = receiver.try_recv() {
        if !failed_output {
            failed_output = render(stderr, mode, &event).is_err();
        }
    }
    result.map(|outcome| {
        if failed_output {
            outcome.with_warning(SafeCliWarning {
                code: "solve.progress_output_failed".to_owned(),
                message:
                    "Progress output was interrupted; the terminal result remains authoritative."
                        .to_owned(),
                details: None,
            })
        } else {
            outcome
        }
    })
}

fn render(
    stderr: &mut impl Write,
    mode: ProgressArg,
    event: &SolveProgressEvent,
) -> std::io::Result<()> {
    // Even safe backend log lines are not necessary to expose useful progress.
    if matches!(event, SolveProgressEvent::LogLine(_)) {
        return Ok(());
    }
    match mode {
        ProgressArg::None => return Ok(()),
        ProgressArg::Jsonl => {
            serde_json::to_writer(
                &mut *stderr,
                &serde_json::json!({
                    "kind": "progress",
                    "publication": "pending",
                    "event": event,
                }),
            )
            .map_err(std::io::Error::other)?;
            writeln!(stderr)?;
        }
        ProgressArg::Human => {
            let message = match event {
                SolveProgressEvent::Queued => "Optimize queued.",
                SolveProgressEvent::Compiling { .. } => "Preparing the immutable scenario.",
                SolveProgressEvent::BackendStarted { .. } => "Optimizer started.",
                SolveProgressEvent::PresolveSummary(_) => "Optimizer preparation completed.",
                SolveProgressEvent::IncumbentFound(_) => {
                    "Candidate found; final verification and publication are pending."
                }
                SolveProgressEvent::BoundImproved(_) => "Optimizer bound improved.",
                SolveProgressEvent::Verifying => "Independently verifying the candidate.",
                SolveProgressEvent::Explaining => "Preparing explanation evidence.",
                SolveProgressEvent::Completed(_) => {
                    "Optimization finished; authoritative finalization is pending."
                }
                SolveProgressEvent::LogLine(_) => return Ok(()),
            };
            writeln!(stderr, "{message}")?;
        }
    }
    stderr.flush()
}
