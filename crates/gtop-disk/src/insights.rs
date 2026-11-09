//! "Worth a look": ranked inspection suggestions, never removal authority.
//!
//! Three kinds of finding, each one a directory a person can judge in a
//! second: reclaimable space ([`crate::classify::Reclaim`]), agent worktrees,
//! and experiments nobody has written to in a month. Findings never nest, so
//! their total is space that really exists once.

use crate::classify::{Category, Reclaim};
use crate::tree::Node;

/// Seconds in a day.
const DAY: i64 = 86_400;

/// An experiment untouched this long is worth a look.
pub const STALE_DAYS: i64 = 30;

/// Smaller than this is not worth a line.
const MIN_BYTES: u64 = 64 * 1024 * 1024;

/// Why a directory is on the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// A known rebuildable or otherwise inspectable class.
    Reclaimable(Reclaim),
    /// A directory of agent worktrees: how many, and the oldest one's age.
    Worktrees {
        count: usize,
        oldest_days: Option<i64>,
    },
    /// Experiments untouched for [`STALE_DAYS`]: only those are counted.
    StaleExperiments { count: usize },
}

/// One line of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Where it is, from the scanned root.
    pub crumbs: Vec<usize>,
    /// Matching children for a subtotal. An empty list means this directory.
    pub children: Vec<usize>,
    /// Observed bytes, counted once. This is not a promise of freed space.
    pub bytes: u64,
    pub finding: Finding,
}

impl Candidate {
    /// Exact directories counted by this finding, relative to the scan root.
    pub fn locations(&self) -> Vec<Vec<usize>> {
        if self.children.is_empty() {
            vec![self.crumbs.clone()]
        } else {
            self.children
                .iter()
                .map(|index| {
                    let mut path = self.crumbs.clone();
                    path.push(*index);
                    path
                })
                .collect()
        }
    }

    pub fn reason(&self) -> String {
        match self.finding {
            Finding::Reclaimable(reason) => format!("{} · inspect before cleanup", reason.label()),
            Finding::Worktrees {
                count,
                oldest_days: None,
            } => {
                format!("{count} worktrees · write time Unknown · inspect")
            }
            Finding::Worktrees {
                count,
                oldest_days: Some(oldest_days),
            } => format!("{count} worktrees · oldest known write {oldest_days} days ago"),
            Finding::StaleExperiments { count } => {
                format!("{count} experiments over {STALE_DAYS} days old · inspect children")
            }
        }
    }
}

/// The `limit` largest findings beneath `root`, largest first. `now` is Unix
/// seconds, passed in so the answer is testable.
pub fn worth_a_look(root: &Node, now: i64, limit: usize) -> Vec<Candidate> {
    let mut found = Vec::new();
    let mut crumbs = Vec::new();
    visit(root, &mut crumbs, now, &mut found);
    found.retain(|candidate| candidate.bytes >= MIN_BYTES);
    found.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.crumbs.cmp(&b.crumbs)));
    found.truncate(limit);
    found
}

fn visit(node: &Node, crumbs: &mut Vec<usize>, now: i64, found: &mut Vec<Candidate>) {
    // Nothing beneath a small directory can reach `MIN_BYTES` either, so a
    // scan of millions of files looks at a few thousand directories.
    if !node.is_dir()
        || node.bytes < MIN_BYTES
        || node.in_cloud
        || node.skipped_volume
        || node.read_error
    {
        return;
    }
    // Topmost only: everything beneath a reclaimable directory goes with it.
    if let Some(reason) = node.reclaim {
        found.push(Candidate {
            crumbs: crumbs.clone(),
            children: Vec::new(),
            bytes: node.bytes,
            finding: Finding::Reclaimable(reason),
        });
        return;
    }
    let name = &*node.name;
    let scratch = node.category == Category::AgentScratch;
    if scratch && name.eq_ignore_ascii_case("worktrees") {
        let trees: Vec<&Node> = node
            .children
            .iter()
            .filter(|child| child.is_dir())
            .collect();
        if !trees.is_empty() {
            let oldest = trees
                .iter()
                .map(|tree| tree.modified)
                .filter(|&time| time > 0)
                .min()
                .map(|time| now.saturating_sub(time).max(0) / DAY);
            found.push(Candidate {
                crumbs: crumbs.clone(),
                children: Vec::new(),
                bytes: node.bytes,
                finding: Finding::Worktrees {
                    count: trees.len(),
                    oldest_days: oldest,
                },
            });
            return;
        }
    }
    let experiments =
        scratch && (name.eq_ignore_ascii_case("tries") || name.eq_ignore_ascii_case("experiments"));
    let mut stale = (Vec::new(), 0_u64);
    for (index, child) in node.children.iter().enumerate() {
        let is_stale = experiments && child.is_dir() && known_stale(child, now);
        if is_stale {
            stale.0.push(index);
            stale.1 = stale.1.saturating_add(child.bytes);
            // A stale experiment is judged whole; its caches go with it.
            continue;
        }
        crumbs.push(index);
        visit(child, crumbs, now, found);
        crumbs.pop();
    }
    if !stale.0.is_empty() {
        found.push(Candidate {
            crumbs: crumbs.clone(),
            bytes: stale.1,
            finding: Finding::StaleExperiments {
                count: stale.0.len(),
            },
            children: stale.0,
        });
    }
}

fn known_stale(node: &Node, now: i64) -> bool {
    node.complete
        && !node.in_cloud
        && !node.skipped_volume
        && !node.read_error
        && node.modified > 0
        && now.saturating_sub(node.modified) > STALE_DAYS * DAY
        && node.children.iter().all(|child| known_stale(child, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::classify;
    use crate::tree::{Metric, NodeKind, aggregate};

    const GIB: u64 = 1024 * 1024 * 1024;
    const NOW: i64 = 1_800_000_000;

    fn file(name: &str, bytes: u64, days_old: i64) -> Node {
        let mut node = Node::entry(name, NodeKind::File, bytes);
        node.modified = NOW - days_old * DAY;
        node
    }

    fn dir(name: &str, children: Vec<Node>) -> Node {
        let mut node = Node::directory(name);
        node.children = children;
        node
    }

    fn scan(mut root: Node) -> Node {
        aggregate(&mut root, Metric::Bytes);
        classify(&mut root);
        root
    }

    fn home() -> Node {
        scan(dir(
            "tobi",
            vec![
                dir(".cache", vec![dir("kache", vec![file("blob", 5 * GIB, 1)])]),
                dir(
                    ".codex",
                    vec![dir(
                        "worktrees",
                        vec![
                            dir("a1", vec![file("x", 3 * GIB, 41)]),
                            dir("b2", vec![file("y", 2 * GIB, 3)]),
                        ],
                    )],
                ),
                dir(
                    "src",
                    vec![dir(
                        "tries",
                        vec![
                            dir("old", vec![file("z", 4 * GIB, 90)]),
                            dir(
                                "fresh",
                                vec![
                                    file("Cargo.toml", 1, 1),
                                    dir("target", vec![file("o", GIB, 1)]),
                                ],
                            ),
                        ],
                    )],
                ),
                dir("Documents", vec![file("tax.pdf", 9 * GIB, 400)]),
                dir("tiny", vec![dir(".cache", vec![file("t", 1024, 1)])]),
            ],
        ))
    }

    #[test]
    fn ranks_findings_largest_first_and_skips_the_tiny() {
        let found = worth_a_look(&home(), NOW, 10);
        let kinds: Vec<&Finding> = found.iter().map(|c| &c.finding).collect();
        assert_eq!(
            kinds,
            vec![
                &Finding::Reclaimable(Reclaim::Regenerable),
                &Finding::Worktrees {
                    count: 2,
                    oldest_days: Some(41)
                },
                &Finding::StaleExperiments { count: 1 },
                &Finding::Reclaimable(Reclaim::BuildOutput),
            ]
        );
        assert_eq!(found[0].bytes, 5 * GIB);
        assert_eq!(found[2].bytes, 4 * GIB, "only the stale experiment counts");
    }

    #[test]
    fn documents_are_never_suggested() {
        let found = worth_a_look(&home(), NOW, 10);
        assert!(found.iter().all(|c| c.crumbs != vec![3]));
    }

    #[test]
    fn crumbs_address_the_finding_from_the_root() {
        let root = home();
        for candidate in worth_a_look(&root, NOW, 10) {
            let node = root.resolve(&candidate.crumbs).expect("resolves");
            assert!(node.is_dir());
        }
    }

    #[test]
    fn the_limit_keeps_the_largest() {
        let found = worth_a_look(&home(), NOW, 2);
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[1].finding,
            Finding::Worktrees {
                count: 2,
                oldest_days: Some(41)
            }
        );
    }

    #[test]
    fn nested_findings_account_for_each_location_once() {
        let root = home();
        let found = worth_a_look(&root, NOW, usize::MAX);
        let locations: Vec<_> = found.iter().flat_map(Candidate::locations).collect();
        for (index, path) in locations.iter().enumerate() {
            for other in &locations[index + 1..] {
                assert!(!path.starts_with(other) && !other.starts_with(path));
            }
        }
        assert_eq!(found.iter().map(|item| item.bytes).sum::<u64>(), 15 * GIB);
        for item in &found {
            assert_eq!(
                item.bytes,
                item.locations()
                    .iter()
                    .map(|path| root.resolve(path).unwrap().bytes)
                    .sum::<u64>()
            );
        }
        let cache = found
            .iter()
            .find(|item| item.finding == Finding::Reclaimable(Reclaim::Regenerable))
            .unwrap();
        assert_eq!(&*root.resolve(&cache.crumbs).unwrap().name, ".cache");
        assert_eq!(
            cache.locations().len(),
            1,
            "nested cache is already covered"
        );
    }

    #[test]
    fn worktree_contains_nested_build_output_without_overlapping_findings() {
        let mut root = home();
        let codex = root
            .children
            .iter_mut()
            .find(|node| &*node.name == ".codex")
            .unwrap();
        let worktrees = codex
            .children
            .iter_mut()
            .find(|node| &*node.name == "worktrees")
            .unwrap();
        let branch = worktrees
            .children
            .iter_mut()
            .find(|node| &*node.name == "a1")
            .unwrap();
        branch.children.push(file("Cargo.toml", 1, 1));
        branch.children.push(dir(
            "target",
            vec![file("build-output", 128 * 1024 * 1024, 1)],
        ));
        let root = scan(root);

        let found = worth_a_look(&root, NOW, usize::MAX);
        let worktree_findings: Vec<_> = found
            .iter()
            .filter(|item| matches!(item.finding, Finding::Worktrees { .. }))
            .collect();
        assert_eq!(worktree_findings.len(), 1);
        assert_eq!(
            &*root.resolve(&worktree_findings[0].crumbs).unwrap().name,
            "worktrees"
        );
        assert!(matches!(
            worktree_findings[0].finding,
            Finding::Worktrees { count: 2, .. }
        ));
        let mut branch_target = worktree_findings[0].crumbs.clone();
        let worktrees = root.resolve(&branch_target).unwrap();
        let branch = worktrees
            .children
            .iter()
            .position(|node| &*node.name == "a1")
            .unwrap();
        branch_target.push(branch);
        let branch_node = root.resolve(&branch_target).unwrap();
        let target = branch_node
            .children
            .iter()
            .position(|node| &*node.name == "target")
            .unwrap();
        branch_target.push(target);
        assert!(found.iter().all(|item| {
            item.locations()
                .iter()
                .all(|path| !path.starts_with(&branch_target))
        }));

        let locations: Vec<_> = found.iter().flat_map(Candidate::locations).collect();
        for (index, path) in locations.iter().enumerate() {
            for other in &locations[index + 1..] {
                assert!(!path.starts_with(other) && !other.starts_with(path));
            }
        }
    }

    #[test]
    fn stale_subtotal_addresses_only_matching_children_and_never_the_mixed_parent() {
        let root = home();
        let found = worth_a_look(&root, NOW, 24);
        let subtotal = found
            .iter()
            .find(|item| matches!(item.finding, Finding::StaleExperiments { .. }))
            .unwrap();
        assert_eq!(&*root.resolve(&subtotal.crumbs).unwrap().name, "tries");
        let locations = subtotal.locations();
        assert_eq!(locations.len(), 1);
        assert_eq!(&*root.resolve(&locations[0]).unwrap().name, "old");
        assert_ne!(locations[0], subtotal.crumbs);
        assert!(subtotal.bytes < root.resolve(&subtotal.crumbs).unwrap().bytes);
        assert!(
            found
                .iter()
                .any(|item| item.finding == Finding::Reclaimable(Reclaim::BuildOutput)),
            "fresh work's target remains a separate suggestion"
        );
    }

    #[test]
    fn unknown_future_boundary_and_incomplete_times_do_not_imply_stale_work() {
        let mut incomplete = dir("partial", vec![file("old", GIB, 90)]);
        incomplete.complete = false;
        let mut unknown = file("unknown", GIB, 90);
        unknown.modified = 0;
        let root = scan(dir(
            "experiments",
            vec![
                dir("mixed-time", vec![file("known-old", GIB, 90), unknown]),
                dir("boundary", vec![file("old", GIB, STALE_DAYS)]),
                dir("future", vec![file("future", GIB, -1)]),
                incomplete,
                dir("old", vec![file("old", GIB, STALE_DAYS + 1)]),
            ],
        ));
        let found = worth_a_look(&root, NOW, 24);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].bytes, GIB);
        assert_eq!(found[0].finding, Finding::StaleExperiments { count: 1 });
        assert_eq!(
            &*root.resolve(&found[0].locations()[0]).unwrap().name,
            "old"
        );
    }

    #[test]
    fn multiple_old_experiments_cover_nested_reclaim_without_counting_fresh_work() {
        let root = scan(dir(
            "home",
            vec![dir(
                "experiments",
                vec![
                    dir(
                        "old-a",
                        vec![dir(".cache", vec![file("blob", 2 * GIB, 60)])],
                    ),
                    dir("old-b", vec![file("source", 3 * GIB, 45)]),
                    dir("fresh", vec![file("source", GIB, 1)]),
                ],
            )],
        ));
        let found = worth_a_look(&root, NOW, 24);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].bytes, 5 * GIB);
        assert_eq!(found[0].finding, Finding::StaleExperiments { count: 2 });
        assert_eq!(found[0].locations().len(), 2);
        assert!(
            found[0].locations().iter().all(|path| root
                .resolve(path)
                .unwrap()
                .name
                .starts_with("old-"))
        );
    }

    #[test]
    fn cloud_and_other_volumes_are_not_inspection_suggestions() {
        let mut root = scan(dir(
            "home",
            vec![
                dir(".cache", vec![file("blob", GIB, 1)]),
                dir("node_modules", vec![file("blob", GIB, 1)]),
            ],
        ));
        root.children[0].in_cloud = true;
        root.children[1].skipped_volume = true;
        assert!(worth_a_look(&root, NOW, 24).is_empty());
    }

    #[test]
    fn known_cache_dependency_and_cargo_reasons_remain_inspection_suggestions() {
        let root = scan(dir(
            "home",
            vec![
                dir(
                    "Library",
                    vec![dir(
                        "pnpm",
                        vec![dir("store", vec![file("packages", GIB, 1)])],
                    )],
                ),
                dir(
                    ".bun",
                    vec![dir(
                        "install",
                        vec![dir("cache", vec![file("packages", GIB, 1)])],
                    )],
                ),
                dir(
                    ".cargo",
                    vec![dir(
                        "registry",
                        vec![dir("cache", vec![file("archives", GIB, 1)])],
                    )],
                ),
                dir(
                    "project",
                    vec![
                        file("Cargo.toml", 1, 1),
                        dir("target", vec![file("binary", GIB, 1)]),
                        dir("node_modules", vec![file("packages", GIB, 1)]),
                    ],
                ),
            ],
        ));
        let found = worth_a_look(&root, NOW, 24);
        assert_eq!(found.len(), 5);
        for reason in [
            Reclaim::Regenerable,
            Reclaim::PackageStore,
            Reclaim::BuildOutput,
            Reclaim::Reinstallable,
        ] {
            assert!(
                found
                    .iter()
                    .any(|item| item.finding == Finding::Reclaimable(reason)),
                "missing {reason:?}"
            );
        }
        assert_eq!(found.iter().map(|item| item.bytes).sum::<u64>(), 5 * GIB);
    }

    #[test]
    fn unknown_worktree_age_stays_unknown() {
        let mut unknown = file("source", GIB, 60);
        unknown.modified = 0;
        let root = scan(dir("worktrees", vec![dir("checkout", vec![unknown])]));
        let found = worth_a_look(&root, NOW, 24);
        assert_eq!(
            found[0].finding,
            Finding::Worktrees {
                count: 1,
                oldest_days: None
            }
        );
        assert!(found[0].reason().contains("Unknown"));
    }
}
