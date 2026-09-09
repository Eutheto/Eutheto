#![forbid(unsafe_code)]

mod cases;
mod cli;
mod contract;
mod files;
mod generation;
mod headless;
mod schemas;

use anyhow::Result;
use clap::{Parser, Subcommand};
use contract::{CaseId, LargeParameters};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Deterministic Workforce corpus and real application-boundary evidence")]
struct Arguments {
    #[arg(long, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/../.."), global = true)]
    repository: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Generate,
    Check,
    Synthetic {
        #[arg(long)]
        people: u16,
        #[arg(long)]
        shifts: u16,
        #[arg(long)]
        output: PathBuf,
    },
    Run {
        #[arg(long)]
        artifact_root: PathBuf,
        #[arg(long)]
        manifest_sha256: String,
        #[arg(long)]
        output: PathBuf,
    },
    #[command(hide = true)]
    Sample {
        #[arg(long)]
        artifact_root: PathBuf,
        #[arg(long)]
        manifest_sha256: String,
        #[arg(long)]
        case: String,
        #[arg(long)]
        post_warmup: bool,
        #[arg(long)]
        output: PathBuf,
    },
    SmokeCli {
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        manifest_sha256: String,
        #[arg(long)]
        output: PathBuf,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match Box::pin(interruptible(Arguments::parse())).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Workforce corpus failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn interruptible(arguments: Arguments) -> Result<()> {
    let cancellation = eutheto_types::CancellationToken::new();
    let mut work = Box::pin(run(arguments, &cancellation));
    tokio::select! {
        biased;
        interrupted = tokio::signal::ctrl_c() => {
            cancellation.cancel();
            // Await the operation's real worker cleanup instead of dropping its future.
            let result = work.await;
            interrupted?;
            result?;
            anyhow::bail!("corpus operation interrupted");
        }
        result = &mut work => result,
    }
}

async fn run(arguments: Arguments, cancellation: &eutheto_types::CancellationToken) -> Result<()> {
    let root = arguments.repository.canonicalize()?;
    match arguments.command {
        Command::Generate => generation::generate(&root, false)?,
        Command::Check => {
            generation::generate(&root, true)?;
            generation::load(&root)?;
            println!(
                "Workforce corpus: deterministic generation, portable semantics and all13 variants verified"
            );
        }
        Command::Synthetic {
            people,
            shifts,
            output,
        } => generation::synthetic(&root, LargeParameters { people, shifts }, &output)?,
        Command::Run {
            artifact_root,
            manifest_sha256,
            output,
        } => {
            headless::run(
                &root,
                &artifact_root,
                &manifest_sha256,
                &output,
                cancellation,
            )
            .await?;
        }
        Command::Sample {
            artifact_root,
            manifest_sha256,
            case,
            post_warmup,
            output,
        } => {
            let case = CaseId::ALL
                .into_iter()
                .find(|id| id.slug() == case)
                .ok_or_else(|| anyhow::anyhow!("unknown corpus case"))?;
            Box::pin(headless::sample(
                &root,
                &artifact_root,
                &manifest_sha256,
                case,
                post_warmup,
                &output,
                cancellation,
            ))
            .await?;
        }
        Command::SmokeCli {
            image,
            manifest_sha256,
            output,
        } => {
            generation::generate(&root, true)?;
            let corpus = generation::load(&root)?;
            let image = image.canonicalize()?;
            let child_cancellation = cancellation.clone();
            let evidence = tokio::task::spawn_blocking(move || {
                cli::exercise(&image, &corpus, &manifest_sha256, &child_cancellation)
            })
            .await??;
            anyhow::ensure!(!cancellation.is_cancelled(), "corpus operation interrupted");
            files::publish_evidence(&output, &evidence)?;
        }
    }
    Ok(())
}
