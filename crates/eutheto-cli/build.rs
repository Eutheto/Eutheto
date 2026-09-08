#[path = "src/native_file.rs"]
mod native_file;

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::PathBuf;

const MANIFEST_PATH_ENV: &str = "EUTHETO_ORTOOLS_MANIFEST_PATH";
const MANIFEST_DIGEST_ENV: &str = "EUTHETO_ORTOOLS_MANIFEST_SHA256";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed={MANIFEST_PATH_ENV}");
    println!("cargo:rerun-if-env-changed={MANIFEST_DIGEST_ENV}");
    let (path, digest) = match (
        std::env::var_os(MANIFEST_PATH_ENV),
        std::env::var_os(MANIFEST_DIGEST_ENV),
    ) {
        (None, None) => {
            // An ordinary source build has no installed-worker authority.
            println!("cargo:rustc-env={MANIFEST_DIGEST_ENV}=");
            return Ok(());
        }
        (Some(path), Some(digest)) => (PathBuf::from(path), digest),
        _ => return Err("CLI worker build requires both manifest handoffs".into()),
    };
    let expected = digest
        .to_str()
        .ok_or("CLI worker digest must be lowercase SHA-256")?;
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("CLI worker digest must be lowercase SHA-256".into());
    }
    println!("cargo:rerun-if-changed={}", path.display());
    let file = native_file::open_regular(&path)?;
    if file.metadata()?.len() > MAX_MANIFEST_BYTES {
        return Err("CLI worker manifest exceeds the byte limit".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("CLI worker manifest exceeds the byte limit".into());
    }
    let mut actual = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut actual, "{byte:02x}")?;
    }
    if actual != expected {
        return Err("CLI worker manifest differs from the trusted build handoff".into());
    }
    println!("cargo:rustc-env={MANIFEST_DIGEST_ENV}={expected}");
    Ok(())
}
