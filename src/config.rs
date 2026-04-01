use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = ".hippocampus.toml";
const DOTDIR_GITIGNORE: &str = "*\n";

/// Resolve `.` and `..` components in a path without requiring it to exist.
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut parts: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let last = parts.last();
                if matches!(last, Some(Component::Normal(_))) {
                    parts.pop();
                } else {
                    parts.push(component);
                }
            }
            c => parts.push(c),
        }
    }
    parts.iter().collect()
}

fn escape_toml_basic_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Per-dotdir config stored in <dotdir>/.hippocampus.toml
#[derive(Debug, Clone)]
pub struct DotdirConfig {
    pub repo: String,
    config_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct RawDotdirConfig {
    repo: String,
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
            let raw: RawDotdirConfig = toml::from_str(&content)
                .with_context(|| format!("Failed to parse {}", dotdir_config_path.display()))?;
            return Ok(DotdirConfig {
                repo: raw.repo,
                config_dir: project_root.join(dotdir),
            });
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
                        return Ok(DotdirConfig {
                            repo,
                            config_dir: project_root.to_path_buf(),
                        });
                    }
                }
            }

            // Use top-level defaults
            if let Some(repo) = raw.repo {
                return Ok(DotdirConfig {
                    repo,
                    config_dir: project_root.to_path_buf(),
                });
            }
        }

        bail!(
            "No config found for '{}'. Run 'tatu init {} --repo <path>' first.",
            dotdir,
            dotdir
        );
    }

    /// Resolve the repo project path stored in this config.
    /// Relative values are interpreted from the config file location.
    /// The returned path has `.` and `..` components resolved.
    pub fn repo_project_path(&self) -> PathBuf {
        let raw = if Path::new(&self.repo).is_relative() {
            self.config_dir.join(&self.repo)
        } else {
            PathBuf::from(&self.repo)
        };
        normalize_path(&raw)
    }

    /// Resolve the repo-side path for this dotdir: repo / dotdir
    pub fn repo_dotdir_path(&self, _project_root: &Path, dotdir: &str) -> PathBuf {
        let repo_abs = self.repo_project_path();
        repo_abs.join(dotdir)
    }
}

fn collect_managed_dotdirs(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }

        let entry_path = entry.path();
        if entry_path.join(CONFIG_FILE).exists() {
            let relative = entry_path
                .strip_prefix(root)
                .with_context(|| format!("Failed to relativize {}", entry_path.display()))?
                .to_string_lossy()
                .into_owned();
            out.push(relative);
            continue;
        }

        collect_managed_dotdirs(root, &entry_path, out)?;
    }
    Ok(())
}

/// Discover managed dotdirs by scanning project_root recursively for directories containing .hippocampus.toml
pub fn discover_dotdirs(project_root: &Path) -> Result<Vec<String>> {
    let mut dotdirs = Vec::new();

    if !project_root.exists() {
        return Ok(dotdirs);
    }

    collect_managed_dotdirs(project_root, project_root, &mut dotdirs)?;

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
        if name == ".git" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            dotdirs.push(name);
        }
    }
    dotdirs.sort();
    Ok(dotdirs)
}

/// Write per-dotdir .hippocampus.toml
pub fn create_dotdir_config(dotdir_path: &Path, repo: &str) -> Result<()> {
    let config_path = dotdir_path.join(CONFIG_FILE);
    let content = format!("repo = \"{}\"\n", escape_toml_basic_string(repo));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hippocampus-config-test-{nanos}"));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    #[test]
    fn discover_dotdirs_includes_non_dot_name_when_config_exists() {
        let root = make_temp_dir();
        let managed = root.join("plan");
        fs::create_dir_all(&managed).expect("failed to create managed dir");
        fs::write(managed.join(CONFIG_FILE), "repo = \"../repo\"\n")
            .expect("failed to write config");

        let found = discover_dotdirs(&root).expect("discover_dotdirs should run");

        assert!(
            found.iter().any(|d| d == "plan"),
            "expected 'plan' to be discovered, got: {found:?}"
        );

        fs::remove_dir_all(&root).expect("failed to cleanup temp dir");
    }

    #[test]
    fn discover_dotdirs_finds_configured_directory_in_deep_path() {
        let root = make_temp_dir();
        let managed = root.join("very").join("deep").join("plan");
        fs::create_dir_all(&managed).expect("failed to create deep managed dir");
        fs::write(managed.join(CONFIG_FILE), "repo = \"../repo\"\n")
            .expect("failed to write config");

        let found = discover_dotdirs(&root).expect("discover_dotdirs should run");
        let expected = Path::new("very")
            .join("deep")
            .join("plan")
            .to_string_lossy()
            .into_owned();

        assert!(
            found.iter().any(|d| d == &expected),
            "expected deep managed dir to be discovered, got: {found:?}"
        );

        fs::remove_dir_all(&root).expect("failed to cleanup temp dir");
    }

    #[test]
    fn repo_path_is_resolved_relative_to_dotdir_config_file() {
        let root = make_temp_dir();
        let dotdir = ".plan";
        let dotdir_path = root.join(dotdir);
        let repo_root = root.join("..repo");

        fs::create_dir_all(&dotdir_path).expect("failed to create dotdir");
        fs::create_dir_all(repo_root.join(dotdir)).expect("failed to create repo dotdir");

        fs::write(dotdir_path.join(CONFIG_FILE), "repo = \"../..repo\"\n")
            .expect("failed to write config");

        let cfg = DotdirConfig::load(&root, dotdir).expect("failed to load dotdir config");
        let resolved = cfg.repo_dotdir_path(&root, dotdir);

        let resolved_norm = fs::canonicalize(&resolved).expect("failed to canonicalize resolved path");
        let expected_norm =
            fs::canonicalize(repo_root.join(dotdir)).expect("failed to canonicalize expected path");
        assert_eq!(resolved_norm, expected_norm);

        fs::remove_dir_all(&root).expect("failed to cleanup temp dir");
    }

    #[test]
    fn repo_project_path_is_resolved_relative_to_dotdir_config_file() {
        let root = make_temp_dir();
        let dotdir = ".plan";
        let dotdir_path = root.join(dotdir);
        let repo_root = root.join("..repo");

        fs::create_dir_all(&dotdir_path).expect("failed to create dotdir");
        fs::create_dir_all(&repo_root).expect("failed to create repo root");
        fs::write(dotdir_path.join(CONFIG_FILE), "repo = \"../..repo\"\n")
            .expect("failed to write config");

        let cfg = DotdirConfig::load(&root, dotdir).expect("failed to load dotdir config");
        let resolved_norm = fs::canonicalize(cfg.repo_project_path())
            .expect("failed to canonicalize resolved repo path");
        let expected_norm = fs::canonicalize(&repo_root)
            .expect("failed to canonicalize expected repo path");

        assert_eq!(resolved_norm, expected_norm);

        fs::remove_dir_all(&root).expect("failed to cleanup temp dir");
    }

    #[test]
    fn create_dotdir_config_escapes_backslashes_for_toml() {
        let root = make_temp_dir();
        let dotdir = root.join(".plan");
        fs::create_dir_all(&dotdir).expect("failed to create dotdir");

        create_dotdir_config(&dotdir, r"..\very\deep\repo").expect("failed to write config");
        let content = fs::read_to_string(dotdir.join(CONFIG_FILE)).expect("failed to read config");

        let parsed: RawDotdirConfig = toml::from_str(&content).expect("config should be valid toml");
        assert_eq!(parsed.repo, r"..\very\deep\repo");

        fs::remove_dir_all(&root).expect("failed to cleanup temp dir");
    }
}
