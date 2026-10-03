//! Workspace discovery.
//!
//! Root resolution: repeatable explicit paths, then the
//! selected saved root list, then a nonempty `GIS_PATH` (colon-separated —
//! explicit arguments are never split), then the launch working directory. A
//! `--desktop` launch with none of those opens selection instead of scanning
//! home. Entries are never shell-expanded or executed; empty, Windows, and
//! relative entries are reported and skipped.
//!
//! Discovery scans a root by finding `.git` markers (directory or file) with
//! symlinks not followed and Git metadata pruned, then asks Git itself for
//! each repository's canonical worktree / git-dir / common-dir identity.
//! Overlapping roots are deduplicated by canonical worktree; linked
//! worktrees stay distinct rows that share their common-dir identity. Invalid
//! candidates and permission failures become diagnostics, never rows.
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::Path;

use crate::git::{self, RepoIdentity};
use crate::model::{RootKind, root_kind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredRepo {
    pub identity: RepoIdentity,
    /// Display name (basename). Duplicate basenames are allowed.
    pub name: String,
}

#[derive(Debug, Default)]
pub struct Discovery {
    pub repos: Vec<DiscoveredRepo>,
    pub diagnostics: Vec<String>,
}

/// Scan `roots` for repositories. Roots are normalized first: WSL UNC paths
/// map to distro paths and Windows drive paths to `/mnt/<drive>`, so a folder
/// chosen in the Windows picker still resolves through WSL Git. Anything that
/// is not an absolute Linux path is reported and skipped.
pub fn discover(roots: &[String]) -> Discovery {
    discover_with_excludes(roots, &[])
}

/// Scan with workspace exclusions: a repository is skipped when its worktree
/// equals an excluded path or lives under one. Generated scratch repositories
/// named with a UUID suffix (`unit-cards-<32 hex>`) are skipped by default —
/// tool test suites create hundreds of them.
pub fn discover_with_excludes(roots: &[String], exclude: &[String]) -> Discovery {
    let exclude: Vec<String> = exclude
        .iter()
        .filter_map(|path| crate::model::to_root_string(path).ok())
        .collect();
    let mut out = Discovery::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut scanned: HashSet<String> = HashSet::new();
    for root in roots {
        let normalized = match crate::model::to_root_string(root) {
            Ok(root) => root,
            Err(distro) => {
                out.diagnostics.push(format!(
                    "{root}: path belongs to WSL distribution '{distro}'"
                ));
                continue;
            }
        };
        if !scanned.insert(normalized.clone()) {
            continue;
        }
        discover_root(&normalized, &exclude, &mut out, &mut seen);
    }
    out
}

/// Whether `path` equals an excluded root or is inside one (`/a/b` excludes
/// `/a/b/c` but not `/a/bc`).
fn is_excluded(path: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|excluded| {
        path == excluded
            || (path.starts_with(excluded) && path.as_bytes().get(excluded.len()) == Some(&b'/'))
    })
}

/// Generated scratch repositories live under (or are named with) a UUID
/// component: tool test suites create hundreds of `phase2-fixture-<32 hex>/…`
/// trees. Components of the scan root itself are never treated as generated.
fn looks_generated(root: &str, path: &str) -> bool {
    let rest = path.strip_prefix(root).unwrap_or(path);
    rest.split('/').any(uuid_component)
}

fn uuid_component(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() < 33 || bytes[bytes.len() - 33] != b'-' {
        return false;
    }
    name[name.len() - 32..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// What a scan root needs before discovery can walk it.
#[derive(Debug)]
enum RootCheck {
    /// `/mnt/<drive>/…`: a native path check is enough; no WSL involved.
    WindowsMount,
    /// A Linux path: needs the distro for `test -d` and everything after.
    Wsl,
    /// Reject the root with this diagnostic.
    Reject(String),
}

/// Classify a root by shape and WSL availability. Windows-mounted paths
/// (`/mnt/c/…`) never need the distro; Linux paths do and are rejected with a
/// clear diagnostic when no distro is reachable, instead of failing on a
/// `wsl.exe` spawn per root.
fn classify_root(root: &str, wsl: bool) -> RootCheck {
    if root.is_empty() {
        return RootCheck::Reject("empty scan root".to_string());
    }
    if root_kind(root) == RootKind::Windows || !root.starts_with('/') {
        return RootCheck::Reject(format!(
            "{root}: only absolute Linux paths inside the WSL distro are supported"
        ));
    }
    if crate::model::to_windows_path(root).is_some() {
        return RootCheck::WindowsMount;
    }
    if !wsl {
        let distro = crate::git::distro();
        return RootCheck::Reject(if distro.is_empty() {
            format!("{root}: WSL is unavailable (no WSL distribution found)")
        } else {
            format!("{root}: WSL is unavailable (Linux paths need the {distro} distribution)")
        });
    }
    RootCheck::Wsl
}

/// Native existence check for a Windows-mount root (no subprocess).
fn windows_mount_is_dir(root: &str) -> bool {
    crate::model::to_windows_path(root)
        .map(|local| std::path::Path::new(&local).is_dir())
        .unwrap_or(false)
}

fn discover_root(root: &str, exclude: &[String], out: &mut Discovery, seen: &mut HashSet<String>) {
    if root.is_empty() {
        out.diagnostics.push("empty scan root".to_string());
        return;
    }
    if is_excluded(root, exclude) {
        out.diagnostics.push(format!("{root}: excluded by settings"));
        return;
    }
    match classify_root(root, git::wsl_available()) {
        RootCheck::Reject(reason) => {
            out.diagnostics.push(reason);
            return;
        }
        RootCheck::WindowsMount => {
            // Native check: Windows-mounted repositories scan and run on
            // native Git, so they work on a machine without WSL.
            if !windows_mount_is_dir(root) {
                out.diagnostics.push(format!("{root}: not an existing directory"));
                return;
            }
        }
        RootCheck::Wsl => match git::run_linux("test", &["-d", root]) {
            Ok(check) if check.success() => {}
            Ok(_) => {
                out.diagnostics.push(format!("{root}: not an existing directory"));
                return;
            }
            Err(err) => {
                out.diagnostics.push(format!("{root}: {err}"));
                return;
            }
        },
    }

    // Candidates in discovery order: the containing worktree first, then every
    // `.git` marker. Windows mounts are walked natively (`/mnt/c` traversal
    // through 9p is minutes-slow) and plain `.git` directories get their
    // identity without Git; linked worktrees and submodules are resolved
    // through Git on a bounded worker pool.
    let mut candidates: Vec<String> = Vec::new();
    if !is_excluded(root, exclude) && git::identity(root).is_ok() {
        candidates.push(root.to_string());
    }
    let mut markers = Markers::default();
    collect_markers(root, exclude, &mut markers, out);
    for repo in &markers.plain {
        add_plain_repo(out, seen, repo);
    }

    resolve_candidates(candidates, out, seen);
    resolve_candidates(markers.needs_git, out, seen);
}

/// `.git` markers under a root, split into plain repositories (identity is the
/// parent plus `/.git`) and cases that must ask Git (linked worktrees,
/// submodules, unusual layouts). WSL metadata is never descended into;
/// symlinks are not followed.
#[derive(Debug, Default)]
struct Markers {
    plain: Vec<String>,
    needs_git: Vec<String>,
}

fn collect_markers(root: &str, exclude: &[String], markers: &mut Markers, out: &mut Discovery) {
    #[cfg(windows)]
    if let Some(diagnostics) = native_markers(root, exclude, markers) {
        out.diagnostics.extend(diagnostics);
        return;
    }

    let found = match git::run_linux("find", &[root, "-name", ".git", "-prune", "-print"]) {
        Ok(found) => found,
        Err(err) => {
            out.diagnostics.push(format!("{root}: could not scan: {err}"));
            return;
        }
    };
    // `find` keeps scanning after permission errors; those lines are facts to
    // show alongside the successful results, not a reason to fail the root.
    for line in found.stderr_text().lines().map(str::trim).filter(|l| !l.is_empty()) {
        out.diagnostics.push(format!("{root}: {line}"));
    }
    for line in found.stdout_text().lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some(parent) = Path::new(line).parent().and_then(Path::to_str) else {
            out.diagnostics.push(format!("{line}: invalid repository marker"));
            continue;
        };
        if is_excluded(parent, exclude) || looks_generated(root, parent) {
            continue;
        }
        markers.needs_git.push(parent.to_string());
    }
}

/// Identity for a plain `.git` directory: the parent is the worktree and the
/// git directory; no subprocess needed.
fn add_plain_repo(out: &mut Discovery, seen: &mut HashSet<String>, repo: &str) {
    add_repo(
        out,
        seen,
        RepoIdentity {
            worktree: std::path::PathBuf::from(repo),
            git_dir: std::path::PathBuf::from(format!("{repo}/.git")),
            common_dir: std::path::PathBuf::from(format!("{repo}/.git")),
        },
    );
}

/// Map `/mnt/<drive>/rest` to its Windows path.
#[cfg(windows)]
fn mount_to_windows(root: &str) -> Option<std::path::PathBuf> {    let rest = root.strip_prefix("/mnt/")?;
    let (drive, tail) = match rest.split_once('/') {
        Some((drive, tail)) => (drive, tail),
        None => (rest, ""),
    };
    if drive.len() != 1 || !drive.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    let upper = drive.to_ascii_uppercase();
    let windows = if tail.is_empty() {
        format!("{upper}:\\")
    } else {
        format!("{upper}:\\{}", tail.replace('/', "\\"))
    };
    Some(std::path::PathBuf::from(windows))
}

/// Native markers for a `/mnt/<drive>` root: the Linux prefix is the full root
/// so every marker path can be handed back to WSL Git unchanged.
#[cfg(windows)]
fn native_markers(root: &str, exclude: &[String], markers: &mut Markers) -> Option<Vec<String>> {
    let windows_root = mount_to_windows(root)?;
    let mut diagnostics = Vec::new();
    walk_markers(&windows_root, root, root, exclude, markers, &mut diagnostics);
    Some(diagnostics)
}

/// Common build/dependency directories pruned by the native walk: they never
/// contain the repositories this tool manages and dominate scan time on large
/// trees.
#[cfg(windows)]
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "out",
    "obj",
    "bin",
    "__pycache__",
    ".venv",
    "venv",
    ".gradle",
    ".next",
    ".nuxt",
    ".cache",
    ".idea",
    ".vs",
];

/// Native Windows walk: fast on NTFS, hidden entries included, reparse points
/// (symlinks/junctions) skipped, `.git` recorded without descending.
#[cfg(windows)]
fn walk_markers(
    dir: &std::path::Path,
    root: &str,
    linux_dir: &str,
    exclude: &[String],
    markers: &mut Markers,
    diagnostics: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            diagnostics.push(format!("{linux_dir}: {err}"));
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let linux_path = format!("{linux_dir}/{}", name.to_string_lossy());
        if name == ".git" {
            let Some(parent) = Path::new(&linux_path).parent().and_then(Path::to_str) else {
                continue;
            };
            if is_excluded(parent, exclude) || looks_generated(root, parent) {
                continue;
            }
            let git_dir = entry.path();
            if git_dir.is_dir() && plain_git_dir(&git_dir) {
                markers.plain.push(parent.to_string());
            } else {
                markers.needs_git.push(parent.to_string());
            }
            continue;
        }
        if SKIP_DIRS.contains(&name.to_string_lossy().to_ascii_lowercase().as_str())
            || is_excluded(&linux_path, exclude)
        {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() || is_reparse_point(&meta) {
            continue;
        }
        if meta.is_dir() {
            walk_markers(&entry.path(), root, &linux_path, exclude, markers, diagnostics);
        }
    }
}

/// A `.git` directory that a plain repository root has: no `commondir`
/// indirection and the files Git itself requires.
#[cfg(windows)]
fn plain_git_dir(git_dir: &std::path::Path) -> bool {
    git_dir.join("HEAD").is_file()
        && git_dir.join("objects").is_dir()
        && !git_dir.join("commondir").exists()
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Resolve every candidate through Git on a bounded worker pool; results are
/// applied in candidate order so diagnostics stay deterministic.
fn resolve_candidates(candidates: Vec<String>, out: &mut Discovery, seen: &mut HashSet<String>) {
    let mut unique = Vec::new();
    let mut local: HashSet<String> = HashSet::new();
    for candidate in candidates {
        if local.insert(candidate.clone()) {
            unique.push(candidate);
        }
    }
    if unique.is_empty() {
        return;
    }
    let workers = crate::process::jobs_from_env()
        .unwrap_or(crate::process::DEFAULT_JOBS)
        .clamp(1, 16);
    if unique.len() == 1 || workers == 1 {
        for candidate in unique {
            match git::identity(&candidate) {
                Ok(identity) => add_repo(out, seen, identity),
                Err(err) => out.diagnostics.push(format!("{candidate}: {err}")),
            }
        }
        return;
    }
    let total = unique.len();
    let queue = std::sync::Arc::new(std::sync::Mutex::new(
        unique.into_iter().enumerate().collect::<std::collections::VecDeque<_>>(),
    ));
    let (sender, receiver) = std::sync::mpsc::channel();
    for _ in 0..workers {
        let queue = queue.clone();
        let sender = sender.clone();
        std::thread::spawn(move || loop {
            let next = queue.lock().unwrap().pop_front();
            let Some((index, candidate)) = next else {
                return;
            };
            let result = git::identity(&candidate);
            if sender.send((index, candidate, result)).is_err() {
                return;
            }
        });
    }
    drop(sender);
    let mut results: Vec<Option<(String, Result<RepoIdentity, String>)>> =
        (0..total).map(|_| None).collect();
    for (index, candidate, result) in receiver {
        results[index] = Some((candidate, result));
    }
    for entry in results.into_iter().flatten() {
        let (candidate, result) = entry;
        match result {
            Ok(identity) => add_repo(out, seen, identity),
            Err(err) => out.diagnostics.push(format!("{candidate}: {err}")),
        }
    }
}

fn add_repo(out: &mut Discovery, seen: &mut HashSet<String>, identity: RepoIdentity) {
    let key = identity.worktree.to_string_lossy().into_owned();
    if !seen.insert(key) {
        return;
    }
    let name = identity
        .worktree
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| identity.worktree.to_string_lossy().into_owned());
    out.repos.push(DiscoveredRepo { identity, name });
}

/// Inputs collected at startup. Kept as plain values so precedence is
/// table-testable without touching process-global environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchInputs {
    /// Repeatable explicit paths (`--root=<path>`); never split.
    pub explicit: Vec<String>,
    /// The selected saved root list, if any.
    pub saved: Vec<String>,
    /// Raw `GIS_PATH` value, if set.
    pub gis_path: Option<String>,
    /// Process launch directory (terminal launch fallback).
    pub launch_dir: Option<String>,
    /// `--desktop`: a no-root desktop start opens selection.
    pub desktop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RootSource {
    Explicit,
    Saved,
    GisPath,
    LaunchDir,
    #[default]
    Selection,
}

#[derive(Debug, Default)]
pub struct RootPlan {
    pub source: RootSource,
    pub roots: Vec<String>,
    pub diagnostics: Vec<String>,
}

/// Apply the documented root precedence, reporting every entry that cannot be
/// used. The returned roots are de-duplicated and in input order.
pub fn resolve_roots(inputs: &LaunchInputs) -> RootPlan {
    let mut diagnostics = Vec::new();

    let explicit = acceptable(&inputs.explicit, "explicit path", &mut diagnostics);
    if !explicit.is_empty() {
        return RootPlan {
            source: RootSource::Explicit,
            roots: dedup(explicit),
            diagnostics,
        };
    }

    let saved = acceptable(&inputs.saved, "saved root", &mut diagnostics);
    if !saved.is_empty() {
        return RootPlan {
            source: RootSource::Saved,
            roots: dedup(saved),
            diagnostics,
        };
    }

    if let Some(raw) = &inputs.gis_path {
        if raw.is_empty() {
            diagnostics.push("GIS_PATH is empty".to_string());
        } else {
            let entries: Vec<String> = raw.split(':').map(str::to_string).collect();
            let accepted = acceptable(&entries, "GIS_PATH entry", &mut diagnostics);
            if !accepted.is_empty() {
                return RootPlan {
                    source: RootSource::GisPath,
                    roots: dedup(accepted),
                    diagnostics,
                };
            }
        }
    }

    if !inputs.desktop
        && let Some(dir) = inputs.launch_dir.as_deref().filter(|dir| !dir.is_empty())
    {
        let accepted = acceptable(&[dir.to_string()], "launch directory", &mut diagnostics);
        if !accepted.is_empty() {
            return RootPlan {
                source: RootSource::LaunchDir,
                roots: dedup(accepted),
                diagnostics,
            };
        }
    }

    RootPlan { source: RootSource::Selection, roots: Vec::new(), diagnostics }
}

/// Keep well-formed WSL entries; report empty, foreign-distro, UNC, and
/// relative entries so a lower-precedence source is never silently
/// substituted. Windows drive paths are normalized to their `/mnt/<drive>`
/// mount and count as WSL entries.
fn acceptable(entries: &[String], label: &str, diagnostics: &mut Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for entry in entries {
        if entry.is_empty() {
            diagnostics.push(format!("empty {label}"));
            continue;
        }
        match crate::model::to_root_string(entry) {
            Ok(root) if root_kind(&root) == RootKind::Wsl && root.starts_with('/') => {
                out.push(root);
            }
            Ok(_) => diagnostics.push(format!(
                "{entry}: {label} is not an absolute Linux path (no shell expansion)"
            )),
            Err(distro) => diagnostics.push(format!(
                "{entry}: path belongs to WSL distribution '{distro}'"
            )),
        }
    }
    out
}

fn dedup(roots: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    roots.into_iter().filter(|root| seen.insert(root.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_roots_need_wsl_only_for_linux_paths() {
        // Windows mounts classify without WSL and are checked natively.
        assert!(matches!(
            classify_root("/mnt/c/dev", false),
            RootCheck::WindowsMount
        ));
        assert!(matches!(
            classify_root("/mnt/c", false),
            RootCheck::WindowsMount
        ));
        // Linux paths need the distro.
        assert!(matches!(classify_root("/home/me/dev", true), RootCheck::Wsl));
        assert!(matches!(
            classify_root("/home/me/dev", false),
            RootCheck::Reject(message) if message.contains("WSL is unavailable")
        ));
        // Shape rejects apply in both modes.
        assert!(matches!(classify_root(r"C:\dev", true), RootCheck::Reject(_)));
        assert!(matches!(classify_root("dev", true), RootCheck::Reject(_)));
        assert!(matches!(classify_root("", true), RootCheck::Reject(_)));
    }

    fn plan(explicit: &[&str], saved: &[&str], gis: Option<&str>, dir: Option<&str>, desktop: bool) -> RootPlan {
        resolve_roots(&LaunchInputs {
            explicit: explicit.iter().map(|s| s.to_string()).collect(),
            saved: saved.iter().map(|s| s.to_string()).collect(),
            gis_path: gis.map(str::to_string),
            launch_dir: dir.map(str::to_string),
            desktop,
        })
    }

    #[test]
    fn explicit_paths_win_and_colons_are_not_split() {
        let result = plan(
            &["/srv/one", "/srv/two:with-colon"],
            &["/saved"],
            Some("/gis"),
            Some("/cwd"),
            false,
        );
        assert_eq!(result.source, RootSource::Explicit);
        assert_eq!(result.roots, vec!["/srv/one", "/srv/two:with-colon"]);
    }

    #[test]
    fn saved_roots_win_over_gis_path_and_launch_dir() {
        let result = plan(&[], &["/saved/a"], Some("/gis"), Some("/cwd"), false);
        assert_eq!(result.source, RootSource::Saved);
        assert_eq!(result.roots, vec!["/saved/a"]);
    }

    #[test]
    fn gis_path_is_colon_split_and_entries_are_reported() {
        let result = plan(&[], &[], Some("/one::/two"), Some("/cwd"), false);
        assert_eq!(result.source, RootSource::GisPath);
        assert_eq!(result.roots, vec!["/one", "/two"]);
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert!(result.diagnostics[0].contains("empty GIS_PATH entry"));

        let windows = plan(&[], &[], Some("/ok"), Some("/cwd"), false);
        assert_eq!(windows.roots, vec!["/ok"]);

        let malformed = plan(&[], &[], Some("C:\\dev;D:\\x"), Some("/cwd"), false);
        // A colon-split Windows-ish value has no usable entry, so resolution
        // falls through to the launch directory and reports the bad entries.
        assert_eq!(malformed.source, RootSource::LaunchDir);
        assert_eq!(malformed.roots, vec!["/cwd"]);
        assert!(
            malformed
                .diagnostics
                .iter()
                .any(|d| d.contains("GIS_PATH entry is not an absolute Linux path")),
            "{:?}",
            malformed.diagnostics
        );
    }

    #[test]
    fn launch_dir_is_the_last_resort_for_a_terminal_launch() {
        let result = plan(&[], &[], None, Some("/home/me/dev"), false);
        assert_eq!(result.source, RootSource::LaunchDir);
        assert_eq!(result.roots, vec!["/home/me/dev"]);
    }

    #[test]
    fn desktop_without_roots_opens_selection_instead_of_the_launch_dir() {
        let result = plan(&[], &[], None, Some("/home/me"), true);
        assert_eq!(result.source, RootSource::Selection);
        assert!(result.roots.is_empty());
    }

    #[test]
    fn empty_and_non_linux_entries_are_reported_never_used() {
        let result = plan(&["", "~/dev", "relative/dir"], &[], None, Some("/cwd"), false);
        assert_eq!(result.source, RootSource::LaunchDir, "fall through to cwd");
        assert_eq!(result.roots, vec!["/cwd"]);
        for needle in ["empty explicit path", "~/dev", "relative/dir"] {
            assert!(
                result.diagnostics.iter().any(|d| d.contains(needle)),
                "missing diagnostic for {needle}: {:?}",
                result.diagnostics
            );
        }

        // A Windows drive folder is a usable workspace via its WSL mount.
        let windows = plan(&["C:\\repos"], &[], None, None, true);
        assert_eq!(windows.source, RootSource::Explicit);
        assert_eq!(windows.roots, vec!["/mnt/c/repos"]);
        assert!(windows.diagnostics.is_empty(), "{:?}", windows.diagnostics);

        // A UNC network share has no WSL mount and is reported instead.
        let unc = plan(&["\\\\server\\share"], &[], None, None, true);
        assert_eq!(unc.source, RootSource::Selection);
        assert!(unc.diagnostics.iter().any(|d| d.contains("server")), "{:?}", unc.diagnostics);
    }

    #[test]
    fn duplicates_are_collapsed_in_order() {
        let result = plan(&["/a", "/b", "/a"], &[], None, None, false);
        assert_eq!(result.roots, vec!["/a", "/b"]);
    }

    #[test]
    fn excludes_and_generated_names_are_filtered() {
        let exclude = vec!["/home/me/scratch".to_string()];
        assert!(is_excluded("/home/me/scratch", &exclude));
        assert!(is_excluded("/home/me/scratch/deep/repo", &exclude));
        assert!(!is_excluded("/home/me/scratchpad", &exclude));
        assert!(!is_excluded("/home/me/other", &exclude));

        assert!(looks_generated("/w", "/w/unit-cards-0123456789abcdef0123456789abcdef"));
        assert!(looks_generated("/w", "/w/gitlink-aAaA0b54eab046289887331fb00b2763"));
        assert!(looks_generated("/w", "/w/phase2-fixture-0123456789abcdef0123456789abcdef/fixture/repo"));
        assert!(!looks_generated("/w/unit-cards-0123456789abcdef0123456789abcdef", "/w/unit-cards-0123456789abcdef0123456789abcdef"));
        assert!(!looks_generated("/w", "/w/Spur"));
        assert!(!looks_generated("/w", "/w/feature-abc"));
        assert!(!looks_generated("/w", "/w/short-0123456789abcdef0123456789abcde"));
    }

    #[cfg(windows)]
    #[test]
    fn native_walk_finds_nested_markers_without_descending_into_git_dirs() {
        let base = std::env::temp_dir().join(format!("spur-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let plain_git = |repo: &std::path::Path| {
            std::fs::create_dir_all(repo.join(".git/objects")).unwrap();
            std::fs::write(repo.join(".git/HEAD"), b"ref: refs/heads/main").unwrap();
        };
        plain_git(&base.join("repo-a"));
        plain_git(&base.join("repo-a/node_modules/hidden-repo"));
        std::fs::create_dir_all(base.join("repo-a/vendor/inner/.git")).unwrap();
        std::fs::create_dir_all(base.join("nested/deep/repo-b")).unwrap();
        std::fs::write(base.join("nested/deep/repo-b/.git"), b"gitdir: elsewhere").unwrap();

        let mut markers = Markers::default();
        let mut diagnostics = Vec::new();
        walk_markers(&base, "/mnt/c/x", "/mnt/c/x", &[], &mut markers, &mut diagnostics);
        assert_eq!(markers.plain, vec!["/mnt/c/x/repo-a"]);
        let needs_git: HashSet<&str> = markers.needs_git.iter().map(String::as_str).collect();
        assert!(needs_git.contains("/mnt/c/x/repo-a/vendor/inner"), "{needs_git:?}");
        assert!(needs_git.contains("/mnt/c/x/nested/deep/repo-b"), "{needs_git:?}");
        assert_eq!(needs_git.len(), 2, "{needs_git:?}");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(windows)]
    #[test]
    fn native_markers_keep_the_full_root_prefix() {
        let base = std::env::temp_dir().join(format!("spur-mount-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("repo-a/.git/objects")).unwrap();
        std::fs::write(base.join("repo-a/.git/HEAD"), b"ref: refs/heads/main").unwrap();
        let linux_root = base
            .to_string_lossy()
            .replace('\\', "/")
            .strip_prefix("C:/")
            .map(|tail| format!("/mnt/c/{tail}"));
        let Some(linux_root) = linux_root else {
            return; // temp directory is not on C:; the mapping test needs a drive
        };

        let mut markers = Markers::default();
        let diagnostics = native_markers(&linux_root, &[], &mut markers).expect("mount mapping");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(markers.plain, vec![format!("{linux_root}/repo-a")]);
        assert!(markers.needs_git.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// Real-Git discovery tests. They need the WSL distro, so they are
/// ignored by default: run them with `cargo test -- --ignored`.
#[cfg(test)]
mod wsl_tests {
    use std::collections::HashSet;

    use super::*;
    use crate::process::wsl_support as wsl;

    struct RestorePermissions(String);

    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = wsl::wsl(&["chmod", "755", &self.0]);
        }
    }

    /// Builds the fixture tree once per test:
    /// - duplicate basenames, a nested repo, a linked worktree, a populated
    ///   submodule, an invalid `.git` file, a bare repo, a repo behind a
    ///   directory symlink outside the root, a symlink loop, a plain dir
    fn build_fixture(dir: &str) {
        let script = format!(
            r#"#!/bin/bash
set -e
R="{dir}"
mkdir -p "$R/root/work/one/app" "$R/root/work/two/app" "$R/root/work/outer/src" \
  "$R/root/work/plain" "$R/root/bad" "$R/outside" "$R/subrepo"
init() {{
  git -C "$1" init -q -b main
  printf 'seed\n' > "$1/file.txt"
  git -C "$1" add file.txt
  git -C "$1" -c user.name=t -c user.email=t@t commit -q -m init
}}
init "$R/root/work/one/app"
init "$R/root/work/two/app"
init "$R/root/work/outer"
init "$R/outside"
init "$R/subrepo"
git -C "$R/root/work/outer" worktree add -q --detach "$R/root/work/linked"
git -C "$R/root/work/outer" -c protocol.file.allow=always submodule add -q "$R/subrepo" sub
git -C "$R/root/work/outer" -c user.name=t -c user.email=t@t commit -q -m sub
printf 'garbage\n' > "$R/root/bad/.git"
git init -q --bare "$R/root/bare.git"
ln -s "$R/outside" "$R/root/work/link-to-outside"
ln -s "$R/root" "$R/root/work/loop"
mkdir -p "$R/root/locked"
init "$R/root/locked"
chmod 000 "$R/root/locked"
"#
        );
        let script_path = format!("{dir}/build.sh");
        wsl::write_script(&script_path, &script);
        wsl::must(&[script_path.as_str()]);
    }

    fn worktrees(discovery: &Discovery) -> Vec<String> {
        discovery
            .repos
            .iter()
            .map(|repo| repo.identity.worktree.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn discovers_the_exact_worktree_set() {
        wsl::watchdog(180);
        let dir = wsl::temp_dir("r01");
        let _restore = RestorePermissions(format!("{dir}/root/locked"));
        build_fixture(&dir);
        let root = format!("{dir}/root");

        let discovery = discover(std::slice::from_ref(&root));
        let found: HashSet<String> = worktrees(&discovery).into_iter().collect();
        let expected: HashSet<String> = [
            format!("{root}/work/one/app"),
            format!("{root}/work/two/app"),
            format!("{root}/work/outer"),
            format!("{root}/work/outer/sub"),
            format!("{root}/work/linked"),
        ]
        .into_iter()
        .collect();
        assert_eq!(found, expected, "diagnostics: {:?}", discovery.diagnostics);

        // Duplicate basenames both survive (identity is the path, not the name).
        let app_count = discovery.repos.iter().filter(|repo| repo.name == "app").count();
        assert_eq!(app_count, 2);

        // The symlinked repo outside the root, the bare repo, and the invalid
        // `.git` file are not rows.
        assert!(!found.iter().any(|path| path.contains("outside")));
        assert!(!found.iter().any(|path| path.contains("bare.git")));
        assert!(
            discovery.diagnostics.iter().any(|d| d.contains("/bad")),
            "invalid candidate reported: {:?}",
            discovery.diagnostics
        );
        // The symlink loop did not hang and produced no rows.
        assert!(!found.iter().any(|path| path.contains("loop")));
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn overlapping_roots_and_roots_inside_a_worktree_deduplicate() {
        wsl::watchdog(180);
        let dir = wsl::temp_dir("r01b");
        let _restore = RestorePermissions(format!("{dir}/root/locked"));
        build_fixture(&dir);
        let root = format!("{dir}/root");

        let single = discover(std::slice::from_ref(&root));
        let overlapping = discover(&[root.clone(), format!("{root}/work/outer")]);
        assert_eq!(
            overlapping.repos.len(),
            single.repos.len(),
            "overlapping roots produced duplicate rows: {:?}",
            worktrees(&overlapping)
        );

        // A root inside a worktree resolves the containing repository, with no
        // input from the process working directory (which is on Windows).
        let inside_root = format!("{root}/work/outer/src");
        let inside = discover(std::slice::from_ref(&inside_root));
        assert_eq!(worktrees(&inside), vec![format!("{root}/work/outer")]);
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn linked_worktrees_share_common_storage_but_stay_distinct_rows() {
        wsl::watchdog(180);
        let dir = wsl::temp_dir("r01c");
        let _restore = RestorePermissions(format!("{dir}/root/locked"));
        build_fixture(&dir);
        let root = format!("{dir}/root");

        let discovery = discover(std::slice::from_ref(&root));
        let outer_path = format!("{root}/work/outer");
        let linked_path = format!("{root}/work/linked");
        let outer = discovery
            .repos
            .iter()
            .find(|repo| repo.identity.worktree.to_string_lossy() == outer_path)
            .expect("outer repo");
        let linked = discovery
            .repos
            .iter()
            .find(|repo| repo.identity.worktree.to_string_lossy() == linked_path)
            .expect("linked worktree");
        assert_ne!(outer.identity.git_dir, linked.identity.git_dir);
        assert!(linked
            .identity
            .git_dir
            .to_string_lossy()
            .contains("/.git/worktrees/linked"));
        assert_eq!(outer.identity.common_dir, linked.identity.common_dir);
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn unreadable_root_keeps_healthy_results_and_reports() {
        wsl::watchdog(180);
        let dir = wsl::temp_dir("r01d");
        let _restore = RestorePermissions(format!("{dir}/root/locked"));
        build_fixture(&dir);
        let healthy = format!("{dir}/root/work/one");
        let locked = format!("{dir}/root/locked");

        let discovery = discover(&[healthy.clone(), locked.clone()]);
        assert_eq!(worktrees(&discovery), vec![format!("{healthy}/app")]);
        assert!(
            discovery.diagnostics.iter().any(|d| d.contains("locked")),
            "permission failure reported: {:?}",
            discovery.diagnostics
        );
    }

    /// The fast native identity for plain `.git` directories under `/mnt/c`
    /// must agree with what Git itself reports, and linked worktrees (a `.git`
    /// file) must still come from Git.
    #[test]
    #[ignore = "requires a WSL distro"]
    #[cfg(windows)]
    fn native_identity_matches_git_on_a_windows_mount() {
        wsl::watchdog(240);
        let base = std::env::temp_dir().join(format!("spur-native-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo_win = base.join("repo");
        let linked_win = base.join("linked worktree");
        std::fs::create_dir_all(&repo_win).unwrap();
        let to_linux = |path: &std::path::Path| -> Option<String> {
            path.to_string_lossy()
                .replace('\\', "/")
                .strip_prefix("C:/")
                .map(|tail| format!("/mnt/c/{tail}"))
        };
        let (Some(base_linux), Some(repo), Some(linked)) =
            (to_linux(&base), to_linux(&repo_win), to_linux(&linked_win))
        else {
            return; // temp directory is not on C:
        };

        wsl::must(&["git", "-C", &repo, "init", "-q", "-b", "main"]);
        wsl::write_file(&format!("{repo}/f.txt"), b"x\n");
        wsl::must(&["git", "-C", &repo, "add", "f.txt"]);
        wsl::must(&[
            "git", "-C", &repo, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "base",
        ]);
        wsl::must(&["git", "-C", &repo, "worktree", "add", "-q", "--detach", &linked]);

        let discovery = discover(std::slice::from_ref(&base_linux));
        let rows: std::collections::HashMap<String, RepoIdentity> = discovery
            .repos
            .iter()
            .map(|row| (row.identity.worktree.to_string_lossy().into_owned(), row.identity.clone()))
            .collect();

        let expected_repo = crate::git::identity(&repo).expect("git identity for repo");
        assert_eq!(rows.get(&repo), Some(&expected_repo), "plain identity differs from Git");
        let expected_linked = crate::git::identity(&linked).expect("git identity for worktree");
        assert_eq!(rows.get(&linked), Some(&expected_linked), "linked worktree identity differs");
        assert_eq!(expected_repo.common_dir, expected_linked.common_dir);

        // Native status/refs parsing for the same mount must see the branch.
        let status = crate::git::collect_status(&repo).expect("native collect_status");
        assert!(status.refs.branches.iter().any(|branch| branch.name == "main"));
        assert!(status.snapshot.entries.is_empty(), "{:?}", status.snapshot.entries.len());
        let _ = std::fs::remove_dir_all(&base);
    }
}
