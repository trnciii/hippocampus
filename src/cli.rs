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
        /// Project-side root directory (default: auto-detect via .git)
        #[arg(long, default_value = "")]
        root: String,
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
            Commands::Init { dotdir, repo, root } => cmd_init(dotdir, &repo, &root),
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

fn scan_root() -> Result<PathBuf> {
    config::find_project_root()
        .context("Could not find project root (.git).")
}

fn project_name() -> String {
    config::find_project_root()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "unnamed".to_string())
}

/// Load configs for all managed dotdirs, return (dotdir_name, config) pairs
fn load_all_dotdir_configs(scan_root: &Path) -> Result<Vec<(String, DotdirConfig)>> {
    let dotdirs = config::discover_dotdirs(scan_root)?;
    let mut configs = Vec::new();
    for dd in dotdirs {
        let cfg = DotdirConfig::load(scan_root, &dd)?;
        configs.push((dd, cfg));
    }
    Ok(configs)
}

/// Get the VCS repo root and backend from a dotdir config.
/// Uses filesystem walk only — no git subprocess — so the root is always
/// a native path usable as Command::current_dir on Windows.
fn repo_root_from_config(cfg: &DotdirConfig) -> Result<(PathBuf, Box<dyn vcs::Vcs>)> {
    let repo_path = cfg.repo_project_path();
    if !repo_path.exists() {
        anyhow::bail!("Repo side: path does not exist: {}\nCheck the 'repo' value in .hippocampus.toml", repo_path.display());
    }
    let (vcs_backend, root) = vcs::detect_vcs(&repo_path)?;
    Ok((root, vcs_backend))
}

/// Returns the subdirectory name of a project within the plans repo.
/// e.g. cfg.repo = "../plans/hippocampus" → PathBuf::from("hippocampus")
fn repo_subdir(cfg: &DotdirConfig) -> Result<PathBuf> {
    cfg.repo_project_path()
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

fn cmd_init(dotdir: Option<String>, repo: &str, root_arg: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = if root_arg.is_empty() {
        config::find_project_root()
            .context("Could not find project root (.git). Use --root to specify explicitly.")?
    } else {
        let p = Path::new(root_arg);
        if p.is_relative() { cwd.join(p) } else { p.to_path_buf() }
    };

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

        // Per-dotdir .hippocampus.toml (repo path is relative to config file location)
        let repo_for_config = if Path::new(repo).is_relative() {
            pathdiff::diff_paths(&repo_abs, &dotdir_path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| repo_abs.to_string_lossy().into_owned())
        } else {
            repo.to_string()
        };
        config::create_dotdir_config(&dotdir_path, &repo_for_config)?;

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
    let root = scan_root()?;
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
    let (repo_root, vcs_backend) = repo_root_from_config(first_cfg)?;
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
    let root = scan_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    let first_cfg = &configs[0].1;
    let (repo_root, vcs_backend) = repo_root_from_config(first_cfg)?;

    vcs_backend.pull(&repo_root)?;
    println!("Pulled.");

    // Sync links after pull
    sync_all_links(&root, &configs)?;

    Ok(())
}

fn cmd_sync(message: Option<String>) -> Result<()> {
    let root = scan_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    let first_cfg = &configs[0].1;
    let (repo_root, vcs_backend) = repo_root_from_config(first_cfg)?;
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
    let root = scan_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    let first_cfg = &configs[0].1;
    let (repo_root, vcs_backend) = repo_root_from_config(first_cfg)?;

    vcs_backend.fetch(&repo_root)?;

    let status = vcs_backend.status(&repo_root)?;
    if status.is_empty() {
        println!("Clean.");
    } else {
        println!("{}", status);
    }
    Ok(())
}

fn cmd_diff() -> Result<()> {
    let root = scan_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    let first_cfg = &configs[0].1;
    let (repo_root, vcs_backend) = repo_root_from_config(first_cfg)?;

    let diff = vcs_backend.diff(&repo_root)?;
    if diff.is_empty() {
        println!("No differences.");
    } else {
        println!("{}", diff);
    }
    Ok(())
}

fn ansi_dim(s: &str) -> String {
    format!("\x1b[2m{}\x1b[0m", s)
}

fn ansi_red(s: &str) -> String {
    format!("\x1b[31m{}\x1b[0m", s)
}

/// Format the repo-side dotdir path with ANSI coloring.
/// Normal: repo_project portion plain, dotdir suffix dim.
/// Missing: deepest existing prefix plain, rest red.
fn format_repo_path_colored(repo_project: &Path, dotdir: &str) -> String {
    use std::path::MAIN_SEPARATOR;
    let full = repo_project.join(dotdir);
    if full.exists() {
        format!("{}{MAIN_SEPARATOR}{}", repo_project.display(), ansi_dim(dotdir))
    } else {
        // Walk components to find deepest existing ancestor.
        let mut existing = PathBuf::new();
        for comp in full.components() {
            let candidate = existing.join(comp);
            if candidate.exists() {
                existing = candidate;
            } else {
                break;
            }
        }
        let full_str = full.to_string_lossy().into_owned();
        let existing_str = existing.to_string_lossy().into_owned();
        if existing_str.is_empty() {
            ansi_red(&full_str)
        } else {
            let rest = &full_str[existing_str.len()..];
            format!("{}{}", existing_str, ansi_red(rest))
        }
    }
}

fn cmd_list() -> Result<()> {
    let root = scan_root()?;
    let configs = load_all_dotdir_configs(&root)?;

    if configs.is_empty() {
        bail!("No managed dotdirs found. Run 'tt init' first.");
    }

    for (dd, cfg) in &configs {
        let repo_project = cfg.repo_project_path();
        println!("{}", dd);
        println!("  ->  {}", format_repo_path_colored(&repo_project, dd));
    }

    Ok(())
}

fn cmd_unlink(dotdir: Option<String>) -> Result<()> {
    let root = scan_root()?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::{LazyLock, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    static CWD_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn make_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("hippocampus-cli-test-{nanos}"));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("failed to run git");
        assert!(
            output.status.success(),
            "git {:?} failed in {}: {}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn scoped_discovery_includes_non_dot_and_deep_dirs_under_current_dir() {
        let root = make_temp_dir();
        let scope = root.join("scope");
        let managed = scope.join("src").join("plans");
        let ignored = root.join("outside").join("plans");

        fs::create_dir_all(&managed).expect("failed to create managed dir");
        fs::create_dir_all(&ignored).expect("failed to create ignored dir");

        fs::write(managed.join(".hippocampus.toml"), "repo = \"../repo\"\n")
            .expect("failed to write managed config");
        fs::write(ignored.join(".hippocampus.toml"), "repo = \"../repo\"\n")
            .expect("failed to write ignored config");

        let configs = load_all_dotdir_configs(&scope).expect("failed to load configs");
        let names: Vec<_> = configs.iter().map(|(name, _)| name.clone()).collect();

        assert!(
            names.iter().any(|name| name == &Path::new("src").join("plans").to_string_lossy()),
            "expected src/plans under current scope, got: {names:?}"
        );
        assert!(
            !names.iter().any(|name| name.contains("outside")),
            "should not include directories outside current scope: {names:?}"
        );

        fs::remove_dir_all(&root).expect("failed to cleanup temp dir");
    }

    #[test]
    fn deep_directory_workflow_supports_all_commands() {
        let _guard = CWD_LOCK.lock().expect("failed to lock cwd");

        let git_available = Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !git_available {
            eprintln!("git not available; skipping test");
            return;
        }

        let original_cwd = std::env::current_dir().expect("failed to get current dir");
        let root = make_temp_dir();

        let remote = root.join("remote.git");
        let plans = root.join("plans");
        let project = root.join("project");

        let remote_str = remote.to_string_lossy().into_owned();
        run_git(&root, &["init", "--bare", &remote_str]);
        run_git(&root, &["clone", &remote_str, "plans"]);

        run_git(&plans, &["config", "user.name", "Test User"]);
        run_git(&plans, &["config", "user.email", "test@example.com"]);
        fs::write(plans.join("README.md"), "seed\n").expect("failed to write seed file");
        run_git(&plans, &["add", "."]);
        run_git(&plans, &["commit", "-m", "seed"]);
        run_git(&plans, &["push", "-u", "origin", "HEAD"]);

        fs::create_dir_all(&project).expect("failed to create project dir");
        run_git(&project, &["init"]);
        run_git(&project, &["config", "user.name", "Test User"]);
        run_git(&project, &["config", "user.email", "test@example.com"]);

        let deep = project.join("very").join("deep").join("cwd");
        fs::create_dir_all(&deep).expect("failed to create deep cwd");

        let repo_project = plans.join("hippocampus");
        let repo_rel = pathdiff::diff_paths(&repo_project, &deep)
            .expect("failed to compute relative repo path")
            .to_string_lossy()
            .into_owned();

        std::env::set_current_dir(&deep).expect("failed to set deep cwd");

        cmd_init(Some(".plan".to_string()), &repo_rel, &project.to_string_lossy()).expect("init should succeed");
        cmd_status().expect("status should succeed");
        cmd_diff().expect("diff should succeed");
        cmd_push(None).expect("push should succeed");
        cmd_pull().expect("pull should succeed");
        cmd_sync(None).expect("sync should succeed");
        cmd_list().expect("list should succeed");
        cmd_unlink(Some(".plan".to_string())).expect("unlink should succeed");

        let config_path = project.join(".plan").join(".hippocampus.toml");
        assert!(
            !config_path.exists(),
            "unlink should remove config file: {}",
            config_path.display()
        );

        std::env::set_current_dir(&original_cwd).expect("failed to restore cwd");
        fs::remove_dir_all(&root).expect("failed to cleanup temp dir");
    }
}
