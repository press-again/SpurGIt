//! Local-branch folder tree for the sidebar (SourceGit-style grouping).
//!
//! Branches sharing a `/`-separated prefix are grouped under folder rows.
//! Folders precede branches at every level and their count covers the whole
//! subtree. The flatten step resolves expansion state here, so the renderer
//! only paints rows.

use std::collections::{BTreeMap, HashSet};

use crate::settings::BranchSort;
use crate::status::BranchInfo;

/// One flattened sidebar row. `depth` drives the indentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BranchRow {
    Folder {
        /// Full folder path (`Feature/Enhancements`); its toggle key.
        path: String,
        /// Last path segment (`Enhancements`).
        name: String,
        /// Number of branches in the subtree, including nested folders.
        count: usize,
        open: bool,
        depth: usize,
    },
    Branch {
        /// Index into the `branches` slice the tree was built from.
        index: usize,
        /// Last path segment (`Ai_1`).
        leaf: String,
        depth: usize,
    },
}

#[derive(Default)]
struct Node {
    branches: Vec<usize>,
    folders: BTreeMap<String, Node>,
}

impl Node {
    fn count(&self) -> usize {
        self.branches.len() + self.folders.values().map(Node::count).sum::<usize>()
    }
}

/// Flatten `branches` into sidebar rows. `closed` holds the folder paths the
/// user collapsed; every other folder starts expanded. `animating` holds the
/// closed folders whose collapse animation has not settled yet: their
/// children stay in the row list (shrinking) until the fade reaches zero.
/// Leaves sort by name or by tip recency.
pub(super) fn flatten(
    branches: &[BranchInfo],
    closed: &HashSet<String>,
    animating: &HashSet<String>,
    sort: BranchSort,
) -> Vec<BranchRow> {
    let mut root = Node::default();
    for (index, branch) in branches.iter().enumerate() {
        let mut node = &mut root;
        let mut parts = branch.name.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                node.branches.push(index);
            } else {
                node = node.folders.entry(part.to_string()).or_default();
            }
        }
    }
    sort_leaves(&mut root, branches, sort);
    let mut rows = Vec::new();
    walk(&root, "", 0, closed, animating, branches, &mut rows);
    rows
}

fn sort_leaves(node: &mut Node, branches: &[BranchInfo], sort: BranchSort) {
    match sort {
        BranchSort::Name => node
            .branches
            .sort_by(|a, b| branches[*a].name.cmp(&branches[*b].name)),
        BranchSort::Recent => node.branches.sort_by(|a, b| {
            branches[*b]
                .updated
                .cmp(&branches[*a].updated)
                .then_with(|| branches[*a].name.cmp(&branches[*b].name))
        }),
    }
    for child in node.folders.values_mut() {
        sort_leaves(child, branches, sort);
    }
}

fn walk(
    node: &Node,
    prefix: &str,
    depth: usize,
    closed: &HashSet<String>,
    animating: &HashSet<String>,
    branches: &[BranchInfo],
    out: &mut Vec<BranchRow>,
) {
    for (name, child) in &node.folders {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let open = !closed.contains(&path) || animating.contains(&path);
        out.push(BranchRow::Folder {
            path: path.clone(),
            name: name.clone(),
            count: child.count(),
            open,
            depth,
        });
        if open {
            walk(child, &path, depth + 1, closed, animating, branches, out);
        }
    }
    for &index in &node.branches {
        out.push(BranchRow::Branch {
            index,
            leaf: branch_leaf(&branches[index].name),
            depth,
        });
    }
}

/// Last `/`-separated segment of a branch name.
pub(super) fn branch_leaf(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// One flattened row of the Remotes list: a remote (its branches nest beneath
/// it) or one of its remote-tracking branches. Branch names stay whole
/// (`feature/x`), no further folder grouping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RemoteRow {
    Remote {
        name: String,
        url: String,
        count: usize,
        open: bool,
    },
    Branch {
        /// Index into the `RemoteBranch` slice the rows were built from.
        index: usize,
        leaf: String,
    },
}

/// Flatten remotes and their remote-tracking branches. Remote names come from
/// the inspector's remote list (with URLs); a remote that only appears in the
/// ref data (details still loading) is still shown. `closed` holds remote
/// names the user collapsed; `animating` keeps a collapsing remote's branches
/// listed until its fade settles.
pub(super) fn remote_rows(
    remotes: &[(String, String)],
    branches: &[crate::status::RemoteBranch],
    closed: &HashSet<String>,
    animating: &HashSet<String>,
) -> Vec<RemoteRow> {
    let mut names: Vec<String> = remotes.iter().map(|(name, _)| name.clone()).collect();
    for branch in branches {
        if !names.contains(&branch.remote) {
            names.push(branch.remote.clone());
        }
    }
    let mut rows = Vec::new();
    for name in names {
        let url = remotes
            .iter()
            .find(|(remote, _)| remote == &name)
            .map(|(_, url)| url.clone())
            .unwrap_or_default();
        let mut children: Vec<usize> = branches
            .iter()
            .enumerate()
            .filter(|(_, branch)| branch.remote == name)
            .map(|(index, _)| index)
            .collect();
        children.sort_by(|a, b| branches[*a].name.cmp(&branches[*b].name));
        let open = !closed.contains(&name) || animating.contains(&name);
        rows.push(RemoteRow::Remote {
            name: name.clone(),
            url,
            count: children.len(),
            open,
        });
        if open {
            rows.extend(children.into_iter().map(|index| RemoteRow::Branch {
                index,
                leaf: branches[index].name.clone(),
            }));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test shim: settled state, nothing animating.
    fn flatten(
        branches: &[BranchInfo],
        closed: &HashSet<String>,
        sort: crate::settings::BranchSort,
    ) -> Vec<BranchRow> {
        super::flatten(branches, closed, &HashSet::new(), sort)
    }

    fn remote_rows(
        remotes: &[(String, String)],
        branches: &[crate::status::RemoteBranch],
        closed: &HashSet<String>,
    ) -> Vec<RemoteRow> {
        super::remote_rows(remotes, branches, closed, &HashSet::new())
    }

    fn b(name: &str) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            current: false,
            upstream: None,
            upstream_gone: false,
            ahead: 0,
            behind: 0,
            updated: 0,
        }
    }

    fn recent(name: &str, updated: i64) -> BranchInfo {
        BranchInfo {
            updated,
            ..b(name)
        }
    }

    fn labels(rows: &[BranchRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                BranchRow::Folder { name, depth, count, open, .. } => {
                    format!(
                        "{}{name}({count}){}",
                        "  ".repeat(*depth),
                        if *open { "" } else { "[-]" }
                    )
                }
                BranchRow::Branch { leaf, depth, .. } => {
                    format!("{}:{leaf}", "  ".repeat(*depth))
                }
            })
            .collect()
    }

    #[test]
    fn folders_group_shared_prefixes_before_branches_with_recursive_counts() {
        let branches = vec![
            b("Feature/Enhancements/Ai_1"),
            b("Feature/IntervalsICU/First-Integration"),
            b("Feature/Redesign"),
            b("Feature/Site-Builder-Rework"),
            b("Dev"),
            b("GLM_Flash"),
        ];
        let rows = flatten(
            &branches,
            &HashSet::new(),
            crate::settings::BranchSort::Name,
        );
        assert_eq!(
            labels(&rows),
            [
                "Feature(4)",
                "  Enhancements(1)",
                "    :Ai_1",
                "  IntervalsICU(1)",
                "    :First-Integration",
                "  :Redesign",
                "  :Site-Builder-Rework",
                ":Dev",
                ":GLM_Flash",
            ]
        );
        // Branch rows keep their index into the input slice.
        let indices: Vec<usize> = rows
            .iter()
            .filter_map(|row| match row {
                BranchRow::Branch { index, .. } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(indices, [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn closing_a_folder_hides_its_whole_subtree_but_keeps_the_row() {
        let branches = vec![
            b("Feature/Enhancements/Ai_1"),
            b("Feature/Redesign"),
            b("Dev"),
        ];
        let mut closed = HashSet::new();
        closed.insert("Feature".to_string());
        assert_eq!(
            labels(&flatten(
                &branches,
                &closed,
                crate::settings::BranchSort::Name
            )),
            ["Feature(2)[-]", ":Dev"]
        );

        let mut closed = HashSet::new();
        closed.insert("Feature/Enhancements".to_string());
        assert_eq!(
            labels(&flatten(
                &branches,
                &closed,
                crate::settings::BranchSort::Name
            )),
            ["Feature(2)", "  Enhancements(1)[-]", "  :Redesign", ":Dev"]
        );
    }

    #[test]
    fn a_branch_named_like_a_folder_prefix_lives_beside_it() {
        let branches = vec![b("feature"), b("feature/x"), b("feature/y")];
        assert_eq!(
            labels(&flatten(
                &branches,
                &HashSet::new(),
                crate::settings::BranchSort::Name
            )),
            ["feature(2)", "  :x", "  :y", ":feature"]
        );
    }

    #[test]
    fn leaves_sort_by_full_name_and_slashes_split_into_leaf_segments() {
        let branches = vec![b("release/2.0"), b("release/1.9"), b("main")];
        assert_eq!(
            labels(&flatten(
                &branches,
                &HashSet::new(),
                crate::settings::BranchSort::Name
            )),
            ["release(2)", "  :1.9", "  :2.0", ":main"]
        );
        assert_eq!(branch_leaf("release/2.0"), "2.0");
        assert_eq!(branch_leaf("main"), "main");
    }

    #[test]
    fn recent_sort_orders_leaves_by_tip_time_with_name_tiebreak() {
        let branches = vec![
            recent("release/2.0", 100),
            recent("release/1.9", 300),
            recent("main", 200),
        ];
        assert_eq!(
            labels(&flatten(
                &branches,
                &HashSet::new(),
                crate::settings::BranchSort::Recent
            )),
            ["release(2)", "  :1.9", "  :2.0", ":main"]
        );
    }

    fn rb(remote: &str, name: &str) -> crate::status::RemoteBranch {
        crate::status::RemoteBranch {
            remote: remote.to_string(),
            name: name.to_string(),
        }
    }

    fn remote_labels(rows: &[RemoteRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                RemoteRow::Remote { name, count, open, .. } => {
                    format!("{name}({count}){}", if *open { "" } else { "[-]" })
                }
                RemoteRow::Branch { leaf, .. } => format!("  {leaf}"),
            })
            .collect()
    }

    #[test]
    fn remotes_nest_their_sorted_branches_and_keep_empty_remotes() {
        let remotes = vec![
            ("origin".to_string(), "url-origin".to_string()),
            ("upstream".to_string(), "url-upstream".to_string()),
        ];
        let branches = vec![
            rb("origin", "main"),
            rb("origin", "feature/x"),
            rb("origin", "dev"),
        ];
        assert_eq!(
            remote_labels(&remote_rows(&remotes, &branches, &HashSet::new())),
            [
                "origin(3)",
                "  dev",
                "  feature/x",
                "  main",
                "upstream(0)",
            ]
        );

        // Collapsing one remote hides only its branches.
        let mut closed = HashSet::new();
        closed.insert("origin".to_string());
        assert_eq!(
            remote_labels(&remote_rows(&remotes, &branches, &closed)),
            ["origin(3)[-]", "upstream(0)"]
        );
    }

    #[test]
    fn remotes_only_known_from_refs_still_get_a_row() {
        let branches = vec![rb("fetched", "main")];
        assert_eq!(
            remote_labels(&remote_rows(&[], &branches, &HashSet::new())),
            ["fetched(1)", "  main"]
        );
    }
}
