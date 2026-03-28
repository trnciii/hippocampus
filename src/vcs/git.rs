use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

use super::Vcs;

pub struct Git;

impl Git {
    fn run(args: &[&str], cwd: &Path) -> Result<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .with_context(|| format!("Failed to run git {}", args.join(" ")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git {} failed: {}", args.join(" "), stderr.trim());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

impl Vcs for Git {
    fn add(&self, repo_root: &Path, path: &Path) -> Result<()> {
        Git::run(&["add", &path.to_string_lossy()], repo_root)?;
        Ok(())
    }

    fn commit(&self, repo_path: &Path, message: &str) -> Result<()> {
        Git::run(&["commit", "-m", message], repo_path)?;
        Ok(())
    }

    fn push(&self, repo_path: &Path) -> Result<()> {
        Git::run(&["push"], repo_path)?;
        Ok(())
    }

    fn pull(&self, repo_path: &Path) -> Result<()> {
        Git::run(&["pull"], repo_path)?;
        Ok(())
    }

    fn status(&self, repo_path: &Path) -> Result<String> {
        Git::run(&["status", "--short"], repo_path)
    }

    fn diff(&self, repo_path: &Path) -> Result<String> {
        Git::run(&["diff"], repo_path)
    }

    fn has_changes(&self, repo_root: &Path, path: &Path) -> Result<bool> {
        let out = Git::run(&["status", "--short", &path.to_string_lossy()], repo_root)?;
        Ok(!out.is_empty())
    }
}
