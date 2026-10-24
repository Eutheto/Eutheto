// SPDX-License-Identifier: Apache-2.0
use sha2::{Digest, Sha256};
use std::{env, fs, io::Read, path::PathBuf};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    worker_sha256: String,
    renderer: Renderer,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Renderer {
    version: u32,
    typst_version: String,
    template_sha256: String,
    fonts_sha256: String,
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=EUTHETO_PDF_MANIFEST_PATH");
    println!("cargo:rerun-if-env-changed=EUTHETO_PDF_MANIFEST_SHA256");
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?)
        .join("recipient-pdf-manifest.json");
    match (
        env::var_os("EUTHETO_PDF_MANIFEST_PATH"),
        env::var_os("EUTHETO_PDF_MANIFEST_SHA256"),
    ) {
        (None, None) => fs::write(output, [])?,
        (Some(path), Some(expected)) => {
            let expected = expected
                .into_string()
                .map_err(|_| "PDF manifest digest must be Unicode")?;
            let path = PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            if !fs::symlink_metadata(&path)?.is_file() {
                return Err("PDF manifest must be a regular file".into());
            }
            let mut bytes = Vec::new();
            fs::File::open(path)?.take(4097).read_to_end(&mut bytes)?;
            if bytes.is_empty()
                || bytes.len() > 4096
                || expected.len() != 64
                || Sha256::digest(&bytes)
                    .iter()
                    .flat_map(|byte| [byte >> 4, byte & 0x0f])
                    .map(|nibble| char::from(b"0123456789abcdef"[usize::from(nibble)]))
                    .collect::<String>()
                    != expected
            {
                return Err("PDF manifest handoff is oversized or has an incorrect digest".into());
            }
            let manifest: Manifest = serde_json::from_slice(&bytes)?;
            if manifest.schema_version != 1
                || manifest.renderer.version != 1
                || manifest.renderer.typst_version.is_empty()
                || manifest.renderer.typst_version.len() > 64
                || !digest(&manifest.worker_sha256)
                || !digest(&manifest.renderer.template_sha256)
                || !digest(&manifest.renderer.fonts_sha256)
            {
                return Err("PDF manifest schema or renderer identity is invalid".into());
            }
            fs::write(output, bytes)?;
        }
        _ => return Err("PDF manifest path and digest must be supplied together".into()),
    }
    Ok(())
}
