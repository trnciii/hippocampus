pub mod git;

use anyhow::Result;
use std::path::{Path, PathBuf};

pub trait Vcs {
    fn add(&self, repo_root: &Path, path: &Path) -> Result<()>;
    fn commit(&self, repo_root: &Path, message: &str) -> Result<()>;
    fn push(&self, repo_root: &Path) -> Result<()>;
    fn pull(&self, repo_root: &Path) -> Result<()>;
    fn fetch(&self, repo_root: &Path) -> Result<()>;
    fn status(&self, repo_root: &Path) -> Result<String>;
    fn diff(&self, repo_root: &Path) -> Result<String>;
    fn has_changes(&self, repo_root: &Path, path: &Path) -> Result<bool>;
}

/// Detect VCS by walking up the filesystem. Returns (backend, vcs_root).
/// The root is determined by Rust path operations so it's always a
/// native (Windows-compatible) path, never a UNIX path from a subprocess.
pub fn detect_vcs(repo_path: &Path) -> Result<(Box<dyn Vcs>, PathBuf)> {
    let start = if repo_path.is_relative() {
        std::env::current_dir()?.join(repo_path)
    } else {
        repo_path.to_path_buf()
    };

    // Walk up to find .git
    let mut dir = start.as_path();
    loop {
        if dir.join(".git").exists() {
            return Ok((Box::new(git::Git), dir.to_path_buf()));
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => anyhow::bail!("Repo side: no git repository found at {}", repo_path.display()),
        }
    }
}
