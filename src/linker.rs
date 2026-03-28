use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

const IGNORED_FILES: &[&str] = &[".gitignore", ".hippocampus.toml"];

/// Create a symlink at `link_path` pointing to `target` using a relative path.
pub fn create_symlink(target: &Path, link_path: &Path) -> Result<()> {
    let link_dir = link_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid link path: {}", link_path.display()))?;
    let rel_target = pathdiff::diff_paths(target, link_dir)
        .unwrap_or_else(|| target.to_path_buf());

    #[cfg(unix)]
    std::os::unix::fs::symlink(&rel_target, link_path)
        .with_context(|| format!("Failed to create symlink {} -> {}", link_path.display(), rel_target.display()))?;

    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&rel_target, link_path)
        .with_context(|| format!("Failed to create symlink {} -> {}. On Windows, enable Developer Mode or run as administrator.", link_path.display(), rel_target.display()))?;

    Ok(())
}

/// Sync symlinks from repo to project: create symlinks for files in repo_dotdir
/// that don't exist in project dotdir yet.
pub fn sync_links(dotdir_path: &Path, repo_dotdir_path: &Path) -> Result<Vec<String>> {
    let mut created = Vec::new();

    if !repo_dotdir_path.exists() {
        return Ok(created);
    }

    for entry in fs::read_dir(repo_dotdir_path)
        .with_context(|| format!("Failed to read {}", repo_dotdir_path.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();

        if IGNORED_FILES.contains(&name.as_str()) {
            continue;
        }
        if name.starts_with('.') {
            continue;
        }

        let link_path = dotdir_path.join(&name);
        let target = repo_dotdir_path.join(&name);

        if !link_path.exists() && !link_path.symlink_metadata().is_ok() {
            create_symlink(&target, &link_path)?;
            created.push(name);
        }
    }

    Ok(created)
}

/// Move real files from project dotdir to repo dotdir, replacing with symlinks.
/// Uses copy+delete instead of rename for cross-filesystem support.
pub fn absorb_files(
    dotdir_path: &Path,
    repo_dotdir_path: &Path,
    files: Option<&[String]>,
) -> Result<Vec<String>> {
    let mut absorbed = Vec::new();

    fs::create_dir_all(repo_dotdir_path)?;

    let entries: Vec<String> = match files {
        Some(file_list) => file_list.to_vec(),
        None => {
            let mut names = Vec::new();
            for entry in fs::read_dir(dotdir_path)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if IGNORED_FILES.contains(&name.as_str()) || name.starts_with('.') {
                    continue;
                }
                let ft = entry.file_type()?;
                if ft.is_file() {
                    names.push(name);
                }
            }
            names
        }
    };

    for name in &entries {
        let src = dotdir_path.join(name);
        let dst = repo_dotdir_path.join(name);

        if !src.exists() {
            eprintln!("  skip: {} (not found)", name);
            continue;
        }

        // Already a symlink — skip
        if src.symlink_metadata()?.file_type().is_symlink() {
            continue;
        }

        // Copy to repo, then remove original
        fs::copy(&src, &dst)
            .with_context(|| format!("Failed to copy {} to {}", src.display(), dst.display()))?;
        fs::remove_file(&src)
            .with_context(|| format!("Failed to remove {}", src.display()))?;

        // Create symlink back
        create_symlink(&dst, &src)?;
        absorbed.push(name.clone());
    }

    Ok(absorbed)
}

/// Delete files from repo dotdir that no longer exist in the project dotdir.
/// A file is considered deleted if there is no project-side entry at all
/// (neither a real file nor a symlink, live or dead).
/// Returns list of deleted filenames.
pub fn prune_deletions(dotdir_path: &Path, repo_dotdir_path: &Path) -> Result<Vec<String>> {
    let mut pruned = Vec::new();
    if !repo_dotdir_path.exists() {
        return Ok(pruned);
    }
    for entry in fs::read_dir(repo_dotdir_path)
        .with_context(|| format!("Failed to read {}", repo_dotdir_path.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if IGNORED_FILES.contains(&name.as_str()) || name.starts_with('.') {
            continue;
        }
        if !entry.file_type()?.is_file() {
            continue;
        }
        // If there is no project-side entry at all, the user deleted it
        if dotdir_path.join(&name).symlink_metadata().is_err() {
            fs::remove_file(entry.path())
                .with_context(|| format!("Failed to delete {}", entry.path().display()))?;
            pruned.push(name);
        }
    }
    Ok(pruned)
}

/// Copy files from repo dotdir back to project dotdir as real files.
/// Used by unlink to restore files before removing the dotdir.
pub fn restore_files(dotdir_path: &Path, repo_dotdir_path: &Path) -> Result<Vec<String>> {
    let mut restored = Vec::new();

    if !repo_dotdir_path.exists() {
        return Ok(restored);
    }

    for entry in fs::read_dir(repo_dotdir_path)
        .with_context(|| format!("Failed to read {}", repo_dotdir_path.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();

        if IGNORED_FILES.contains(&name.as_str()) || name.starts_with('.') {
            continue;
        }

        let src = repo_dotdir_path.join(&name);
        let dst = dotdir_path.join(&name);

        // Remove existing symlink if present
        if dst.symlink_metadata().is_ok() {
            fs::remove_file(&dst)?;
        }

        // Copy real file from repo
        if src.is_file() {
            fs::copy(&src, &dst)
                .with_context(|| format!("Failed to copy {} to {}", src.display(), dst.display()))?;
            restored.push(name);
        }
    }

    Ok(restored)
}
