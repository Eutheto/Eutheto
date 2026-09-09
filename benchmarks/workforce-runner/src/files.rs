use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use eutheto_types::{CancellationToken, OperationControl};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub(crate) fn open_regular(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path).context("opening direct corpus file")?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x0000_0400 == 0,
            "reparse-point input"
        );
    }
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "input must be a direct regular file"
    );
    Ok(file)
}

pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    read_controlled(path, limit, &control())
}

pub(crate) fn read_controlled(
    path: &Path,
    limit: u64,
    control: &OperationControl,
) -> Result<Vec<u8>> {
    control
        .check()
        .map_err(|_| anyhow::anyhow!("input operation interrupted"))?;
    let mut file = open_regular(path)?;
    let length = file.metadata()?.len();
    ensure!(length <= limit, "input byte limit exceeded");
    let mut bytes = Vec::with_capacity(usize::try_from(length)?);
    let mut buffer = [0_u8; 16_384];
    loop {
        control
            .check()
            .map_err(|_| anyhow::anyhow!("input operation interrupted"))?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        ensure!(
            u64::try_from(bytes.len())?
                .checked_add(u64::try_from(count)?)
                .is_some_and(|total| total <= limit),
            "input byte limit exceeded"
        );
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(bytes)
}

pub(crate) fn hash_file(path: &Path, limit: u64) -> Result<String> {
    let mut file = open_regular(path)?;
    ensure!(
        file.metadata()?.len() <= limit,
        "hash input byte limit exceeded"
    );
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 16_384];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(u64::try_from(count)?)
            .context("hash length overflow")?;
        ensure!(bytes <= limit, "hash input byte limit exceeded");
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn digest(value: &str) -> Result<[u8; 32]> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid SHA-256"
    );
    let mut result = [0_u8; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(result)
}

/// Corpus paths are a closed repository-relative inventory, not arbitrary file selectors.
pub(crate) fn contained(root: &Path, relative: &str) -> Result<PathBuf> {
    eutheto_export::validate_portable_path(relative, 256)
        .map_err(|_| anyhow::anyhow!("unsafe corpus path"))?;
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("unsafe corpus path");
        };
        path.push(name);
        let metadata = std::fs::symlink_metadata(&path).context("checking corpus containment")?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "symlink corpus component"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x0000_0400 == 0,
                "reparse-point corpus component"
            );
        }
    }
    ensure!(
        path.canonicalize()?.starts_with(root.canonicalize()?),
        "corpus path escapes repository"
    );
    Ok(path)
}

pub(crate) fn control() -> OperationControl {
    OperationControl::Cancellation(CancellationToken::new())
}

pub(crate) fn publish_json(destination: &Path, value: &impl Serialize) -> Result<()> {
    let control = control();
    let publication = eutheto_export::prepare_json_atomic_controlled(destination, value, &control)?;
    publication.publish_controlled(&control)?;
    Ok(())
}

pub(crate) fn publish_evidence(destination: &Path, value: &impl Serialize) -> Result<()> {
    eutheto_domain_api::bounded_json_size(value, crate::contract::MAX_EVIDENCE_BYTES)?;
    let text = serde_json::to_string(value)?;
    let control = control();
    // Internal typed evidence includes binary digests, not portable scenario content.
    // Preserve the shared bounded, checked, atomic no-clobber byte-publication boundary.
    let publication = eutheto_export::prepare_text_atomic_controlled(destination, &text, &control)?;
    publication.publish_controlled(&control)?;
    Ok(())
}

/// Creates only direct repository directories; generated publication never follows a leaf link.
pub(crate) fn generated_destination(root: &Path, relative: &str) -> Result<PathBuf> {
    eutheto_export::validate_portable_path(relative, 256)
        .map_err(|_| anyhow::anyhow!("unsafe generated path"))?;
    let relative = Path::new(relative);
    let parent = relative.parent().context("missing generated parent")?;
    let mut directory = root.to_path_buf();
    for component in parent.components() {
        let Component::Normal(name) = component else {
            anyhow::bail!("unsafe generated component");
        };
        directory.push(name);
        match std::fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = std::fs::symlink_metadata(&directory)?;
        ensure!(
            metadata.is_dir() && !link_like(&metadata),
            "generated parent must be a direct directory"
        );
    }
    let destination = directory.join(relative.file_name().context("missing generated filename")?);
    match std::fs::symlink_metadata(&destination) {
        Ok(metadata) => ensure!(
            metadata.is_file() && !link_like(&metadata),
            "generated destination must be a direct regular file"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(destination)
}

fn link_like(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x0000_0400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}
