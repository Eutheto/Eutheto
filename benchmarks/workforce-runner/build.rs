use std::error::Error;
use std::fmt::Write;
use std::path::{Path, PathBuf};

fn collect(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed={}", directory.display());
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err("runner source inventory contains a symlink".into());
        }
        if kind.is_dir() {
            collect(&entry.path(), paths)?;
        } else if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "rs")
        {
            paths.push(entry.path());
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let root =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest directory")?);
    let target = std::env::var("TARGET")?;
    println!("cargo:rustc-env=EUTHETO_BENCH_TARGET={target}");
    println!(
        "cargo:rustc-env=EUTHETO_BENCH_PROFILE={}",
        std::env::var("PROFILE")?
    );
    let compiler = std::process::Command::new(std::env::var_os("RUSTC").ok_or("missing compiler")?)
        .arg("--version")
        .output()?;
    if !compiler.status.success() {
        return Err("compiler version command failed".into());
    }
    println!(
        "cargo:rustc-env=EUTHETO_BENCH_RUSTC={}",
        std::str::from_utf8(&compiler.stdout)?.trim()
    );
    let mut paths = vec![root.join("Cargo.toml"), root.join("build.rs")];
    collect(&root.join("src"), &mut paths)?;
    // Match the normalized portable-path byte order used by runtime hash framing.
    paths.sort_by_cached_key(|path| path.to_string_lossy().replace('\\', "/"));
    let mut source = String::from("pub(crate) const BUILD_SOURCES: &[(&str, &[u8])] = &[\n");
    for path in paths {
        let relative = path
            .strip_prefix(&root)?
            .to_str()
            .ok_or("non-UTF8 source path")?
            .replace('\\', "/");
        println!("cargo:rerun-if-changed={}", path.display());
        writeln!(
            source,
            "({relative:?}, include_bytes!({:?})),",
            path.to_str().ok_or("non-UTF8 source path")?
        )?;
    }
    source.push_str("];\n");
    let output =
        PathBuf::from(std::env::var_os("OUT_DIR").ok_or("missing build output directory")?);
    std::fs::write(output.join("source_inventory.rs"), source)?;
    Ok(())
}
