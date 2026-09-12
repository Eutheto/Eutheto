include!("src/generated_command_catalog.rs");

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

const MAX_SOLVER_MANIFEST_BYTES: u64 = 1024 * 1024;
const SOLVER_MANIFEST_DIGEST_ENV: &str = "EUTHETO_ORTOOLS_MANIFEST_SHA256";
const UNTRUSTED_MANIFEST_SHA256: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("CARGO_FEATURE_BUNDLED_ORTOOLS").is_some() {
        println!("cargo:rerun-if-env-changed={SOLVER_MANIFEST_DIGEST_ENV}");
        if std::env::var_os(SOLVER_MANIFEST_DIGEST_ENV).is_some() {
            embed_solver_manifest_digest()?;
        } else {
            println!("cargo:rustc-env={SOLVER_MANIFEST_DIGEST_ENV}={UNTRUSTED_MANIFEST_SHA256}");
        }
    }
    let mut attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(REGISTERED_COMMANDS));
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest =
            PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        let windows = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            // Tauri's resource helper links only application binaries. The library
            // unit-test executable also needs Common Controls v6 for RFD's
            // TaskDialogIndirect import. Embed once for every MSVC link target.
            println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
            tauri_build::WindowsAttributes::new_without_app_manifest()
        } else {
            tauri_build::WindowsAttributes::new()
                .app_manifest(include_str!("windows-app-manifest.xml"))
        };
        attributes = attributes.windows_attributes(windows);
    }
    tauri_build::try_build(attributes)?;
    Ok(())
}

fn embed_solver_manifest_digest() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or("Cargo did not provide the desktop crate manifest directory")?,
    )
    .join("sidecar/resources/solver-manifest.json");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let expected = std::env::var(SOLVER_MANIFEST_DIGEST_ENV)
        .map_err(|_| "bundled OR-Tools build requires a trusted manifest digest handoff")?;
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("trusted OR-Tools manifest digest must be lowercase SHA-256".into());
    }

    let metadata = std::fs::symlink_metadata(&manifest)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_SOLVER_MANIFEST_BYTES
    {
        return Err("bundled OR-Tools manifest must be a bounded direct regular file".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&manifest)?
        .take(MAX_SOLVER_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SOLVER_MANIFEST_BYTES {
        return Err("bundled OR-Tools manifest exceeds the byte limit".into());
    }

    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}")?;
    }
    if encoded != expected {
        return Err("staged OR-Tools manifest differs from the trusted build handoff".into());
    }
    println!("cargo:rustc-env=EUTHETO_ORTOOLS_MANIFEST_SHA256={expected}");
    Ok(())
}
