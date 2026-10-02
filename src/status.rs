//! Porcelain v2 status, ref metadata, gis flags, and pull eligibility.
//!
//! The parser works on raw bytes: records are NUL-separated (`-z`), paths are
//! never un-quoted or re-encoded, and rename/copy records consume the
//! following NUL field as the original path. Unknown `#` headers are ignored
//! for forward compatibility; malformed mandatory fields are explicit errors,
//! never a clean default. Index (`X`) and worktree (`Y`) state stay separate.
#![allow(dead_code)]

use std::collections::HashSet;

use crate::model::Flag;

/// One side of a status entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Unchanged,
    Modified,
    TypeChanged,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
}

impl Change {
    fn from_byte(byte: u8) -> Result<Self, String> {
        Ok(match byte {
            b'.' => Change::Unchanged,
            b'M' => Change::Modified,
            b'T' => Change::TypeChanged,
            b'A' => Change::Added,
            b'D' => Change::Deleted,
            b'R' => Change::Renamed,
            b'C' => Change::Copied,
            b'U' => Change::Unmerged,
            other => {
                return Err(format!(
                    "unsupported status character {:?}",
                    other as char
                ));
            }
        })
    }
}

/// One changed record (`1`/`2`/`u`) with its raw path bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub index: Change,
    pub worktree: Change,
    pub path: Vec<u8>,
    /// Original path of a rename/copy record.
    pub orig_path: Option<Vec<u8>>,
    /// Record belongs to a submodule (the `S…` sub field).
    pub submodule: bool,
    pub unmerged: bool,
}

impl StatusEntry {
    pub fn is_index_change(&self) -> bool {
        !matches!(
            self.index,
            Change::Unchanged | Change::Unmerged
        )
    }

    pub fn is_worktree_change(&self) -> bool {
        !matches!(
            self.worktree,
            Change::Unchanged | Change::Unmerged
        )
    }
}

/// Everything `git status --porcelain=v2 --branch --show-stash -z` reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusSnapshot {
    /// `None` while unborn (no commits yet).
    pub head_oid: Option<String>,
    /// Current branch name; `None` when detached.
    pub branch: Option<String>,
    pub detached: bool,
    pub unborn: bool,
    /// Configured upstream of the current branch, if any.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub stash: u32,
    pub entries: Vec<StatusEntry>,
    pub untracked: Vec<Vec<u8>>,
}

impl StatusSnapshot {
    pub fn has_conflicts(&self) -> bool {
        self.entries.iter().any(|entry| entry.unmerged)
    }

    pub fn has_untracked(&self) -> bool {
        !self.untracked.is_empty()
    }

    pub fn has_index_changes(&self) -> bool {
        self.entries.iter().any(StatusEntry::is_index_change)
    }

    pub fn has_worktree_changes(&self) -> bool {
        self.entries.iter().any(StatusEntry::is_worktree_change)
    }

    /// No changed entries and no untracked content. Stashes do not count.
    pub fn is_clean(&self) -> bool {
        self.entries.is_empty() && self.untracked.is_empty()
    }

    /// Gis-style symbols. Colour is
    /// applied at render time; `Clean` appears only when nothing else does.
    pub fn flags(&self, upstream_gone: bool) -> Vec<Flag> {
        let mut flags = Vec::new();
        if self.stash > 0 {
            flags.push(Flag::Stash);
        }
        if self.has_untracked() {
            flags.push(Flag::Untracked);
        }
        if self
            .entries
            .iter()
            .any(|entry| matches!(entry.index, Change::Modified) || matches!(entry.worktree, Change::Modified))
        {
            flags.push(Flag::Modified);
        }
        if self
            .entries
            .iter()
            .any(|entry| entry.index == Change::Added)
        {
            flags.push(Flag::AddedStaged);
        }
        if self.entries.iter().any(|entry| {
            matches!(entry.index, Change::Deleted) || matches!(entry.worktree, Change::Deleted)
        }) {
            flags.push(Flag::Deleted);
        }
        if self
            .entries
            .iter()
            .any(|entry| entry.index == Change::Renamed)
        {
            flags.push(Flag::Renamed);
        }
        if self.has_conflicts() {
            flags.push(Flag::Conflicted);
        }
        if self.ahead > 0 && self.behind > 0 {
            flags.push(Flag::Diverged);
        } else if self.ahead > 0 {
            flags.push(Flag::Ahead);
        } else if self.behind > 0 {
            flags.push(Flag::Behind);
        }
        if upstream_gone {
            flags.push(Flag::UpstreamMissing);
        }
        if flags.is_empty() {
            flags.push(Flag::Clean);
        }
        flags
    }
}

/// One changed file on one side (index or worktree), with its raw path bytes
/// so Git commands never see a display-sanitized path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeItem {
    pub path: Vec<u8>,
    /// Original path of a rename/copy record.
    pub orig_path: Option<Vec<u8>>,
    /// Change on this side (`Added` for an untracked file).
    pub kind: Change,
    pub untracked: bool,
}

impl ChangeItem {
    /// Display path: `old → new` for renames, sanitized, never used as an
    /// argument.
    pub fn display_path(&self) -> String {
        match &self.orig_path {
            Some(orig) => format!(
                "{} → {}",
                crate::process::sanitize(orig),
                crate::process::sanitize(&self.path)
            ),
            None => crate::process::sanitize(&self.path),
        }
    }
}

/// The two Local Changes sections: worktree-side and index-side changes. A
/// partially staged file (`MM`) appears in both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeLists {
    pub unstaged: Vec<ChangeItem>,
    pub staged: Vec<ChangeItem>,
}

impl ChangeLists {
    pub fn is_empty(&self) -> bool {
        self.unstaged.is_empty() && self.staged.is_empty()
    }

    pub fn contains(&self, path: &[u8], staged: bool) -> bool {
        let list = if staged { &self.staged } else { &self.unstaged };
        list.iter().any(|item| item.path == path)
    }
}

/// Split a status snapshot into the SourceGit-style UNSTAGED/STAGED lists.
/// Index (`X`) and worktree (`Y`) state stay separate; unmerged entries are
/// shown as conflicts on the worktree side, and untracked files as additions.
pub fn change_lists(snapshot: &StatusSnapshot) -> ChangeLists {
    let mut lists = ChangeLists::default();
    for entry in &snapshot.entries {
        if entry.unmerged {
            lists.unstaged.push(ChangeItem {
                path: entry.path.clone(),
                orig_path: entry.orig_path.clone(),
                kind: Change::Unmerged,
                untracked: false,
            });
            continue;
        }
        if entry.is_worktree_change() {
            lists.unstaged.push(ChangeItem {
                path: entry.path.clone(),
                orig_path: entry.orig_path.clone(),
                kind: entry.worktree,
                untracked: false,
            });
        }
        if entry.is_index_change() {
            lists.staged.push(ChangeItem {
                path: entry.path.clone(),
                orig_path: entry.orig_path.clone(),
                kind: entry.index,
                untracked: false,
            });
        }
    }
    for path in &snapshot.untracked {
        lists.unstaged.push(ChangeItem {
            path: path.clone(),
            orig_path: None,
            kind: Change::Added,
            untracked: true,
        });
    }
    lists
}

/// One local branch with its upstream state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub current: bool,
    /// Short configured upstream (`origin/main`); `None` when unset.
    pub upstream: Option<String>,
    /// Upstream configured but its ref is missing locally (`✗`).
    pub upstream_gone: bool,
    /// Commits ahead of / behind the upstream (0 without one).
    pub ahead: u32,
    pub behind: u32,
    /// Tip commit time (unix seconds) for recency sorting; 0 when unknown.
    pub updated: i64,
}

/// Ahead/behind counts from a `%(upstream:track)` value (`[ahead 2]`,
/// `[behind 1]`, `[ahead 2, behind 1]`, `[gone]`, or empty).
pub fn parse_track_counts(track: &str) -> (u32, u32) {
    let track = track.trim().trim_start_matches('[').trim_end_matches(']');
    let mut ahead = 0;
    let mut behind = 0;
    for part in track.split(',') {
        let mut words = part.split_whitespace();
        match (words.next(), words.next()) {
            (Some("ahead"), Some(n)) => ahead = n.parse().unwrap_or(0),
            (Some("behind"), Some(n)) => behind = n.parse().unwrap_or(0),
            _ => {}
        }
    }
    (ahead, behind)
}

/// One remote-tracking branch (`origin/feature/x`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranch {
    /// Remote name (`origin`).
    pub remote: String,
    /// Branch path after the remote (`feature/x`).
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefInfo {
    pub branches: Vec<BranchInfo>,
    /// Remote-tracking branches, excluding each remote's `HEAD` symref.
    pub remote_branches: Vec<RemoteBranch>,
    /// Local name behind `refs/remotes/origin/HEAD`, when it exists.
    pub default_branch: Option<String>,
    /// Hash over every ref objectname seen in the last query. Changes whenever
    /// a branch/remote/tag moves, so the UI can refresh history immediately
    /// instead of waiting for a full reload.
    pub revision: u64,
}

impl RefInfo {
    /// `(+N)`: all other local branches, excluding the current branch and the
    /// known default branch *when a local branch with that name exists*.
    pub fn extra_branches(&self) -> u32 {
        let names: HashSet<&str> = self.branches.iter().map(|b| b.name.as_str()).collect();
        let default = self
            .default_branch
            .as_deref()
            .filter(|name| names.contains(name));
        self.branches
            .iter()
            .filter(|branch| !branch.current && Some(branch.name.as_str()) != default)
            .count() as u32
    }
}

/// Parse NUL-separated porcelain v2 bytes.
pub fn parse_porcelain_v2(bytes: &[u8]) -> Result<StatusSnapshot, String> {
    let records: Vec<&[u8]> = bytes.split(|&byte| byte == 0).collect();
    let mut snapshot = StatusSnapshot::default();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() {
            continue;
        }
        match record[0] {
            b'#' => parse_header(record, &mut snapshot)?,
            b'1' => {
                let fields = split_fields(record, 9, "ordinary")?;
                let (index_change, worktree_change) = parse_xy(fields[1])?;
                validate_ordinary_modes(&fields)?;
                snapshot.entries.push(StatusEntry {
                    index: index_change,
                    worktree: worktree_change,
                    path: fields[8].to_vec(),
                    orig_path: None,
                    submodule: is_submodule(fields[2]),
                    unmerged: false,
                });
            }
            b'2' => {
                let fields = split_fields(record, 10, "rename/copy")?;
                let (index_change, worktree_change) = parse_xy(fields[1])?;
                validate_ordinary_modes(&fields)?;
                let orig = records.get(index).ok_or_else(|| {
                    format!(
                        "rename record for {:?} is missing its original path",
                        String::from_utf8_lossy(fields[9])
                    )
                })?;
                index += 1;
                if orig.is_empty() {
                    return Err("rename record has an empty original path".to_string());
                }
                snapshot.entries.push(StatusEntry {
                    index: index_change,
                    worktree: worktree_change,
                    path: fields[9].to_vec(),
                    orig_path: Some(orig.to_vec()),
                    submodule: is_submodule(fields[2]),
                    unmerged: false,
                });
            }
            b'u' => {
                let fields = split_fields(record, 11, "unmerged")?;
                let (index_change, worktree_change) = parse_xy(fields[1])?;
                if fields[2].len() != 4 {
                    return Err(format!(
                        "unmerged record has a malformed submodule field {:?}",
                        String::from_utf8_lossy(fields[2])
                    ));
                }
                for mode in [fields[3], fields[4], fields[5], fields[6]] {
                    if !is_octal(mode) {
                        return Err(format!(
                            "unmerged record has a malformed mode {:?}",
                            String::from_utf8_lossy(mode)
                        ));
                    }
                }
                for oid in [fields[7], fields[8], fields[9]] {
                    if !is_hex(oid) {
                        return Err(format!(
                            "unmerged record has a malformed object id {:?}",
                            String::from_utf8_lossy(oid)
                        ));
                    }
                }
                snapshot.entries.push(StatusEntry {
                    index: index_change,
                    worktree: worktree_change,
                    path: fields[10].to_vec(),
                    orig_path: None,
                    submodule: is_submodule(fields[2]),
                    unmerged: true,
                });
            }
            b'?' => {
                let path = &record[2..];
                if path.is_empty() {
                    return Err("untracked record has an empty path".to_string());
                }
                snapshot.untracked.push(path.to_vec());
            }
            b'!' => {} // ignored entries are not requested; tolerate them
            other => {
                return Err(format!(
                    "unsupported status record starting with {:?}",
                    other as char
                ));
            }
        }
    }
    Ok(snapshot)
}

fn parse_header(record: &[u8], snapshot: &mut StatusSnapshot) -> Result<(), String> {
    let text = String::from_utf8_lossy(record);
    let Some(header) = text.strip_prefix("# ") else {
        return Ok(());
    };
    if let Some(value) = header.strip_prefix("branch.oid ") {
        snapshot.unborn = value == "(initial)";
        if !snapshot.unborn {
            snapshot.head_oid = Some(value.to_string());
        }
    } else if let Some(value) = header.strip_prefix("branch.head ") {
        if value == "(detached)" {
            snapshot.detached = true;
            snapshot.branch = None;
        } else {
            snapshot.branch = Some(value.to_string());
        }
    } else if let Some(value) = header.strip_prefix("branch.upstream ") {
        snapshot.upstream = Some(value.to_string());
    } else if let Some(value) = header.strip_prefix("branch.ab ") {
        let mut parts = value.split_whitespace();
        let ahead = parts.next().and_then(|p| p.strip_prefix('+'));
        let behind = parts.next().and_then(|p| p.strip_prefix('-'));
        match (ahead.and_then(|v| v.parse::<u32>().ok()), behind.and_then(|v| v.parse::<u32>().ok())) {
            (Some(ahead), Some(behind)) => {
                snapshot.ahead = ahead;
                snapshot.behind = behind;
            }
            _ => return Err(format!("malformed branch.ab header {value:?}")),
        }
    } else if let Some(value) = header.strip_prefix("stash ") {
        snapshot.stash = value
            .trim()
            .parse::<u32>()
            .map_err(|_| format!("malformed stash header {value:?}"))?;
    }
    // Unknown headers are ignored so a newer Git cannot break parsing.
    Ok(())
}

fn split_fields<'a>(record: &'a [u8], count: usize, kind: &str) -> Result<Vec<&'a [u8]>, String> {
    let fields: Vec<&[u8]> = record.splitn(count, |&byte| byte == b' ').collect();
    if fields.len() != count {
        return Err(format!(
            "malformed {kind} status record (want {count} fields): {:?}",
            String::from_utf8_lossy(record)
        ));
    }
    Ok(fields)
}

fn parse_xy(field: &[u8]) -> Result<(Change, Change), String> {
    if field.len() != 2 {
        return Err(format!(
            "malformed XY field {:?}",
            String::from_utf8_lossy(field)
        ));
    }
    Ok((Change::from_byte(field[0])?, Change::from_byte(field[1])?))
}

/// Fields for `1`/`2`: `1 XY sub mH mI mW hH hI …` — modes 2..=4 octal,
/// hashes 5..=6 hex.
fn validate_ordinary_modes(fields: &[&[u8]]) -> Result<(), String> {
    for mode in [fields[3], fields[4], fields[5]] {
        if !is_octal(mode) {
            return Err(format!(
                "malformed file mode {:?}",
                String::from_utf8_lossy(mode)
            ));
        }
    }
    for oid in [fields[6], fields[7]] {
        if !is_hex(oid) {
            return Err(format!(
                "malformed object id {:?}",
                String::from_utf8_lossy(oid)
            ));
        }
    }
    Ok(())
}

fn is_submodule(field: &[u8]) -> bool {
    field.first() == Some(&b'S') && field.len() == 4
}

fn is_octal(field: &[u8]) -> bool {
    !field.is_empty() && field.iter().all(|b| (b'0'..=b'7').contains(b))
}

fn is_hex(field: &[u8]) -> bool {
    !field.is_empty() && field.iter().all(u8::is_ascii_hexdigit)
}

/// Pull eligibility decision. The
/// caller must already have a successful status; errors are never eligible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullDecision {
    Eligible { branch: String, upstream: String },
    Skip(String),
}

impl PullDecision {
    pub fn is_eligible(&self) -> bool {
        matches!(self, PullDecision::Eligible { .. })
    }
}

/// Decide whether one worktree may be fast-forwarded. `markers` are active
/// operation marker names (`MERGE_HEAD`, …) and `populated_submodules` the
/// count of initialized submodules; both are only needed when everything else
/// already passed.
pub fn pull_decision(
    snapshot: &StatusSnapshot,
    upstream_gone: bool,
    markers: &[String],
    populated_submodules: u32,
) -> PullDecision {
    if snapshot.unborn {
        return PullDecision::Skip("unborn branch".to_string());
    }
    if snapshot.detached || snapshot.branch.is_none() {
        return PullDecision::Skip("detached HEAD".to_string());
    }
    let branch = snapshot.branch.clone().unwrap_or_default();
    let Some(upstream) = snapshot.upstream.clone() else {
        return PullDecision::Skip("no upstream configured".to_string());
    };
    if upstream_gone {
        return PullDecision::Skip(format!("upstream {upstream} is missing locally"));
    }
    if snapshot.ahead > 0 && snapshot.behind > 0 {
        return PullDecision::Skip(format!(
            "diverged from {upstream} (ahead {}, behind {})",
            snapshot.ahead, snapshot.behind
        ));
    }
    if snapshot.ahead > 0 {
        return PullDecision::Skip(format!("ahead of {upstream}"));
    }
    if snapshot.behind == 0 {
        return PullDecision::Skip("already up to date".to_string());
    }
    if snapshot.has_conflicts() {
        return PullDecision::Skip("unresolved conflicts".to_string());
    }
    if !markers.is_empty() {
        return PullDecision::Skip(format!("operation in progress: {}", markers.join(", ")));
    }
    if snapshot.has_index_changes() {
        return PullDecision::Skip("staged changes".to_string());
    }
    if snapshot.has_worktree_changes() {
        return PullDecision::Skip("unstaged changes".to_string());
    }
    if snapshot.has_untracked() {
        return PullDecision::Skip("untracked content".to_string());
    }
    if populated_submodules > 0 {
        return PullDecision::Skip(format!(
            "has {populated_submodules} populated submodule(s); Release 1 skips these"
        ));
    }
    PullDecision::Eligible { branch, upstream }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Join records with NUL terminators, as `-z` output does.
    fn z(records: &[&str]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for record in records {
            bytes.extend_from_slice(record.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    fn paths(entries: &[StatusEntry]) -> Vec<Vec<u8>> {
        entries.iter().map(|entry| entry.path.clone()).collect()
    }

    #[test]
    fn unknown_headers_are_ignored_and_known_facts_survive() {
        let bytes = z(&[
            "# branch.oid abc123",
            "# branch.future thing=1",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -3",
            "# stash 1",
        ]);
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        assert_eq!(snapshot.head_oid.as_deref(), Some("abc123"));
        assert_eq!(snapshot.branch.as_deref(), Some("main"));
        assert_eq!(snapshot.upstream.as_deref(), Some("origin/main"));
        assert_eq!((snapshot.ahead, snapshot.behind), (2, 3));
        assert_eq!(snapshot.stash, 1);
    }

    #[test]
    fn truncated_rename_is_an_error_not_a_lost_file() {
        let bytes = z(&["2 R. N... 100644 100644 100644 aaaa bbbb R100 new.txt"]);
        let err = parse_porcelain_v2(&bytes).unwrap_err();
        assert!(err.contains("original path"), "{err}");
    }

    #[test]
    fn malformed_numbers_are_errors_never_clean_defaults() {
        let bad_ab = z(&["# branch.ab +x -1"]);
        assert!(parse_porcelain_v2(&bad_ab).is_err());
        let bad_stash = z(&["# stash many"]);
        assert!(parse_porcelain_v2(&bad_stash).is_err());
        let bad_mode = z(&["1 M. N... 10x644 100644 100644 aaaa bbbb f.txt"]);
        assert!(parse_porcelain_v2(&bad_mode).is_err());
        let bad_xy = z(&["1 MX N... 100644 100644 100644 aaaa bbbb f.txt"]);
        assert!(parse_porcelain_v2(&bad_xy).is_err());
        let unknown_record = z(&["9 weird"]);
        assert!(parse_porcelain_v2(&unknown_record).is_err());
    }

    #[test]
    fn conflict_records_stay_unmerged() {
        let bytes = z(&[
            "u UU N... 100644 100644 100644 100644 df96 ba29 2299 f.txt",
        ]);
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        assert!(snapshot.has_conflicts());
        assert_eq!(snapshot.entries[0].index, Change::Unmerged);
        assert_eq!(snapshot.entries[0].worktree, Change::Unmerged);
        assert_eq!(snapshot.entries[0].path, b"f.txt");
        let flags = snapshot.flags(false);
        assert!(flags.contains(&Flag::Conflicted));
        assert!(!flags.contains(&Flag::Clean));
    }

    fn change_names(items: &[ChangeItem]) -> Vec<String> {
        items
            .iter()
            .map(|item| String::from_utf8_lossy(&item.path).into_owned())
            .collect()
    }

    #[test]
    fn change_lists_split_index_and_worktree_sides() {
        let bytes = z(&[
            "# branch.head main",
            "1 .M N... 100644 100644 100644 aaaa bbbb worktree-only.txt",
            "1 M. N... 100644 100644 100644 aaaa bbbb index-only.txt",
            "1 MM N... 100644 100644 100644 aaaa bbbb both.txt",
            "1 A. N... 000000 100644 100644 0000 aaaa added.txt",
            "1 .D N... 100644 100644 000000 aaaa 0000 deleted.txt",
            "2 R. N... 100644 100644 100644 aaaa bbbb R100 renamed.txt",
            "old-name.txt",
            "? untracked file.txt",
        ]);
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        let lists = change_lists(&snapshot);

        assert_eq!(
            change_names(&lists.unstaged),
            [
                "worktree-only.txt",
                "both.txt",
                "deleted.txt",
                "untracked file.txt"
            ]
        );
        assert_eq!(
            change_names(&lists.staged),
            ["index-only.txt", "both.txt", "added.txt", "renamed.txt"]
        );
        assert!(lists.contains(b"both.txt", true));
        assert!(lists.contains(b"both.txt", false));
        assert!(!lists.contains(b"index-only.txt", false));

        let both = lists.unstaged.iter().find(|i| i.path == b"both.txt").unwrap();
        assert_eq!(both.kind, Change::Modified);
        let staged_both = lists.staged.iter().find(|i| i.path == b"both.txt").unwrap();
        assert_eq!(staged_both.kind, Change::Modified);

        let untracked = lists.unstaged.last().unwrap();
        assert!(untracked.untracked);
        assert_eq!(untracked.kind, Change::Added);

        let renamed = lists.staged.iter().find(|i| i.path == b"renamed.txt").unwrap();
        assert_eq!(renamed.kind, Change::Renamed);
        assert_eq!(renamed.orig_path.as_deref(), Some(&b"old-name.txt"[..]));
        assert_eq!(renamed.display_path(), "old-name.txt → renamed.txt");
        assert!(!lists.is_empty());
    }

    #[test]
    fn unmerged_entries_are_worktree_side_conflicts() {
        let bytes = z(&["u UU N... 100644 100644 100644 100644 df96 ba29 2299 conflict.txt"]);
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        let lists = change_lists(&snapshot);
        assert_eq!(change_names(&lists.unstaged), ["conflict.txt"]);
        assert_eq!(lists.unstaged[0].kind, Change::Unmerged);
        assert!(lists.staged.is_empty());
    }

    #[test]
    fn staged_only_files_never_appear_unstaged_and_vice_versa() {
        let staged = z(&["1 M. N... 100644 100644 100644 aaaa bbbb staged.txt"]);
        let lists = change_lists(&parse_porcelain_v2(&staged).unwrap());
        assert!(lists.unstaged.is_empty());
        assert_eq!(change_names(&lists.staged), ["staged.txt"]);

        let unstaged = z(&["1 .M N... 100644 100644 100644 aaaa bbbb loose.txt"]);
        let lists = change_lists(&parse_porcelain_v2(&unstaged).unwrap());
        assert!(lists.staged.is_empty());
        assert_eq!(change_names(&lists.unstaged), ["loose.txt"]);
    }

    #[test]
    fn record_order_does_not_change_the_semantic_snapshot() {
        let first = z(&[
            "# branch.head main",
            "1 M. N... 100644 100644 100644 aaaa bbbb one.txt",
            "1 .D N... 100644 100644 000000 cccc dddd two.txt",
            "? untracked.txt",
        ]);
        let second = z(&[
            "# branch.head main",
            "? untracked.txt",
            "1 .D N... 100644 100644 000000 cccc dddd two.txt",
            "1 M. N... 100644 100644 100644 aaaa bbbb one.txt",
        ]);
        let mut a = parse_porcelain_v2(&first).unwrap();
        let mut b = parse_porcelain_v2(&second).unwrap();
        a.entries.sort_by(|x, y| x.path.cmp(&y.path));
        b.entries.sort_by(|x, y| x.path.cmp(&y.path));
        a.untracked.sort();
        b.untracked.sort();
        assert_eq!(a, b);
    }

    #[test]
    fn parses_ordinary_rename_untracked_bytes_and_headers() {
        let bytes = z(&[
            "# branch.oid 3f93361a66aa881b179c73a1e4a4fd736ca0fd45",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +0 -4",
            "# stash 2",
            "1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 3e757656cf36eca53338e520d134963a44f793f8 added.txt",
            "1 .D N... 100644 100644 000000 2fa992c0b8b5c6acd2bdd4fa31de29d29799bdd5 2fa992c0b8b5c6acd2bdd4fa31de29d29799bdd5 delete-me.txt",
            "2 R. N... 100644 100644 100644 4286f428e3b19fe84de503916ce0e7dc8deefea1 4286f428e3b19fe84de503916ce0e7dc8deefea1 R100 ren2.txt",
            "ren.txt",
            "1 MM N... 100644 100644 100644 f70f10e4db19068f79bc43844b49f3eece45c4e8 223b7836fb19fdf64ba2d3cd6173c6a283141f78 tracked.txt",
            "? with space.txt",
            "? bad\u{fffd}name",
        ]);
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        assert_eq!(snapshot.branch.as_deref(), Some("main"));
        assert_eq!(snapshot.upstream.as_deref(), Some("origin/main"));
        assert_eq!((snapshot.ahead, snapshot.behind), (0, 4));
        assert_eq!(snapshot.stash, 2);
        assert_eq!(snapshot.entries.len(), 4);
        assert_eq!(paths(&snapshot.entries), vec![
            b"added.txt".to_vec(),
            b"delete-me.txt".to_vec(),
            b"ren2.txt".to_vec(),
            b"tracked.txt".to_vec(),
        ]);
        let rename = &snapshot.entries[2];
        assert_eq!(rename.index, Change::Renamed);
        assert_eq!(rename.orig_path.as_deref(), Some(b"ren.txt".as_ref()));
        let both_sides = &snapshot.entries[3];
        assert_eq!(both_sides.index, Change::Modified);
        assert_eq!(both_sides.worktree, Change::Modified);
        assert!(both_sides.is_index_change() && both_sides.is_worktree_change());
        assert_eq!(snapshot.untracked, vec![
            b"with space.txt".to_vec(),
            "bad\u{fffd}name".as_bytes().to_vec(),
        ]);
    }

    #[test]
    fn paths_with_newlines_survive_because_records_are_nul_separated() {
        let mut bytes = Vec::new();
        for record in [
            b"# branch.head main".as_slice(),
            b"? tab\tname",
            b"? new\nline.txt",
        ] {
            bytes.extend_from_slice(record);
            bytes.push(0);
        }
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        assert_eq!(snapshot.untracked, vec![
            b"tab\tname".to_vec(),
            b"new\nline.txt".to_vec(),
        ]);
    }

    #[test]
    fn unborn_and_detached_are_explicit() {
        let unborn = parse_porcelain_v2(&z(&[
            "# branch.oid (initial)",
            "# branch.head main",
        ]))
        .unwrap();
        assert!(unborn.unborn && !unborn.detached);
        assert_eq!(unborn.branch.as_deref(), Some("main"));

        let detached = parse_porcelain_v2(&z(&[
            "# branch.oid abc123",
            "# branch.head (detached)",
        ]))
        .unwrap();
        assert!(detached.detached && !detached.unborn);
        assert_eq!(detached.branch, None);
    }

    #[test]
    fn staged_modification_is_modified_not_added() {
        let bytes = z(&["1 M. N... 100644 100644 100644 aaaa bbbb tracked.txt"]);
        let snapshot = parse_porcelain_v2(&bytes).unwrap();
        let flags = snapshot.flags(false);
        assert!(flags.contains(&Flag::Modified), "{flags:?}");
        assert!(!flags.contains(&Flag::AddedStaged), "{flags:?}");

        let added = z(&["1 A. N... 000000 100644 100644 000000 bbbb new.txt"]);
        let flags = parse_porcelain_v2(&added).unwrap().flags(false);
        assert!(flags.contains(&Flag::AddedStaged), "{flags:?}");
        assert!(!flags.contains(&Flag::Modified), "{flags:?}");
    }

    #[test]
    fn flags_cover_every_concept_symbol_and_clean_is_only_empty() {
        let clean = parse_porcelain_v2(&z(&["# branch.head main"])).unwrap();
        assert_eq!(clean.flags(false), vec![Flag::Clean]);

        let rich = parse_porcelain_v2(&z(&[
            "# branch.head main",
            "# stash 1",
            "1 A. N... 000000 100644 100644 000000 aaaa added.txt",
            "1 MM N... 100644 100644 100644 aaaa bbbb tracked.txt",
            "1 .D N... 100644 100644 000000 aaaa aaaa gone.txt",
            "2 R. N... 100644 100644 100644 aaaa aaaa R100 new.txt",
            "old.txt",
            "? loose.txt",
        ]))
        .unwrap();
        let flags = rich.flags(false);
        for expected in [
            Flag::Stash,
            Flag::Untracked,
            Flag::Modified,
            Flag::AddedStaged,
            Flag::Deleted,
            Flag::Renamed,
        ] {
            assert!(flags.contains(&expected), "missing {expected:?}: {flags:?}");
        }
        assert!(!flags.contains(&Flag::Clean));

        let sync = parse_porcelain_v2(&z(&["# branch.head main", "# branch.ab +2 -0"])).unwrap();
        assert_eq!(sync.flags(false), vec![Flag::Ahead]);
        let behind = parse_porcelain_v2(&z(&["# branch.head main", "# branch.ab +0 -3"])).unwrap();
        assert_eq!(behind.flags(false), vec![Flag::Behind]);
        let diverged = parse_porcelain_v2(&z(&["# branch.head main", "# branch.ab +1 -3"])).unwrap();
        assert_eq!(diverged.flags(false), vec![Flag::Diverged]);
        assert_eq!(
            clean.flags(true),
            vec![Flag::UpstreamMissing],
            "a gone upstream must not look clean"
        );
    }

    fn branch(name: &str, current: bool) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            current,
            upstream: None,
            upstream_gone: false,
            ahead: 0,
            behind: 0,
            updated: 0,
        }
    }

    #[test]
    fn track_counts_cover_ahead_behind_gone_and_empty() {
        assert_eq!(parse_track_counts("[ahead 2]"), (2, 0));
        assert_eq!(parse_track_counts("[behind 3]"), (0, 3));
        assert_eq!(parse_track_counts("[ahead 2, behind 3]"), (2, 3));
        assert_eq!(parse_track_counts("[gone]"), (0, 0));
        assert_eq!(parse_track_counts(""), (0, 0));
        assert_eq!(parse_track_counts("[ahead x]"), (0, 0));
    }

    #[test]
    fn extra_branches_excludes_only_names_that_exist() {
        let refs = RefInfo {
            branches: vec![branch("main", true), branch("feature/x", false)],
            default_branch: Some("main".into()),
            ..RefInfo::default()
        };
        // current == default: only the current is excluded.
        assert_eq!(refs.extra_branches(), 1);

        let refs = RefInfo {
            branches: vec![
                branch("main", false),
                branch("work", true),
                branch("other", false),
            ],
            default_branch: Some("main".into()),
            ..RefInfo::default()
        };
        // default has a local branch: it is excluded along with the current.
        assert_eq!(refs.extra_branches(), 1);

        let refs = RefInfo {
            branches: vec![branch("work", true), branch("other", false)],
            default_branch: Some("main".into()),
            ..RefInfo::default()
        };
        // default is only a remote ref: nothing extra is subtracted.
        assert_eq!(refs.extra_branches(), 1);

        let refs = RefInfo {
            branches: vec![branch("work", true), branch("other", false)],
            default_branch: None,
            ..RefInfo::default()
        };
        // origin/HEAD absent: only the current is excluded.
        assert_eq!(refs.extra_branches(), 1);

        let refs = RefInfo {
            branches: vec![branch("solo", true)],
            default_branch: Some("solo".into()),
            ..RefInfo::default()
        };
        assert_eq!(refs.extra_branches(), 0);
    }

    fn snapshot_for_decision() -> StatusSnapshot {
        parse_porcelain_v2(&z(&[
            "# branch.oid abc",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +0 -2",
        ]))
        .unwrap()
    }

    #[test]
    fn eligibility_requires_a_clean_behind_only_branch_with_a_live_upstream() {
        let snapshot = snapshot_for_decision();
        assert_eq!(
            pull_decision(&snapshot, false, &[], 0),
            PullDecision::Eligible { branch: "main".into(), upstream: "origin/main".into() }
        );
        assert!(matches!(pull_decision(&snapshot, true, &[], 0), PullDecision::Skip(_)));

        let dirty = parse_porcelain_v2(&z(&[
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +0 -2",
            "1 .M N... 100644 100644 100644 aaaa bbbb f.txt",
        ]))
        .unwrap();
        let reason = match pull_decision(&dirty, false, &[], 0) {
            PullDecision::Skip(reason) => reason,
            other => panic!("expected skip, got {other:?}"),
        };
        assert!(reason.contains("unstaged"), "{reason}");

        let staged = parse_porcelain_v2(&z(&[
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +0 -2",
            "1 M. N... 100644 100644 100644 aaaa bbbb f.txt",
        ]))
        .unwrap();
        let reason = match pull_decision(&staged, false, &[], 0) {
            PullDecision::Skip(reason) => reason,
            other => panic!("expected skip, got {other:?}"),
        };
        assert!(reason.contains("staged"), "{reason}");

        assert!(matches!(
            pull_decision(&snapshot, false, &["MERGE_HEAD".into()], 0),
            PullDecision::Skip(reason) if reason.contains("MERGE_HEAD")
        ));
        assert!(matches!(
            pull_decision(&snapshot, false, &[], 1),
            PullDecision::Skip(reason) if reason.contains("submodule")
        ));

        let up_to_date = parse_porcelain_v2(&z(&[
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +0 -0",
        ]))
        .unwrap();
        assert!(matches!(
            pull_decision(&up_to_date, false, &[], 0),
            PullDecision::Skip(reason) if reason.contains("up to date")
        ));

        let no_upstream =
            parse_porcelain_v2(&z(&["# branch.head main", "# branch.ab +0 -2"])).unwrap();
        assert!(matches!(
            pull_decision(&no_upstream, false, &[], 0),
            PullDecision::Skip(reason) if reason.contains("no upstream")
        ));

        let diverged = parse_porcelain_v2(&z(&[
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +1 -2",
        ]))
        .unwrap();
        assert!(matches!(
            pull_decision(&diverged, false, &[], 0),
            PullDecision::Skip(reason) if reason.contains("diverged")
        ));

        let detached =
            parse_porcelain_v2(&z(&["# branch.head (detached)", "# branch.oid abc"])).unwrap();
        assert!(matches!(
            pull_decision(&detached, false, &[], 0),
            PullDecision::Skip(reason) if reason.contains("detached")
        ));

        let unborn =
            parse_porcelain_v2(&z(&["# branch.head main", "# branch.oid (initial)"])).unwrap();
        assert!(matches!(
            pull_decision(&unborn, false, &[], 0),
            PullDecision::Skip(reason) if reason.contains("unborn")
        ));

        let conflicted = parse_porcelain_v2(&z(&[
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +0 -2",
            "u UU N... 100644 100644 100644 100644 aaaa bbbb cccc f.txt",
        ]))
        .unwrap();
        let reason = match pull_decision(&conflicted, false, &[], 0) {
            PullDecision::Skip(reason) => reason,
            other => panic!("expected skip, got {other:?}"),
        };
        assert!(reason.contains("conflict"), "{reason}");
    }
}

/// Real-repository status/ref tests plus the marker and submodule queries.
/// They need the WSL distro, so they are ignored by
/// default: run them with `cargo test -- --ignored`.
#[cfg(test)]
mod wsl_tests {
    use super::*;
    use crate::git;
    use crate::process::wsl_support as wsl;

    fn run_fixture(dir: &str, name: &str, script: &str) {
        let path = format!("{dir}/{name}.sh");
        wsl::write_script(&path, script);
        wsl::must(&[path.as_str()]);
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn status_keeps_index_and_worktree_truth_and_path_bytes() {
        wsl::watchdog(180);
        let dir = wsl::temp_dir("r02");
        run_fixture(
            &dir,
            "build",
            &format!(
                r#"#!/bin/bash
set -e
R="{dir}/repo"
mkdir -p "$R"; cd "$R"
git init -q -b main
git config user.name t
git config user.email t@t
# challenge the explicit query option: the config says hide untracked files
git config status.showUntrackedFiles false
printf 'A\n' > tracked.txt
printf 'gone\n' > delete-me.txt
printf 'R\n' > ren.txt
git add .
git commit -q -m base
printf 'B\n' > tracked.txt
git add tracked.txt
printf 'C\n' > tracked.txt
printf 'new\n' > added.txt
git add added.txt
rm delete-me.txt
git mv ren.txt ren2.txt
printf 'u\n' > untracked.txt
printf 'space\n' > 'with space.txt'
printf 'tab\n' > "$(printf 'tab\tname')"
printf 'nl\n' > "$(printf 'new\nline.txt')"
printf 'uni\n' > 'ünïcode-fïle.txt'
printf 'bad\n' > "$(printf 'bad\377name')"
"#
            ),
        );
        let repo = format!("{dir}/repo");

        // Production entry point (runner + parser).
        let snapshot = git::status_snapshot(&repo).expect("status");

        // Oracle through Git and the filesystem, not the parser.
        assert_eq!(
            wsl::must(&["git", "-C", &repo, "show", ":tracked.txt"]),
            b"B\n",
            "index should hold version B"
        );
        assert_eq!(
            wsl::must(&["cat", &format!("{repo}/tracked.txt")]),
            b"C\n",
            "worktree should hold version C"
        );

        let entry = |path: &[u8]| {
            snapshot
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .unwrap_or_else(|| panic!("missing entry {}", String::from_utf8_lossy(path)))
        };
        let tracked = entry(b"tracked.txt");
        assert_eq!((tracked.index, tracked.worktree), (Change::Modified, Change::Modified));
        let added = entry(b"added.txt");
        assert_eq!((added.index, added.worktree), (Change::Added, Change::Unchanged));
        let deleted = entry(b"delete-me.txt");
        assert_eq!((deleted.index, deleted.worktree), (Change::Unchanged, Change::Deleted));
        let renamed = entry(b"ren2.txt");
        assert_eq!(renamed.index, Change::Renamed);
        assert_eq!(renamed.orig_path.as_deref(), Some(b"ren.txt".as_ref()));
        assert!(
            snapshot.entries.iter().all(|entry| entry.path != b"ren.txt"),
            "the rename's original path must not become a second entry"
        );

        // Untracked content is visible despite status.showUntrackedFiles=false,
        // and path bytes survive exactly (space, tab, newline, Unicode,
        // non-UTF-8).
        let mut got = snapshot.untracked.clone();
        got.sort();
        let mut expected: Vec<Vec<u8>> = vec![
            b"untracked.txt".to_vec(),
            b"with space.txt".to_vec(),
            b"tab\tname".to_vec(),
            b"new\nline.txt".to_vec(),
            "ünïcode-fïle.txt".as_bytes().to_vec(),
            b"bad\xffname".to_vec(),
        ];
        expected.sort();
        assert_eq!(got, expected);

        let flags = snapshot.flags(false);
        for expected in [
            Flag::Modified,
            Flag::AddedStaged,
            Flag::Deleted,
            Flag::Renamed,
            Flag::Untracked,
        ] {
            assert!(flags.contains(&expected), "missing {expected:?}: {flags:?}");
        }
        assert!(!flags.contains(&Flag::Clean));
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn stashes_and_conflicts_are_visible_and_never_clean() {
        wsl::watchdog(180);
        let dir = wsl::temp_dir("r02b");
        run_fixture(
            &dir,
            "build",
            &format!(
                r#"#!/bin/bash
set -e
# two stashes
S="{dir}/stash"
mkdir -p "$S"; cd "$S"
git init -q -b main
git config user.name t
git config user.email t@t
printf 'base\n' > f.txt
git add .; git commit -q -m base
printf 'one\n' >> f.txt; git stash push -q -m one
printf 'two\n' >> f.txt; git stash push -q -m two
# real unresolved merge conflict on a branch that is behind its upstream
CR="{dir}/conflict-remote.git"
git init -q --bare -b main "$CR"
git clone -q "$CR" "{dir}/conflict"
C="{dir}/conflict"
cd "$C"
git config user.name t
git config user.email t@t
printf 'base\n' > f.txt
git add .; git commit -q -m base
git push -q origin main
git clone -q "$CR" "{dir}/conflict-publisher"
cd "{dir}/conflict-publisher"
git config user.name t
git config user.email t@t
printf 'remote-one\n' > f.txt
git add .; git commit -q -m one
git push -q origin main
printf 'extra\n' > g.txt
git add .; git commit -q -m two
git push -q origin main
cd "$C"
git fetch -q origin
# main moves to the first remote commit (behind 1), then a side branch based
# on the fork point changes the same line -> real conflict with main
git merge -q --ff-only origin/main~1
git checkout -q -b side origin/main~2
printf 'side\n' > f.txt
git add .; git commit -q -m side
git checkout -q main
git merge side >/dev/null 2>&1 || true
"#
            ),
        );

        let stash = git::collect_status(&format!("{dir}/stash")).expect("stash status");
        assert_eq!(stash.snapshot.stash, 2);
        assert!(stash.snapshot.flags(false).contains(&Flag::Stash));
        assert!(!stash.snapshot.flags(false).contains(&Flag::Clean));

        let conflict = git::collect_status(&format!("{dir}/conflict")).expect("conflict status");
        assert!(conflict.snapshot.has_conflicts());
        let unmerged = conflict
            .snapshot
            .entries
            .iter()
            .find(|entry| entry.unmerged)
            .expect("unmerged entry");
        assert_eq!(unmerged.path, b"f.txt");
        assert_eq!(unmerged.index, Change::Unmerged);
        let flags = conflict.snapshot.flags(false);
        assert!(flags.contains(&Flag::Conflicted), "{flags:?}");
        assert!(!flags.contains(&Flag::Clean));
        let decision = pull_decision(
            &conflict.snapshot,
            conflict.upstream_gone(),
            &[], // conflicts must block before marker or cleanliness checks
            0,
        );
        assert!(matches!(
            decision,
            PullDecision::Skip(reason) if reason.contains("conflict")
        ));
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn branch_information_matches_a_known_graph() {
        wsl::watchdog(240);
        let dir = wsl::temp_dir("r03");
        run_fixture(
            &dir,
            "build",
            &format!(
                r#"#!/bin/bash
set -e
R="{dir}"
mkdir -p "$R"; cd "$R"
git init -q --bare -b main "$R/remote.git"
git clone -q "$R/remote.git" "$R/publisher"
git -C "$R/publisher" config user.name t
git -C "$R/publisher" config user.email t@t
printf 'one\n' > "$R/publisher/f.txt"
git -C "$R/publisher" add .
git -C "$R/publisher" commit -q -m one
git -C "$R/publisher" push -q origin main
for name in sync behind ahead diverged gone remote-default nohead; do
  git clone -q "$R/remote.git" "$R/$name"
  git -C "$R/$name" config user.name t
  git -C "$R/$name" config user.email t@t
done
# behind-only: publisher moves ahead, subject fetches
printf 'two\n' > "$R/publisher/f.txt"
git -C "$R/publisher" commit -qam two
git -C "$R/publisher" push -q origin main
git -C "$R/behind" fetch -q origin
git -C "$R/behind" branch extra1
git -C "$R/behind" branch extra2
# ahead-only
printf 'local-ahead\n' > "$R/ahead/local.txt"
git -C "$R/ahead" add .
git -C "$R/ahead" commit -q -m ahead
# diverged: fetch the publisher's next commit, then add local history
printf 'three\n' > "$R/publisher/f.txt"
git -C "$R/publisher" commit -qam three
git -C "$R/publisher" push -q origin main
git -C "$R/diverged" fetch -q origin
printf 'local-div\n' > "$R/diverged/local.txt"
git -C "$R/diverged" add .
git -C "$R/diverged" commit -q -m div
# gone upstream: pushed, then deleted remotely and pruned
git -C "$R/gone" checkout -q -b feature/gone
git -C "$R/gone" push -q -u origin feature/gone
git -C "$R/publisher" push -q origin --delete feature/gone
git -C "$R/gone" fetch -q --prune origin
# non-origin upstream and a branch containing '/', plus a no-upstream branch
git -C "$R/sync" checkout -q -b feature/deep
git -C "$R/sync" remote add upstream2 "$R/remote.git"
git -C "$R/sync" push -q -u upstream2 feature/deep
git -C "$R/sync" branch loose
# default known only as a remote ref
git -C "$R/remote-default" checkout -q -b work
git -C "$R/remote-default" branch second
git -C "$R/remote-default" branch -D main
# origin/HEAD absent
git -C "$R/nohead" update-ref -d refs/remotes/origin/HEAD
git -C "$R/nohead" checkout -q -b work
# unborn and detached
mkdir -p "$R/unborn"
git -C "$R/unborn" init -q -b main
git -C "$R/unborn" config user.name t
git -C "$R/unborn" config user.email t@t
git clone -q "$R/remote.git" "$R/detached"
git -C "$R/detached" checkout -q --detach
"#
            ),
        );

        // Synchronized behind-only repo: exact counts and identity.
        let behind = git::collect_status(&format!("{dir}/behind")).expect("behind");
        assert_eq!((behind.snapshot.ahead, behind.snapshot.behind), (0, 1));
        assert_eq!(behind.snapshot.branch.as_deref(), Some("main"));
        assert_eq!(behind.snapshot.upstream.as_deref(), Some("origin/main"));
        assert!(!behind.upstream_gone());
        assert_eq!(behind.refs.default_branch.as_deref(), Some("main"));
        // current == default: main is excluded once; extra1 and extra2 count.
        assert_eq!(behind.refs.extra_branches(), 2);
        assert_eq!(behind.snapshot.flags(false), vec![Flag::Behind]);

        let ahead = git::collect_status(&format!("{dir}/ahead")).expect("ahead");
        assert_eq!((ahead.snapshot.ahead, ahead.snapshot.behind), (1, 0));
        assert_eq!(ahead.snapshot.flags(false), vec![Flag::Ahead]);

        let diverged = git::collect_status(&format!("{dir}/diverged")).expect("diverged");
        // Cloned at "one", fetched "two" and "three" (behind 2), then added a
        // local commit (ahead 1).
        assert_eq!((diverged.snapshot.ahead, diverged.snapshot.behind), (1, 2));
        assert_eq!(diverged.snapshot.flags(false), vec![Flag::Diverged]);

        // Non-origin upstream; the branch name with '/' survives.
        let sync = git::collect_status(&format!("{dir}/sync")).expect("sync");
        assert_eq!(sync.snapshot.branch.as_deref(), Some("feature/deep"));
        assert_eq!(sync.snapshot.upstream.as_deref(), Some("upstream2/feature/deep"));
        assert_eq!((sync.snapshot.ahead, sync.snapshot.behind), (0, 0));
        assert_eq!(sync.snapshot.flags(false), vec![Flag::Clean]);
        // current != default (default main has a local branch): main and the
        // current are excluded, only `loose` counts.
        assert_eq!(sync.refs.default_branch.as_deref(), Some("main"));
        assert_eq!(sync.refs.extra_branches(), 1);

        // Gone upstream differs from no upstream.
        let gone = git::collect_status(&format!("{dir}/gone")).expect("gone");
        assert_eq!(gone.snapshot.upstream.as_deref(), Some("origin/feature/gone"));
        assert!(gone.upstream_gone());
        assert!(gone.snapshot.flags(gone.upstream_gone()).contains(&Flag::UpstreamMissing));
        let decision = pull_decision(&gone.snapshot, gone.upstream_gone(), &[], 0);
        assert!(
            matches!(&decision, PullDecision::Skip(reason) if reason.contains("missing locally")),
            "{decision:?}"
        );

        // Default highlighted only as a remote ref: nothing local to exclude.
        let remote_default =
            git::collect_status(&format!("{dir}/remote-default")).expect("remote-default");
        assert_eq!(remote_default.snapshot.branch.as_deref(), Some("work"));
        assert_eq!(remote_default.refs.default_branch.as_deref(), Some("main"));
        assert_eq!(remote_default.refs.extra_branches(), 1);

        // origin/HEAD absent: only the current branch is excluded.
        let nohead = git::collect_status(&format!("{dir}/nohead")).expect("nohead");
        assert_eq!(nohead.refs.default_branch, None);
        assert_eq!(nohead.refs.extra_branches(), 1);

        // Detached and unborn stay explicit and are never pull targets.
        let detached = git::collect_status(&format!("{dir}/detached")).expect("detached");
        assert!(detached.snapshot.detached);
        assert_eq!(detached.snapshot.branch, None);
        assert!(matches!(
            pull_decision(&detached.snapshot, false, &[], 0),
            PullDecision::Skip(reason) if reason.contains("detached")
        ));

        let unborn = git::collect_status(&format!("{dir}/unborn")).expect("unborn");
        assert!(unborn.snapshot.unborn);
        assert_eq!(unborn.snapshot.branch.as_deref(), Some("main"));
        assert!(matches!(
            pull_decision(&unborn.snapshot, false, &[], 0),
            PullDecision::Skip(reason) if reason.contains("unborn")
        ));

        // A clean branch with no configured upstream is not "gone" and is
        // still not eligible for a pull.
        let no_upstream = format!("{dir}/no-upstream");
        wsl::must(&["git", "clone", "-q", &format!("{dir}/remote.git"), &no_upstream]);
        wsl::must(&["git", "-C", &no_upstream, "checkout", "-q", "-b", "loose"]);
        let loose = git::collect_status(&no_upstream).expect("no upstream");
        assert_eq!(loose.snapshot.upstream, None);
        assert!(!loose.upstream_gone());
        assert!(matches!(
            pull_decision(&loose.snapshot, false, &[], 0),
            PullDecision::Skip(reason) if reason.contains("no upstream")
        ));

        // The behind-only repo is the one eligible case here.
        let decision = pull_decision(&behind.snapshot, behind.upstream_gone(), &[], 0);
        assert_eq!(
            decision,
            PullDecision::Eligible { branch: "main".into(), upstream: "origin/main".into() }
        );
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn operation_markers_and_submodules_are_resolved_through_git() {
        wsl::watchdog(180);
        let dir = wsl::temp_dir("r05m");
        run_fixture(
            &dir,
            "build",
            &format!(
                r#"#!/bin/bash
set -e
W="{dir}/work"
mkdir -p "$W"; cd "$W"
git init -q -b main
git config user.name t
git config user.email t@t
printf 'x\n' > f.txt
git add .; git commit -q -m base
# a linked worktree must resolve its own git dir
git worktree add -q --detach "$W/linked"
touch "$(git -C "$W/linked" rev-parse --git-path MERGE_HEAD)"
# populated submodule
S="{dir}/super"
mkdir -p "$S"; cd "$S"
git init -q -b main
git config user.name t
git config user.email t@t
printf 'top\n' > top.txt
git add .; git commit -q -m top
git -c protocol.file.allow=always submodule add -q "{dir}/work" sub
git commit -q -m sub
"#
            ),
        );

        let work = format!("{dir}/work");
        assert_eq!(git::operation_markers(&work).unwrap(), Vec::<String>::new());
        // The marker lives in the linked worktree's admin dir; Git resolves it.
        let linked = format!("{dir}/work/linked");
        assert_eq!(git::operation_markers(&linked).unwrap(), vec!["MERGE_HEAD".to_string()]);

        let superproject = format!("{dir}/super");
        assert_eq!(git::populated_submodules(&superproject).unwrap(), 1);
        wsl::must(&["git", "-C", &superproject, "submodule", "deinit", "-f", "-q", "sub"]);
        assert_eq!(git::populated_submodules(&superproject).unwrap(), 0);
    }
}
