//! Overview controller: workspace rows, refresh generations, stale
//! snapshots, filtering/sorting/selection, and the auto-refresh clock seam.
//! GPUI-free so the refresh lifecycle is testable with controlled completion
//! order.
//!
//! Identity rules: a row id is the canonical worktree path. A workspace load
//! bumps the generation; a query ticket carries the generation plus a
//! per-row serial, and results that do not match both are dropped. Sorting
//! and filtering never touch row identity, and action targets are captured
//! from ids (never row positions).
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::discovery::DiscoveredRepo;
use crate::git::CollectedStatus;
use crate::model::{Filter, Flag, RowState};

/// Conservative auto-refresh interval (CONCEPT: bounded auto-refresh).
pub const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(15);

/// Which rows an operation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetScope {
    Selected,
    /// The entire workspace, including rows hidden by filter/search.
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Branch,
    Sync,
}

impl SortKey {
    pub const ALL: [SortKey; 3] = [SortKey::Name, SortKey::Branch, SortKey::Sync];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Branch => "Branch",
            SortKey::Sync => "Sync",
        }
    }
}

/// One repository row. `snapshot` is the last successful collection; a failed
/// refresh keeps it and marks the row stale instead of replacing it.
#[derive(Debug, Clone)]
pub struct OverviewRow {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Shared-storage identity for conflict serialization.
    pub common_dir: PathBuf,
    pub snapshot: Option<CollectedStatus>,
    pub state: RowState,
    /// When the last successful collection finished (shown to the user).
    pub last_collected: Option<Instant>,
    /// Last successful app-initiated fetch; never implies knowledge of
    /// external fetches.
    pub last_fetched: Option<Instant>,
    serial: u64,
    /// Bumped whenever a query result is applied (success or failure); render
    /// caches key on this, not on the query serial.
    revision: u64,
    /// True for a row registered without discovery (an explicitly opened
    /// repository): [`Overview::load_workspace`] keeps it across a rescan.
    known: bool,
}

impl OverviewRow {
    pub fn flags(&self) -> Vec<Flag> {
        match &self.snapshot {
            Some(collected) => collected.snapshot.flags(collected.upstream_gone()),
            None => Vec::new(),
        }
    }

    /// Serial of the newest status query begun for this row. A query whose
    /// ticket carries a larger serial started after this moment.
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// Registered by an explicit open or drop and not (yet) discovered.
    pub fn is_known(&self) -> bool {
        self.known
    }

    pub fn has_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    /// `⇡a ⇣b ⇕a/b` label from the last snapshot.
    pub fn sync_label(&self) -> String {
        let (ahead, behind) = self
            .snapshot
            .as_ref()
            .map(|c| (c.snapshot.ahead, c.snapshot.behind))
            .unwrap_or((0, 0));
        match (ahead, behind) {
            (0, 0) => "–".to_string(),
            (a, 0) => format!("⇡{a}"),
            (0, b) => format!("⇣{b}"),
            (a, b) => format!("⇕{a}/{b}"),
        }
    }

    pub fn branch_label(&self) -> String {
        match &self.snapshot {
            Some(collected) => {
                if collected.snapshot.unborn {
                    format!("{} (unborn)", collected.snapshot.branch.as_deref().unwrap_or("—"))
                } else if collected.snapshot.detached {
                    "detached".to_string()
                } else {
                    collected.snapshot.branch.clone().unwrap_or_else(|| "—".to_string())
                }
            }
            None => "—".to_string(),
        }
    }

    pub fn matches_filter(&self, filter: Filter) -> bool {
        match filter {
            Filter::All => true,
            Filter::Changed => self.flags().iter().any(|flag| {
                !matches!(
                    flag,
                    Flag::Clean | Flag::Ahead | Flag::Behind | Flag::Diverged | Flag::UpstreamMissing
                )
            }),
            Filter::Behind => self
                .snapshot
                .as_ref()
                .is_some_and(|c| c.snapshot.behind > 0),
            Filter::Conflicts => self.flags().contains(&Flag::Conflicted),
            Filter::Errors => matches!(self.state, RowState::Stale { .. }),
        }
    }

    pub fn matches_search(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        needle.is_empty()
            || self.name.to_lowercase().contains(&needle)
            || self.path.to_string_lossy().to_lowercase().contains(&needle)
            || self.branch_label().to_lowercase().contains(&needle)
    }

    /// Whether this row is eligible for a pull right now.
    pub fn pull_decision(&self) -> crate::status::PullDecision {
        match &self.snapshot {
            Some(collected) => crate::status::pull_decision(
                &collected.snapshot,
                collected.upstream_gone(),
                &[],
                0,
            ),
            None => crate::status::PullDecision::Skip("no status collected".to_string()),
        }
    }
}

/// Identifies one in-flight query. Results are applied only when both the
/// workspace generation and the row serial still match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    pub generation: u64,
    pub serial: u64,
}

#[derive(Debug)]
pub struct Overview {
    generation: u64,
    rows: Vec<OverviewRow>,
    selected: HashSet<String>,
    filter: Filter,
    search: String,
    sort: SortKey,
    pub refresh_loop: RefreshLoop,
}

impl Default for Overview {
    fn default() -> Self {
        Self::new()
    }
}

impl Overview {
    pub fn new() -> Self {
        Self {
            generation: 0,
            rows: Vec::new(),
            selected: HashSet::new(),
            filter: Filter::All,
            search: String::new(),
            sort: SortKey::Name,
            refresh_loop: RefreshLoop::default(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn rows(&self) -> &[OverviewRow] {
        &self.rows
    }

    pub fn row(&self, id: &str) -> Option<&OverviewRow> {
        self.rows.iter().find(|row| row.id == id)
    }

    /// Monotonic revision of the row's applied content (snapshot/error); it
    /// advances only when that content actually changes, so identical polls
    /// never invalidate render caches.
    pub fn row_revision(&self, id: &str) -> u64 {
        self.row(id).map(|row| row.revision).unwrap_or(0)
    }

    /// Replace the workspace with a discovered set, bumping the generation so
    /// every in-flight result is invalidated. Snapshots, timestamps, and
    /// selection survive for rows whose identity is unchanged. Rows that were
    /// registered without discovery (`upsert_known`: an explicitly opened
    /// repository outside the roots) survive the rescan too — dropping them
    /// blanked the counts and diff of the open tab.
    pub fn load_workspace(&mut self, discovered: &[DiscoveredRepo]) -> u64 {
        self.generation += 1;
        let mut previous: HashMap<String, OverviewRow> = self
            .rows
            .drain(..)
            .map(|row| (row.id.clone(), row))
            .collect();
        let mut rows: Vec<OverviewRow> = discovered
            .iter()
            .map(|repo| {
                let id = repo.identity.worktree.to_string_lossy().into_owned();
                match previous.remove(&id) {
                    Some(mut row) => {
                        // Discovery owns the row from now on: a later rescan
                        // that no longer finds it drops it like any other.
                        row.known = false;
                        row.name = repo.name.clone();
                        row.path = repo.identity.worktree.clone();
                        row.common_dir = repo.identity.common_dir.clone();
                        row
                    }
                    None => OverviewRow {
                        id,
                        name: repo.name.clone(),
                        path: repo.identity.worktree.clone(),
                        common_dir: repo.identity.common_dir.clone(),
                        snapshot: None,
                        state: RowState::Ready,
                        last_collected: None,
                        last_fetched: None,
                        serial: 0,
                        revision: 0,
                        known: false,
                    },
                }
            })
            .collect();
        for (id, row) in previous {
            if row.known && !rows.iter().any(|existing| existing.id == id) {
                rows.push(row);
            }
        }
        self.rows = rows;
        let ids: HashSet<&str> = self.rows.iter().map(|row| row.id.as_str()).collect();
        self.selected.retain(|id| ids.contains(id.as_str()));
        self.generation
    }

    /// Register one row known without discovery (a restored/open tab), so
    /// its status can be collected before the workspace scan finishes.
    /// An existing row is left untouched; a new
    /// row starts unknown (Loading) — never clean. Returns whether a row was
    /// inserted.
    /// Remove a row that was registered without discovery (its tab closed),
    /// so a closed repository outside the roots stops being listed and
    /// polled. Discovered rows are left alone. Returns whether one was removed.
    pub fn forget_known(&mut self, id: &str) -> bool {
        let before = self.rows.len();
        self.rows.retain(|row| !(row.known && row.id == id));
        self.selected.retain(|selected| selected != id);
        self.rows.len() != before
    }

    pub fn upsert_known(
        &mut self,
        id: &str,
        name: &str,
        path: &std::path::Path,
        common_dir: &std::path::Path,
    ) -> bool {
        if self.rows.iter().any(|row| row.id == id) {
            return false;
        }
        self.rows.push(OverviewRow {
            id: id.to_string(),
            name: name.to_string(),
            path: path.to_path_buf(),
            common_dir: common_dir.to_path_buf(),
            snapshot: None,
            state: RowState::Loading,
            last_collected: None,
            last_fetched: None,
            serial: 0,
            revision: 0,
            known: true,
        });
        true
    }

    /// Start a query for one row: bumps its serial and marks it loading while
    /// keeping the previous snapshot.
    pub fn begin_query(&mut self, id: &str) -> Option<Ticket> {
        let row = self.rows.iter_mut().find(|row| row.id == id)?;
        row.serial += 1;
        row.state = RowState::Loading;
        Some(Ticket { generation: self.generation, serial: row.serial })
    }

    /// Start a background re-query. A row that already shows a snapshot keeps
    /// its state (no "collecting…" flicker); a row with no data yet shows
    /// Loading. The serial bump still drops older in-flight results.
    pub fn begin_refresh(&mut self, id: &str) -> Option<Ticket> {
        let row = self.rows.iter_mut().find(|row| row.id == id)?;
        row.serial += 1;
        if row.snapshot.is_none() {
            row.state = RowState::Loading;
        }
        Some(Ticket { generation: self.generation, serial: row.serial })
    }

    /// Apply a finished query. Returns false (and changes nothing) when the
    /// ticket is stale, when the workspace changed meanwhile, or when a newer
    /// query for the same row is already in flight or finished.
    ///
    /// `revision` advances only when the snapshot or error state actually
    /// changes; an identical poll updates the collection time but leaves the
    /// render caches (Local Changes rows, branch lists) untouched.
    pub fn apply_result(
        &mut self,
        ticket: Ticket,
        id: &str,
        result: Result<CollectedStatus, String>,
        now: Instant,
    ) -> bool {
        if ticket.generation != self.generation {
            return false;
        }
        let Some(row) = self.rows.iter_mut().find(|row| row.id == id) else {
            return false;
        };
        if ticket.serial != row.serial {
            return false;
        }
        match result {
            Ok(collected) => {
                if row.snapshot.as_ref() != Some(&collected) {
                    row.revision = row.revision.wrapping_add(1);
                }
                row.snapshot = Some(collected);
                row.state = RowState::Ready;
                row.last_collected = Some(now);
            }
            Err(error) => {
                // Keep the previous snapshot and mark it stale; never fabricate
                // a clean state and never advance "last collected".
                let changed = !matches!(
                    &row.state,
                    RowState::Stale { error: previous } if previous == &error
                );
                if changed {
                    row.revision = row.revision.wrapping_add(1);
                }
                row.state = RowState::Stale { error };
            }
        }
        true
    }

    /// Record a successful app-initiated fetch (never an external one).
    pub fn note_fetch(&mut self, id: &str, now: Instant) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.id == id) {
            row.last_fetched = Some(now);
        }
    }

    // ---- filter / search / sort / selection ----

    pub fn filter(&self) -> Filter {
        self.filter
    }

    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
    }

    pub fn search(&self) -> &str {
        &self.search
    }

    pub fn set_search(&mut self, search: impl Into<String>) {
        self.search = search.into();
    }

    pub fn sort(&self) -> SortKey {
        self.sort
    }

    pub fn set_sort(&mut self, sort: SortKey) {
        self.sort = sort;
    }

    /// Rows after filtering, searching, and sorting. Selection is identity
    /// based, so reordering here never moves it.
    pub fn visible_rows(&self) -> Vec<&OverviewRow> {
        let mut rows: Vec<&OverviewRow> = self
            .rows
            .iter()
            .filter(|row| row.matches_filter(self.filter) && row.matches_search(&self.search))
            .collect();
        rows.sort_by(|a, b| {
            let key = match self.sort {
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::Branch => a.branch_label().cmp(&b.branch_label()),
                SortKey::Sync => {
                    let weight = |row: &OverviewRow| {
                        row.snapshot.as_ref().map(|c| (c.snapshot.behind, c.snapshot.ahead)).unwrap_or((0, 0))
                    };
                    weight(b).cmp(&weight(a))
                }
            };
            key.then_with(|| a.id.cmp(&b.id))
        });
        rows
    }

    pub fn visible_ids(&self) -> Vec<String> {
        self.visible_rows()
            .into_iter()
            .map(|row| row.id.clone())
            .collect()
    }

    pub fn select(&mut self, id: &str, additive: bool) {
        if !additive {
            self.selected.clear();
        }
        if self.rows.iter().any(|row| row.id == id) {
            self.selected.insert(id.to_string());
        }
    }

    pub fn deselect(&mut self, id: &str) {
        self.selected.remove(id);
    }

    pub fn is_selected(&self, id: &str) -> bool {
        self.selected.contains(id)
    }

    pub fn selected_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.rows.iter().filter(|row| self.selected.contains(&row.id)).map(|row| row.id.clone()).collect();
        ids.sort();
        ids
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }

    /// Capture the target set for an action. `Selected` never expands to
    /// visible rows; `All` includes rows hidden by filter or search.
    pub fn action_targets(&self, scope: TargetScope) -> Vec<String> {
        match scope {
            TargetScope::Selected => self.selected_ids(),
            TargetScope::All => {
                let mut ids: Vec<String> = self.rows.iter().map(|row| row.id.clone()).collect();
                ids.sort();
                ids
            }
        }
    }
}

/// Auto-refresh clock seam: the UI calls [`should_start`](Self::should_start)
/// on a timer tick; tests can advance synthetic instants. `in_flight` is what
/// prevents overlapping rounds.
#[derive(Debug)]
pub struct RefreshLoop {
    enabled: bool,
    interval: Duration,
    last_round: Option<Instant>,
    in_flight: bool,
}

impl Default for RefreshLoop {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: AUTO_REFRESH_INTERVAL,
            last_round: None,
            in_flight: false,
        }
    }
}

impl RefreshLoop {
    pub fn with_interval(interval: Duration) -> Self {
        Self { interval, ..Self::default() }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    pub fn should_start(&self, now: Instant) -> bool {
        self.enabled
            && !self.in_flight
            && self
                .last_round
                .is_none_or(|last| now.saturating_duration_since(last) >= self.interval)
    }

    pub fn round_started(&mut self, now: Instant) {
        self.in_flight = true;
        self.last_round = Some(now);
    }

    pub fn round_finished(&mut self) {
        self.in_flight = false;
    }
}

/// Run `git::collect_status` for every target with a bounded worker pool,
/// streaming `(id, result)` in completion order. The production refresh path.
///
/// The queue is FIFO: the caller's order (active tab, then open tabs, then
/// the rest — see `SpurShell::start_refresh_round`) is the dispatch order.
pub fn spawn_status_round(
    targets: Vec<(String, String)>,
) -> std::sync::mpsc::Receiver<(String, Result<CollectedStatus, String>)> {
    let workers = crate::process::jobs_from_env().unwrap_or(crate::process::DEFAULT_JOBS);
    spawn_status_round_with(targets, workers, crate::git::collect_status)
}

/// [`spawn_status_round`] with an injected collector and worker count, so the
/// dispatch order is deterministically testable.
pub(crate) fn spawn_status_round_with<F>(
    targets: Vec<(String, String)>,
    workers: usize,
    collect: F,
) -> std::sync::mpsc::Receiver<(String, Result<CollectedStatus, String>)>
where
    F: Fn(&str) -> Result<CollectedStatus, String> + Send + Sync + 'static,
{
    let (sender, receiver) = std::sync::mpsc::channel();
    let queue = std::sync::Arc::new(std::sync::Mutex::new(
        std::collections::VecDeque::from(targets),
    ));
    let collect = std::sync::Arc::new(collect);
    for _ in 0..workers.max(1) {
        let queue = queue.clone();
        let sender = sender.clone();
        let collect = collect.clone();
        std::thread::spawn(move || loop {
            let next = queue.lock().unwrap().pop_front();
            let Some((id, path)) = next else {
                return;
            };
            let result = collect(&path);
            if sender.send((id, result)).is_err() {
                return;
            }
        });
    }
    drop(sender);
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::parse_porcelain_v2;

    fn now() -> Instant {
        Instant::now()
    }

    fn discovered(name: &str, path: &str) -> DiscoveredRepo {
        DiscoveredRepo {
            identity: crate::git::RepoIdentity {
                worktree: PathBuf::from(path),
                git_dir: PathBuf::from(format!("{path}/.git")),
                common_dir: PathBuf::from(format!("{path}/.git")),
            },
            name: name.to_string(),
        }
    }

    fn collected(records: &[&str]) -> CollectedStatus {
        let mut bytes = Vec::new();
        for record in records {
            bytes.extend_from_slice(record.as_bytes());
            bytes.push(0);
        }
        CollectedStatus {
            snapshot: parse_porcelain_v2(&bytes).expect("valid fixture records"),
            refs: crate::status::RefInfo::default(),
        }
    }

    fn dirty() -> CollectedStatus {
        collected(&[
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +0 -2",
            "1 .M N... 100644 100644 100644 aaaa bbbb f.txt",
        ])
    }

    fn clean() -> CollectedStatus {
        collected(&[
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +0 -2",
        ])
    }

    #[test]
    fn late_results_cannot_overwrite_a_newer_view() {
        let mut overview = Overview::new();
        // Workspace A has duplicate basenames in different directories.
        overview.load_workspace(
            &[
                discovered("app", "/w/a/app"),
                discovered("app", "/w/a/tools/app"),
            ],
        );
        let ticket_a = overview.begin_query("/w/a/app").unwrap();

        // Switch to workspace B before A answers.
        overview.load_workspace(&[discovered("app", "/w/b/app")]);
        let ticket_b = overview.begin_query("/w/b/app").unwrap();
        assert!(overview.apply_result(ticket_b, "/w/b/app", Ok(clean()), now()));
        // A's reply must be dropped even though the path/name look similar.
        assert!(!overview.apply_result(ticket_a, "/w/a/app", Ok(dirty()), now()));
        assert_eq!(overview.rows().len(), 1);
        assert!(overview.row("/w/b/app").unwrap().has_snapshot());

        // A -> B -> A: a ticket from the first A must not apply to the new A.
        overview.load_workspace(&[
            discovered("app", "/w/a/app"),
            discovered("app", "/w/a/tools/app"),
        ]);
        assert!(!overview.apply_result(ticket_a, "/w/a/app", Ok(dirty()), now()));
        assert!(!overview.row("/w/a/app").unwrap().has_snapshot());

        // The generation guard is decisive for a row that survives a workspace
        // change with the same id and serial (overlapping roots).
        overview.load_workspace(&[discovered("app", "/w/shared")]);
        let shared_ticket = overview.begin_query("/w/shared").unwrap();
        overview.load_workspace(&[discovered("app", "/w/shared")]);
        assert!(!overview.apply_result(shared_ticket, "/w/shared", Ok(dirty()), now()));
        assert!(!overview.row("/w/shared").unwrap().has_snapshot());

        // An older failed reply arriving after a newer success is dropped.
        let ticket_old = overview.begin_query("/w/shared").unwrap();
        let ticket_new = overview.begin_query("/w/shared").unwrap();
        assert!(overview.apply_result(ticket_new, "/w/shared", Ok(clean()), now()));
        assert!(!overview.apply_result(ticket_old, "/w/shared", Err("old failure".into()), now()));
        let row = overview.row("/w/shared").unwrap();
        assert!(matches!(row.state, RowState::Ready), "{:?}", row.state);
        assert!(row.has_snapshot());
    }

    #[test]
    fn a_known_open_path_is_queryable_before_discovery_and_merges_cleanly() {
        let mut overview = Overview::new();
        // A restored tab is registered before any scan.
        assert!(overview.upsert_known(
            "/w/restored",
            "restored",
            PathBuf::from("/w/restored").as_path(),
            PathBuf::from("/w/restored").as_path(),
        ));
        assert!(!overview.upsert_known(
            "/w/restored",
            "restored",
            PathBuf::from("/w/restored").as_path(),
            PathBuf::from("/w/restored").as_path(),
        ));
        let row = overview.row("/w/restored").unwrap();
        assert!(matches!(row.state, RowState::Loading));
        assert!(!row.has_snapshot(), "an unknown path must not look clean");

        let ticket = overview.begin_refresh("/w/restored").unwrap();
        assert!(overview.apply_result(ticket, "/w/restored", Ok(dirty()), now()));
        assert!(overview.row("/w/restored").unwrap().has_snapshot());

        // Discovery merges the row by identity and keeps the fresh snapshot
        // and the provisional row's status.
        overview.load_workspace(&[discovered("restored", "/w/restored"), discovered("b", "/w/b")]);
        assert_eq!(overview.rows().len(), 2);
        assert!(overview.row("/w/restored").unwrap().has_snapshot());
        assert!(!overview.row("/w/b").unwrap().has_snapshot());
    }

    #[test]
    fn the_status_round_dispatches_in_the_given_priority_order() {
        // Active-first sorting in `start_refresh_round` only works when the
        // worker pool consumes the list front-first.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_worker = seen.clone();
        let receiver = spawn_status_round_with(
            vec![
                ("active".to_string(), "/w/active".to_string()),
                ("open".to_string(), "/w/open".to_string()),
                ("other".to_string(), "/w/other".to_string()),
            ],
            1,
            move |path| {
                seen_worker.lock().unwrap().push(path.to_string());
                Err("recorded".to_string())
            },
        );
        while receiver.recv().is_ok() {}
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["/w/active", "/w/open", "/w/other"],
            "the first target must be dispatched first"
        );
    }

    #[test]
    fn unchanged_polls_do_not_advance_the_content_revision() {
        let mut overview = Overview::new();
        overview.load_workspace(&[discovered("a", "/w/a")]);
        let ticket = overview.begin_query("/w/a").unwrap();
        assert!(overview.apply_result(ticket, "/w/a", Ok(clean()), now()));
        let revision = overview.row_revision("/w/a");

        // Identical result: collection time moves, the revision does not.
        let ticket = overview.begin_refresh("/w/a").unwrap();
        assert!(overview.apply_result(ticket, "/w/a", Ok(clean()), now()));
        assert_eq!(overview.row_revision("/w/a"), revision);

        // A changed snapshot bumps it; a repeated failure string does not.
        let ticket = overview.begin_refresh("/w/a").unwrap();
        assert!(overview.apply_result(ticket, "/w/a", Ok(dirty()), now()));
        assert_ne!(overview.row_revision("/w/a"), revision);
        let ticket = overview.begin_refresh("/w/a").unwrap();
        assert!(overview.apply_result(ticket, "/w/a", Err("boom".into()), now()));
        let failed = overview.row_revision("/w/a");
        let ticket = overview.begin_refresh("/w/a").unwrap();
        assert!(overview.apply_result(ticket, "/w/a", Err("boom".into()), now()));
        assert_eq!(overview.row_revision("/w/a"), failed);
    }

    #[test]
    fn failures_keep_the_previous_snapshot_and_mark_it_stale() {
        let mut overview = Overview::new();
        overview.load_workspace(&[discovered("a", "/w/a"), discovered("b", "/w/b")]);
        let a = overview.begin_query("/w/a").unwrap();
        assert!(overview.apply_result(a, "/w/a", Ok(dirty()), now()));
        let b = overview.begin_query("/w/b").unwrap();
        assert!(overview.apply_result(b, "/w/b", Ok(clean()), now()));
        let before = overview.row("/w/a").unwrap().last_collected;
        let flags_before = overview.row("/w/a").unwrap().flags();

        // A real query failure on A keeps its snapshot visible and stale.
        let a = overview.begin_query("/w/a").unwrap();
        assert!(overview.apply_result(a, "/w/a", Err("status failed".into()), now()));
        let row_a = overview.row("/w/a").unwrap();
        assert!(matches!(row_a.state, RowState::Stale { ref error } if error.contains("status failed")));
        assert_eq!(row_a.last_collected, before, "stale data must not look fresh");
        assert_eq!(row_a.flags(), flags_before);
        assert!(row_a.has_snapshot());

        // A initial failure has no fabricated clean snapshot.
        overview.load_workspace(&[
            discovered("a", "/w/a"),
            discovered("b", "/w/b"),
            discovered("c", "/w/c"),
        ]);
        let c = overview.begin_query("/w/c").unwrap();
        assert!(overview.apply_result(c, "/w/c", Err("never succeeded".into()), now()));
        let row_c = overview.row("/w/c").unwrap();
        assert!(!row_c.has_snapshot());
        assert!(matches!(row_c.state, RowState::Stale { .. }));
        assert!(!row_c.flags().contains(&Flag::Clean));
        // Errors filter still shows it (and the still-stale A from earlier).
        overview.set_filter(Filter::Errors);
        assert_eq!(
            overview.visible_rows().iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "c"]
        );

        // Recovery clears the error only after a successful fresh query.
        let c = overview.begin_query("/w/c").unwrap();
        assert!(overview.apply_result(c, "/w/c", Ok(clean()), now()));
        let row_c = overview.row("/w/c").unwrap();
        assert!(matches!(row_c.state, RowState::Ready));
        assert!(row_c.has_snapshot());
    }

    #[test]
    fn targets_follow_identity_not_row_positions() {
        let mut overview = Overview::new();
        overview.load_workspace(&[
            discovered("app", "/w/one/app"),
            discovered("app", "/w/two/app"),
            discovered("zebra", "/w/zebra"),
        ]);
        overview.select("/w/two/app", false);
        assert_eq!(overview.action_targets(TargetScope::Selected), vec!["/w/two/app"]);

        // Sorting, filtering, and searching change the visible order/set but
        // never the captured target.
        overview.set_sort(SortKey::Sync);
        overview.set_search("app");
        overview.set_filter(Filter::Behind);
        assert!(overview.visible_rows().is_empty(), "filter hides rows");
        assert_eq!(overview.action_targets(TargetScope::Selected), vec!["/w/two/app"]);

        // Fetch all means the whole workspace, including hidden rows.
        assert_eq!(
            overview.action_targets(TargetScope::All),
            vec!["/w/one/app", "/w/two/app", "/w/zebra"]
        );
    }

    #[test]
    fn known_rows_survive_a_rescan() {
        let mut overview = Overview::new();
        // An explicitly opened repository outside the roots, with data.
        assert!(overview.upsert_known(
            "/w/dropped",
            "dropped",
            std::path::Path::new("/w/dropped"),
            std::path::Path::new("/w/dropped"),
        ));
        let ticket = overview.begin_query("/w/dropped").unwrap();
        assert!(overview.apply_result(ticket, "/w/dropped", Ok(clean()), now()));

        // A rescan that discovers other repositories keeps it and its data;
        // dropping the row blanked the open tab's counts.
        overview.load_workspace(&[discovered("a", "/w/a")]);
        let row = overview.row("/w/dropped").expect("known row survives a rescan");
        assert!(row.has_snapshot(), "the snapshot must survive too");
        assert!(overview.row("/w/a").is_some());

        // Discovered rows still follow discovery; only the manually
        // registered one is kept.
        overview.load_workspace(&[]);
        assert!(overview.row("/w/a").is_none());
        assert!(overview.row("/w/dropped").is_some());

        // Closing its tab forgets it, so it is neither listed nor polled.
        assert!(overview.forget_known("/w/dropped"));
        assert!(overview.row("/w/dropped").is_none());

        // Once discovery finds a known row it is discovery-owned: forgetting
        // leaves it, and a later scan without it drops it.
        assert!(overview.upsert_known(
            "/w/b",
            "b",
            std::path::Path::new("/w/b"),
            std::path::Path::new("/w/b"),
        ));
        overview.load_workspace(&[discovered("b", "/w/b")]);
        assert!(!overview.row("/w/b").unwrap().is_known());
        assert!(!overview.forget_known("/w/b"), "a discovered row is not forgotten");
        overview.load_workspace(&[]);
        assert!(overview.row("/w/b").is_none());
    }

    #[test]
    fn background_refresh_keeps_the_visible_state() {
        let mut overview = Overview::new();
        overview.load_workspace(&[discovered("a", "/w/a"), discovered("b", "/w/b")]);

        // A row with data keeps its Ready state while refreshing: background
        // polls must not flicker the "collecting" chip.
        let first = overview.begin_query("/w/a").unwrap();
        assert!(overview.apply_result(first, "/w/a", Ok(clean()), now()));
        let ticket = overview.begin_refresh("/w/a").unwrap();
        assert!(
            matches!(overview.row("/w/a").unwrap().state, RowState::Ready),
            "background refresh must not flip the row to Loading"
        );
        assert!(overview.row("/w/a").unwrap().has_snapshot());
        // The new ticket still wins.
        assert!(overview.apply_result(ticket, "/w/a", Ok(dirty()), now()));

        // A row with no data yet shows Loading on its first refresh.
        let empty = overview.begin_refresh("/w/b").unwrap();
        assert!(matches!(overview.row("/w/b").unwrap().state, RowState::Loading));
        assert!(overview.apply_result(empty, "/w/b", Ok(clean()), now()));
    }

    #[test]
    fn refresh_loop_never_overlaps_and_respects_the_interval() {        let mut clock = Instant::now();
        let mut refresh = RefreshLoop::with_interval(Duration::from_secs(15));
        assert!(refresh.should_start(clock));
        refresh.round_started(clock);
        assert!(refresh.in_flight());
        assert!(!refresh.should_start(clock + Duration::from_secs(60)));

        refresh.round_finished();
        clock += Duration::from_secs(14);
        assert!(!refresh.should_start(clock));
        clock += Duration::from_secs(1);
        assert!(refresh.should_start(clock));

        refresh.set_enabled(false);
        assert!(!refresh.should_start(clock + Duration::from_secs(60)));
        refresh.set_enabled(true);
        assert!(refresh.should_start(clock + Duration::from_secs(60)));
    }

    #[test]
    fn visible_rows_sort_search_and_filter_without_losing_identity() {
        let mut overview = Overview::new();
        overview.load_workspace(&[
            discovered("beta", "/w/beta"),
            discovered("alpha", "/w/alpha"),
        ]);
        let a = overview.begin_query("/w/alpha").unwrap();
        overview.apply_result(a, "/w/alpha", Ok(dirty()), now());
        let b = overview.begin_query("/w/beta").unwrap();
        overview.apply_result(b, "/w/beta", Ok(clean()), now());

        let names: Vec<&str> = overview.visible_rows().iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);

        overview.set_sort(SortKey::Sync);
        let names: Vec<&str> = overview.visible_rows().iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"], "dirty row sorts first by behind");

        overview.set_filter(Filter::Errors);
        assert!(overview.visible_rows().is_empty());
        overview.set_search("bet");
        overview.set_filter(Filter::All);
        let names: Vec<&str> = overview.visible_rows().iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, vec!["beta"]);

        // Selection survives a rescan of the same workspace.
        overview.set_search("");
        overview.select("/w/beta", false);
        overview.load_workspace(&[
            discovered("beta", "/w/beta"),
            discovered("alpha", "/w/alpha"),
        ]);
        assert!(overview.is_selected("/w/beta"));
        assert!(!overview.is_selected("/w/alpha"));
    }
}

/// Real-repository refresh tests. They need the WSL distro, so
/// they are ignored by default: run them with `cargo test -- --ignored`.
#[cfg(test)]
mod wsl_tests {
    use super::*;
    use crate::process::wsl_support as wsl;

    fn now() -> Instant {
        Instant::now()
    }

    struct RestorePermissions(String);

    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = wsl::wsl(&["chmod", "755", &self.0]);
        }
    }

    fn run_fixture(dir: &str, script: &str) {
        let path = format!("{dir}/build.sh");
        wsl::write_script(&path, script);
        wsl::must(&[path.as_str()]);
    }

    fn repo_row(path: &str) -> DiscoveredRepo {
        DiscoveredRepo {
            identity: crate::git::identity(path).expect("identity"),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
        }
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn a_real_failed_query_keeps_the_row_and_recovers() {
        wsl::watchdog(180);
        let dir = wsl::temp_dir("r04");
        run_fixture(
            &dir,
            &format!(
                r#"#!/bin/bash
set -e
R="{dir}/repo"
mkdir -p "$R"; cd "$R"
git init -q -b main
git config user.name t
git config user.email t@t
printf 'one\n' > tracked.txt
git add .; git commit -q -m base
printf 'dirty\n' > tracked.txt
"#
            ),
        );
        let repo = format!("{dir}/repo");
        let _restore = RestorePermissions(repo.clone());

        let mut overview = Overview::new();
        overview.load_workspace(&[repo_row(&repo)]);
        let id = repo.clone();

        let ticket = overview.begin_query(&id).unwrap();
        let first = crate::git::collect_status(&repo).expect("initial status");
        overview.apply_result(ticket, &id, Ok(first), now());
        let flags_before = overview.row(&id).unwrap().flags();
        let collected_before = overview.row(&id).unwrap().last_collected;

        // A real query failure: the worktree becomes unreadable. The row stays
        // visible with its previous snapshot explicitly stale.
        wsl::must(&["chmod", "000", &repo]);
        let ticket = overview.begin_query(&id).unwrap();
        let failed = crate::git::collect_status(&repo);
        assert!(failed.is_err(), "chmod 000 must make the query fail");
        assert!(overview.apply_result(ticket, &id, failed, now()));
        let row = overview.row(&id).unwrap();
        assert!(matches!(row.state, RowState::Stale { .. }), "{:?}", row.state);
        assert_eq!(row.flags(), flags_before);
        assert_eq!(row.last_collected, collected_before);
        overview.set_filter(Filter::Errors);
        assert_eq!(overview.visible_rows().len(), 1);

        // Recovery clears the error only after a fresh success.
        wsl::must(&["chmod", "755", &repo]);
        let ticket = overview.begin_query(&id).unwrap();
        let recovered = crate::git::collect_status(&repo).expect("recovered status");
        assert!(overview.apply_result(ticket, &id, Ok(recovered), now()));
        let row = overview.row(&id).unwrap();
        assert!(matches!(row.state, RowState::Ready));
        assert_eq!(row.flags(), flags_before);
        assert!(row.last_collected.is_some_and(|t| Some(t) != collected_before));
    }

    #[test]
    #[ignore = "requires a WSL distro"]
    fn refresh_is_local_and_non_mutating_with_a_scheduler_smoke() {
        wsl::watchdog(240);
        let dir = wsl::temp_dir("r05");
        let sentinel = format!("{dir}/transport-sentinel");
        run_fixture(
            &dir,
            &format!(
                r#"#!/bin/bash
set -e
R="{dir}"
git init -q --bare -b main "$R/remote.git"
git clone -q "$R/remote.git" "$R/pub"
git -C "$R/pub" config user.name t
git -C "$R/pub" config user.email t@t
printf 'one\n' > "$R/pub/tracked.txt"
git -C "$R/pub" add .
git -C "$R/pub" commit -q -m base
git -C "$R/pub" push -q origin main
git clone -q "$R/remote.git" "$R/repo"
git -C "$R/repo" config user.name t
git -C "$R/repo" config user.email t@t
# the subject is behind one commit
printf 'two\n' > "$R/pub/tracked.txt"
git -C "$R/pub" commit -qam two
git -C "$R/pub" push -q origin main
git -C "$R/repo" fetch -q origin
# stash + untracked content
printf 'stashed\n' >> "$R/repo/tracked.txt"
git -C "$R/repo" stash push -q -m one
printf 'loose\n' > "$R/repo/untracked.txt"
# a transport trap: any fetch would run the helper and create the sentinel
git -C "$R/repo" config protocol.ext.allow always
git -C "$R/repo" remote set-url origin "ext::sh -c 'touch {sentinel}'"
"#
            ),
        );
        let repo = format!("{dir}/repo");
        let index = format!("{repo}/.git/index");
        let lock = format!("{repo}/.git/index.lock");
        let index_before = wsl::must(&["cat", &index]);

        // Case 1: stat-dirty tracked file, no lock. A refresh must not write
        // the index (that is what --no-optional-locks is for).
        wsl::must(&["touch", &format!("{repo}/tracked.txt")]);
        let collected = crate::git::collect_status(&repo).expect("refresh");
        assert_eq!(collected.snapshot.behind, 1);
        assert_eq!(collected.snapshot.stash, 1);
        assert!(
            collected
                .snapshot
                .untracked
                .iter()
                .any(|path| path == b"untracked.txt")
        );
        assert_eq!(wsl::must(&["cat", &index]), index_before, "refresh wrote the index");

        // Case 2: a pre-existing foreign lock stays exactly as it is.
        wsl::write_file(&lock, b"foreign lock marker");
        wsl::must(&["touch", &format!("{repo}/tracked.txt")]);
        let collected = crate::git::collect_status(&repo).expect("refresh with lock");
        assert_eq!(collected.snapshot.behind, 1);
        assert_eq!(
            wsl::must(&["cat", &lock]),
            b"foreign lock marker",
            "refresh changed or removed a foreign lock"
        );
        assert_eq!(wsl::must(&["cat", &index]), index_before, "index changed with a lock present");

        // No transport was attempted (the ext helper would create the file).
        assert!(!wsl::exists(&sentinel), "refresh performed a network/transport operation");

        // Scheduler seam: no overlapping rounds, interval respected, and one
        // real-time smoke that the tick drives the production refresh entry.
        let mut refresh = RefreshLoop::with_interval(Duration::from_millis(50));
        assert!(refresh.should_start(Instant::now()));
        refresh.round_started(Instant::now());
        assert!(!refresh.should_start(Instant::now()), "overlapping round allowed");
        refresh.round_finished();
        let mut rounds = 0;
        let smoke = wsl::poll_until(Duration::from_secs(5), || {
            if refresh.should_start(Instant::now()) {
                refresh.round_started(Instant::now());
                let ok = crate::git::collect_status(&repo).is_ok();
                refresh.round_finished();
                rounds += 1;
                ok
            } else {
                false
            }
        });
        assert!(smoke && rounds == 1, "the scheduled tick did not reach the refresh entry");
        assert!(!wsl::exists(&sentinel));
    }
}
