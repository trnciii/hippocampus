use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{self, DotdirConfig};
use crate::linker;
use crate::vcs;

#[derive(Parser)]
#[command(name = "tatu", about = "Manage dot-directories via symlinks to a separate repo")]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize tt for dot-directories
    Init {
        /// Dot-directory to set up (e.g. ".plan"). Omit to set up all from repo.
        dotdir: Option<String>,
        /// Path to the repo-side project root
        #[arg(short, long)]
        repo: String,
    },
    /// Commit and push changes to the repo
    Push {
        /// Commit message (auto-generated if omitted)
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Pull latest from repo and sync symlinks
    Pull,
    /// Pull then push (stop on conflict)
    Sync {
        /// Commit message
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Show repo VCS status
    Status,
    /// Show repo VCS diff
    Diff,
    /// List dotdirs in repo
    List,
    /// Remove dotdir from project, restoring files from repo
    Unlink {
        /// Dot-directory to remove (omit for all)
        dotdir: Option<String>,
    },
}

impl Cli {
    pub fn parse_args() -> Self {
        Parser::parse()
    }

    pub fn run(self) -> Result<()> {
        match self.command {
            Commands::Init { dotdir, repo } => cmd_init(dotdir, &repo),
            Commands::Push { message } => cmd_push(message),
            Commands::Pull => cmd_pull(),
            Commands::Sync { message } => cmd_sync(message),
            Commands::Status => cmd_status(),
            Commands::Diff => cmd_diff(),
            Commands::List => cmd_list(),
            Commands::Unlink { dotdir } => cmd_unlink(dotdir),
        }
    }
}

fn project_root() -> Result<PathBuf> {
    config::find_project_root()
}

fn project_name() -> String {
    config::find_project_root()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "unnamed".to_string())
}

/// Load configs for all managed dotdirs, return (dotdir_name, config) pairs
fn load_all_dotdir_configs(project_root: &Path) -> Result<Vec<(String, DotdirConfig)>> {
    let dotdirs = config::discover_dotdirs(project_root)?;
    let mut configs = Vec::new();
    for dd in dotdirs {
        let cfg = DotdirConfig::load(project_root, &dd)?;
        configs.push((dd, cfg));
    }
    Ok(configs)
}

/// Get the VCS repo root and backend from a dotdir config.
/// Uses filesystem walk only — no git subprocess — so the root is always
/// a native path usable as Command::current_dir on Windows.
fn repo_root_from_config(project_root: &Path, cfg: &DotdirConfig) -> Result<(PathBuf, Box<dyn vcs::Vcs>)> {
    let repo_path = if Path::new(&cfg.repo).is_relative() {
        project_root.join(&cfg.repo)
    } else {
        PathBuf::from(&cfg.repo)
    };
    if !repo_path.exists() {
        anyhow::bail!("Repo side: path does not exist: {}\nCheck the 'repo' value in .hippocampus.toml", repo_path.display());
    }
    let (vcs_backend, root) = vcs::detect_vcs(&repo_path)?;
    Ok((root, vcs_backend))
}

/// Returns the subdirectory name of a project within the plans repo.
/// e.g. cfg.repo = "../plans/hippocampus" → PathBuf::from("hippocampus")
fn repo_subdir(cfg: &DotdirConfig) -> Result<PathBuf> {
    Path::new(&cfg.repo)
        .file_name()
        .map(PathBuf::from)
        .context("repo path has no file name component")
}

/// Prune repo files that no longer exist in any managed project dotdir
fn prune_all(root: &Path, configs: &[(String, DotdirConfig)]) -> Result<()> {
    for (dd, cfg) in configs {
        let dotdir_path = root.join(dd);
        let repo_dotdir = cfg.repo_dotdir_path(root, dd);
        if dotdir_path.exists() && repo_dotdir.exists() {
            let pruned = linker::prune_deletions(&dotdir_path, &repo_dotdir)?;
            for f in &pruned {
                println!("  deleted: {}/{}", dd, f);
            }
        }
    }
    Ok(())
}

/// Absorb real files in all managed dotdirs, return list of absorbed files
fn absorb_all(root: &Path, configs: &[(String, DotdirConfig)]) -> Result<Vec<String>> {
    let mut all_absorbed = Vec::new();
    for (dd, cfg) in configs {
        let dotdir_path = root.join(dd);
        let repo_dotdir = cfg.repo_dotdir_path(root, dd);
        if dotdir_path.exists() {
            let absorbed = linker::absorb_files(&dotdir_path, &repo_dotdir, None)?;
            for f in &absorbed {
                println!("  absorbed: {}/{}", dd, f);
            }
            all_absorbed.extend(absorbed);
        }
    }
    Ok(all_absorbed)
}

/// Sync links for all managed dotdirs
fn sync_all_links(root: &Path, configs: &[(String, DotdirConfig)]) -> Result<()> {
    for (dd, cfg) in configs {
        let dotdir_path = root.join(dd);
        let repo_dotdir = cfg.repo_dotdir_path(root, dd);
        if dotdir_path.exists() {
            let created = linker::sync_links(&dotdir_path, &repo_dotdir)?;
            for f in &created {
                println!("  linked: {}/{}", dd, f);
            }
        }
    }
    Ok(())
}

fn cmd_init(dotdir: Option<String>, repo: &str) -> Result<()> {
    let root = project_root()?;
    let cwd = std::env::current_dir()?;

    let repo_abs = if Path::new(repo).is_relative() {
        cwd.join(repo)
    } else {
        PathBuf::from(repo)
    };

    let dotdirs = match dotdir {
        Some(d) => vec![d],
        None => {
            let discovered = config::discover_repo_dotdirs(&repo_abs)?;
            if discovered.is_empty() {
                println!("No dotdirs found in repo. Use 'tt init <dotdir> -r <path>' to create one.");
                return Ok(());
            }
            discovered
        }
    };

    for dd in &dotdirs {
        let dotdir_path = root.join(dd);
        let repo_dotdir = repo_abs.join(dd);

        // Create repo-side directory
        fs::create_dir_all(&repo_dotdir)
            .with_context(|| format!("Failed to create {}", repo_dotdir.display()))?;

        // Create project-side dotdir
        fs::create_dir_all(&dotdir_path)?;

        // .gitignore in dotdir
        config::ensure_dotdir_gitignore(&dotdir_path)?;

        // Per-dotdir .hippocampus.toml
        config::create_dotdir_config(&dotdir_path, repo)?;

        // Absorb existing real files to repo
        let absorbed = linker::absorb_files(&dotdir_path, &repo_dotdir, None)?;
        for f in &absorbed {
            println!("  absorbed: {}/{}", dd, f);
        }

        // Symlink repo files back to project
        let linked = linker::sync_links(&dotdir_path, &repo_dotdir)?;

        let total = absorbed.len() + linked.len();
        if total == 0 {
            println!("Initialized {} (empty)", dd);
        } else {
            let mut parts = Vec::new();
            if !absorbed.is_empty() {
                parts.push(format!("absorbed: {}", absorbed.join(", ")));
            }
            if !linked.is_empty() {
                parts.push(format!("linked: {}", linked.join(", ")));
            }
            println!("Initialized {} ({})", dd, parts.join(", "));
        }
    }

    // Commit and push to repo
    let (vcs_backend, repo_root) = vcs::detect_vcs(&repo_abs)?;
    let subdir = repo_abs
        .file_name()
        .map(PathBuf::from)
        .context("repo path has no file name component")?;
    if vcs_backend.has_changes(&repo_root, &subdir)? {
        let dd_list = dotdirs.join(" ");
        let msg = format!("init {} {}", project_name(), dd_list);
        vcs_backend.add(&repo_root, &subdir)?;
        vcs_backend.commit(&repo_root, &msg)?;
        vcs_backend.push(&repo_root)?;
        println!("Committed and pushed: {}", msg);
    }

    Ok(())
}

fn cmd_push(message: Option<String>) -> Result<()> {
    let root = project_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    // Prune deleted files from repo, then absorb real files
    prune_all(&root, &configs)?;
    absorb_all(&root, &configs)?;

    // Sync links (repo → project)
    sync_all_links(&root, &configs)?;

    // VCS operations
    let first_cfg = &configs[0].1;
    let (repo_root, vcs_backend) = repo_root_from_config(&root, first_cfg)?;
    let subdir = repo_subdir(first_cfg)?;

    if !vcs_backend.has_changes(&repo_root, &subdir)? {
        println!("Nothing to push.");
        return Ok(());
    }

    let msg = message.unwrap_or_else(|| {
        let dd_list: Vec<_> = configs.iter().map(|(dd, _)| dd.as_str()).collect();
        format!("update {} {}", project_name(), dd_list.join(" "))
    });

    vcs_backend.add(&repo_root, &subdir)?;
    vcs_backend.commit(&repo_root, &msg)?;
    println!("Committed: {}", msg);

    vcs_backend.push(&repo_root)?;
    println!("Pushed.");

    Ok(())
}

fn cmd_pull() -> Result<()> {
    let root = project_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    let first_cfg = &configs[0].1;
    let (repo_root, vcs_backend) = repo_root_from_config(&root, first_cfg)?;

    vcs_backend.pull(&repo_root)?;
    println!("Pulled.");

    // Sync links after pull
    sync_all_links(&root, &configs)?;

    Ok(())
}

fn cmd_sync(message: Option<String>) -> Result<()> {
    let root = project_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    let first_cfg = &configs[0].1;
    let (repo_root, vcs_backend) = repo_root_from_config(&root, first_cfg)?;
    let subdir = repo_subdir(first_cfg)?;

    // Local changes first: prune deletions, absorb real files
    prune_all(&root, &configs)?;
    absorb_all(&root, &configs)?;

    // Push local changes before pulling, so we never confuse
    // "remote-added file" with "locally deleted file"
    if vcs_backend.has_changes(&repo_root, &subdir)? {
        let msg = message.as_deref().map(str::to_owned).unwrap_or_else(|| {
            let dd_list: Vec<_> = configs.iter().map(|(dd, _)| dd.as_str()).collect();
            format!("update {} {}", project_name(), dd_list.join(" "))
        });
        vcs_backend.add(&repo_root, &subdir)?;
        vcs_backend.commit(&repo_root, &msg)?;
        println!("Committed: {}", msg);
        vcs_backend.push(&repo_root)?;
        println!("Pushed.");
    } else {
        println!("Nothing to push.");
    }

    // Pull remote changes
    if let Err(e) = vcs_backend.pull(&repo_root) {
        bail!("Pull failed (possible conflict): {}", e);
    }
    println!("Pulled.");

    // Sync links for any newly pulled files
    sync_all_links(&root, &configs)?;

    Ok(())
}

fn cmd_status() -> Result<()> {
    let root = project_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    let first_cfg = &configs[0].1;
    let (repo_root, vcs_backend) = repo_root_from_config(&root, first_cfg)?;

    let status = vcs_backend.status(&repo_root)?;
    if status.is_empty() {
        println!("Clean.");
    } else {
        println!("{}", status);
    }
    Ok(())
}

fn cmd_diff() -> Result<()> {
    let root = project_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    let first_cfg = &configs[0].1;
    let (repo_root, vcs_backend) = repo_root_from_config(&root, first_cfg)?;

    let diff = vcs_backend.diff(&repo_root)?;
    if diff.is_empty() {
        println!("No differences.");
    } else {
        println!("{}", diff);
    }
    Ok(())
}

fn cmd_list() -> Result<()> {
    let root = project_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    // repo points to the project dir in the plans repo; parent is the plans repo root
    let first_cfg = &configs[0].1;
    let repo_project = if Path::new(&first_cfg.repo).is_relative() {
        root.join(&first_cfg.repo)
    } else {
        PathBuf::from(&first_cfg.repo)
    };

    let repo_base = match repo_project.parent() {
        Some(p) => p.to_path_buf(),
        None => {
            println!("Cannot determine repo root from: {}", repo_project.display());
            return Ok(());
        }
    };

    if !repo_base.exists() {
        println!("Repo not found: {}", repo_base.display());
        return Ok(());
    }

    for entry in fs::read_dir(&repo_base)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let project_dir = repo_base.join(&name);
        let mut dotdirs = Vec::new();
        for sub in fs::read_dir(&project_dir)? {
            let sub = sub?;
            let sub_name = sub.file_name().to_string_lossy().into_owned();
            if sub_name.starts_with('.') && sub.file_type()?.is_dir() {
                dotdirs.push(sub_name);
            }
        }
        dotdirs.sort();
        if dotdirs.is_empty() {
            println!("{}/", name);
        } else {
            for dd in &dotdirs {
                println!("{}/{}", name, dd);
            }
        }
    }

    Ok(())
}

fn cmd_unlink(dotdir: Option<String>) -> Result<()> {
    let root = project_root()?;

    let dotdirs = match dotdir {
        Some(d) => vec![d],
        None => config::discover_dotdirs(&root)?,
    };

    for dd in &dotdirs {
        let dotdir_path = root.join(dd);
        if !dotdir_path.exists() {
            continue;
        }

        // Try to restore files from repo before removing
        if let Ok(cfg) = DotdirConfig::load(&root, dd) {
            let repo_dotdir = cfg.repo_dotdir_path(&root, dd);
            let restored = linker::restore_files(&dotdir_path, &repo_dotdir)?;
            for f in &restored {
                println!("  restored: {}/{}", dd, f);
            }
            // Remove config and gitignore, keep restored files
            let config_path = dotdir_path.join(".hippocampus.toml");
            let gitignore_path = dotdir_path.join(".gitignore");
            if config_path.exists() {
                fs::remove_file(&config_path)?;
            }
            if gitignore_path.exists() {
                fs::remove_file(&gitignore_path)?;
            }
            // Remove any remaining symlinks
            for entry in fs::read_dir(&dotdir_path)? {
                let entry = entry?;
                if entry.file_type()?.is_symlink() {
                    fs::remove_file(entry.path())?;
                }
            }
        } else {
            fs::remove_dir_all(&dotdir_path)
                .with_context(|| format!("Failed to remove {}", dotdir_path.display()))?;
        }

        println!("Unlinked {}", dd);
    }

    Ok(())
}
