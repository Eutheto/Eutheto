mod architecture;
mod fixtures;
mod generate;
mod phase02_generate;
mod protocol;
mod protocol_generate;
mod release;
mod solver;
mod solver_artifact;
mod solver_manifest;
mod source_contract;
mod supply_chain;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "xtask",
    about = "Repository-owned deterministic maintenance tasks"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Regenerate checked-in deterministic sources.
    Generate,
    /// Verify that checked-in generated sources have no drift.
    GenerateCheck,
    /// Verify versioned worker-protocol assets.
    Protocol {
        #[command(subcommand)]
        command: ProtocolCommand,
    },
    /// Validate repository fixture discovery and filesystem safety.
    Fixtures {
        #[command(subcommand)]
        command: FixturesCommand,
    },
    /// Verify workspace dependency direction and required architecture crates.
    Architecture {
        #[command(subcommand)]
        command: ArchitectureCommand,
    },
    /// Native solver-worker build and deferred installation operations.
    Solver {
        #[command(subcommand)]
        command: SolverCommand,
    },
    /// Third-party license artifact operations.
    Licenses {
        #[command(subcommand)]
        command: GenerateCommand,
    },
    /// Software-bill-of-materials operations.
    Sbom {
        #[command(subcommand)]
        command: GenerateCommand,
    },
    /// Release verification and assembly operations.
    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ArchitectureCommand {
    /// Reject missing architecture crates and forbidden transitive dependency paths.
    Verify,
}

#[derive(Debug, Subcommand)]
enum ProtocolCommand {
    /// Verify every checked-in semantic-JSON/framed-protobuf fixture pair and declared limit.
    Verify,
}

#[derive(Debug, Subcommand)]
enum FixturesCommand {
    /// Discover fixture roots and reject empty files, symlinks, and invalid paths.
    Validate,
}

#[derive(Debug, Subcommand)]
enum SolverCommand {
    /// Build, validate, publish, and stage the native Windows `x86_64` worker.
    BuildNative,
    /// Build a desktop bundle from one trusted, manifest-bound sidecar handoff.
    BuildDesktop,
    /// Build and atomically publish a manifest-bound CLI and worker image.
    BuildCli,
    /// Exercise a relocated packaged CLI through real file and stored Workforce solves.
    SmokeCli {
        #[arg(long)]
        image: PathBuf,
    },
    /// Build the pinned Nix worker and atomically stage its Tauri sidecar.
    InstallFromNix,
    /// Validate and handshake with one exact packaged worker/resource pair.
    Smoke {
        #[arg(long)]
        executable: PathBuf,
        #[arg(long)]
        resource_root: PathBuf,
        /// Trusted SHA-256 of the manifest embedded by the desktop build.
        #[arg(long)]
        manifest_sha256: String,
    },
    /// Finalize a pristine target artifact from reviewed repository and build authorities.
    FinalizeArtifact {
        #[arg(long)]
        authority_root: PathBuf,
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        artifact_root: PathBuf,
        #[arg(long)]
        target_triple: String,
        #[arg(long)]
        compiler_identity: String,
        #[arg(long)]
        compiler_version: String,
        #[arg(long)]
        source_date: String,
    },
    /// Assemble canonical installed solver evidence from explicit inputs.
    AssembleManifest {
        #[arg(long)]
        source_contract: PathBuf,
        #[arg(long)]
        protocol_schema: PathBuf,
        #[arg(long)]
        protocol_policy: PathBuf,
        #[arg(long)]
        build_evidence: PathBuf,
        #[arg(long)]
        payload_evidence: PathBuf,
        #[arg(long)]
        artifact_root: PathBuf,
    },
    /// Validate canonical manifest bytes, authority, and every referenced artifact.
    ValidateManifest {
        #[arg(long)]
        source_contract: PathBuf,
        #[arg(long)]
        protocol_schema: PathBuf,
        #[arg(long)]
        protocol_policy: PathBuf,
        #[arg(long)]
        artifact_root: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum GenerateCommand {
    Generate,
}

#[derive(Debug, Subcommand)]
enum ReleaseCommand {
    VerifyClean,
    AssembleManifest,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Solver { command } => run_solver(command),
        command => {
            let root = repository_root()?;
            match command {
                Command::Generate => generate::generate(&root),
                Command::GenerateCheck => {
                    generate::check(&root)?;
                    supply_chain::check_licenses(&root)?;
                    supply_chain::check_sbom(&root)?;
                    Ok(())
                }
                Command::Protocol {
                    command: ProtocolCommand::Verify,
                } => protocol::verify(&root),
                Command::Fixtures {
                    command: FixturesCommand::Validate,
                } => fixtures::validate(&root),
                Command::Architecture {
                    command: ArchitectureCommand::Verify,
                } => architecture::verify(&root),
                Command::Solver { .. } => {
                    unreachable!("solver commands are handled before root lookup")
                }
                Command::Licenses {
                    command: GenerateCommand::Generate,
                } => supply_chain::generate_licenses(&root).map_err(anyhow::Error::from),
                Command::Sbom {
                    command: GenerateCommand::Generate,
                } => supply_chain::generate_sbom(&root).map_err(anyhow::Error::from),
                Command::Release { command } => match command {
                    ReleaseCommand::VerifyClean => release::verify_clean(&root),
                    ReleaseCommand::AssembleManifest => release::assemble_manifest(),
                },
            }
        }
    }
}
fn run_solver(command: SolverCommand) -> Result<()> {
    match command {
        SolverCommand::FinalizeArtifact {
            authority_root,
            work_root,
            artifact_root,
            target_triple,
            compiler_identity,
            compiler_version,
            source_date,
        } => solver::finalize_artifact(
            &authority_root,
            &work_root,
            &artifact_root,
            &target_triple,
            &compiler_identity,
            &compiler_version,
            &source_date,
        ),
        SolverCommand::AssembleManifest {
            source_contract,
            protocol_schema,
            protocol_policy,
            build_evidence,
            payload_evidence,
            artifact_root,
        } => solver::assemble_manifest(
            &source_contract,
            &protocol_schema,
            &protocol_policy,
            &build_evidence,
            &payload_evidence,
            &artifact_root,
        ),
        SolverCommand::ValidateManifest {
            source_contract,
            protocol_schema,
            protocol_policy,
            artifact_root,
        } => solver::validate_manifest(
            &source_contract,
            &protocol_schema,
            &protocol_policy,
            &artifact_root,
        ),
        SolverCommand::BuildNative => solver::build_native(&repository_root()?),
        SolverCommand::BuildDesktop => solver::build_desktop(&repository_root()?),
        SolverCommand::BuildCli => solver::build_cli(&repository_root()?),
        SolverCommand::SmokeCli { image } => solver::smoke_cli(&repository_root()?, &image),
        SolverCommand::InstallFromNix => solver::install_from_nix(&repository_root()?),
        SolverCommand::Smoke {
            executable,
            resource_root,
            manifest_sha256,
        } => solver::smoke(
            &repository_root()?,
            &executable,
            &resource_root,
            &manifest_sha256,
        ),
    }
}

fn repository_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .context("xtask manifest directory has no repository parent")
}
