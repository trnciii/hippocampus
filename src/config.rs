use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = ".hippocampus.toml";
const DOTDIR_GITIGNORE: &str = "*\n";

/// Per-dotdir config stored in <dotdir>/.hippocampus.toml
#[derive(Debug, Deserialize, Clone)]
pub struct DotdirConfig {
    pub repo: String,
}

/// Optional project-root config for defaults
#[derive(Debug, Deserialize)]
struct RootConfig {
    repo: Option<String>,
    #[serde(flatten)]
    overrides: BTreeMap<String, toml::Value>,
}

/// Find VCS root by walking up from cwd looking for .git
pub fn find_project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join(".git").exists() {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => bail!("No git repository found. Run from within a git project."),
        }
    }
}

impl DotdirConfig {
    /// Load config for a specific dotdir. Resolution order:
    /// 1. <dotdir>/.hippocampus.toml (primary)
    /// 2. <project_root>/.hippocampus.toml with [<dotdir>] section
    /// 3. <project_root>/.hippocampus.toml top-level defaults
    pub fn load(project_root: &Path, dotdir: &str) -> Result<Self> {
        let dotdir_config_path = project_root.join(dotdir).join(CONFIG_FILE);

        // Try per-dotdir config first
        if dotdir_config_path.exists() {
            let content = fs::read_to_string(&dotdir_config_path)
                .with_context(|| format!("Failed to read {}", dotdir_config_path.display()))?;
            let cfg: DotdirConfig = toml::from_str(&content)
                .with_context(|| format!("Failed to parse {}", dotdir_config_path.display()))?;
            return Ok(cfg);
        }

        // Fall back to project-root config
        let root_config_path = project_root.join(CONFIG_FILE);
        if root_config_path.exists() {
            let content = fs::read_to_string(&root_config_path)?;
            let raw: RootConfig = toml::from_str(&content)
                .with_context(|| "Failed to parse root .hippocampus.toml")?;

            // Check for dotdir-specific section
            if let Some(value) = raw.overrides.get(dotdir) {
                if let Some(table) = value.as_table() {
                    let repo = table
                        .get("repo")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .or(raw.repo.clone());
                    if let Some(repo) = repo {
                        return Ok(DotdirConfig { repo });
                    }
                }
            }

            // Use top-level defaults
            if let Some(repo) = raw.repo {
                return Ok(DotdirConfig { repo });
            }
        }

        bail!(
            "No config found for '{}'. Run 'tatu init {} --repo <path>' first.",
            dotdir,
            dotdir
        );
    }

    /// Resolve the repo-side path for this dotdir: repo / dotdir
    pub fn repo_dotdir_path(&self, project_root: &Path, dotdir: &str) -> PathBuf {
        let repo_abs = if Path::new(&self.repo).is_relative() {
            project_root.join(&self.repo)
        } else {
            PathBuf::from(&self.repo)
        };
        repo_abs.join(dotdir)
    }
}

/// Discover managed dotdirs by scanning project_root for dot-directories containing .hippocampus.toml
pub fn discover_dotdirs(project_root: &Path) -> Result<Vec<String>> {
    let mut dotdirs = Vec::new();

    if !project_root.exists() {
        return Ok(dotdirs);
    }

    for entry in fs::read_dir(project_root)
        .with_context(|| format!("Failed to read {}", project_root.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && entry.file_type()?.is_dir() {
            if entry.path().join(CONFIG_FILE).exists() {
                dotdirs.push(name);
            }
        }
    }

    dotdirs.sort();
    Ok(dotdirs)
}

/// Discover dotdirs from repo side (for init without specifying dotdir)
pub fn discover_repo_dotdirs(repo_path: &Path) -> Result<Vec<String>> {
    if !repo_path.exists() {
        return Ok(vec![]);
    }
    let mut dotdirs = Vec::new();
    for entry in fs::read_dir(repo_path)
        .with_context(|| format!("Failed to read {}", repo_path.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && entry.file_type()?.is_dir() {
            dotdirs.push(name);
        }
    }
    dotdirs.sort();
    Ok(dotdirs)
}

/// Write per-dotdir .hippocampus.toml
pub fn create_dotdir_config(dotdir_path: &Path, repo: &str) -> Result<()> {
    let config_path = dotdir_path.join(CONFIG_FILE);
    let content = format!("repo = \"{repo}\"\n");
    fs::write(&config_path, &content)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;
    Ok(())
}

pub fn ensure_dotdir_gitignore(dotdir_path: &Path) -> Result<()> {
    let gitignore_path = dotdir_path.join(".gitignore");
    if !gitignore_path.exists() {
        fs::write(&gitignore_path, DOTDIR_GITIGNORE)?;
    }
    Ok(())
}
