use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

pub fn ensure_inside_repo(repo_root: &Path, path: &Path) -> Result<PathBuf> {
    let root = std::fs::canonicalize(repo_root).context("canonicalize root")?;
    let abs = std::fs::canonicalize(repo_root.join(path))
        .or_else(|_| -> std::result::Result<PathBuf, std::io::Error> { Ok(root.join(path)) })?;
    if !abs.starts_with(&root) {
        anyhow::bail!("path escapes repo root");
    }
    Ok(abs)
}

pub fn backup_path(backup_root: &Path, abs: &Path, repo_root: &Path) -> Result<PathBuf> {
    let rel = abs
        .strip_prefix(std::fs::canonicalize(repo_root)?)
        .unwrap_or(abs);
    Ok(backup_root.join(rel))
}

pub fn backup_and_write(abs: &Path, new_contents: &str, backup_file: &Path) -> Result<()> {
    if let Some(parent) = backup_file.parent() {
        fs::create_dir_all(parent).ok();
    }
    if abs.exists() {
        if let Err(e) = fs::copy(abs, backup_file) {
            eprintln!("warning: failed to backup {}: {e}", abs.display());
        }
    }
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut tmp = NamedTempFile::new_in(abs.parent().unwrap_or(Path::new(".")))
        .context("create temp file")?;
    std::io::Write::write_all(&mut tmp, new_contents.as_bytes()).context("write temp")?;
    tmp.persist(abs).context("atomic swap")?;
    Ok(())
}

pub fn smol_dir() -> Result<PathBuf> {
    let here = std::env::current_dir()?;
    let p = here.join(".smol");
    std::fs::create_dir_all(&p).ok();
    Ok(p)
}
