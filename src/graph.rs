//! Commit-graph lane layout (GPUI-free).
//!
//! Commits arrive in date order (`git log --date-order`: children before
//! parents, otherwise newest first). `layout` assigns each commit a lane and
//! records, per row, which lanes carry lines through it and how each parent is
//! connected. The UI paints one canvas per row from that description; nothing
//! here knows about rendering.
//!
//! Lane rule (SourceGit): the first parent always keeps the node's lane.
//! Several lanes may wait for the same hash; when the commit arrives, the
//! leftmost waiting lane hosts the node and every other waiting lane ends in
//! this row, converging into the node. Extra parents join a lane already
//! waiting for them (leftmost) or open the leftmost free lane. Converging
//! lanes are freed only after the parents are placed, so no extra parent
//! reuses one in the same row.
//!
//! Invariant: a lane in `top` that is neither `node` nor in `bottom`
//! converges into the node. No extra field is needed for that.
//!
//! Color rule: every connector takes the color of the lane that is *not* the
//! node's lane — a merge line uses its target lane, a converging line its
//! source lane.
//!
//! When a parent is not in the loaded page (paging boundary), the line simply
//! continues off the bottom of the last row.

use crate::model::HistoryCommit;

/// Lane spacing when the graph fits its reserved column width.
pub const LANE_W: f32 = 18.0;
/// Narrowest lane spacing; below this the column grows instead.
pub const MIN_LANE_W: f32 = 11.0;
/// Lanes reserved even for a one-lane graph: four lanes at full spacing cover
/// most topologies, so paging rarely needs to grow the column.
pub const MIN_RESERVED_LANES: usize = 4;

/// One commit's place in the graph, aligned by index with the commit list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// Lane of this commit's node.
    pub node: usize,
    /// Lanes with a line entering from the top edge.
    pub top: Vec<usize>,
    /// Lanes with a line leaving through the bottom edge.
    pub bottom: Vec<usize>,
    /// Lanes of the extra parents (index ≥ 1): one merge line per entry,
    /// from the node down into that lane. Straight continuations (first
    /// parent, passing lanes) are described by `top`/`bottom` alone.
    pub edges: Vec<usize>,
    pub is_merge: bool,
}

impl GraphRow {
    /// Highest lane index touched anywhere in the row (for column sizing).
    pub fn max_lane(&self) -> usize {
        self.top
            .iter()
            .chain(self.bottom.iter())
            .copied()
            .chain(self.edges.iter().copied())
            .chain(std::iter::once(self.node))
            .max()
            .unwrap_or(0)
    }
}

/// Lane state carried across pages so each page lays out only its new commits
/// (the alternative — re-laying out the whole accumulated history per page —
/// is quadratic in page count). Clone it into the background task that reads
/// the page; publish it back with the finished rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutState {
    /// `lanes[i]` is the hash lane `i` currently expects from below, if any.
    lanes: Vec<Option<String>>,
    /// Highest lane touched so far (for column sizing).
    cols: usize,
}

impl LayoutState {
    /// Assign `graph` for the next page of commits, in order.
    pub fn apply(&mut self, commits: &mut [HistoryCommit]) {
        for c in commits.iter_mut() {
            // Every lane waiting for this commit: the leftmost hosts the
            // node, the rest converge into it in this row.
            let waiting: Vec<usize> = self
                .lanes
                .iter()
                .enumerate()
                .filter(|(_, h)| h.as_deref() == Some(c.hash.as_str()))
                .map(|(ix, _)| ix)
                .collect();
            let node = match waiting.first() {
                Some(&ix) => ix,
                None => free_lane(&mut self.lanes),
            };
            let top: Vec<usize> = active(&self.lanes);
            self.lanes[node] = None;

            let mut edges = Vec::new();
            for (parent_ix, parent) in c.parents.iter().enumerate() {
                if parent_ix == 0 {
                    self.lanes[node] = Some(parent.clone());
                    continue;
                }
                let j = match self
                    .lanes
                    .iter()
                    .position(|h| h.as_deref() == Some(parent.as_str()))
                {
                    Some(j) => j,
                    None => {
                        let j = free_lane(&mut self.lanes);
                        self.lanes[j] = Some(parent.clone());
                        j
                    }
                };
                edges.push(j);
            }
            // Freed only now so no extra parent above could reuse a
            // converging lane in the same row.
            for &ix in waiting.iter().skip(1) {
                self.lanes[ix] = None;
            }

            c.graph = GraphRow {
                node,
                top,
                bottom: active(&self.lanes),
                edges,
                is_merge: c.parents.len() > 1,
            };
            self.cols = self.cols.max(c.graph.max_lane() + 1);
        }
    }

    /// Highest lane count laid out so far (at least 1).
    pub fn cols(&self) -> usize {
        self.cols.max(1)
    }
}

/// Fill `commit.graph` for every commit in the slice (test convenience; the
/// UI applies [`LayoutState`] one page at a time).
#[cfg(test)]
pub fn layout(commits: &mut [HistoryCommit]) {
    LayoutState::default().apply(commits);
}

/// Column sizing for the graph, in logical pixels: `(lane spacing, width)`.
///
/// `reserved` is the lane count reserved for the whole session (taken from the
/// first page), so loading older history cannot move the commit text. Wider
/// pages compress the lane spacing down to [`MIN_LANE_W`]; only a graph that
/// no longer fits at that floor grows the column.
pub fn column_metrics(cols: usize, reserved: usize) -> (f32, f32) {
    let cols = cols.max(1);
    let reserved = reserved.max(MIN_RESERVED_LANES);
    let reserved_width = reserved as f32 * LANE_W;
    if cols <= reserved {
        return (LANE_W, reserved_width);
    }
    let lane_w = (reserved_width / cols as f32).max(MIN_LANE_W);
    (lane_w, (cols as f32 * lane_w).max(reserved_width))
}

fn free_lane(lanes: &mut Vec<Option<String>>) -> usize {
    match lanes.iter().position(|h| h.is_none()) {
        Some(ix) => ix,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

fn active(lanes: &[Option<String>]) -> Vec<usize> {
    lanes
        .iter()
        .enumerate()
        .filter(|(_, h)| h.is_some())
        .map(|(ix, _)| ix)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hash: &str, parents: &[&str]) -> HistoryCommit {
        HistoryCommit {
            hash: hash.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            ..Default::default()
        }
    }

    fn rows(commits: &mut [HistoryCommit]) -> Vec<GraphRow> {
        layout(commits);
        commits.iter().map(|c| c.graph.clone()).collect()
    }

    #[test]
    fn linear_mainline_stays_in_one_lane() {
        let mut commits = [c("b", &["a"]), c("a", &[])];
        let r = rows(&mut commits);
        assert_eq!(r[0].node, 0);
        assert!(r[0].top.is_empty());
        assert_eq!(r[0].bottom, vec![0]);
        assert!(r[0].edges.is_empty());
        assert_eq!(r[1].node, 0);
        assert_eq!(r[1].top, vec![0]);
        assert!(r[1].bottom.is_empty());
        assert!(!r[1].is_merge);
    }

    #[test]
    fn merge_fans_out_and_rejoins() {
        // m -> a, b; both a and b -> c
        let mut commits = [
            c("m", &["a", "b"]),
            c("a", &["c"]),
            c("b", &["c"]),
            c("c", &[]),
        ];
        let r = rows(&mut commits);

        assert_eq!(r[0].node, 0);
        assert_eq!(r[0].edges, vec![1]);
        assert_eq!(r[0].bottom, vec![0, 1]);
        assert!(r[0].is_merge);

        assert_eq!(r[1].node, 0);
        assert_eq!(r[1].top, vec![0, 1]);
        assert_eq!(r[1].bottom, vec![0, 1]);

        assert_eq!(r[2].node, 1);
        assert_eq!(r[2].top, vec![0, 1]);
        assert!(r[2].edges.is_empty());
        assert_eq!(r[2].bottom, vec![0, 1]);

        assert_eq!(r[3].node, 0);
        assert_eq!(r[3].top, vec![0, 1]);
        assert!(r[3].bottom.is_empty());
    }

    #[test]
    fn mainline_keeps_its_lane_when_a_lane_to_the_right_waits_for_its_first_parent() {
        // Screenshot regression: `s` waits for p in lane 1 before merge m
        // (lane 0) names p as first parent. m keeps lane 0; p collects the
        // waiting lanes when it arrives.
        let mut commits = [
            c("top", &["m"]),
            c("s", &["p"]),
            c("m", &["p", "q"]),
            c("q", &["p"]),
            c("p", &[]),
        ];
        let r = rows(&mut commits);
        assert_eq!(r[2].node, 0);
        assert_eq!(r[2].edges, vec![2]);
        assert_eq!(r[2].bottom, vec![0, 1, 2]);
        assert_eq!(r[4].node, 0);
        assert_eq!(r[4].top, vec![0, 1, 2]);
        assert!(r[4].bottom.is_empty());
    }

    #[test]
    fn converging_lane_is_not_reused_in_the_same_row() {
        let mut commits = [
            c("x", &["p"]),
            c("y", &["p"]),
            c("p", &["a", "b"]),
            c("a", &[]),
            c("b", &[]),
        ];
        let r = rows(&mut commits);
        assert_eq!(r[2].node, 0);
        assert_eq!(r[2].top, vec![0, 1]);
        assert_eq!(r[2].edges, vec![2]);
        assert_eq!(r[2].bottom, vec![0, 2]);
    }

    #[test]
    fn octopus_allocates_one_lane_per_extra_parent() {
        let mut commits = [c("o", &["a", "b", "c"])];
        let r = rows(&mut commits);
        assert_eq!(r[0].node, 0);
        assert_eq!(r[0].edges, vec![1, 2]);
        assert_eq!(r[0].bottom, vec![0, 1, 2]);
        assert!(r[0].is_merge);
    }

    #[test]
    fn independent_roots_reuse_freed_lanes() {
        let mut commits = [c("one", &[]), c("two", &[])];
        let r = rows(&mut commits);
        assert_eq!(r[0].node, 0);
        assert!(r[0].bottom.is_empty());
        assert_eq!(r[1].node, 0);
        assert!(r[1].top.is_empty());
        assert!(r[1].bottom.is_empty());
    }

    #[test]
    fn parent_missing_at_page_boundary_keeps_lane_open() {
        let mut commits = [c("tip", &["older-not-loaded"])];
        let r = rows(&mut commits);
        assert_eq!(r[0].node, 0);
        assert_eq!(r[0].bottom, vec![0]);
        assert!(r[0].edges.is_empty());
    }

    #[test]
    fn tip_below_merge_lands_back_in_leftmost_free_lane() {
        // m merges a and b; after b ends, an unrelated tip t needs a lane.
        let mut commits = [
            c("m", &["a", "b"]),
            c("a", &["c"]),
            c("b", &["c"]),
            c("c", &[]),
            c("t", &[]),
        ];
        let r = rows(&mut commits);
        assert_eq!(r[3].top, vec![0, 1]);
        assert_eq!(r[4].node, 0);
        assert!(r[4].top.is_empty());
        assert!(r[4].bottom.is_empty());
    }

    #[test]
    fn incremental_layout_matches_a_fresh_layout_across_page_boundaries() {
        // A merge whose parents straddle the page boundary plus a side branch:
        // the second page must resume with the same lane state.
        let commits = vec![
            c("m", &["a", "b"]),
            c("a", &["c"]),
            c("b", &["c"]),
            c("c", &[]),
            c("tip", &["m"]),
        ];
        let mut one_shot = commits.clone();
        layout(&mut one_shot);

        let mut state = LayoutState::default();
        let mut first = commits[..2].to_vec();
        let mut second = commits[2..].to_vec();
        state.apply(&mut first);
        state.apply(&mut second);
        let incremental: Vec<GraphRow> = first
            .into_iter()
            .chain(second)
            .map(|commit| commit.graph)
            .collect();
        let fresh: Vec<GraphRow> = one_shot.into_iter().map(|commit| commit.graph).collect();
        assert_eq!(incremental, fresh);
        assert_eq!(state.cols(), 2);
    }

    #[test]
    fn column_width_is_stable_until_the_lane_floor() {
        // Fits the reservation: full spacing, reserved width.
        assert_eq!(column_metrics(2, 4), (LANE_W, 4.0 * LANE_W));
        // Wider than reserved but above the floor: same width, tighter lanes.
        let (lane_w, width) = column_metrics(6, 4);
        assert!(lane_w < LANE_W && lane_w >= MIN_LANE_W);
        assert!((width - 4.0 * LANE_W).abs() < 0.01);
        // Past the floor the column has to grow.
        let (floor_w, grown) = column_metrics(32, 4);
        assert_eq!(floor_w, MIN_LANE_W);
        assert!(grown > 4.0 * LANE_W);
        // A one-lane graph still reserves the minimum (4 lanes).
        assert_eq!(column_metrics(1, 0), (LANE_W, 4.0 * LANE_W));
    }
}
