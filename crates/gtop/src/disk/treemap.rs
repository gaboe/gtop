use super::*;
use gtop_disk::exploration::{age_label, draw_depth, escape, Escape, History, Mode, Viewport};
use gtop_disk::{
    tree::{Metric, Node},
    treemap::{self, LayoutOptions, Rect, Tile, TileKind},
};
use std::rc::Rc;

#[derive(Default)]
pub(super) struct MapState {
    mode: Mode,
    depth: u32,
    filter: String,
    committed_filter: String,
    filtering: bool,
    applied: bool,
    viewport: Viewport,
    history: History,
    siblings: Option<PathBuf>,
    sibling_index: usize,
    sibling_scroll: UniformListScrollHandle,
    scan_root: Option<(PathBuf, Arc<Node>)>,
    selected: Option<TileKind>,
    hovered: Option<TileKind>,
    frame: Rc<RefCell<MapFrame>>,
    list_scroll: UniformListScrollHandle,
    listing: RefCell<Option<Listing>>,
    matches: RefCell<Option<FilterCache>>,
    committed_matches: RefCell<Option<FilterCache>>,
    insights: RefCell<Option<InsightCache>>,
    finding_cursor: Option<usize>,
    finding_scroll: UniformListScrollHandle,
    pub(super) previous_result: bool,
    pub(super) pending_selection: Option<disk_scan::Row>,
    #[cfg(test)]
    clock: Option<i64>,
    #[cfg(test)]
    filter_searches: std::cell::Cell<usize>,
}

struct InsightCache {
    root: Arc<Node>,
    now: i64,
    findings: Rc<Vec<gtop_disk::insights::Candidate>>,
}

struct FilterCache {
    root: Arc<Node>,
    query: String,
    mode: Mode,
    matches: Option<Rc<gtop_disk::filter::Matches>>,
}

struct Listing {
    root: Arc<Node>,
    mode: Mode,
    depth: u32,
    filter: String,
    entries: Rc<Vec<Entry>>,
    applied: bool,
}

#[derive(Default)]
struct MapFrame {
    origin: Point<Pixels>,
    size: Size<Pixels>,
    root: Option<Arc<Node>>,
    tiles: Vec<Tile>,
    #[cfg(test)]
    painted: Vec<(TileKind, Rect, String, u32)>,
    #[cfg(test)]
    paint_count: usize,
    #[cfg(test)]
    hatched: Vec<(TileKind, Bounds<Pixels>)>,
    #[cfg(test)]
    coverage_labels: Vec<(TileKind, String, Bounds<Pixels>)>,
    mode: Mode,
    depth: u32,
    filter: String,
}

impl MapState {
    fn findings(
        &self,
        root: &Arc<Node>,
        observed: std::time::SystemTime,
    ) -> Rc<Vec<gtop_disk::insights::Candidate>> {
        let now = observed
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        #[cfg(test)]
        let now = self.clock.unwrap_or(now);
        let mut cache = self.insights.borrow_mut();
        if let Some(cached) = cache.as_ref() {
            if Arc::ptr_eq(&cached.root, root) && cached.now == now {
                return cached.findings.clone();
            }
        }
        let findings = Rc::new(gtop_disk::insights::worth_a_look(root, now, 24));
        *cache = Some(InsightCache {
            root: root.clone(),
            now,
            findings: findings.clone(),
        });
        findings
    }
    fn draw_depth(&self) -> u32 {
        if self.depth == 0 {
            4
        } else {
            self.depth
        }
    }
    pub(super) fn reset_navigation(&mut self) {
        self.finding_cursor = None;
        self.selected = None;
        self.pending_selection = None;
        self.previous_result = false;
        self.hovered = None;
        self.viewport = Viewport::default();
        self.siblings = None;
        *self.frame.borrow_mut() = MapFrame::default();
        *self.listing.borrow_mut() = None;
    }
    fn matches(&self, root: &Arc<Node>) -> Option<Rc<gtop_disk::filter::Matches>> {
        self.cached_matches(root, &self.filter, &self.matches)
    }
    fn layout_matches(&self, root: &Arc<Node>) -> Option<Rc<gtop_disk::filter::Matches>> {
        if !self.applied {
            return None;
        }
        if self.committed_filter == self.filter {
            return self.matches(root);
        }
        self.cached_matches(root, &self.committed_filter, &self.committed_matches)
    }
    fn cached_matches(
        &self,
        root: &Arc<Node>,
        query: &str,
        cache: &RefCell<Option<FilterCache>>,
    ) -> Option<Rc<gtop_disk::filter::Matches>> {
        let mut cache = cache.borrow_mut();
        if let Some(cached) = cache.as_ref() {
            if Arc::ptr_eq(&cached.root, root) && cached.query == query && cached.mode == self.mode
            {
                return cached.matches.clone();
            }
        }
        #[cfg(test)]
        self.filter_searches.set(self.filter_searches.get() + 1);
        let matches = gtop_disk::filter::filter(root, &[], query).map(Rc::new);
        *cache = Some(FilterCache {
            root: root.clone(),
            query: query.to_owned(),
            mode: self.mode,
            matches: matches.clone(),
        });
        matches
    }
    fn entries(&self, root: &Arc<Node>) -> Rc<Vec<Entry>> {
        let mut listing = self.listing.borrow_mut();
        if let Some(cached) = &*listing {
            if Arc::ptr_eq(&cached.root, root)
                && cached.mode == self.mode
                && cached.depth == self.draw_depth()
                && cached.filter == self.committed_filter
                && cached.applied == self.applied
            {
                return cached.entries.clone();
            }
        }
        let matches = self.layout_matches(root);
        let rows = entries_filtered(
            root,
            self.mode.metric(),
            self.draw_depth(),
            if self.applied {
                matches.as_deref()
            } else {
                None
            },
        );
        let entries = Rc::new(rows);
        *listing = Some(Listing {
            root: root.clone(),
            mode: self.mode,
            depth: self.draw_depth(),
            filter: self.committed_filter.clone(),
            entries: entries.clone(),
            applied: self.applied,
        });
        entries
    }
}

struct Entry {
    kind: TileKind,
    label: String,
    visible_label: Option<String>,
    depth: usize,
}

fn finding_locations(
    findings: &[gtop_disk::insights::Candidate],
) -> Vec<(usize, Vec<usize>, bool)> {
    let mut rows = Vec::new();
    for (index, finding) in findings.iter().enumerate() {
        let locations = finding.locations();
        rows.push((index, locations[0].clone(), false));
        if !finding.children.is_empty() {
            rows.extend(locations.into_iter().map(|crumbs| (index, crumbs, true)));
        }
    }
    rows
}

#[cfg(test)]
fn entries(root: &Node) -> Vec<Entry> {
    entries_with(root, Metric::Bytes, 4)
}
#[cfg(test)]
fn entries_with(root: &Node, metric: Metric, depth: u32) -> Vec<Entry> {
    entries_filtered(root, metric, depth, None)
}

fn entries_filtered(
    root: &Node,
    metric: Metric,
    depth: u32,
    matches: Option<&gtop_disk::filter::Matches>,
) -> Vec<Entry> {
    fn visit(
        node: &Node,
        crumbs: &mut Vec<usize>,
        out: &mut Vec<Entry>,
        metric: Metric,
        depth: u32,
        matches: Option<&gtop_disk::filter::Matches>,
    ) {
        let mut children: Vec<_> = node
            .children
            .iter()
            .enumerate()
            .filter(|(index, child)| {
                let mut path = crumbs.clone();
                path.push(*index);
                matches.map_or(child.value(metric) > 0, |found| found.keep(&path).is_some())
            })
            .collect();
        children.sort_by_key(|(index, child)| {
            let mut path = crumbs.clone();
            path.push(*index);
            std::cmp::Reverse(matches.map_or(child.value(metric), |found| {
                found.keep(&path).map_or(0, |keep| {
                    gtop_disk::filter::Matches::value(keep, child, metric)
                })
            }))
        });
        let positive_count = children
            .iter()
            .filter(|(index, child)| {
                let mut path = crumbs.clone();
                path.push(*index);
                matches.map_or(child.value(metric), |found| {
                    found.keep(&path).map_or(0, |keep| {
                        gtop_disk::filter::Matches::value(keep, child, metric)
                    })
                }) > 0
            })
            .count();
        let kept = positive_count.min(LayoutOptions::default().max_children);
        let mut positive_rank = 0;
        for (index, child) in &children {
            let mut path = crumbs.clone();
            path.push(*index);
            let weight = matches.map_or(child.value(metric), |found| {
                found.keep(&path).map_or(0, |keep| {
                    gtop_disk::filter::Matches::value(keep, child, metric)
                })
            });
            if weight > 0 && positive_rank == kept && positive_count > kept {
                out.push(Entry {
                    kind: TileKind::Others {
                        crumbs: crumbs.clone(),
                        count: positive_count - kept,
                    },
                    label: format!(
                        "Other · {} items · {}",
                        positive_count - kept,
                        bytes(
                            children
                                .iter()
                                .map(|(index, node)| {
                                    let mut path = crumbs.clone();
                                    path.push(*index);
                                    let keep = matches.and_then(|found| found.keep(&path));
                                    let weight = matches.map_or(node.value(metric), |_| {
                                        keep.map_or(0, |keep| {
                                            gtop_disk::filter::Matches::value(keep, node, metric)
                                        })
                                    });
                                    let bytes = matches.map_or(node.bytes, |_| {
                                        keep.map_or(0, |keep| {
                                            gtop_disk::filter::Matches::value(
                                                keep,
                                                node,
                                                Metric::Bytes,
                                            )
                                        })
                                    });
                                    (weight, bytes)
                                })
                                .filter(|(weight, _)| *weight > 0)
                                .skip(kept)
                                .map(|(_, bytes)| bytes)
                                .sum()
                        )
                    ),
                    visible_label: None,
                    depth: crumbs.len(),
                });
            }

            crumbs.push(*index);
            out.push(Entry {
                kind: TileKind::Node {
                    crumbs: crumbs.clone(),
                },
                label: format!(
                    "{} · {}{} · {}",
                    child.name,
                    bytes(child.bytes),
                    coverage(child),
                    gtop_disk::classify::description(child)
                ),
                visible_label: Some(format!(
                    "{} · {} · {}{}",
                    child.category.label(),
                    child.name,
                    bytes(child.bytes),
                    coverage(child)
                )),
                depth: crumbs.len() - 1,
            });
            if (weight == 0 || positive_rank < kept) && crumbs.len() < depth as usize {
                visit(child, crumbs, out, metric, depth, matches);
            }
            crumbs.pop();
            if weight > 0 {
                positive_rank += 1;
            }
        }
        for (index, child) in node.children.iter().enumerate().filter(|(index, child)| {
            let mut path = crumbs.clone();
            path.push(*index);
            matches.is_none() && child.value(metric) == 0
        }) {
            let mut path = crumbs.clone();
            path.push(index);
            out.push(Entry {
                kind: TileKind::Node { crumbs: path },
                label: format!(
                    "{} · {}{} · {}",
                    child.name,
                    bytes(child.bytes),
                    coverage(child),
                    gtop_disk::classify::description(child)
                ),
                visible_label: Some(format!(
                    "{} · {} · {}{}",
                    child.category.label(),
                    child.name,
                    bytes(child.bytes),
                    coverage(child)
                )),
                depth: crumbs.len(),
            });
        }
    }
    let mut out = Vec::new();
    visit(root, &mut Vec::new(), &mut out, metric, depth, matches);
    out
}

fn coverage_tag(node: &Node) -> Option<&'static str> {
    if node.in_cloud {
        Some("CLOUD")
    } else if node.skipped_volume {
        Some("VOL")
    } else if !node.complete || node.read_error || node.unreadable > 0 {
        Some("PART")
    } else if node.shared {
        Some("SHARED")
    } else {
        None
    }
}

fn coverage(node: &Node) -> &'static str {
    if node.in_cloud {
        " · in cloud"
    } else if node.skipped_volume {
        " · other volume"
    } else if !node.complete || node.read_error || node.unreadable > 0 {
        " · incomplete"
    } else if node.shared {
        " · counted elsewhere"
    } else {
        ""
    }
}

fn node_path(root: &Node, base: &Path, crumbs: &[usize]) -> Option<PathBuf> {
    let mut path = base.to_path_buf();
    let mut node = root;
    for &index in crumbs {
        node = node.children.get(index)?;
        path.push(node.filesystem_name());
    }
    Some(path)
}

impl Disk {
    fn inspect_finding(&mut self, crumbs: Vec<usize>) {
        self.map.filter.clear();
        self.map.committed_filter.clear();
        self.map.applied = false;
        self.map.filtering = false;
        self.map.viewport = Viewport::default();
        self.map.depth = self.map.draw_depth().max(crumbs.len() as u32 + 1);
        self.map_select(TileKind::Node { crumbs });
    }
    pub(super) fn map_filtering(&self) -> bool {
        self.map.filtering
    }
    fn set_map_mode(&mut self, mode: Mode) {
        self.invalidate_proposal();
        self.map.mode = mode;
    }
    pub(super) fn remember_map_root(&mut self, folder: &Folder) {
        if folder.path == folder.root {
            if let Some(root) = &folder.hierarchy {
                self.map.scan_root = Some((folder.root.clone(), root.clone()));
            }
        }
    }
    fn map_siblings(&self, path: &Path) -> Vec<PathBuf> {
        let Some((base, root)) = &self.map.scan_root else {
            return Vec::new();
        };
        let Some(parent) = path.parent().filter(|parent| parent.starts_with(base)) else {
            return Vec::new();
        };
        let Ok(relative) = parent.strip_prefix(base) else {
            return Vec::new();
        };
        let mut node = root.as_ref();
        for part in relative.components() {
            let Some(child) = node
                .children
                .iter()
                .find(|child| child.filesystem_name() == part.as_os_str())
            else {
                return Vec::new();
            };
            node = child;
        }
        node.children
            .iter()
            .filter(|child| child.is_dir() && !child.in_cloud && !child.skipped_volume)
            .map(|child| parent.join(child.filesystem_name()))
            .collect()
    }
    pub(super) fn map_visit(&mut self, path: PathBuf) {
        self.map.history.visit(path);
    }
    pub(super) fn map_controls_key(&mut self, key: &str) -> bool {
        if !self.map.filtering
            && self.map.finding_cursor.is_some()
            && matches!(
                key,
                "up" | "k" | "down" | "j" | "tab" | "shift-tab" | "enter" | "escape"
            )
        {
            return self.map_key(key);
        }
        self.map.finding_cursor = None;
        if self.map.filtering {
            match key {
                "escape" => {
                    self.map.filter.clear();
                    self.map.committed_filter.clear();
                    self.map.filtering = false;
                    self.map.applied = false;
                }
                "enter" => {
                    self.map.filtering = false;
                    self.map.applied = true;
                    self.map.committed_filter = self.map.filter.clone();
                    *self.map.listing.borrow_mut() = None;
                }
                "backspace" => {
                    self.map.filter.pop();
                }
                "space" => self.map.filter.push(' '),
                key if key.chars().count() == 1 => self.map.filter.push_str(key),
                _ => {}
            }
            return true;
        }
        if let Some(path) = self.map.siblings.clone() {
            let siblings = self.map_siblings(&path);
            match key {
                "escape" => self.map.siblings = None,
                "down" | "j" | "tab" | "up" | "k" | "shift-tab" => {
                    self.map.sibling_index = if matches!(key, "down" | "j" | "tab") {
                        (self.map.sibling_index + 1).min(siblings.len().saturating_sub(1))
                    } else {
                        self.map.sibling_index.saturating_sub(1)
                    };
                    self.map
                        .sibling_scroll
                        .scroll_to_item(self.map.sibling_index, ScrollStrategy::Nearest);
                }
                "enter" => {
                    if let Some(path) = siblings.get(self.map.sibling_index) {
                        self.enter(path.clone());
                    }
                }
                _ => return self.map_command(key),
            }
            return true;
        }
        self.map_command(key)
    }

    fn map_command(&mut self, key: &str) -> bool {
        match key {
            "/" => {
                self.map.filtering = true;
            }
            "m" => self.set_map_mode(self.map.mode.next()),
            "[" | "]" => self.map.depth = draw_depth(self.map.draw_depth(), key == "]"),
            "0" => self.map.viewport = Viewport::default(),
            "+" | "=" | "-" => {
                let size = self.map.frame.borrow().size;
                self.map.viewport.zoom(
                    if key == "-" { 0.8 } else { 1.25 },
                    size.width.as_f32() / 2.,
                    size.height.as_f32() / 2.,
                );
            }
            "escape" => match escape(
                !self.map.filter.is_empty(),
                self.map.viewport,
                self.map.selected.is_some(),
            ) {
                Escape::Filter => {
                    self.map.filter.clear();
                    self.map.committed_filter.clear();
                    self.map.applied = false;
                }
                Escape::Viewport => self.map.viewport = Viewport::default(),
                Escape::Selection => {
                    self.map.selected = None;
                    self.map.pending_selection = None;
                    self.invalidate_proposal();
                }
                Escape::Close => return false,
            },
            "h" | "left" | "l" | "right" | "up" | "k" | "down" | "j"
                if !self.map.frame.borrow().tiles.is_empty() =>
            {
                use gtop_disk::exploration::{spatial_neighbor, Direction};
                let direction = match key {
                    "h" | "left" => Direction::Left,
                    "l" | "right" => Direction::Right,
                    "up" | "k" => Direction::Up,
                    _ => Direction::Down,
                };
                let target = spatial_neighbor(
                    &self.map.frame.borrow().tiles,
                    self.map.selected.as_ref(),
                    direction,
                );
                if let Some(kind) = target {
                    self.map_select(kind);
                }
            }
            "b" | "f" => self.map_travel(key == "f"),
            _ => return false,
        }
        true
    }

    fn map_travel(&mut self, forward: bool) {
        let previous = self.map.history.clone();
        let generation = self.generation();
        let navigation = self.navigation;
        if let Some(path) = self.map.history.travel(forward) {
            if self
                .session
                .as_ref()
                .is_some_and(|session| path.starts_with(&session.root))
            {
                self.browse(path, 0);
            } else {
                self.start(path, false);
            }
            if self.generation() == generation && self.navigation == navigation {
                self.map.history = previous;
            }
        }
    }

    fn change_scan_option(&mut self, hidden: bool) {
        let folder = self.folder.clone();
        let previous = self.options.clone();
        if hidden {
            self.options.include_hidden = !self.options.include_hidden;
        } else {
            self.options.apparent_size = !self.options.apparent_size;
        }
        let generation = self.generation();
        self.start_over();
        if self.generation() == generation {
            self.options = previous;
        } else {
            self.folder = folder.map(|mut folder| {
                folder.state = ScanState::Scanning;
                folder
            });
            self.map.previous_result = self
                .folder
                .as_ref()
                .is_some_and(|folder| folder.hierarchy.is_some());
            self.set_scan_status(
                self.generation(),
                if self.map.previous_result {
                    "Previous result is stale. Scanning with new options…"
                } else {
                    "Scanning with new options…"
                },
            );
        }
    }

    pub(super) fn restore_map_selection(&mut self, folder: &Folder) {
        let previous = self.map.pending_selection.take().or_else(|| self.map_row());
        self.map.reset_navigation();
        if folder.state == ScanState::Scanning
            || self
                .session
                .as_ref()
                .is_some_and(|session| session.activity == super::ScanActivity::Interrupted)
        {
            self.map.pending_selection = previous.clone();
        }
        if let (Some(row), Some(root)) = (previous, folder.hierarchy.as_ref()) {
            let Ok(relative) = row.path.strip_prefix(&folder.path) else {
                return;
            };
            let mut node = root.as_ref();
            let mut crumbs = Vec::new();
            for component in relative.components() {
                let Some(index) = node
                    .children
                    .iter()
                    .position(|child| child.filesystem_name() == component.as_os_str())
                else {
                    return;
                };
                crumbs.push(index);
                node = &node.children[index];
            }
            if node.identity() == Some((row.identity.device, row.identity.inode)) {
                self.map.selected = Some(TileKind::Node { crumbs });
            }
        }
    }
    #[cfg(test)]
    pub(super) fn select_map_entry_for_test(&mut self, index: usize) {
        let root = self.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
        let kind = self.map.entries(root)[index].kind.clone();
        self.map_select(kind);
    }

    #[cfg(test)]
    pub(super) fn selected_map_entry_for_test(&self) -> usize {
        let root = self.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
        self.map
            .entries(root)
            .iter()
            .position(|entry| Some(&entry.kind) == self.map.selected.as_ref())
            .unwrap()
    }

    fn map_select(&mut self, kind: TileKind) {
        self.map.finding_cursor = None;
        self.invalidate_proposal();
        self.map.selected = Some(kind);
        self.map.pending_selection = None;
    }

    pub(super) fn map_row(&self) -> Option<disk_scan::Row> {
        let folder = self.folder.as_ref()?;
        let root = folder.hierarchy.as_ref()?;
        let TileKind::Node { crumbs } = self.map.selected.as_ref()? else {
            return None;
        };
        let node = root.resolve(crumbs)?;
        let path = node_path(root, &folder.path, crumbs)?;
        let (device, inode) = node.identity().unwrap_or_default();
        Some(disk_scan::Row {
            path,
            identity: disk_scan::FileId { device, inode },
            directory: node.is_dir(),
            allocated: node.bytes,
            descendants: node
                .files
                .saturating_add(u64::from(node.dirs))
                .saturating_sub(u64::from(node.is_dir())),
            unreadable: node.unreadable,
            shared: node.shared,
            skipped_volume: node.skipped_volume,
            in_cloud: node.in_cloud,
            cleanable: None,
        })
    }

    pub(super) fn map_key(&mut self, key: &str) -> bool {
        let Some(root) = self
            .folder
            .as_ref()
            .and_then(|folder| folder.hierarchy.clone())
        else {
            return false;
        };
        let findings = self
            .map
            .findings(&root, self.folder.as_ref().unwrap().observed);
        let locations = finding_locations(&findings);
        if key == "w" && !locations.is_empty() {
            self.map.finding_cursor = Some(0);
            self.map
                .finding_scroll
                .scroll_to_item(0, ScrollStrategy::Top);
            return true;
        }
        if let Some(index) = self.map.finding_cursor {
            match key {
                "up" | "k" | "shift-tab" | "down" | "j" | "tab" => {
                    let next = if matches!(key, "down" | "j" | "tab") {
                        (index + 1).min(locations.len().saturating_sub(1))
                    } else {
                        index.saturating_sub(1)
                    };
                    self.map.finding_cursor = Some(next);
                    self.map
                        .finding_scroll
                        .scroll_to_item(next, ScrollStrategy::Nearest);
                    return true;
                }
                "enter" => {
                    if let Some((_, crumbs, _)) = locations.get(index) {
                        self.inspect_finding(crumbs.clone());
                    }
                    return true;
                }
                "escape" => {
                    self.map.finding_cursor = None;
                    return true;
                }
                _ => self.map.finding_cursor = None,
            }
        }
        match key {
            "down" | "j" | "up" | "k" | "tab" | "shift-tab" => {
                let entries = self.map.entries(&root);
                let selected =
                    self.map.selected.as_ref().and_then(|selected| {
                        entries.iter().position(|entry| &entry.kind == selected)
                    });
                let forward = matches!(key, "down" | "j" | "tab");
                if matches!(key, "tab" | "shift-tab")
                    && (entries.is_empty()
                        || selected.is_some_and(|index| {
                            if forward {
                                index + 1 == entries.len()
                            } else {
                                index == 0
                            }
                        }))
                {
                    return false;
                }
                let next = match (selected, matches!(key, "down" | "j" | "tab")) {
                    (Some(index), true) => (index + 1).min(entries.len().saturating_sub(1)),
                    (Some(index), false) => index.saturating_sub(1),
                    (None, _) => 0,
                };
                if let Some(entry) = entries.get(next) {
                    self.map_select(entry.kind.clone());
                    self.map
                        .list_scroll
                        .scroll_to_item(next, ScrollStrategy::Nearest);
                }
            }
            "enter" | "right" | "l" => {
                if let Some(selected @ TileKind::Others { .. }) = &self.map.selected {
                    let entries = self.map.entries(&root);
                    if let Some(next) = entries
                        .iter()
                        .position(|entry| &entry.kind == selected)
                        .and_then(|index| {
                            entries
                                .get(index + 1)
                                .map(|entry| (index + 1, entry.kind.clone()))
                        })
                    {
                        self.map_select(next.1);
                        self.map
                            .list_scroll
                            .scroll_to_item(next.0, ScrollStrategy::Nearest);
                    }
                    return true;
                }
                if let Some(row) = self
                    .map_row()
                    .filter(|row| row.directory && !row.in_cloud && !row.skipped_volume)
                {
                    self.enter(row.path);
                }
            }
            "x" => {
                if let Some(row) = self.map_row() {
                    self.request_trash(&row);
                }
            }
            _ => return false,
        }
        true
    }
}

impl Monitor {
    pub(crate) fn treemap_workspace(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let disk = self.disk.as_ref().unwrap();
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let hierarchy = disk
            .folder
            .as_ref()
            .and_then(|folder| folder.hierarchy.clone());
        let selected = disk.map.selected.clone();
        let hovered = disk.map.hovered.clone();
        let measured = disk.map.frame.clone();
        let paint_frame = measured.clone();
        let mode = disk.map.mode;
        let depth = disk.map.draw_depth();
        let viewport = disk.map.viewport;
        let filter = disk.map.committed_filter.clone();
        let matches = hierarchy
            .as_ref()
            .and_then(|root| disk.map.layout_matches(root));
        let paint_matches = hierarchy.as_ref().and_then(|root| disk.map.matches(root));
        let applied = disk.map.applied;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        #[cfg(test)]
        let now = disk.map.clock.unwrap_or(now);
        let canvas = div()
            .id("disk-treemap")
            .debug_selector(|| "disk-treemap".into())
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .bg(rgb(t.well))
            .border_1()
            .border_color(rgb(t.well_rule))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    let disk = this.disk.as_mut().unwrap();
                    let hit = {
                        let frame = disk.map.frame.borrow();
                        let (x, y) = disk.map.viewport.inverse(
                            (event.position.x - frame.origin.x).as_f32(),
                            (event.position.y - frame.origin.y).as_f32(),
                        );
                        treemap::hit(&frame.tiles, x, y).map(|tile| tile.kind.clone())
                    };
                    if let Some(kind) = hit {
                        disk.map_select(kind);
                        if event.click_count == 2 {
                            disk.map_key("enter");
                        }
                    }
                    window.focus(&this.focus, cx);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                let disk = this.disk.as_mut().unwrap();
                let delta = event.delta.pixel_delta(px(20.));
                let origin = disk.map.frame.borrow().origin;
                if event.modifiers.shift {
                    disk.map.viewport.pan(delta.x.as_f32(), delta.y.as_f32());
                } else {
                    disk.map.viewport.zoom(
                        (delta.y.as_f32() * 0.002).exp(),
                        (event.position.x - origin.x).as_f32(),
                        (event.position.y - origin.y).as_f32(),
                    );
                }
                window.focus(&this.focus, cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let disk = this.disk.as_mut().unwrap();
                let hit = {
                    let frame = disk.map.frame.borrow();
                    let (x, y) = disk.map.viewport.inverse(
                        (event.position.x - frame.origin.x).as_f32(),
                        (event.position.y - frame.origin.y).as_f32(),
                    );
                    treemap::hit(&frame.tiles, x, y).map(|tile| tile.kind.clone())
                };
                if disk.map.hovered != hit {
                    disk.map.hovered = hit;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.disk.as_mut().unwrap().map.hovered = None;
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |bounds, _, _| {
                        let mut frame = measured.borrow_mut();
                        let same_root = match (&frame.root, &hierarchy) {
                            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                            (None, None) => true,
                            _ => false,
                        };
                        if !same_root
                            || frame.size != bounds.size
                            || frame.mode != mode
                            || frame.depth != depth
                            || frame.filter != format!("{applied}:{filter}")
                        {
                            frame.tiles = hierarchy.as_ref().map_or_else(Vec::new, |root| {
                                treemap::layout_filtered(
                                    root,
                                    &[],
                                    Rect::new(
                                        0.,
                                        0.,
                                        bounds.size.width.as_f32(),
                                        bounds.size.height.as_f32(),
                                    ),
                                    mode.metric(),
                                    &LayoutOptions {
                                        max_depth: depth,
                                        ..Default::default()
                                    },
                                    if applied { matches.as_deref() } else { None },
                                )
                            });
                            frame.size = bounds.size;
                            frame.root = hierarchy.clone();
                            frame.mode = mode;
                            frame.depth = depth;
                            frame.filter = format!("{applied}:{filter}");
                        }
                        frame.origin = bounds.origin;
                        let mut tiles = frame.tiles.clone();
                        for tile in &mut tiles {
                            tile.rect = viewport.rect(tile.rect);
                            tile.header = tile.header.map(|band| viewport.rect(band));
                        }
                        (hierarchy.clone(), tiles)
                    },
                    move |bounds, (root, tiles), window, cx| {
                        let Some(root) = root else { return };
                        let matches = &paint_matches;
                        #[cfg(test)]
                        {
                            let mut frame = paint_frame.borrow_mut();
                            frame.painted.clear();
                            frame.hatched.clear();
                            frame.coverage_labels.clear();
                            frame.paint_count += 1;
                        }
                        for tile in tiles {
                            let dimmed = matches
                                .as_ref()
                                .is_some_and(|found| found.keep(tile.crumbs()).is_none());
                            let age = root
                                .resolve(tile.crumbs())
                                .map(|node| age_label(node.modified, now));
                            let rect = Bounds::new(
                                bounds.origin + point(px(tile.rect.x), px(tile.rect.y)),
                                size(px(tile.rect.w), px(tile.rect.h)),
                            );
                            let edge = if selected.as_ref() == Some(&tile.kind) {
                                t.base.led
                            } else if hovered.as_ref() == Some(&tile.kind) {
                                t.rule_hi
                            } else {
                                t.rule
                            };
                            let node = root.resolve(tile.crumbs());
                            let style = crate::palette::kind_style(
                                &t,
                                node.map_or(Default::default(), |node| node.category),
                                tile.depth as usize,
                            );
                            let fill = if selected.as_ref() == Some(&tile.kind) {
                                t.selection
                            } else if dimmed {
                                t.well
                            } else if mode == Mode::Age {
                                match age {
                                    Some("Today") => t.selection,
                                    Some("This week") => {
                                        crate::palette::mix(t.base.led, t.well, 0.16)
                                    }
                                    Some("This month") => {
                                        crate::palette::mix(t.base.led, t.well, 0.1)
                                    }
                                    Some("This year") => {
                                        crate::palette::mix(t.base.hot, t.well, 0.1)
                                    }
                                    Some("Over a year") => {
                                        crate::palette::mix(t.base.hot, t.well, 0.16)
                                    }
                                    _ => t.base.plate,
                                }
                            } else {
                                style.fill
                            };
                            window.paint_quad(quad(
                                rect,
                                Corners::default(),
                                rgb(fill),
                                Edges::all(px(1.)),
                                rgb(edge),
                                BorderStyle::Solid,
                            ));
                            if !dimmed
                                && node.is_some_and(|node| node.reclaim.is_some() && !node.in_cloud)
                            {
                                let hatch_rect = tile.rect.inset(1.);
                                let clip = Bounds::new(
                                    bounds.origin + point(px(hatch_rect.x), px(hatch_rect.y)),
                                    size(px(hatch_rect.w), px(hatch_rect.h)),
                                );
                                window.with_content_mask(
                                    Some(ContentMask { bounds: clip }),
                                    |window| {
                                        #[cfg(test)]
                                        paint_frame.borrow_mut().hatched.push((
                                            tile.kind.clone(),
                                            window.content_mask().bounds,
                                        ));
                                        let mut path = PathBuilder::stroke(px(1.));
                                        for [(x1, y1), (x2, y2)] in
                                            treemap::hatch_segments(hatch_rect)
                                        {
                                            path.move_to(bounds.origin + point(px(x1), px(y1)));
                                            path.line_to(bounds.origin + point(px(x2), px(y2)));
                                        }
                                        if let Ok(path) = path.build() {
                                            let hatch = if fill == style.fill {
                                                style.hatch
                                            } else {
                                                crate::palette::mix(
                                                    crate::palette::readable_text(
                                                        fill,
                                                        t.base.silk,
                                                    ),
                                                    fill,
                                                    0.10,
                                                )
                                            };
                                            window.paint_path(path, rgb(hatch));
                                        }
                                    },
                                );
                            }
                            if let Some(header) = tile.header {
                                window.paint_quad(gpui::fill(
                                    Bounds::new(
                                        bounds.origin + point(px(header.x + 1.), px(header.y + 1.)),
                                        size(px(3.), px((header.h - 2.).max(0.))),
                                    ),
                                    rgb(style.strip),
                                ));
                            }
                            let text = if fill == style.fill {
                                style.text
                            } else {
                                crate::palette::readable_text(fill, t.base.silk)
                            };
                            let (name, size_text) = match &tile.kind {
                                TileKind::Node { crumbs } => root.resolve(crumbs).map_or(
                                    (String::new(), String::new()),
                                    |node| {
                                        (node.name.to_string(), {
                                            let value = if mode == Mode::Age {
                                                format!(
                                                    "{} · {}",
                                                    bytes(node.bytes),
                                                    age_label(node.modified, now)
                                                )
                                            } else if mode == Mode::Files {
                                                format!("{} files", node.files)
                                            } else {
                                                bytes(node.bytes)
                                            };
                                            if let Some(tag) = coverage_tag(node) {
                                                format!("{tag} · {value}")
                                            } else {
                                                value
                                            }
                                        })
                                    },
                                ),
                                TileKind::Others { count, .. } => {
                                    (format!("Other · {count} items"), String::new())
                                }
                            };
                            #[cfg(test)]
                            paint_frame.borrow_mut().painted.push((
                                tile.kind.clone(),
                                tile.rect,
                                size_text.clone(),
                                fill,
                            ));
                            let band = tile.header.unwrap_or(tile.rect);
                            let mask = Bounds::new(
                                bounds.origin + point(px(band.x), px(band.y)),
                                size(px(band.w), px(band.h)),
                            );
                            let minimum_height = if tile.header.is_some() { 12. } else { 16. };
                            if mask.size.width < px(44.) || mask.size.height < px(minimum_height) {
                                continue;
                            }
                            let size_line = (tile.header.is_some()
                                || node.is_some_and(|node| !coverage(node).is_empty()))
                            .then(|| {
                                window.text_system().shape_line(
                                    size_text.clone().into(),
                                    px(11.),
                                    &[TextRun {
                                        len: size_text.len(),
                                        font: font("JetBrains Mono"),
                                        color: rgb(text).into(),
                                        ..Default::default()
                                    }],
                                    None,
                                )
                            });
                            let size_line = size_line.map(|line| {
                                if line.width > mask.size.width - px(8.) {
                                    if let Some(tag) = node.and_then(coverage_tag) {
                                        return window.text_system().shape_line(
                                            tag.into(),
                                            px(11.),
                                            &[TextRun {
                                                len: tag.len(),
                                                font: font("JetBrains Mono"),
                                                color: rgb(text).into(),
                                                ..Default::default()
                                            }],
                                            None,
                                        );
                                    }
                                }
                                line
                            });
                            let label = if size_line.is_some() {
                                name
                            } else {
                                format!("{name} {size_text}")
                            };
                            let line = window.text_system().shape_line(
                                label.clone().into(),
                                px(if tile.depth == 0 { 12. } else { 11. }),
                                &[TextRun {
                                    len: label.len(),
                                    font: font("JetBrains Mono"),
                                    color: rgb(text).into(),
                                    ..Default::default()
                                }],
                                None,
                            );
                            let mut name_mask = mask;
                            if let Some(line) = &size_line {
                                name_mask.size.width =
                                    (mask.size.width - line.width - px(12.)).max(px(0.));
                            }
                            window.with_content_mask(
                                Some(ContentMask { bounds: name_mask }),
                                |window| {
                                    let _ = line.paint(
                                        mask.origin + point(px(6.), px(1.)),
                                        px(17.),
                                        TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    );
                                },
                            );
                            if let Some(line) = size_line {
                                #[cfg(test)]
                                if let Some(tag) = node.and_then(coverage_tag) {
                                    assert!(line.width <= mask.size.width - px(8.));
                                    assert!(line.text.starts_with(tag));
                                    paint_frame.borrow_mut().coverage_labels.push((
                                        tile.kind.clone(),
                                        line.text.to_string(),
                                        mask,
                                    ));
                                }
                                window.with_content_mask(
                                    Some(ContentMask { bounds: mask }),
                                    |window| {
                                        let _ = line.paint(
                                            point(
                                                mask.right() - line.width - px(4.),
                                                mask.top() + px(1.),
                                            ),
                                            px(17.),
                                            TextAlign::Left,
                                            None,
                                            window,
                                            cx,
                                        );
                                    },
                                );
                            }
                        }
                        // The selection edge stays visible over descendants.
                        if let Some(tile) = paint_frame
                            .borrow()
                            .tiles
                            .iter()
                            .find(|tile| selected.as_ref() == Some(&tile.kind))
                        {
                            let transformed = viewport.rect(tile.rect);
                            let rect = Bounds::new(
                                bounds.origin + point(px(transformed.x), px(transformed.y)),
                                size(px(transformed.w), px(transformed.h)),
                            );
                            window.paint_quad(quad(
                                rect,
                                Corners::default(),
                                transparent_black(),
                                Edges::all(px(2.)),
                                rgb(t.base.led),
                                BorderStyle::Solid,
                            ));
                        }
                    },
                )
                .size_full(),
            );
        let path = disk.folder.as_ref().map_or_else(
            || self.disk.as_ref().unwrap().status.clone(),
            |folder| {
                format!(
                    "{}{}",
                    folder.path.display(),
                    if disk.map.previous_result {
                        " · Previous result"
                    } else {
                        ""
                    }
                )
            },
        );
        let mut crumbs = div().flex().flex_wrap().gap(px(4.));
        if let Some(folder) = &disk.folder {
            let mut ancestors: Vec<_> = folder
                .path
                .ancestors()
                .take_while(|path| path.starts_with(&folder.root))
                .collect();
            ancestors.reverse();
            for (index, path) in ancestors.into_iter().enumerate() {
                let path = path.to_path_buf();
                let sibling_path = path.clone();
                let label = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned();
                crumbs = crumbs.child(
                    Button::new(("map-crumb", index))
                        .label(label)
                        .max_w(px(180.))
                        .rack(&t)
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.disk.as_mut().unwrap().enter(path.clone());
                            window.focus(&this.focus, cx);
                            cx.notify();
                        })),
                );
                crumbs = crumbs.child(
                    div()
                        .debug_selector(move || format!("map-siblings-{index}"))
                        .child(
                            Button::new(("map-siblings", index))
                                .label("▾")
                                .rack(&t)
                                .small()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let map = &mut this.disk.as_mut().unwrap().map;
                                    map.sibling_index = 0;
                                    map.siblings = if map.siblings.as_ref() == Some(&sibling_path) {
                                        None
                                    } else {
                                        Some(sibling_path.clone())
                                    };
                                    window.focus(&this.focus, cx);
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        let siblings = disk.map.siblings.as_ref().map(|path| {
            let paths = disk.map_siblings(path);
            let selected = disk.map.sibling_index;
            div()
                .id("map-sibling-menu")
                .debug_selector(|| "map-sibling-menu".into())
                .h(px(160.))
                .flex_shrink_0()
                .overflow_hidden()
                .child(
                    uniform_list(
                        "map-sibling-rows",
                        paths.len(),
                        cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| {
                                    let path = paths[index].clone();
                                    div()
                                        .debug_selector(move || {
                                            format!("map-sibling-entry-{index}")
                                        })
                                        .h(px(28.))
                                        .w_full()
                                        .bg(rgb(if selected == index {
                                            t.selection
                                        } else {
                                            t.base.plate
                                        }))
                                        .cursor_pointer()
                                        .child(
                                            path.file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .into_owned(),
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, window, cx| {
                                                this.disk.as_mut().unwrap().enter(path.clone());
                                                window.focus(&this.focus, cx);
                                                cx.stop_propagation();
                                                cx.notify();
                                            }),
                                        )
                                })
                                .collect()
                        }),
                    )
                    .track_scroll(&disk.map.sibling_scroll)
                    .size_full(),
                )
        });
        let hover = disk
            .map
            .hovered
            .as_ref()
            .and_then(|kind| {
                let TileKind::Node { crumbs } = kind else {
                    return None;
                };
                let folder = disk.folder.as_ref()?;
                node_path(folder.hierarchy.as_ref()?, &folder.path, crumbs)
                    .map(|path| path.display().to_string())
            })
            .unwrap_or_default();
        let mut controls = div().flex().flex_wrap().gap(px(4.));
        for (index, mode) in [Mode::Size, Mode::Files, Mode::Age].into_iter().enumerate() {
            controls = controls.child(
                div()
                    .debug_selector(move || format!("map-mode-{index}"))
                    .child(
                        Button::new(("map-mode", index))
                            .label(format!(
                                "{}{:?}",
                                if disk.map.mode == mode { "● " } else { "" },
                                mode
                            ))
                            .rack(&t)
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.disk.as_mut().unwrap().set_map_mode(mode);
                                window.focus(&this.focus, cx);
                                cx.notify();
                            })),
                    ),
            );
        }
        for (id, label, key) in [
            ("map-depth-less", "Depth −".to_string(), "["),
            (
                "map-depth-more",
                format!("{} +", disk.map.draw_depth()),
                "]",
            ),
            ("map-filter", format!("/ Filter {}", disk.map.filter), "/"),
            ("map-back", "Back".into(), "b"),
            ("map-forward", "Forward".into(), "f"),
            ("map-reset", "Reset 0".into(), "0"),
        ] {
            controls = controls.child(
                div().debug_selector(move || id.into()).child(
                    Button::new(id)
                        .label(label)
                        .rack(&t)
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let disk = this.disk.as_mut().unwrap();
                            disk.map.finding_cursor = None;
                            disk.map_command(key);
                            window.focus(&this.focus, cx);
                            cx.notify();
                        })),
                ),
            );
        }
        for (id, label, hidden) in [
            (
                "map-hidden",
                format!(
                    "Hidden {}",
                    if disk.options.include_hidden {
                        "on"
                    } else {
                        "off"
                    }
                ),
                true,
            ),
            (
                "map-apparent",
                format!(
                    "Apparent {}",
                    if disk.options.apparent_size {
                        "on"
                    } else {
                        "off"
                    }
                ),
                false,
            ),
        ] {
            controls = controls.child(
                div().debug_selector(move || id.into()).child(
                    Button::new(id)
                        .label(label)
                        .rack(&t)
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.disk.as_mut().unwrap().change_scan_option(hidden);
                            window.focus(&this.focus, cx);
                            cx.notify();
                        })),
                ),
            );
        }
        controls = controls.child(
            div().debug_selector(|| "map-choose-folder".into()).child(
                Button::new("map-choose-folder")
                    .label("Choose folder")
                    .rack(&t)
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        let request = cx.prompt_for_paths(PathPromptOptions {
                            files: false,
                            directories: true,
                            multiple: false,
                            prompt: Some("Scan folder".into()),
                        });
                        cx.spawn(async move |entity, cx| {
                            if let Ok(Ok(Some(paths))) = request.await {
                                if let Some(path) = paths.into_iter().next() {
                                    let _ = entity.update(cx, |this, cx| {
                                        this.disk.as_mut().unwrap().start(path, false);
                                        cx.notify();
                                    });
                                }
                            }
                        })
                        .detach();
                    })),
            ),
        );
        let matched = disk
            .folder
            .as_ref()
            .and_then(|folder| folder.hierarchy.as_ref())
            .and_then(|root| disk.map.matches(root))
            .map(|found| {
                format!(
                    "{} matched bytes · {} scan bytes{}",
                    found.bytes,
                    disk.scan_bytes.unwrap_or(0),
                    if disk.map.filtering {
                        " · Enter applies"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_default();
        let mut legend = div()
            .debug_selector(|| "map-kind-legend".into())
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .text_size(px(10.));
        for kind in gtop_disk::classify::Category::ALL {
            let style = crate::palette::kind_style(&t, kind, 0);
            legend = legend.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(div().w(px(8.)).h(px(8.)).bg(rgb(style.strip)))
                    .child(kind.label()),
            );
        }
        legend = legend.child("//// Rebuildable or reclaimable · inspect before cleanup");
        div().flex().size_full().gap(px(8.)).min_w_0()
            .child(div().flex().flex_col().flex_1().min_w_0().min_h_0().gap(px(8.))
                .child(div().bg(rgb(t.base.plate)).border_1().border_color(rgb(t.rule_hi)).p(px(12.))
                    .child(crate::rack::label(if disk.options.apparent_size { "TREEMAP · APPARENT SIZE" } else { "TREEMAP · ALLOCATED BLOCKS" }, &t))
                    .child(current_path(path)).child(crumbs).children(siblings).child(controls).child(div().text_size(px(10.)).child(matched)))
                .child(legend)
                .child(canvas)
                .child(div().h(px(20.)).overflow_hidden().text_size(px(10.)).child(hover))
                .child(div().bg(rgb(t.base.plate)).border_1().border_color(rgb(t.rule)).p(px(8.))
                    .child("D Top · arrows/HJKL move · Tab list/controls · Esc canvas · Enter open · Backspace up · X Trash")
                    .child("M mode · / filter · [ ] depth · + − zoom · 0 reset · B/F history")
                    .child(div().text_size(px(10.)).child("Observed sizes. Unknown space has no tile area. Hover informs; click selects."))))
            .child(self.resizable_disk_inspector(window, cx))
    }

    fn resizable_disk_inspector(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let width = self
            .settings
            .disk_inspector_width
            .for_window(window.bounds().size.width.into());
        let handle = crate::column_widths::handle_left()
            .debug_selector(|| "disk-inspector-resize".into())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let disk = this.disk.as_mut().unwrap();
                    if event.click_count == 2 {
                        disk.inspector_resize.finish();
                        this.settings.disk_inspector_width = Default::default();
                        this.persist_settings(window, cx);
                    } else {
                        disk.inspector_resize.begin(
                            event.position.x.into(),
                            this.settings
                                .disk_inspector_width
                                .for_window(window.bounds().size.width.into()),
                        );
                    }
                    window.focus(&this.focus, cx);
                    cx.notify();
                }),
            );
        let events = crate::column_widths::events(
            cx,
            self.disk.as_ref().unwrap().inspector_resize.active(),
            |_, _, _, _| {},
            |this, event, window, cx| {
                if let Some(width) = this
                    .disk
                    .as_ref()
                    .unwrap()
                    .inspector_resize
                    .width_at(event.position.x.into())
                {
                    this.settings.disk_inspector_width =
                        width.for_window(window.bounds().size.width.into());
                    cx.stop_propagation();
                    cx.notify();
                }
            },
            |this, _, window, cx| {
                if this.disk.as_mut().unwrap().inspector_resize.finish() {
                    this.persist_settings(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            },
        );
        div()
            .relative()
            .h_full()
            .flex_shrink_0()
            .w(px(width.pixels()))
            .children(self.disk_panel(crate::details::DetailWidth::new(width.pixels()), cx))
            .child(handle)
            .child(events)
    }

    pub(super) fn treemap_inspector(&self, cx: &mut Context<Self>) -> Div {
        let disk = self.disk.as_ref().unwrap();
        let folder = disk.folder.as_ref().unwrap();
        let root = folder.hierarchy.as_ref().unwrap();
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let mut selection = div()
            .id("disk-selection")
            .debug_selector(|| "disk-selection".into())
            .flex_shrink_0()
            .min_w_0()
            .overflow_hidden()
            .py(px(8.))
            .child(crate::rack::label("SELECTION", &t));
        if let Some(row) = disk.map_row() {
            selection = selection
                .child(
                    div()
                        .id("disk-selection-name")
                        .debug_selector(|| "disk-selection-name".into())
                        .min_w_0()
                        .font_family("Archivo Display")
                        .text_size(px(24.))
                        .truncate()
                        .child(
                            row.path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        ),
                )
                .child(current_path(row.path.display().to_string()))
                .child(crate::rack::lit(bytes(row.allocated), 42., false, &t))
                .child(
                    div()
                        .text_size(px(11.))
                        .child(format!("{} bytes", row.allocated)),
                )
                .children(
                    disk.selection_details()
                        .into_iter()
                        .map(|text| div().min_w_0().text_size(px(10.)).child(text)),
                )
                .child(
                    Button::new("map-trash")
                        .label("Review cleanup · X")
                        .rack(&t)
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.disk.as_mut().unwrap().request_trash(&row);
                            window.focus(&this.focus, cx);
                            cx.notify();
                        })),
                );
        } else if let Some(TileKind::Others { count, .. }) = &disk.map.selected {
            selection = selection
                .child(format!("Other · {count} items"))
                .child("Grouped items cannot be moved to Trash as one item.");
        } else {
            selection = selection.child("Click a tile or choose a folder below.");
        }
        let entries = disk.map.entries(root);
        let findings = disk.map.findings(root, folder.observed);
        let finding_rows = finding_locations(&findings);
        let has_findings = !finding_rows.is_empty();
        let finding_root = root.clone();
        let finding_base = folder.path.clone();
        let finding_cursor = disk.map.finding_cursor;
        let finding_scan_bytes = root.bytes;
        let finding_list = uniform_list(
            "disk-findings-rows",
            finding_rows.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| {
                        let (finding_index, crumbs, child) = &finding_rows[index];
                        let finding = &findings[*finding_index];
                        let node = finding_root.resolve(crumbs).unwrap();
                        let display = if *child {
                            crumbs.as_slice()
                        } else {
                            finding.crumbs.as_slice()
                        };
                        let path = node_path(&finding_root, &finding_base, display).unwrap();
                        let shown_bytes = if *child { node.bytes } else { finding.bytes };
                        let reason = if *child {
                            "Matching old experiment · inspect before cleanup".to_string()
                        } else {
                            finding.reason()
                        };
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        let title = if !*child && !finding.children.is_empty() {
                            format!("{name} · old subtotal")
                        } else {
                            name.into_owned()
                        };
                        let colour = crate::palette::kind_style(&t, node.category, 0).strip;
                        let label = format!(
                            "{} · {} · {} · {}",
                            path.display(),
                            bytes(shown_bytes),
                            reason,
                            gtop_disk::classify::description(node)
                        );
                        let target = crumbs.clone();
                        div()
                            .id(("disk-finding", index))
                            .debug_selector(move || format!("disk-finding-{index}"))
                            .role(Role::ListItem)
                            .aria_label(label)
                            .aria_selected(finding_cursor == Some(index))
                            .h(px(76.))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .border_1()
                            .border_color(rgb(if finding_cursor == Some(index) {
                                t.base.led
                            } else {
                                t.rule
                            }))
                            .bg(rgb(if finding_cursor == Some(index) {
                                t.selection
                            } else {
                                t.base.plate
                            }))
                            .px(px(if *child { 10. } else { 4. }))
                            .py(px(3.))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(4.))
                                    .justify_between()
                                    .child(
                                        div().min_w_0().truncate().text_size(px(11.)).child(title),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(11.))
                                            .child(bytes(shown_bytes)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(t.silk_dim))
                                    .child(reason),
                            )
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .truncate()
                                    .child(path.display().to_string()),
                            )
                            .child(
                                div().h(px(3.)).w_full().bg(rgb(t.well)).child(
                                    div()
                                        .h_full()
                                        .w(relative(if finding_scan_bytes == 0 {
                                            0.
                                        } else {
                                            (shown_bytes as f32 / finding_scan_bytes as f32)
                                                .clamp(0., 1.)
                                        }))
                                        .bg(rgb(colour)),
                                ),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.disk.as_mut().unwrap().inspect_finding(target.clone());
                                    window.focus(&this.focus, cx);
                                    cx.notify();
                                }),
                            )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&disk.map.finding_scroll)
        .flex_1()
        .min_h_0();
        let selected = disk.map.selected.clone();
        let list = uniform_list(
            "map-folder-rows",
            entries.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| {
                        let entry = &entries[index];
                        let kind = entry.kind.clone();
                        let active = selected.as_ref() == Some(&kind);
                        let other = matches!(kind, TileKind::Others { .. });
                        div()
                            .id(("map-entry", index))
                            .debug_selector(move || {
                                if other {
                                    "map-entry-other".into()
                                } else {
                                    format!("map-entry-{index}")
                                }
                            })
                            .role(Role::ListItem)
                            .aria_label(entry.label.clone())
                            .aria_selected(active)
                            .h(px(26.))
                            .flex_shrink_0()
                            .pl(px(4. + entry.depth as f32 * 10.))
                            .pr(px(4.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .bg(rgb(if active { t.selection } else { t.base.plate }))
                            .child(entry.visible_label.as_ref().unwrap_or(&entry.label).clone())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.disk.as_mut().unwrap().map_select(kind.clone());
                                    window.focus(&this.focus, cx);
                                    cx.notify();
                                }),
                            )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&disk.map.list_scroll)
        .flex_1()
        .min_h_0();
        div()
            .flex()
            .flex_col()
            .h_full()
            .min_h_0()
            .px(px(12.))
            .gap(px(6.))
            .child(selection)
            .child(crate::rack::label(
                format!("{:?} · {} observed", folder.state, bytes(folder.allocated)),
                &t,
            ))
            .child(div().text_size(px(10.)).child(format!(
                "{} unreadable entries · incomplete sizes can contain more",
                folder.errors
            )))
            .child(crate::rack::label("WORTH A LOOK · W", &t))
            .when(has_findings, |this| {
                this.child(
                    div()
                        .id("disk-findings")
                        .debug_selector(|| "disk-findings".into())
                        .role(Role::List)
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(76.))
                        .child(finding_list),
                )
            })
            .when(!has_findings, |this| {
                this.child(
                    div()
                        .text_size(px(10.))
                        .child("No large inspection suggestions."),
                )
            })
            .when(has_findings, |this| {
                this.child(
                    div()
                        .text_size(px(10.))
                        .child("W findings · ↑↓ choose · Enter inspect · Esc return"),
                )
            })
            .child(crate::rack::label("FOLDERS AND FILES", &t))
            .child(
                div()
                    .id("map-folder-list")
                    .debug_selector(|| "map-folder-list".into())
                    .role(Role::List)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(26.))
                    .child(list),
            )
            .child(self.disk_volume_readout())
    }

    fn disk_volume_readout(&self) -> Stateful<Div> {
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let disk = self.disk.as_ref().unwrap();
        let volume = disk.volume.as_ref();
        let space = volume.and_then(|v| v.space);
        let identity = volume.map_or("Unknown", |v| v.identity_label());
        let identity_selector = identity.to_string();
        let space_selector = space.map_or_else(
            || "Free Unknown · Used Unknown · Total Unknown".into(),
            |v| {
                format!(
                    "{} free · {} used · {} total",
                    bytes(v.available),
                    bytes(v.used()),
                    bytes(v.total)
                )
            },
        );
        let free = space.map_or_else(
            || "Unknown".into(),
            |v| format!("{} free", bytes(v.available)),
        );
        div()
            .id("disk-volume")
            .debug_selector(|| "disk-volume".into())
            .flex_shrink_0()
            .min_w_0()
            .overflow_hidden()
            .border_t_1()
            .border_color(rgb(t.rule))
            .py(px(8.))
            .child(crate::rack::label("SCAN VOLUME", &t))
            .child(
                div()
                    .id("disk-volume-identity")
                    .debug_selector(move || identity_selector.clone())
                    .role(Role::Label)
                    .aria_label(identity.to_string())
                    .truncate()
                    .text_size(px(10.))
                    .child(identity.to_string()),
            )
            .child(crate::rack::lit(free, 22., false, &t))
            .child(
                div()
                    .id("disk-volume-space")
                    .debug_selector(move || space_selector.clone())
                    .role(Role::Label)
                    .aria_label(space.map_or_else(
                        || "Free Unknown · Used Unknown · Total Unknown".into(),
                        |v| {
                            format!(
                                "{} free · {} used · {} total",
                                bytes(v.available),
                                bytes(v.used()),
                                bytes(v.total)
                            )
                        },
                    ))
                    .text_size(px(10.))
                    .child(space.map_or_else(
                        || "Used Unknown · Total Unknown".into(),
                        |v| format!("{} used · {} total", bytes(v.used()), bytes(v.total)),
                    )),
            )
    }
}

impl Disk {
    fn selection_details(&self) -> Vec<String> {
        let Some(folder) = &self.folder else {
            return vec![];
        };
        let Some(root) = &folder.hierarchy else {
            return vec![];
        };
        let Some(TileKind::Node { crumbs }) = &self.map.selected else {
            return vec![];
        };
        let Some(node) = root.resolve(crumbs) else {
            return vec![];
        };
        let parent = crumbs
            .split_last()
            .and_then(|(_, parent)| root.resolve(parent));
        let parent_bytes = parent.map(|node| node.bytes);
        let scan = self
            .scan_bytes
            .or_else(|| (folder.path == folder.root).then_some(root.bytes));
        let scan_share = scan.map_or_else(
            || "Unknown".into(),
            |denom| inspector_model::share(node.bytes, denom),
        );
        let mut coverage = vec![];
        if node.in_cloud {
            coverage.push("Cloud content was not read".to_string());
        }
        if node.skipped_volume {
            coverage.push("Other volume was not scanned".to_string());
        }
        if !node.complete || node.read_error {
            coverage.push("Incomplete; size can contain more".to_string());
        }
        if node.unreadable > 0 {
            coverage.push(format!("{} unreadable entries", node.unreadable));
        }
        if node.shared {
            coverage.push("Counted elsewhere".to_string());
        }
        if coverage.is_empty() {
            coverage.push("Complete observed coverage".to_string());
        }
        let last_write = if node.modified <= 0 {
            "Unknown".into()
        } else {
            chrono::DateTime::from_timestamp(node.modified, 0).map_or_else(
                || "Unknown".into(),
                |date| date.format("%Y-%m-%d %H:%M UTC").to_string(),
            )
        };
        vec![
            format!(
                "{} of parent · {} bytes",
                parent_bytes.map_or_else(
                    || "Unknown".into(),
                    |v| inspector_model::share(node.bytes, v)
                ),
                parent_bytes.map_or_else(|| "Unknown".into(), |v| v.to_string())
            ),
            format!(
                "{scan_share} of scan root · {}",
                scan.map_or_else(|| "Unknown bytes".into(), |v| format!("{v} bytes"))
            ),
            format!("{} files · {} directories", node.files, node.dirs),
            format!("Newest subtree write · {last_write}"),
            format!("Kind · {}", gtop_disk::classify::description(node)),
            coverage.join(" · "),
        ]
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) fn capture_test_build(output: PathBuf) {
    tests::capture(output);
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    use std::{fs, time::Duration};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "gtop-treemap-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            fs::create_dir_all(path.join("projects/gtop/target")).unwrap();
            fs::write(path.join("projects/gtop/target/binary"), vec![1; 2 << 20]).unwrap();
            fs::write(path.join("projects/gtop/target/symbols"), vec![1; 1 << 20]).unwrap();
            fs::write(path.join("projects/gtop/equal-a"), vec![1; 128 << 10]).unwrap();
            fs::write(path.join("projects/gtop/equal-b"), vec![1; 128 << 10]).unwrap();
            fs::write(path.join("projects/gtop/zero"), []).unwrap();
            fs::create_dir_all(path.join("tiny")).unwrap();
            for i in 0..160 {
                fs::write(path.join("tiny").join(format!("file-{i:03}")), [1; 4096]).unwrap();
            }
            fs::create_dir_all(path.join("a-very-long-directory-name-".repeat(7))).unwrap();
            fs::write(
                path.join("a-very-long-directory-name-".repeat(7))
                    .join("document"),
                vec![1; 512 << 10],
            )
            .unwrap();
            fs::create_dir_all(path.join("unknown")).unwrap();
            fs::write(path.join("unknown/observed"), vec![1; 32 << 10]).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn shared_pair() -> Self {
            let path = std::env::temp_dir().join(format!(
                "gtop-treemap-shared-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("a"), [1; 4096]).unwrap();
            fs::hard_link(path.join("a"), path.join("b")).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn disk(&self) -> Disk {
            let mut disk = Disk::with_home(self.0.clone());
            wait_folder(&mut disk, &self.0);
            let folder = disk.folder.as_mut().unwrap();
            let root = Arc::make_mut(folder.hierarchy.as_mut().unwrap());
            let unknown = root
                .children
                .iter_mut()
                .find(|node| &*node.name == "unknown")
                .unwrap();
            unknown.complete = false;
            unknown.unreadable = 1;
            folder.errors = 1;
            disk
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn wait_folder(disk: &mut Disk, path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            disk.consume_mailbox();
            if disk
                .folder
                .as_ref()
                .is_some_and(|folder| folder.path == path && folder.state == ScanState::Complete)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "folder scan did not finish: {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn init(cx: &mut App) {
        gpui_component::init(cx);
        crate::theme::init(cx);
    }

    #[gpui::test]
    async fn kinds_narrow_cloud_tile_keeps_coverage_outside_filename(cx: &mut TestAppContext) {
        cx.update(init);
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(kinds_snapshot_disk());
            monitor
        });
        for (width, height) in [(1200., 720.), (1480., 930.)] {
            visual.simulate_resize(size(px(width), px(height)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            monitor.read_with(visual, |monitor, _| {
                let disk = monitor.disk.as_ref().unwrap();
                let frame = disk.map.frame.borrow();
                let root = frame.root.as_ref().unwrap();
                let cloud = frame
                    .tiles
                    .iter()
                    .find(|tile| {
                        root.resolve(tile.crumbs())
                            .is_some_and(|node| &*node.name == "CloudStorage")
                    })
                    .unwrap();
                let (_, tag, mask) = frame
                    .coverage_labels
                    .iter()
                    .find(|(kind, _, _)| *kind == cloud.kind)
                    .unwrap();
                assert!(tag.starts_with("CLOUD"));
                assert!(mask.size.width >= px(44.));
            });
        }
        monitor.update(visual, |monitor, _| monitor.disk = None);
    }

    #[gpui::test]
    async fn shared_hardlink_tile_shows_counted_elsewhere_in_files_mode(cx: &mut TestAppContext) {
        cx.update(init);
        let fixture = Fixture::shared_pair();
        let mut disk = Disk::with_home(fixture.0.clone());
        wait_folder(&mut disk, &fixture.0);
        disk.map.mode = Mode::Files;
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            let frame = disk.map.frame.borrow();
            let root = frame.root.as_ref().unwrap();
            let (index, shared) = root
                .children
                .iter()
                .enumerate()
                .find(|(_, node)| node.shared)
                .unwrap();
            assert_eq!(shared.bytes, 0);
            assert_eq!(shared.files, 1);
            let kind = TileKind::Node {
                crumbs: vec![index],
            };
            let (_, label, _) = frame
                .coverage_labels
                .iter()
                .find(|(tile, _, _)| *tile == kind)
                .unwrap();
            assert!(label.starts_with("SHARED · 1 files"), "{label}");
        });
        monitor.update(visual, |monitor, _| monitor.disk = None);
        drop(fixture);
    }

    #[gpui::test]
    async fn kinds_palette_changes_repaint_without_rescan_and_keep_focus_then_settle(
        cx: &mut TestAppContext,
    ) {
        cx.update(init);
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(kinds_snapshot_disk());
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("map-kind-legend").is_some());
        let original = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            (
                disk.folder
                    .as_ref()
                    .unwrap()
                    .hierarchy
                    .as_ref()
                    .unwrap()
                    .clone(),
                disk.generation(),
            )
        });
        let click = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            let frame = disk.map.frame.borrow();
            let tile = frame
                .tiles
                .iter()
                .find(|tile| {
                    tile.crumbs().len() > 1
                        && original
                            .0
                            .resolve(tile.crumbs())
                            .is_some_and(|node| &*node.name == "target")
                })
                .unwrap();
            let band = tile.header.unwrap_or(tile.rect);
            frame.origin + point(px(band.x + band.w / 2.), px(band.y + band.h / 2.))
        });
        visual.simulate_click(click, Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let stable = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            disk.map
                .frame
                .borrow()
                .painted
                .iter()
                .find(|tile| {
                    Some(&tile.0) != disk.map.selected.as_ref()
                        && original
                            .0
                            .resolve(match &tile.0 {
                                TileKind::Node { crumbs } => crumbs,
                                _ => return false,
                            })
                            .is_some()
                })
                .unwrap()
                .clone()
        });
        monitor.update(visual, |monitor, cx| {
            monitor.settings.active = 4;
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert!(Arc::ptr_eq(
                &original.0,
                disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap()
            ));
            assert_eq!(original.1, disk.generation());
            let frame = disk.map.frame.borrow();
            let witness = frame
                .painted
                .iter()
                .find(|tile| tile.0 == stable.0)
                .unwrap();
            let layout = frame
                .tiles
                .iter()
                .find(|tile| tile.kind == stable.0)
                .unwrap();
            let node = original.0.resolve(layout.crumbs()).unwrap();
            let expected = crate::palette::kind_style(
                &crate::palette::presets()[4].base.tokens(),
                node.category,
                layout.depth as usize,
            )
            .fill;
            assert_eq!(
                witness.3, expected,
                "unselected tile must use the new palette"
            );
            assert_ne!(stable.3, witness.3);
            assert!(disk.selection_details()[4].contains("build output"));
            let frame = disk.map.frame.borrow();
            assert!(!frame.hatched.is_empty());
            for (kind, clip) in &frame.hatched {
                let tile = frame.painted.iter().find(|tile| &tile.0 == kind).unwrap();
                let inset = tile.1.inset(1.);
                let expected = Bounds::new(
                    frame.origin + point(px(inset.x), px(inset.y)),
                    size(px(inset.w), px(inset.h)),
                );
                for (actual, expected) in [
                    (clip.left(), expected.left()),
                    (clip.top(), expected.top()),
                    (clip.right(), expected.right()),
                    (clip.bottom(), expected.bottom()),
                ] {
                    assert!(
                        (f32::from(actual) - f32::from(expected)).abs() < 0.0001,
                        "applied hatch mask differs: {clip:?} vs {expected:?}"
                    );
                }
                let layout_tile = frame.tiles.iter().find(|tile| &tile.kind == kind).unwrap();
                let node = original.0.resolve(layout_tile.crumbs()).unwrap();
                assert!(!node.in_cloud);
                assert!(node.reclaim.is_some());
            }
            for name in ["target", "CloudStorage"] {
                let index = original
                    .0
                    .children
                    .iter()
                    .position(|node| &*node.name == name)
                    .unwrap();
                assert!(!frame.hatched.iter().any(|(kind, _)| *kind
                    == TileKind::Node {
                        crumbs: vec![index]
                    }));
            }
            let entries = disk.map.entries(&original.0);
            for entry in entries.iter() {
                if let TileKind::Node { crumbs } = &entry.kind {
                    assert!(!entry
                        .visible_label
                        .as_ref()
                        .unwrap()
                        .contains("inspect before cleanup"));
                    assert!(entry.label.contains(&gtop_disk::classify::description(
                        original.0.resolve(crumbs).unwrap()
                    )));
                }
            }
        });
        visual.run_until_parked();
        // Public key dispatch draws dirty windows before it delivers the unhandled key.
        for _ in 0..3 {
            visual.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse("f24").unwrap(),
            });
            visual.run_until_parked();
        }
        let before = monitor.read_with(visual, |monitor, _| {
            monitor
                .disk
                .as_ref()
                .unwrap()
                .map
                .frame
                .borrow()
                .paint_count
        });
        for _ in 0..120 {
            visual.update(|window, cx| assert_eq!(window.simulate_next_frame(cx), 0));
            visual.executor().advance_clock(Duration::from_millis(16));
            visual.run_until_parked();
            visual.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse("f24").unwrap(),
            });
            visual.run_until_parked();
        }
        assert_eq!(
            before,
            monitor.read_with(visual, |monitor, _| monitor
                .disk
                .as_ref()
                .unwrap()
                .map
                .frame
                .borrow()
                .paint_count)
        );
        monitor.update(visual, |monitor, _| monitor.disk = None);
    }

    #[gpui::test]
    async fn fix1_filter_cache_age_and_painted_zoom(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = age_snapshot_disk();
        disk.map.mode = Mode::Age;
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        monitor.read_with(visual, |monitor, _| {
            let t = monitor.settings.base().tokens();
            let disk = monitor.disk.as_ref().unwrap();
            let frame = disk.map.frame.borrow();
            let root = disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
            for (index, label, fill) in [
                (0, "Today", t.selection),
                (
                    1,
                    "This week",
                    crate::palette::mix(t.base.led, t.well, 0.16),
                ),
                (
                    2,
                    "This month",
                    crate::palette::mix(t.base.led, t.well, 0.1),
                ),
                (3, "Unknown age", t.base.plate),
            ] {
                let kind = TileKind::Node {
                    crumbs: vec![index],
                };
                let painted = frame.painted.iter().find(|tile| tile.0 == kind).unwrap();
                assert!(
                    painted.2.contains(label),
                    "{}: {}",
                    root.children[index].name,
                    painted.2
                );
                assert_eq!(painted.3, fill, "age fill {label}");
            }
        });
        for key in ["/", "n", "o", "m", "a", "t", "c", "h", "enter", "/", "z"] {
            visual.simulate_keystrokes(key);
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
        }
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let searches = monitor.read_with(visual, |monitor, _| {
            monitor.disk.as_ref().unwrap().map.filter_searches.get()
        });
        for _ in 0..5 {
            monitor.update(visual, |_, cx| cx.notify());
            visual.update(|window, cx| window.draw(cx).clear(cx));
        }
        monitor.read_with(visual, |monitor, _| {
            assert_eq!(
                monitor.disk.as_ref().unwrap().map.filter_searches.get(),
                searches,
                "unchanged redraw searches"
            )
        });
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            let root = disk
                .folder
                .as_ref()
                .unwrap()
                .hierarchy
                .as_ref()
                .unwrap()
                .clone();
            disk.map.filter.push_str("-edited");
            disk.map.matches(&root);
            assert_eq!(disk.map.filter_searches.get(), searches + 1);
            disk.map.mode = Mode::Files;
            disk.map.matches(&root);
            assert_eq!(disk.map.filter_searches.get(), searches + 2);
            disk.map.mode = Mode::Age;
            let changed_tree = Arc::new((*root).clone());
            disk.map.matches(&changed_tree);
            assert_eq!(disk.map.filter_searches.get(), searches + 3);
        });
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let (origin, extent) = monitor.read_with(visual, |monitor, _| {
            let frame = monitor.disk.as_ref().unwrap().map.frame.borrow();
            (frame.origin, frame.size)
        });
        let pointer = origin + point(extent.width * 0.23, extent.height * 0.31);
        for (shift, delta) in [
            (false, point(px(0.), px(220.))),
            (true, point(px(37.), px(-11.))),
        ] {
            visual.update(|window, cx| {
                window.dispatch_event(
                    ScrollWheelEvent {
                        position: pointer,
                        delta: ScrollDelta::Pixels(delta),
                        modifiers: Modifiers {
                            shift,
                            ..Default::default()
                        },
                        touch_phase: TouchPhase::Moved,
                    }
                    .to_platform_input(),
                    cx,
                );
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
            });
        }
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let points = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            let frame = disk.map.frame.borrow();
            let scale = (220f32 * 0.002).exp();
            assert!((disk.map.viewport.scale - scale).abs() < 0.0001);
            let anchor_x = extent.width.as_f32() * 0.23;
            let anchor_y = extent.height.as_f32() * 0.31;
            assert!((disk.map.viewport.x - (anchor_x * (1. - scale) + 37.)).abs() < 0.001);
            assert!((disk.map.viewport.y - (anchor_y * (1. - scale) - 11.)).abs() < 0.001);
            for (kind, rect, _, _) in &frame.painted {
                let old = frame
                    .tiles
                    .iter()
                    .find(|tile| &tile.kind == kind)
                    .unwrap()
                    .rect;
                assert!((rect.x - (old.x * scale + anchor_x * (1. - scale) + 37.)).abs() < 0.001);
                assert!((rect.w - old.w * scale).abs() < 0.001);
                assert!((rect.y - (old.y * scale + anchor_y * (1. - scale) - 11.)).abs() < 0.001);
                assert!((rect.h - old.h * scale).abs() < 0.001);
            }
            let mut points = Vec::new();
            for y in (5..extent.height.as_f32() as usize).step_by(13) {
                for x in (5..extent.width.as_f32() as usize).step_by(13) {
                    let expected = frame
                        .painted
                        .iter()
                        .rev()
                        .find(|tile| tile.1.contains(x as f32, y as f32));
                    if let Some(expected) = expected {
                        let old = treemap::hit(&frame.tiles, x as f32, y as f32);
                        if old.is_none_or(|old| old.kind != expected.0) {
                            points.push((
                                origin + point(px(x as f32), px(y as f32)),
                                expected.0.clone(),
                            ));
                            if points.len() == 3 {
                                return points;
                            }
                        }
                    }
                }
            }
            points
        });
        assert_eq!(points.len(), 3, "clicks must cross old paint boundaries");
        for (point, expected) in points {
            visual.simulate_click(point, Modifiers::default());
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            monitor.read_with(visual, |monitor, _| {
                assert_eq!(monitor.disk.as_ref().unwrap().map.selected, Some(expected))
            });
        }
    }

    #[gpui::test]
    async fn fix1_list_boundaries_leave_focus_and_escape_returns(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        let root = disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
        disk.map.selected = disk
            .map
            .entries(root)
            .last()
            .map(|entry| entry.kind.clone());
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.simulate_keystrokes("tab");
        visual.update(|window, cx| {
            assert_ne!(
                window.focused(cx),
                Some(monitor.read(cx).focus.clone()),
                "Tab leaves final row"
            )
        });
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            let root = disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
            disk.map.selected = disk
                .map
                .entries(root)
                .first()
                .map(|entry| entry.kind.clone());
        });
        visual.simulate_keystrokes("shift-tab");
        visual.update(|window, cx| {
            assert_ne!(
                window.focused(cx),
                Some(monitor.read(cx).focus.clone()),
                "Shift-Tab leaves first row"
            )
        });
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
    }

    #[gpui::test]
    async fn fix1_hundreds_of_siblings_scroll_by_key_and_pointer(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        let (sender, pending) = mpsc::sync_channel(4);
        disk.sender = sender;
        let mut parent = Node::directory("root");
        parent.children = (0..400)
            .map(|index| {
                let mut node = Node::directory(format!("sibling-{index:03}-long-directory-name"));
                node.children
                    .push(Node::entry("file", gtop_disk::tree::NodeKind::File, 4096));
                gtop_disk::tree::aggregate(&mut node, Metric::Bytes);
                node
            })
            .collect();
        gtop_disk::tree::aggregate(&mut parent, Metric::Bytes);
        disk.map.scan_root = Some(("/Users/demo".into(), Arc::new(parent.clone())));
        disk.map.siblings =
            Some(Path::new("/Users/demo").join(parent.children[0].filesystem_name()));
        let last = Path::new("/Users/demo").join(parent.children[399].filesystem_name());
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let canvas = visual.debug_bounds("disk-treemap").unwrap();
        assert!(
            canvas.size.height > px(100.),
            "sibling choices must leave the canvas usable"
        );
        let menu = visual.debug_bounds("map-sibling-menu").unwrap();
        assert!(menu.size.height <= px(160.));
        for _ in 0..399 {
            visual.simulate_keystrokes("down");
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
        }
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let last_bounds = visual.debug_bounds("map-sibling-entry-399").unwrap();
        assert!(last_bounds.top() >= menu.top() && last_bounds.bottom() <= menu.bottom());
        visual.simulate_keystrokes("enter");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        let Command::Browse(path, ..) = pending.recv().unwrap() else {
            panic!("browse")
        };
        assert_eq!(path, last);
        monitor.update(visual, |monitor, cx| {
            let disk = monitor.disk.as_mut().unwrap();
            disk.map.siblings = Some("/Users/demo/sibling-000-long-directory-name".into());
            disk.map.sibling_index = 0;
            disk.map
                .sibling_scroll
                .scroll_to_item(0, ScrollStrategy::Nearest);
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.update(|window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position: menu.center(),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-20000.))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let last_bounds = visual.debug_bounds("map-sibling-entry-399").unwrap();
        assert!(last_bounds.top() >= menu.top() && last_bounds.bottom() <= menu.bottom());
        visual.simulate_click(last_bounds.center(), Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        let Command::Browse(path, ..) = pending.recv().unwrap() else {
            panic!("browse")
        };
        assert_eq!(path, last);
    }

    #[gpui::test]
    async fn fix1_filter_preview_changes_only_highlights(cx: &mut TestAppContext) {
        cx.update(init);
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(snapshot_disk());
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let rectangles = |visual: &gpui::VisualTestContext| {
            monitor.read_with(visual, |monitor, _| {
                monitor
                    .disk
                    .as_ref()
                    .unwrap()
                    .map
                    .frame
                    .borrow()
                    .tiles
                    .iter()
                    .map(|tile| (tile.kind.clone(), tile.rect))
                    .collect::<Vec<_>>()
            })
        };
        let unfiltered = rectangles(visual);
        for key in ["/", "t", "a", "r", "g", "e", "t"] {
            visual.simulate_keystrokes(key);
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(rectangles(visual), unfiltered);
        }
        visual.simulate_keystrokes("enter");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let applied_rectangles = rectangles(visual);
        assert_ne!(applied_rectangles, unfiltered);
        for key in ["/", "z", "backspace"] {
            visual.simulate_keystrokes(key);
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(
                rectangles(visual),
                applied_rectangles,
                "editing an applied query previews without filtering geometry"
            );
        }
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(rectangles(visual), unfiltered);
    }

    #[test]
    fn fix1_terminal_folder_reply_keeps_previous_label() {
        for state in [
            ScanState::Cancelled,
            ScanState::Failed("root missing".into()),
        ] {
            let mut disk = snapshot_disk();
            let (sender, _pending) = mpsc::sync_channel(4);
            disk.sender = sender;
            disk.change_scan_option(true);
            let mut failed = disk.folder.clone().unwrap();
            failed.hierarchy = None;
            failed.state = state.clone();
            disk.mailbox.lock().unwrap().folder = Some((disk.generation(), 0, failed));
            assert!(disk.consume_mailbox());
            assert_eq!(disk.folder.as_ref().unwrap().state, state);
            assert!(disk.folder.as_ref().unwrap().hierarchy.is_some());
            assert!(disk.status.starts_with("Previous result"));
            assert!(!disk.status.contains("Scanning"));
        }
        let mut disk = snapshot_disk();
        let (sender, _pending) = mpsc::sync_channel(4);
        disk.sender = sender;
        disk.start("/new-root".into(), false);
        disk.mailbox.lock().unwrap().outcome = Some((disk.generation(), ScanState::Cancelled));
        disk.consume_mailbox();
        assert_eq!(disk.status, "Scan cancelled.");
        assert!(disk.folder.is_none());
    }

    #[test]
    fn fix1_reopening_filter_keeps_preview_geometry() {
        let mut disk = snapshot_disk();
        disk.map_controls_key("/");
        disk.map_controls_key("t");
        disk.map_controls_key("enter");
        disk.map_controls_key("/");
        disk.map_controls_key("z");
        assert!(disk.map.filtering);
        assert!(disk.map.applied);
        assert_eq!(disk.map.committed_filter, "t");
        assert_eq!(disk.map.filter, "tz");
    }

    #[test]
    fn fix1_filtered_list_has_no_obsolete_other() {
        let mut disk = snapshot_disk();
        disk.map.filter = "file-159".into();
        disk.map.committed_filter = disk.map.filter.clone();
        disk.map.applied = true;
        let root = disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
        let rows = disk.map.entries(root);
        assert!(!rows
            .iter()
            .any(|row| matches!(row.kind, TileKind::Others { .. })));
        assert!(rows.iter().any(|row| row.label.contains("file-159")));
    }

    #[test]
    fn fix3_other_reports_only_tail_bytes_in_each_mode() {
        let mut disk = snapshot_disk();
        for mode in [Mode::Size, Mode::Files, Mode::Age] {
            for filter in ["", "file-"] {
                disk.map.mode = mode;
                disk.map.filter = filter.into();
                disk.map.committed_filter = filter.into();
                disk.map.applied = !filter.is_empty();
                disk.map.listing.borrow_mut().take();
                let root = disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
                let rows = disk.map.entries(root);
                let other = rows
                    .iter()
                    .find(|row| matches!(row.kind, TileKind::Others { .. }))
                    .expect("tiny folder has an Other row");
                assert_eq!(
                    other.label, "Other · 64 items · 262.1 KB",
                    "mode {mode:?}, filter {filter:?}"
                );
            }
        }
    }

    #[test]
    fn fix3_partial_other_reports_only_matched_tail_bytes() {
        use gtop_disk::tree::{aggregate, NodeKind};

        let mut root = Node::directory("Home");
        for index in 0..97 {
            let mut directory = Node::directory(format!("directory-{index:02}"));
            directory.children = vec![
                Node::entry("matching-file", NodeKind::File, 4096),
                Node::entry("unmatched-file", NodeKind::File, 1 << 20),
            ];
            aggregate(&mut directory, Metric::Bytes);
            root.children.push(directory);
        }
        aggregate(&mut root, Metric::Bytes);
        let root = Arc::new(root);
        let mut disk = snapshot_disk();
        disk.map.mode = Mode::Size;
        disk.map.filter = "matching-file".into();
        disk.map.committed_filter = disk.map.filter.clone();
        disk.map.applied = true;
        disk.map.listing.borrow_mut().take();

        let rows = disk.map.entries(&root);
        let other = rows
            .iter()
            .find(|row| matches!(row.kind, TileKind::Others { .. }))
            .expect("the 97th matching directory is in Other");
        assert_eq!(other.label, "Other · 1 items · 4.1 KB");
    }

    #[gpui::test]
    async fn fix2_filtered_zero_byte_match_stays_keyboard_reachable(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        let target = vec![0, 0, 3];
        for mode in [Mode::Size, Mode::Files, Mode::Age] {
            disk.map.mode = mode;
            disk.map.filter = "zero".into();
            disk.map.committed_filter = "zero".into();
            disk.map.applied = true;
            let root = disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
            let rows = disk.map.entries(root);
            assert!(
                rows.iter().any(|row| {
                    matches!(&row.kind, TileKind::Node { crumbs } if crumbs == &target)
                }),
                "zero-byte match missing in {mode:?}"
            );
        }
        disk.map.mode = Mode::Size;
        disk.map.listing.borrow_mut().take();
        let root = disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
        let expected = disk.map.entries(root).len();
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        for _ in 0..expected {
            visual.simulate_keystrokes("tab");
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
        }
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            let root = disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
            let selected = disk.map.selected.as_ref().unwrap();
            assert!(matches!(selected, TileKind::Node { crumbs } if crumbs == &target));
            assert!(disk.map.entries(root).iter().any(|row| {
                matches!(&row.kind, TileKind::Node { crumbs } if crumbs == &target)
            }));
        });
    }

    #[gpui::test]
    async fn fix2_successful_scans_clear_rendered_scanning_status(cx: &mut TestAppContext) {
        cx.update(init);
        let fixture = Fixture::new();
        let root = fixture.0.clone();
        let disk_root = root.clone();
        let (monitor, visual) = cx.add_window_view(move |window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(Disk::with_home(disk_root));
            monitor.disk.as_mut().unwrap().open = true;
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        for scan in 0..2 {
            if scan == 1 {
                monitor.update(visual, |monitor, _| {
                    let disk = monitor.disk.as_mut().unwrap();
                    disk.change_scan_option(true);
                    assert!(disk.map.previous_result);
                    assert!(disk.status.starts_with("Previous result"));
                    assert!(disk.status.contains("Scanning"));
                });
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let complete = monitor.update(visual, |monitor, _| {
                    let disk = monitor.disk.as_mut().unwrap();
                    disk.consume_mailbox();
                    disk.session
                        .as_ref()
                        .is_some_and(|session| session.outcome == Some(ScanState::Complete))
                        && disk.folder.as_ref().is_some_and(|folder| {
                            folder.path == root && folder.state == ScanState::Complete
                        })
                });
                visual.update(|window, cx| window.draw(cx).clear(cx));
                if complete {
                    break;
                }
                assert!(Instant::now() < deadline, "scan did not complete");
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(visual.debug_bounds("disk-status").is_some());
            monitor.read_with(visual, |monitor, _| {
                let disk = monitor.disk.as_ref().unwrap();
                assert!(disk.status.is_empty(), "rendered status: {}", disk.status);
                assert!(disk.folder.as_ref().unwrap().path == root);
            });
        }
    }

    #[gpui::test]
    async fn fix3_option_rescan_without_previous_hierarchy_clears_status(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        let mut completed_folder = disk.folder.take().unwrap();
        completed_folder.state = ScanState::Complete;
        disk.session.as_mut().unwrap().outcome = None;
        let (sender, _pending) = mpsc::sync_channel(4);
        disk.sender = sender;
        disk.status = "Scanning…".into();

        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let hidden_button = visual.debug_bounds("map-hidden").unwrap();
        visual.simulate_click(hidden_button.center(), Modifiers::default());
        let generation = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert!(!disk.map.previous_result);
            assert!(disk.status.contains("Scanning with new options"));
            disk.generation()
        });
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            let mut mailbox = disk.mailbox.lock().unwrap();
            mailbox.folder = Some((generation, 0, completed_folder));
            mailbox.outcome = Some((generation, ScanState::Complete));
            drop(mailbox);
            assert!(disk.consume_mailbox());
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("disk-status").is_some());
        monitor.read_with(visual, |monitor, _| {
            assert!(monitor.disk.as_ref().unwrap().status.is_empty());
        });
    }

    #[gpui::test]
    async fn fix4_cancel_after_progress_folder_reports_cancelled(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        let mut progress = disk.folder.clone().unwrap();
        progress.hierarchy = None;
        progress.state = ScanState::Scanning;
        let (sender, _pending) = mpsc::sync_channel(4);
        disk.sender = sender;
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let rescan = visual.debug_bounds("disk-rescan").unwrap();
        visual.simulate_click(rescan.center(), Modifiers::default());
        let generation = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert_eq!(disk.status, "Scanning…");
            disk.generation()
        });
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            disk.mailbox.lock().unwrap().folder = Some((generation, 0, progress.clone()));
            assert!(disk.consume_mailbox());
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert!(disk.folder.as_ref().unwrap().hierarchy.is_none());
            assert!(!disk.map.previous_result);
        });
        let cancel_button = visual.debug_bounds("disk-cancel").unwrap();
        visual.simulate_click(cancel_button.center(), Modifiers::default());
        let cancel = monitor.read_with(visual, |monitor, _| {
            monitor.disk.as_ref().unwrap().cancel.clone()
        });
        assert!(cancel.load(Ordering::Relaxed));
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            let mut terminal = progress;
            terminal.state = ScanState::Cancelled;
            let generation = disk.generation();
            let mut mailbox = disk.mailbox.lock().unwrap();
            mailbox.folder = Some((generation, 0, terminal));
            mailbox.outcome = Some((generation, ScanState::Cancelled));
            drop(mailbox);
            assert!(disk.consume_mailbox());
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("disk-status").is_some());
        monitor.read_with(visual, |monitor, _| {
            assert_eq!(monitor.disk.as_ref().unwrap().status, "Scan cancelled.");
        });
    }

    #[gpui::test]
    async fn fix4_cancel_after_hidden_rescan_progress_reports_cancelled(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        let mut progress = disk.folder.clone().unwrap();
        progress.hierarchy = None;
        progress.state = ScanState::Scanning;
        let (sender, _pending) = mpsc::sync_channel(4);
        disk.sender = sender;
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let rescan = visual.debug_bounds("disk-rescan").unwrap();
        visual.simulate_click(rescan.center(), Modifiers::default());
        let first_generation = monitor.read_with(visual, |monitor, _| {
            monitor.disk.as_ref().unwrap().generation()
        });
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            disk.mailbox.lock().unwrap().folder = Some((first_generation, 0, progress.clone()));
            assert!(disk.consume_mailbox());
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert!(disk.folder.as_ref().unwrap().hierarchy.is_none());
            assert!(!disk.map.previous_result);
        });
        let hidden = visual.debug_bounds("map-hidden").unwrap();
        visual.simulate_click(hidden.center(), Modifiers::default());
        let generation = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert!(disk.generation() > first_generation);
            assert!(disk.status.contains("Scanning with new options"));
            disk.generation()
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let cancel_button = visual.debug_bounds("disk-cancel").unwrap();
        visual.simulate_click(cancel_button.center(), Modifiers::default());
        assert!(monitor.read_with(visual, |monitor, _| {
            monitor
                .disk
                .as_ref()
                .unwrap()
                .cancel
                .load(Ordering::Relaxed)
        }));
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            let mut terminal = progress;
            terminal.state = ScanState::Cancelled;
            let mut mailbox = disk.mailbox.lock().unwrap();
            mailbox.folder = Some((generation, 0, terminal));
            mailbox.outcome = Some((generation, ScanState::Cancelled));
            drop(mailbox);
            assert!(disk.consume_mailbox());
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("disk-status").is_some());
        monitor.read_with(visual, |monitor, _| {
            assert_eq!(monitor.disk.as_ref().unwrap().status, "Scan cancelled.");
        });
    }

    #[gpui::test]
    async fn fix4_failure_after_progress_folder_reports_failure(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        let mut progress = disk.folder.clone().unwrap();
        progress.hierarchy = None;
        progress.state = ScanState::Scanning;
        let (sender, _pending) = mpsc::sync_channel(4);
        disk.sender = sender;
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let rescan = visual.debug_bounds("disk-rescan").unwrap();
        visual.simulate_click(rescan.center(), Modifiers::default());
        let generation = monitor.read_with(visual, |monitor, _| {
            monitor.disk.as_ref().unwrap().generation()
        });
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            disk.mailbox.lock().unwrap().folder = Some((generation, 0, progress.clone()));
            assert!(disk.consume_mailbox());
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("disk-status").is_some());
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert!(disk.folder.as_ref().unwrap().hierarchy.is_none());
            assert!(!disk.map.previous_result);
        });
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            let error = "disk vanished";
            let mut terminal = progress;
            terminal.state = ScanState::Failed(error.into());
            let mut mailbox = disk.mailbox.lock().unwrap();
            mailbox.folder = Some((generation, 0, terminal));
            mailbox.outcome = Some((generation, ScanState::Failed(error.into())));
            drop(mailbox);
            assert!(disk.consume_mailbox());
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("disk-status").is_some());
        monitor.read_with(visual, |monitor, _| {
            assert_eq!(
                monitor.disk.as_ref().unwrap().status,
                "Scan failed: disk vanished"
            );
        });
    }

    #[gpui::test]
    async fn fix4_top_disk_entry_opens_disk_and_keeps_treemap_focus(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        disk.open = false;
        let (sender, _pending) = mpsc::sync_channel(4);
        disk.sender = sender;
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("disk-open-hint").is_some());
        let disk_entry = visual.debug_bounds("disk-open").unwrap();
        visual.simulate_click(disk_entry.center(), Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        assert!(monitor.read_with(visual, |monitor, _| { monitor.disk.as_ref().unwrap().open }));
        visual.simulate_keystrokes("m");
        assert!(monitor.read_with(visual, |monitor, _| {
            monitor.disk.as_ref().unwrap().map.mode == Mode::Files
        }));
    }

    #[test]
    fn fix1_rejected_history_preserves_next_destination() {
        let mut disk = snapshot_disk();
        let (sender, pending) = mpsc::sync_channel(1);
        disk.sender = sender;
        for path in ["/Users/demo/a", "/Users/demo/b", "/Users/demo/c"] {
            disk.map_visit(path.into());
        }
        disk.sender
            .try_send(Command::Browse(
                "/busy".into(),
                0,
                1,
                1,
                disk.cancel.clone(),
            ))
            .unwrap();
        disk.map_travel(false);
        pending.recv().unwrap();
        disk.map_travel(false);
        let Command::Browse(path, ..) = pending.recv().unwrap() else {
            panic!("browse")
        };
        assert_eq!(path, Path::new("/Users/demo/b"));
    }

    #[test]
    fn fix1_terminal_replacement_reports_failure_and_cancel() {
        for state in [
            ScanState::Failed("root missing".into()),
            ScanState::Cancelled,
        ] {
            let mut disk = snapshot_disk();
            let (sender, _pending) = mpsc::sync_channel(4);
            disk.sender = sender;
            disk.change_scan_option(true);
            let generation = disk.generation();
            disk.mailbox.lock().unwrap().outcome = Some((generation, state.clone()));
            disk.consume_mailbox();
            assert!(!disk.status.contains("Scanning"), "{}", disk.status);
            assert!(disk.status.contains("Previous result"), "{}", disk.status);
            assert!(
                disk.status
                    .contains(if matches!(state, ScanState::Cancelled) {
                        "cancelled"
                    } else {
                        "root missing"
                    }),
                "{}",
                disk.status
            );
        }
    }

    #[test]
    fn fix1_closed_cancel_reply_preserves_restart_selection() {
        use std::os::unix::ffi::OsStringExt;
        let mut disk = snapshot_disk();
        let root = Arc::make_mut(disk.folder.as_mut().unwrap().hierarchy.as_mut().unwrap());
        let node = &mut root.children[0].children[0].children[0].children[0];
        node.inode = Some((42, std::num::NonZeroU64::new(99).unwrap()));
        node.raw_name =
            Some(std::ffi::OsString::from_vec(b"binary-\xff".to_vec()).into_boxed_os_str());
        disk.map_select(TileKind::Node {
            crumbs: vec![0, 0, 0, 0],
        });
        let selected = disk.map_row().unwrap();
        let complete = disk.folder.clone().unwrap();
        let (sender, _pending) = mpsc::sync_channel(8);
        disk.sender = sender;
        disk.start_over();
        disk.close();
        let mut cancelled = complete.clone();
        Arc::make_mut(cancelled.hierarchy.as_mut().unwrap())
            .children
            .clear();
        cancelled.state = ScanState::Cancelled;
        {
            let mut mailbox = disk.mailbox.lock().unwrap();
            mailbox.folder = Some((disk.generation(), 0, cancelled));
            mailbox.outcome = Some((disk.generation(), ScanState::Cancelled));
        }
        disk.consume_mailbox();
        disk.reopen(std::time::Instant::now(), false);
        disk.mailbox.lock().unwrap().folder = Some((disk.generation(), 0, complete));
        assert!(disk.consume_mailbox());
        let restored = disk
            .map_row()
            .expect("restart must restore the selected item");
        assert_eq!(restored.path, selected.path);
        assert_eq!(restored.identity, selected.identity);
    }

    #[test]
    fn fix1_replacement_scan_restores_selection() {
        for option in [false, true] {
            let mut disk = snapshot_disk();
            let root = Arc::make_mut(disk.folder.as_mut().unwrap().hierarchy.as_mut().unwrap());
            root.children[0].inode = Some((42, std::num::NonZeroU64::new(99).unwrap()));
            disk.map_select(TileKind::Node { crumbs: vec![0] });
            let selected = disk.map_row().unwrap();
            let mut replacement = disk.folder.clone().unwrap();
            Arc::make_mut(replacement.hierarchy.as_mut().unwrap())
                .children
                .reverse();
            let (sender, _pending) = mpsc::sync_channel(4);
            disk.sender = sender;
            if option {
                disk.change_scan_option(true);
            } else {
                disk.start_over();
                disk.start_over();
            }
            let mut starting = replacement.clone();
            starting.state = ScanState::Scanning;
            starting.hierarchy = None;
            disk.mailbox.lock().unwrap().folder = Some((disk.generation(), 0, starting));
            assert!(disk.consume_mailbox());
            let mut partial = replacement.clone();
            Arc::make_mut(partial.hierarchy.as_mut().unwrap())
                .children
                .clear();
            partial.state = ScanState::Scanning;
            disk.mailbox.lock().unwrap().folder = Some((disk.generation(), 0, partial));
            assert!(disk.consume_mailbox());
            assert!(disk.map.selected.is_none());
            disk.mailbox.lock().unwrap().folder = Some((disk.generation(), 0, replacement));
            assert!(disk.consume_mailbox());
            let restored = disk
                .map_row()
                .expect("selection restored after production start");
            assert_eq!(restored.path, selected.path);
            assert_eq!(restored.identity, selected.identity);
        }
    }

    #[gpui::test]
    async fn fix1_visible_controls_do_not_type(cx: &mut TestAppContext) {
        cx.update(init);
        let disk = snapshot_disk();
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        for key in ["+", "/", "t"] {
            visual.simulate_keystrokes(key);
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
        }
        visual.update(|window, cx| window.draw(cx).clear(cx));
        for selector in [
            "map-reset",
            "map-depth-more",
            "map-depth-less",
            "map-filter",
        ] {
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Modifiers::default());
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            monitor.read_with(visual, |monitor, _| {
                let disk = monitor.disk.as_ref().unwrap();
                assert_eq!(disk.map.filter, "t");
                if selector == "map-reset" {
                    assert_eq!(disk.map.viewport, Viewport::default());
                }
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
        }
    }

    #[test]
    fn resort_restores_raw_path_and_identity_and_rejects_replacement() {
        let mut disk = snapshot_disk();
        let root = Arc::make_mut(disk.folder.as_mut().unwrap().hierarchy.as_mut().unwrap());
        root.children[0].inode = Some((42, std::num::NonZeroU64::new(99).unwrap()));
        disk.map_select(TileKind::Node { crumbs: vec![0] });
        let selected = disk.map_row().unwrap();
        let mut replacement = disk.folder.clone().unwrap();
        Arc::make_mut(replacement.hierarchy.as_mut().unwrap())
            .children
            .reverse();
        disk.restore_map_selection(&replacement);
        disk.folder = Some(replacement.clone());
        let restored = disk.map_row().unwrap();
        assert_eq!(restored.path, selected.path);
        assert_eq!(restored.identity, selected.identity);
        let TileKind::Node { crumbs } = disk.map.selected.as_ref().unwrap() else {
            panic!("node");
        };
        let index = crumbs[0];
        Arc::make_mut(replacement.hierarchy.as_mut().unwrap()).children[index].inode =
            Some((42, std::num::NonZeroU64::new(100).unwrap()));
        disk.restore_map_selection(&replacement);
        assert!(disk.map.selected.is_none());
    }

    #[test]
    fn map_selection_uses_raw_names_and_observed_identity() {
        use std::os::unix::ffi::OsStringExt;
        let mut disk = snapshot_disk();
        let folder = disk.folder.as_mut().unwrap();
        let root = Arc::make_mut(folder.hierarchy.as_mut().unwrap());
        for (index, byte) in [0xff, 0xfe].into_iter().enumerate() {
            let raw = std::ffi::OsString::from_vec(vec![b'f', byte]);
            let node = &mut root.children[index];
            node.name = raw.to_string_lossy().into_owned().into();
            node.raw_name = Some(raw.into_boxed_os_str());
            node.inode = Some((42, std::num::NonZeroU64::new(index as u64 + 10).unwrap()));
        }
        for (index, byte) in [0xff, 0xfe].into_iter().enumerate() {
            disk.map_select(TileKind::Node {
                crumbs: vec![index],
            });
            let row = disk.map_row().unwrap();
            assert_eq!(
                row.path,
                Path::new("/Users/demo").join(std::ffi::OsString::from_vec(vec![b'f', byte]))
            );
            assert_eq!(
                row.identity,
                disk_scan::FileId {
                    device: 42,
                    inode: index as u64 + 10
                }
            );
        }
    }

    #[gpui::test]
    async fn modes_filter_depth_zoom_and_pending_scan_options(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        let (sender, pending) = mpsc::sync_channel(32);
        disk.sender = sender;
        disk.remember_map_root(&disk.folder.clone().unwrap());
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1480.), px(930.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let rectangles = monitor.read_with(visual, |monitor, _| {
            monitor
                .disk
                .as_ref()
                .unwrap()
                .map
                .frame
                .borrow()
                .tiles
                .iter()
                .map(|tile| (tile.kind.clone(), tile.rect))
                .collect::<Vec<_>>()
        });
        for (selector, mode) in [
            ("map-mode-2", Mode::Age),
            ("map-mode-1", Mode::Files),
            ("map-mode-0", Mode::Size),
        ] {
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Modifiers::default());
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            monitor.read_with(visual, |monitor, _| {
                let disk = monitor.disk.as_ref().unwrap();
                assert_eq!(disk.map.mode, mode);
                let current = disk
                    .map
                    .frame
                    .borrow()
                    .tiles
                    .iter()
                    .map(|tile| (tile.kind.clone(), tile.rect))
                    .collect::<Vec<_>>();
                if mode == Mode::Age {
                    assert_eq!(current, rectangles);
                }
                if mode == Mode::Files {
                    assert_ne!(current, rectangles);
                }
            });
        }
        for key in [
            "]", "[", "m", "m", "m", "down", "j", "up", "k", "left", "h", "right", "l", "tab", "+",
            "-", "0",
        ] {
            visual.simulate_keystrokes(key);
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
        }
        for selector in [
            "map-depth-less",
            "map-depth-more",
            "map-reset",
            "map-filter",
        ] {
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Modifiers::default());
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
        }
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        assert!(pending.try_recv().is_err(), "view controls must never scan");
        let center = visual.debug_bounds("disk-treemap").unwrap().center();
        let before = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            let frame = disk.map.frame.borrow();
            disk.map.viewport.inverse(
                (center.x - frame.origin.x).as_f32(),
                (center.y - frame.origin.y).as_f32(),
            )
        });
        visual.update(|window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position: center,
                    delta: ScrollDelta::Pixels(point(px(0.), px(250.))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            let frame = disk.map.frame.borrow();
            let after = disk.map.viewport.inverse(
                (center.x - frame.origin.x).as_f32(),
                (center.y - frame.origin.y).as_f32(),
            );
            assert!((before.0 - after.0).abs() < 0.001 && (before.1 - after.1).abs() < 0.001);
        });
        let expected = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            let frame = disk.map.frame.borrow();
            let (x, y) = disk.map.viewport.inverse(
                (center.x - frame.origin.x).as_f32(),
                (center.y - frame.origin.y).as_f32(),
            );
            treemap::hit(&frame.tiles, x, y).unwrap().kind.clone()
        });
        visual.simulate_click(center, Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.read_with(visual, |monitor, _| {
            assert_eq!(monitor.disk.as_ref().unwrap().map.selected, Some(expected))
        });
        visual.update(|window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position: center,
                    delta: ScrollDelta::Pixels(point(px(10.), px(-15.))),
                    modifiers: Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                    touch_phase: TouchPhase::Moved,
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        visual.simulate_keystrokes("/");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        for key in ["t", "a", "r", "g", "e", "t", "enter"] {
            visual.simulate_keystrokes(key);
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
        }
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert!(disk.map.filter.is_empty());
            assert_ne!(disk.map.viewport, Viewport::default());
            assert!(disk.map.selected.is_some());
        });
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert_eq!(disk.map.viewport, Viewport::default());
            assert!(disk.map.selected.is_some());
        });
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.read_with(visual, |monitor, _| {
            assert!(monitor.disk.as_ref().unwrap().map.selected.is_none())
        });
        let stale = monitor.read_with(visual, |monitor, _| {
            monitor.disk.as_ref().unwrap().folder.clone().unwrap()
        });
        for selector in ["map-hidden", "map-apparent"] {
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Modifiers::default());
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
        }
        let Command::Scan(_, true, first_token, first_generation, first_options, _) =
            pending.try_recv().unwrap()
        else {
            panic!("first scan");
        };
        let Command::Scan(root, true, _, generation, options, _) = pending.try_recv().unwrap()
        else {
            panic!("second scan while first pending");
        };
        assert!(first_token.load(Ordering::Relaxed));
        assert!(!first_options.include_hidden && !first_options.apparent_size);
        assert!(!options.include_hidden && options.apparent_size);
        assert!(generation > first_generation);
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            disk.mailbox.lock().unwrap().folder = Some((first_generation, 0, stale.clone()));
            assert!(!disk.consume_mailbox());
            assert_eq!(disk.session.as_ref().unwrap().root, root);
            assert_eq!(disk.generation(), generation);
            assert!(disk.map.selected.is_none());
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let choose = visual.debug_bounds("map-choose-folder").unwrap();
        visual.simulate_click(choose.center(), Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        assert!(visual.did_prompt_for_paths());
        visual.simulate_path_prompt_response(|options| {
            assert!(!options.files && options.directories && !options.multiple);
            Some(vec![PathBuf::from("/Volumes/External/chosen")])
        });
        visual.run_until_parked();
        let Command::Scan(chosen, false, _, chosen_generation, _, _) = pending.try_recv().unwrap()
        else {
            panic!("chosen root scan");
        };
        assert_eq!(chosen, Path::new("/Volumes/External/chosen"));
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            disk.mailbox.lock().unwrap().folder = Some((generation, 0, stale));
            assert!(!disk.consume_mailbox());
            assert_eq!(disk.session.as_ref().unwrap().root, chosen);
            assert_eq!(disk.generation(), chosen_generation);
        });
        monitor.update(visual, |monitor, _| monitor.disk = None);
    }

    #[gpui::test]
    async fn sibling_choices_and_history_use_observed_paths(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        let original = disk.folder.clone().unwrap();
        disk.remember_map_root(&original);
        disk.map_visit(original.path.clone());
        let root = original.hierarchy.as_ref().unwrap();
        let index = root
            .children
            .iter()
            .position(|node| &*node.name == "projects")
            .unwrap();
        let child = root.children[index].clone();
        disk.map_select(TileKind::Node {
            crumbs: vec![index],
        });
        let (sender, pending) = mpsc::sync_channel(32);
        disk.sender = sender;
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1480.), px(930.)));
        visual.simulate_keystrokes("enter");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        let Command::Browse(projects, _, generation, navigation, _) = pending.try_recv().unwrap()
        else {
            panic!("browse");
        };
        let mut folder = original;
        folder.path = projects.clone();
        folder.allocated = child.bytes;
        folder.hierarchy = Some(Arc::new(child));
        monitor.update(visual, |monitor, _| {
            let disk = monitor.disk.as_mut().unwrap();
            disk.mailbox.lock().unwrap().folder = Some((generation, navigation, folder));
            assert!(disk.consume_mailbox());
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let arrow = visual.debug_bounds("map-siblings-1").unwrap();
        visual.simulate_click(arrow.center(), Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let (target_index, target) = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            disk.map_siblings(&projects)
                .into_iter()
                .enumerate()
                .find(|(_, path)| path.file_name().is_some_and(|name| name == "tiny"))
                .unwrap()
        });
        assert_eq!(target_index, 1);
        let option = visual.debug_bounds("map-sibling-entry-1").unwrap();
        visual.simulate_click(option.center(), Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        assert!(
            matches!(pending.try_recv().unwrap(),Command::Browse(path,_,_,_,_) if path==target)
        );
        for (selector, expected) in [("map-back", projects), ("map-forward", target)] {
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Modifiers::default());
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            assert!(
                matches!(pending.try_recv().unwrap(),Command::Browse(path,_,_,_,_) if path==expected)
            );
        }
        monitor.update(visual, |monitor, _| monitor.disk = None);
    }

    #[gpui::test]
    async fn treemap_real_click_navigation_other_and_immediate_focus(cx: &mut TestAppContext) {
        cx.update(init);
        let fixture = Fixture::new();
        let disk = fixture.disk();
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1480.), px(930.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            visual.debug_bounds("memory-window").is_none(),
            "Treemap must use the whole window without the system tower"
        );
        let (position, kind, path) = monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            let folder = disk.folder.as_ref().unwrap();
            let root = folder.hierarchy.as_ref().unwrap();
            let frame = disk.map.frame.borrow();
            let tile = frame.tiles.iter().find(|tile| {
                tile.header.is_some() && matches!(&tile.kind, TileKind::Node { crumbs } if root.resolve(crumbs).is_some_and(|node| &*node.name == "projects"))
            }).unwrap();
            let band = tile.header.unwrap();
            (frame.origin + point(px(band.x + band.w / 2.), px(band.y + band.h / 2.)), tile.kind.clone(),
                node_path(root, &folder.path, tile.crumbs()).unwrap())
        });
        let (leaf_position, leaf_kind) = monitor.read_with(visual, |monitor, _| {
            let frame = monitor.disk.as_ref().unwrap().map.frame.borrow();
            let leaf = frame
                .tiles
                .iter()
                .find(|tile| tile.depth == 3 && tile.header.is_none())
                .unwrap();
            let position = frame.origin
                + point(
                    px(leaf.rect.x + leaf.rect.w / 2.),
                    px(leaf.rect.y + leaf.rect.h / 2.),
                );
            assert_eq!(
                treemap::hit(
                    &frame.tiles,
                    (position.x - frame.origin.x).as_f32(),
                    (position.y - frame.origin.y).as_f32()
                )
                .unwrap()
                .kind,
                leaf.kind
            );
            (position, leaf.kind.clone())
        });
        visual.simulate_click(leaf_position, Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.update(|window, cx| window.simulate_mouse_move(position, cx));
        monitor.read_with(visual, |monitor, _| {
            assert_eq!(
                monitor.disk.as_ref().unwrap().map.selected.as_ref(),
                Some(&leaf_kind),
                "hover must not change the selected action target"
            );
        });
        visual.simulate_click(position, Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.read_with(visual, |monitor, _| {
            assert_eq!(
                monitor.disk.as_ref().unwrap().map.selected.as_ref(),
                Some(&kind)
            )
        });
        visual.simulate_keystrokes("enter");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.update(visual, |monitor, _| {
            wait_folder(monitor.disk.as_mut().unwrap(), &path)
        });
        assert_eq!(
            monitor.read_with(visual, |m, _| m
                .disk
                .as_ref()
                .unwrap()
                .folder
                .as_ref()
                .unwrap()
                .path
                .clone()),
            path
        );
        visual.simulate_keystrokes("backspace");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.update(visual, |monitor, _| {
            wait_folder(monitor.disk.as_mut().unwrap(), &fixture.0)
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let other_index = monitor.read_with(visual, |monitor, _| {
            let root = monitor
                .disk
                .as_ref()
                .unwrap()
                .folder
                .as_ref()
                .unwrap()
                .hierarchy
                .as_ref()
                .unwrap();
            entries(root)
                .iter()
                .position(|entry| matches!(entry.kind, TileKind::Others { .. }))
                .unwrap()
        });
        for _ in 0..=other_index {
            visual.simulate_keystrokes("tab");
        }
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let list = visual.debug_bounds("map-folder-list").unwrap();
        let other = visual.debug_bounds("map-entry-other").unwrap_or_else(|| {
            panic!(
                "selected {:?}",
                monitor.read_with(visual, |monitor, _| monitor
                    .disk
                    .as_ref()
                    .unwrap()
                    .map
                    .selected
                    .clone())
            )
        });
        assert!(other.top() >= list.top() && other.bottom() <= list.bottom());
        visual.simulate_click(other.center(), Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert!(matches!(
                disk.map.selected,
                Some(TileKind::Others { count: 64, .. })
            ));
            assert!(disk.map_row().is_none(), "Other has no actionable path");
        });
        visual.simulate_keystrokes("d");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        assert!(!monitor.read_with(visual, |monitor, _| monitor.disk.as_ref().unwrap().open));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("memory-window").is_some());
        visual.simulate_keystrokes("d");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        assert!(monitor.read_with(visual, |monitor, _| monitor.disk.as_ref().unwrap().open));
        monitor.update(visual, |monitor, _| monitor.disk = None);
    }

    #[gpui::test]
    async fn inspector_fixed_sections_resize_persist_and_unknown_volume(cx: &mut TestAppContext) {
        use futures::StreamExt;
        cx.update(init);
        let mut disk = snapshot_disk();
        let root = disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
        let long = root
            .children
            .iter()
            .position(|node| node.name.starts_with("a-very-long"))
            .unwrap();
        disk.map_select(TileKind::Node { crumbs: vec![long] });
        let (writes, mut saved) = futures::channel::mpsc::unbounded();
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            monitor.settings_writes = Some(writes);
            window.focus(&monitor.focus, cx);
            monitor
        });
        for (width, height) in [(1480., 930.), (1200., 720.)] {
            visual.simulate_resize(size(px(width), px(height)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let selection = visual.debug_bounds("disk-selection").unwrap();
            let name = visual.debug_bounds("disk-selection-name").unwrap();
            let volume = visual.debug_bounds("disk-volume").unwrap();
            let list = visual.debug_bounds("map-folder-list").unwrap();
            let canvas = visual.debug_bounds("disk-treemap").unwrap();
            assert!(name.size.width <= selection.size.width);
            assert!(selection.right() <= px(width));
            assert!(volume.bottom() <= px(height));
            assert!(
                list.size.height > px(20.),
                "middle must remain usable: {list:?}"
            );
            assert!(canvas.size.width >= px(600.));
            assert!(list.top() >= selection.bottom());
            assert!(list.bottom() <= volume.top());
            let first = visual.debug_bounds("map-entry-0").unwrap();
            visual.update(|window, cx| {
                window.dispatch_event(
                    MouseMoveEvent {
                        position: list.center(),
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
                window.dispatch_event(
                    ScrollWheelEvent {
                        position: list.center(),
                        delta: ScrollDelta::Pixels(point(px(0.), px(-10000.))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                    }
                    .to_platform_input(),
                    cx,
                );
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(visual.debug_bounds("disk-selection").unwrap(), selection);
            assert_eq!(visual.debug_bounds("disk-volume").unwrap(), volume);
            assert!(visual
                .debug_bounds("map-entry-0")
                .is_none_or(|bounds| bounds != first));
            monitor.update(visual, |monitor, cx| {
                monitor
                    .disk
                    .as_mut()
                    .unwrap()
                    .map
                    .list_scroll
                    .scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            });
        }
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let start = visual
            .debug_bounds("disk-inspector-resize")
            .unwrap()
            .center();
        let end = start - point(px(110.), px(0.));
        visual.update(|window, cx| {
            window.dispatch_event(
                MouseDownEvent {
                    position: start,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
            window.dispatch_event(
                MouseMoveEvent {
                    position: end,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
            window.dispatch_event(
                MouseUpEvent {
                    position: end,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        let stored = saved.next().await.unwrap();
        assert_eq!(stored.disk_inspector_width.pixels(), 490.);
        assert_eq!(stored.detail_width, crate::details::DetailWidth::default());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        monitor.update(visual, |monitor, cx| {
            monitor.settings = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let inspector = visual.debug_bounds("disk-selection").unwrap();
        assert_eq!(inspector.size.width, px(464.));
        let position = visual
            .debug_bounds("disk-inspector-resize")
            .unwrap()
            .center();
        visual.update(|window, cx| {
            window.dispatch_event(
                MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 2,
                    modifiers: Modifiers::default(),
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
            window.dispatch_event(
                MouseUpEvent {
                    position,
                    button: MouseButton::Left,
                    click_count: 2,
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        assert_eq!(
            saved.next().await.unwrap().disk_inspector_width,
            inspector_model::InspectorWidth::default()
        );
        monitor.update(visual, |monitor, cx| {
            let root = Path::new("/Volumes/External/projects");
            monitor.disk.as_mut().unwrap().volume = Some(
                gtop_disk::inspector::VolumeReadout::query_with(root, |received| {
                    assert_eq!(received, root);
                    Ok((
                        "External · /dev/disk9".into(),
                        gtop_disk::space::SpaceInfo {
                            total: 100,
                            free: 40,
                            available: 30,
                        },
                    ))
                }),
            );
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("External · /dev/disk9").is_some());
        assert!(visual
            .debug_bounds("30.0 B free · 60.0 B used · 100.0 B total")
            .is_some());
        monitor.update(visual, |monitor, cx| {
            monitor.disk.as_mut().unwrap().volume =
                Some(gtop_disk::inspector::VolumeReadout::query_with(
                    Path::new("/Volumes/External/projects"),
                    |_| Err(std::io::Error::other("injected query failure")),
                ));
            cx.notify();
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual
            .debug_bounds("Free Unknown · Used Unknown · Total Unknown")
            .is_some());
        assert!(visual
            .debug_bounds("30.0 B free · 60.0 B used · 100.0 B total")
            .is_none());
        monitor.update(visual, |monitor, _| monitor.disk = None);
    }

    #[gpui::test]
    async fn closing_treemap_finishes_an_interrupted_inspector_resize(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = snapshot_disk();
        let (sender, _receiver) = std::sync::mpsc::sync_channel(4);
        disk.sender = sender;
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let original_width = monitor.read_with(visual, |monitor, _| {
            monitor
                .settings
                .disk_inspector_width
                .for_window(1200.)
                .pixels()
        });
        let handle = visual
            .debug_bounds("disk-inspector-resize")
            .unwrap()
            .center();

        visual.update(|window, cx| {
            window.dispatch_event(
                MouseDownEvent {
                    position: handle,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        visual.simulate_keystrokes("d");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        assert!(!monitor.read_with(visual, |monitor, _| monitor.disk.as_ref().unwrap().open));
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let top_release = visual.debug_bounds("memory-window").unwrap().center();
        visual.update(|window, cx| {
            window.dispatch_event(
                MouseUpEvent {
                    position: top_release,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        visual.simulate_keystrokes("d");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let canvas = visual.debug_bounds("disk-treemap").unwrap();
        let down = canvas.center();
        let moved = down + point(px(120.), px(0.));
        visual.update(|window, cx| {
            window.dispatch_event(
                MouseDownEvent {
                    position: down,
                    button: MouseButton::Left,
                    click_count: 1,
                    modifiers: Modifiers::default(),
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
            window.dispatch_event(
                MouseMoveEvent {
                    position: moved,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()));
        });
        assert_eq!(
            monitor.read_with(visual, |monitor, _| {
                monitor
                    .settings
                    .disk_inspector_width
                    .for_window(1200.)
                    .pixels()
            }),
            original_width
        );
        monitor.update(visual, |monitor, _| monitor.disk = None);
    }

    #[test]
    fn inspector_selection_metadata_preserves_scan_denominator_while_browsing() {
        let mut disk = snapshot_disk();
        let root = disk
            .folder
            .as_ref()
            .unwrap()
            .hierarchy
            .as_ref()
            .unwrap()
            .clone();
        let index = root
            .children
            .iter()
            .position(|node| &*node.name == "projects")
            .unwrap();
        let mut viewed = root.children[index].clone();
        viewed.modified = 1_700_000_000;
        viewed.in_cloud = true;
        viewed.complete = false;
        viewed.unreadable = 3;
        let bytes = viewed.bytes;
        let files = viewed.files;
        let dirs = viewed.dirs;
        disk.folder.as_mut().unwrap().hierarchy = Some(Arc::new(viewed));
        disk.folder.as_mut().unwrap().path.push("projects");
        disk.map_select(TileKind::Node { crumbs: vec![] });
        let lines = disk.selection_details();
        assert_eq!(lines[0], "Unknown of parent · Unknown bytes");
        assert_eq!(
            lines[1],
            format!(
                "{} of scan root · {} bytes",
                inspector_model::share(bytes, root.bytes),
                root.bytes
            )
        );
        assert_eq!(lines[2], format!("{files} files · {dirs} directories"));
        assert_eq!(lines[3], "Newest subtree write · 2023-11-14 22:13 UTC");
        assert_eq!(lines[4], "Kind · Cloud entry · content not read");
        assert!(lines[5].contains("Cloud content was not read"));
        assert!(lines[5].contains("Incomplete"));
        assert!(lines[5].contains("3 unreadable entries"));
        disk.mailbox.lock().unwrap().scan_bytes = Some((1, root.bytes + 1));
        disk.consume_mailbox();
        assert_eq!(disk.scan_bytes, Some(root.bytes + 1));
        disk.mailbox.lock().unwrap().scan_bytes = Some((0, 1));
        disk.consume_mailbox();
        assert_eq!(disk.scan_bytes, Some(root.bytes + 1));
    }

    #[test]
    fn inspector_nested_selection_uses_parent_and_scan_root_denominators() {
        let mut disk = snapshot_disk();
        let root = disk
            .folder
            .as_ref()
            .unwrap()
            .hierarchy
            .as_ref()
            .unwrap()
            .clone();
        let projects = root
            .children
            .iter()
            .position(|node| &*node.name == "projects")
            .unwrap();
        let gtop = root.children[projects]
            .children
            .iter()
            .position(|node| &*node.name == "gtop")
            .unwrap();
        let target = root.children[projects].children[gtop]
            .children
            .iter()
            .position(|node| &*node.name == "target")
            .unwrap();
        let target_bytes = root.children[projects].children[gtop].children[target].bytes;
        disk.map_select(TileKind::Node {
            crumbs: vec![projects, gtop, target],
        });

        let lines = disk.selection_details();

        assert_eq!(target_bytes, 3_145_728);
        assert_eq!(lines[0], "92.3% of parent · 3407872 bytes");
        assert_eq!(lines[1], "68.1% of scan root · 4620288 bytes");
    }

    #[gpui::test]
    async fn treemap_other_members_remain_reachable(cx: &mut TestAppContext) {
        cx.update(init);
        for count in [97, 320] {
            let fixture = Fixture(
                std::env::temp_dir().join(format!("gtop-other-{}-{count}", std::process::id())),
            );
            fs::create_dir_all(&fixture.0).unwrap();
            for i in 0..96 {
                fs::write(fixture.0.join(format!("large-{i:03}")), [1; 8192]).unwrap();
            }
            fs::create_dir_all(fixture.0.join("omitted-dir")).unwrap();
            fs::write(fixture.0.join("omitted-dir/inside"), [1; 4096]).unwrap();
            for i in 97..count {
                fs::write(fixture.0.join(format!("small-{i:03}")), [1; 4096]).unwrap();
            }
            let path = fixture.0.canonicalize().unwrap();
            let mut disk = Disk::with_home(path.clone());
            wait_folder(&mut disk, &path);
            let mut children = disk
                .folder
                .as_ref()
                .unwrap()
                .hierarchy
                .as_ref()
                .unwrap()
                .children
                .iter()
                .collect::<Vec<_>>();
            children.sort_by_key(|child| std::cmp::Reverse(child.bytes));
            let omitted = children[96..]
                .iter()
                .map(|node| path.join(node.filesystem_name()))
                .collect::<Vec<_>>();
            assert!(omitted.contains(&path.join("omitted-dir")));
            let (monitor, visual) = cx.add_window_view(|window, cx| {
                let mut monitor = crate::draw_benchmark::fixture(window, cx);
                monitor.disk = Some(disk);
                window.focus(&monitor.focus, cx);
                monitor
            });
            visual.simulate_resize(size(px(1200.), px(720.)));
            for _ in 0..97 {
                visual.simulate_keystrokes("tab");
                visual.update(|window, cx| {
                    assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
                });
            }
            monitor.read_with(visual, |monitor, _| {
                assert!(monitor.disk.as_ref().unwrap().map_row().is_none())
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let other = visual.debug_bounds("map-entry-other").unwrap_or_else(|| {
                panic!(
                    "selected {:?}",
                    monitor.read_with(visual, |monitor, _| monitor
                        .disk
                        .as_ref()
                        .unwrap()
                        .map
                        .selected
                        .clone())
                )
            });
            visual.simulate_click(other.center(), Modifiers::default());
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            visual.simulate_keystrokes("enter");
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            for (index, expected) in omitted.iter().enumerate() {
                if index > 0 {
                    visual.simulate_keystrokes("tab");
                    visual.update(|window, cx| {
                        assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
                    });
                }
                monitor.read_with(visual, |monitor, _| {
                    assert_eq!(
                        monitor.disk.as_ref().unwrap().map_row().map(|row| row.path),
                        Some(expected.clone()),
                        "child {} must be reachable",
                        index + 97
                    )
                });
            }
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let list = visual.debug_bounds("map-folder-list").unwrap();
            let last = visual
                .debug_bounds(if count == 97 {
                    "map-entry-97"
                } else {
                    "map-entry-320"
                })
                .unwrap();
            assert!(last.top() >= list.top() && last.bottom() <= list.bottom());
            for _ in 1..omitted.len() {
                visual.simulate_keystrokes("shift-tab");
            }
            let directory_index = omitted
                .iter()
                .position(|item| item == &path.join("omitted-dir"))
                .unwrap();
            for _ in 0..directory_index {
                visual.simulate_keystrokes("tab");
            }
            visual.simulate_keystrokes("enter");
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            monitor.update(visual, |monitor, _| {
                wait_folder(monitor.disk.as_mut().unwrap(), &path.join("omitted-dir"))
            });
            visual.simulate_keystrokes("backspace");
            visual.update(|window, cx| {
                assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
            });
            monitor.update(visual, |monitor, _| {
                wait_folder(monitor.disk.as_mut().unwrap(), &path)
            });
            if count > 97 {
                for _ in 0..97 {
                    visual.simulate_keystrokes("tab");
                }
                visual.simulate_keystrokes("enter");
                visual.update(|window, cx| {
                    assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
                });
                let file_index = omitted
                    .iter()
                    .position(|item| {
                        item.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("small-")
                    })
                    .unwrap();
                for _ in 0..file_index {
                    visual.simulate_keystrokes("tab");
                }
                visual.simulate_keystrokes("x");
                visual.update(|window, cx| {
                    assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
                });
                monitor.update(visual, |monitor, _| {
                    let disk = monitor.disk.as_mut().unwrap();
                    let deadline = Instant::now() + Duration::from_secs(30);
                    while disk.confirmation.is_none() && Instant::now() < deadline {
                        disk.consume_mailbox();
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    assert_eq!(
                        disk.confirmation.as_ref().map(|proposal| &proposal.path),
                        Some(&omitted[file_index]),
                        "{}",
                        disk.status
                    );
                });
                assert!(omitted[file_index].exists());
            }
            monitor.update(visual, |monitor, _| monitor.disk = None);
        }
    }

    #[test]
    #[ignore = "headless Metal captures require GTOP_TREEMAP_RENDER_DIR"]
    fn treemap_headless_captures() {
        let output = PathBuf::from(
            std::env::var_os("GTOP_TREEMAP_RENDER_DIR").expect("capture output directory"),
        );
        capture(output);
    }

    pub(super) fn capture(output: PathBuf) {
        fs::create_dir_all(&output).unwrap();
        let mut cx = HeadlessAppContext::with_platform(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(gpui_kit_assets::Assets),
            || {
                Ok(Some(Box::new(
                    gpui_apple::metal_renderer::MetalHeadlessRenderer::new(),
                )))
            },
        );
        cx.update(init);
        for bytes in [
            include_bytes!("../../assets/fonts/DotoDisplay.ttf").as_slice(),
            include_bytes!("../../assets/fonts/ArchivoDisplay.ttf").as_slice(),
            include_bytes!("../../assets/fonts/JetBrainsMono.ttf").as_slice(),
            include_bytes!("../../assets/fonts/ArchivoLabels.ttf").as_slice(),
            include_bytes!("../../assets/fonts/Silkscreen.ttf").as_slice(),
        ] {
            cx.text_system()
                .add_fonts(vec![std::borrow::Cow::Borrowed(bytes)])
                .unwrap();
        }
        for (width, height) in [(1480., 930.), (1200., 720.)] {
            let mut disk = insights_snapshot_disk();
            let folder = disk.folder.as_ref().unwrap();
            let findings = disk
                .map
                .findings(folder.hierarchy.as_ref().unwrap(), folder.observed);
            let stale = findings
                .iter()
                .find(|finding| !finding.children.is_empty())
                .unwrap();
            disk.inspect_finding(stale.locations()[0].clone());
            let handle = cx
                .open_window(size(px(width), px(height)), |window, cx| {
                    let monitor = cx.new(|cx| {
                        let mut monitor = crate::draw_benchmark::fixture(window, cx);
                        monitor.disk = Some(disk);
                        monitor
                    });
                    cx.new(|cx| gpui_component::Root::new(monitor, window, cx))
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
            cx.capture_screenshot(handle.into())
                .unwrap()
                .save(output.join(format!("insights-{width:.0}x{height:.0}.png")))
                .unwrap();
        }
        for (palette_index, width, height) in [
            (0, 1480., 930.),
            (0, 1200., 720.),
            (4, 1200., 720.),
            (1, 1200., 720.),
            (2, 1200., 720.),
            (3, 1200., 720.),
        ] {
            let mut disk = kinds_snapshot_disk();
            let root = disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap();
            let project = root
                .children
                .iter()
                .position(|node| &*node.name == "projects")
                .unwrap();
            let target = root.children[project].children[0]
                .children
                .iter()
                .position(|node| &*node.name == "target")
                .unwrap();
            disk.map_select(TileKind::Node {
                crumbs: vec![project, 0, target],
            });
            let handle = cx
                .open_window(size(px(width), px(height)), |window, cx| {
                    let monitor = cx.new(|cx| {
                        let mut monitor = crate::draw_benchmark::fixture(window, cx);
                        monitor.settings.active = palette_index;
                        monitor.disk = Some(disk);
                        monitor
                    });
                    cx.new(|cx| gpui_component::Root::new(monitor, window, cx))
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
            cx.capture_screenshot(handle.into())
                .unwrap()
                .save(output.join(format!(
                    "kinds-{}-{width:.0}x{height:.0}.png",
                    crate::palette::presets()[palette_index].name.to_lowercase()
                )))
                .unwrap();
        }
        for (width, height, ages) in [
            (1480., 930., false),
            (1200., 720., false),
            (1200., 720., true),
        ] {
            let mut disk = if ages {
                age_snapshot_disk()
            } else {
                snapshot_disk()
            };
            disk.map.mode = Mode::Age;
            let index = disk
                .folder
                .as_ref()
                .unwrap()
                .hierarchy
                .as_ref()
                .unwrap()
                .children
                .iter()
                .position(|node| &*node.name == "projects")
                .unwrap();
            disk.map_select(TileKind::Node {
                crumbs: vec![index],
            });
            let handle = cx
                .open_window(size(px(width), px(height)), |window, cx| {
                    let monitor = cx.new(|cx| {
                        let mut monitor = crate::draw_benchmark::fixture(window, cx);
                        monitor.disk = Some(disk);
                        monitor
                    });
                    cx.new(|cx| gpui_component::Root::new(monitor, window, cx))
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
            cx.capture_screenshot(handle.into())
                .unwrap()
                .save(output.join(format!(
                    "modes-{}{width:.0}x{height:.0}.png",
                    if ages { "age-" } else { "" }
                )))
                .unwrap();
        }
        let handle = cx
            .open_window(size(px(1200.), px(720.)), |window, cx| {
                let monitor = cx.new(|cx| crate::draw_benchmark::fixture(window, cx));
                cx.new(|cx| gpui_component::Root::new(monitor, window, cx))
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.capture_screenshot(handle.into())
            .unwrap()
            .save(output.join("top-disk-entry-1200x720.png"))
            .unwrap();
    }

    const AGE_NOW: i64 = 1_800_000_000;
    fn insights_snapshot_disk() -> Disk {
        use gtop_disk::tree::{aggregate, NodeKind};
        fn dir(name: &str, children: Vec<Node>) -> Node {
            let mut node = Node::directory(name);
            node.children = children;
            node
        }
        fn file(name: &str, mib: u64, days: i64) -> Node {
            let mut node = Node::entry(name, NodeKind::File, mib << 20);
            node.modified = AGE_NOW - days * 86_400;
            node
        }
        let mut disk = kinds_snapshot_disk();
        disk.map.clock = Some(AGE_NOW);
        let folder = disk.folder.as_mut().unwrap();
        let root = Arc::make_mut(folder.hierarchy.as_mut().unwrap());
        let mut unknown = file("unknown-write", 128, 60);
        unknown.modified = 0;
        root.children.retain(|node| &*node.name != "worktrees");
        root.children.extend([
            dir(
                ".cache",
                vec![dir("nested-cache", vec![file("cache-data", 900, 1)])],
            ),
            dir(
                "worktrees",
                vec![
                    dir("feature-a", vec![file("source", 700, 45)]),
                    dir("feature-b", vec![file("source", 400, 2)]),
                ],
            ),
            dir(
                "experiments",
                vec![
                    dir(
                        "old-parser",
                        vec![
                            file("Cargo.toml", 1, 70),
                            dir("target", vec![file("build", 500, 60)]),
                        ],
                    ),
                    dir("old-layout", vec![file("sketch", 300, 50)]),
                    dir(
                        "fresh-editor",
                        vec![
                            file("source", 400, 1),
                            dir("node_modules", vec![file("dependency", 200, 1)]),
                        ],
                    ),
                    dir("unknown-time", vec![file("old-known", 200, 70), unknown]),
                ],
            ),
        ]);
        aggregate(root, Metric::Bytes);
        gtop_disk::classify::classify(root);
        folder.allocated = root.bytes;
        folder.items = root.files + u64::from(root.dirs) - 1;
        disk.scan_bytes = Some(root.bytes);
        disk
    }

    #[gpui::test]
    async fn insights_pointer_and_keyboard_inspect_every_location_without_cleanup(
        cx: &mut TestAppContext,
    ) {
        cx.update(init);
        let scratch = Fixture::shared_pair();
        let sentinel = fs::read(scratch.0.join("a")).unwrap();
        let mut disk = insights_snapshot_disk();
        disk.home = scratch.0.clone();
        disk.folder.as_mut().unwrap().path = scratch.0.clone();
        disk.folder.as_mut().unwrap().root = scratch.0.clone();
        let (sender, commands) = mpsc::sync_channel(1);
        disk.sender = sender;
        let folder = disk.folder.as_ref().unwrap();
        let root = folder.hierarchy.as_ref().unwrap().clone();
        let findings = disk.map.findings(&root, folder.observed);
        let rows = finding_locations(&findings);
        let fixture_path = |parts: &[&str]| {
            let mut node = root.as_ref();
            let mut crumbs = Vec::new();
            for part in parts {
                let (index, child) = node
                    .children
                    .iter()
                    .enumerate()
                    .find(|(_, child)| child.filesystem_name() == std::ffi::OsStr::new(part))
                    .unwrap_or_else(|| panic!("fixture path component {part:?} exists"));
                crumbs.push(index);
                node = child;
            }
            crumbs
        };
        let expected_paths = [
            fixture_path(&["worktrees"]),
            fixture_path(&[".cache"]),
            fixture_path(&["experiments", "old-parser"]),
            fixture_path(&["experiments", "old-parser"]),
            fixture_path(&["experiments", "old-layout"]),
            fixture_path(&["experiments", "fresh-editor", "node_modules"]),
        ];
        assert_eq!(rows.len(), expected_paths.len());
        for (row, expected) in rows.iter().zip(expected_paths.iter()) {
            assert_eq!(&row.1, expected, "finding path comes from fixture names");
        }
        assert_eq!(
            rows[2].1,
            fixture_path(&["experiments", "old-parser"]),
            "the subtotal header selects its first matching child"
        );
        assert_ne!(
            rows[2].1,
            fixture_path(&["experiments"]),
            "the subtotal header never selects the mixed parent"
        );
        assert_eq!(rows[3].1, fixture_path(&["experiments", "old-parser"]));
        assert_eq!(rows[4].1, fixture_path(&["experiments", "old-layout"]));
        let selectors = [
            "disk-finding-0",
            "disk-finding-1",
            "disk-finding-2",
            "disk-finding-3",
            "disk-finding-4",
            "disk-finding-5",
        ];
        assert_eq!(rows.len(), selectors.len());
        let subtotal = findings
            .iter()
            .position(|finding| !finding.children.is_empty())
            .unwrap();
        let matching: Vec<_> = rows
            .iter()
            .filter(|(index, _, child)| *index == subtotal && *child)
            .collect();
        assert_eq!(matching.len(), 2);
        assert!(matching.iter().all(|(_, crumbs, _)| root
            .resolve(crumbs)
            .unwrap()
            .name
            .starts_with("old-")));
        disk.inspect_finding(rows[0].1.clone());
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        for (width, height) in [(1480., 930.), (1200., 720.)] {
            visual.simulate_resize(size(px(width), px(height)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let selection = visual.debug_bounds("disk-selection").unwrap();
            let volume = visual.debug_bounds("disk-volume").unwrap();
            let area = visual.debug_bounds("disk-findings").unwrap();
            assert!(area.size.height >= px(76.));
            assert!(area.top() >= selection.bottom() && area.bottom() <= volume.top());
            for (index, (_, target, _)) in rows.iter().enumerate() {
                visual.simulate_keystrokes("w");
                visual.update(|window, cx| {
                    assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
                });
                for _ in 0..index {
                    visual.simulate_keystrokes("down");
                    visual.update(|window, cx| {
                        assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
                    });
                }
                monitor.read_with(visual, |monitor, _| {
                    assert_eq!(
                        monitor.disk.as_ref().unwrap().map.finding_cursor,
                        Some(index)
                    )
                });
                visual.update(|window, cx| window.draw(cx).clear(cx));
                let area = visual.debug_bounds("disk-findings").unwrap();
                let row = visual.debug_bounds(selectors[index]).unwrap();
                assert!(
                    row.top() >= area.top() - px(0.1) && row.bottom() <= area.bottom() + px(0.1),
                    "keyboard scroll reaches finding {index}"
                );
                visual.simulate_keystrokes("enter");
                visual.update(|window, cx| {
                    assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
                });
                monitor.read_with(visual, |monitor, _| {
                    let disk = monitor.disk.as_ref().unwrap();
                    assert_eq!(
                        disk.map.selected,
                        Some(TileKind::Node {
                            crumbs: target.clone()
                        })
                    );
                    assert!(disk.confirmation.is_none());
                    assert_eq!(disk.folder.as_ref().unwrap().path, scratch.0);
                    assert!(Arc::ptr_eq(
                        disk.folder.as_ref().unwrap().hierarchy.as_ref().unwrap(),
                        &root
                    ));
                });
                assert!(matches!(
                    commands.try_recv(),
                    Err(mpsc::TryRecvError::Empty)
                ));
                monitor.update(visual, |monitor, cx| {
                    let other = if *target == rows[0].1 {
                        &rows.last().unwrap().1
                    } else {
                        &rows[0].1
                    };
                    monitor
                        .disk
                        .as_mut()
                        .unwrap()
                        .inspect_finding(other.clone());
                    cx.notify();
                });
                visual.update(|window, cx| window.draw(cx).clear(cx));
                let area = visual.debug_bounds("disk-findings").unwrap();
                let row = visual.debug_bounds(selectors[index]).unwrap();
                let click = point(
                    row.center().x,
                    row.center()
                        .y
                        .clamp(area.top() + px(1.), area.bottom() - px(1.)),
                );
                visual.simulate_click(click, Modifiers::default());
                visual.update(|window, cx| {
                    assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
                });
                monitor.read_with(visual, |monitor, _| {
                    let disk = monitor.disk.as_ref().unwrap();
                    assert_eq!(
                        disk.map.selected,
                        Some(TileKind::Node {
                            crumbs: target.clone()
                        })
                    );
                    assert!(disk.confirmation.is_none());
                });
                assert!(matches!(
                    commands.try_recv(),
                    Err(mpsc::TryRecvError::Empty)
                ));
                assert_eq!(fs::read(scratch.0.join("a")).unwrap(), sentinel);
                assert_eq!(fs::read(scratch.0.join("b")).unwrap(), sentinel);
                assert!(!scratch.0.join(".Trash").exists());
            }
        }
    }

    #[gpui::test]
    async fn findings_filter_input_wins_after_filter_button_click(cx: &mut TestAppContext) {
        cx.update(init);
        let mut disk = insights_snapshot_disk();
        let initial_selection = TileKind::Node { crumbs: vec![] };
        disk.map_select(initial_selection.clone());
        let (monitor, visual) = cx.add_window_view(|window, cx| {
            let mut monitor = crate::draw_benchmark::fixture(window, cx);
            monitor.disk = Some(disk);
            window.focus(&monitor.focus, cx);
            monitor
        });
        visual.simulate_resize(size(px(1200.), px(720.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));

        visual.simulate_keystrokes("w");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        let filter = visual.debug_bounds("map-filter").unwrap();
        visual.simulate_click(filter.center(), Modifiers::default());
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.simulate_keystrokes("j");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.simulate_keystrokes("enter");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });

        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert_eq!(disk.map.filter, "j");
            assert_eq!(disk.map.committed_filter, "j");
            assert!(disk.map.applied);
            assert!(!disk.map.filtering);
            assert_eq!(disk.map.selected, Some(initial_selection.clone()));
        });

        visual.simulate_keystrokes("w");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.simulate_keystrokes("/");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.simulate_keystrokes("k");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.simulate_keystrokes("enter");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert_eq!(disk.map.filter, "jk");
            assert_eq!(disk.map.committed_filter, "jk");
            assert_eq!(disk.map.selected, Some(initial_selection.clone()));
        });

        visual.simulate_keystrokes("/");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        monitor.read_with(visual, |monitor, _| {
            let disk = monitor.disk.as_ref().unwrap();
            assert!(disk.map.filter.is_empty());
            assert!(!disk.map.filtering);
            assert_eq!(disk.map.selected, Some(initial_selection));
        });
    }

    fn kinds_snapshot_disk() -> Disk {
        use gtop_disk::tree::{aggregate, NodeKind};
        fn dir(name: &str, bytes: u64) -> Node {
            let mut node = Node::directory(name);
            node.children
                .push(Node::entry("content", NodeKind::File, bytes));
            node
        }
        let mut disk = snapshot_disk();
        let folder = disk.folder.as_mut().unwrap();
        let root = Arc::make_mut(folder.hierarchy.as_mut().unwrap());
        let project = root
            .children
            .iter_mut()
            .find(|node| &*node.name == "projects")
            .unwrap();
        project.children[0]
            .children
            .push(Node::entry("Cargo.toml", NodeKind::File, 500));
        project.children[0].children.extend([
            dir("node_modules", 900 << 10),
            dir(".venv", 700 << 10),
            dir("Pods", 600 << 10),
        ]);
        let mut cloud = Node::directory("CloudStorage");
        cloud.directory_bytes = 650 << 10;
        cloud.in_cloud = true;
        cloud.complete = false;
        root.children.extend([
            dir("worktrees", 2 << 20),
            dir(".cargo", 2 << 20),
            cloud,
            dir(".git", 1 << 20),
            dir("Movies", 2 << 20),
            dir("Documents", 1 << 20),
            dir("Caches", 2 << 20),
            dir("target", 800 << 10),
        ]);
        aggregate(root, Metric::Bytes);
        gtop_disk::classify::classify(root);
        folder.allocated = root.bytes;
        folder.items = root.files + u64::from(root.dirs) - 1;
        disk.scan_bytes = Some(root.bytes);
        disk
    }
    fn age_snapshot_disk() -> Disk {
        let mut disk = snapshot_disk();
        disk.map.clock = Some(AGE_NOW);
        let root = Arc::make_mut(disk.folder.as_mut().unwrap().hierarchy.as_mut().unwrap());
        fn set_age(node: &mut Node, modified: i64) {
            node.modified = modified;
            for child in &mut node.children {
                set_age(child, modified);
            }
        }
        for (node, ago) in root
            .children
            .iter_mut()
            .zip([3600, 3 * 86400, 20 * 86400, AGE_NOW])
        {
            set_age(node, AGE_NOW - ago);
        }
        disk
    }

    fn snapshot_disk() -> Disk {
        use gtop_disk::tree::{aggregate, NodeKind};
        fn file(name: &str, bytes: u64) -> Node {
            Node::entry(name, NodeKind::File, bytes)
        }
        fn dir(name: &str, children: Vec<Node>) -> Node {
            let mut node = Node::directory(name);
            node.children = children;
            aggregate(&mut node, Metric::Bytes);
            node
        }
        let mut unknown = dir("unknown", vec![file("observed", 32 << 10)]);
        unknown.complete = false;
        unknown.unreadable = 1;
        let root = Arc::new(dir(
            "Home",
            vec![
                dir(
                    "projects",
                    vec![dir(
                        "gtop",
                        vec![
                            dir(
                                "target",
                                vec![file("binary", 2 << 20), file("symbols", 1 << 20)],
                            ),
                            file("equal-a", 128 << 10),
                            file("equal-b", 128 << 10),
                            file("zero", 0),
                        ],
                    )],
                ),
                dir(
                    &"a-very-long-directory-name-".repeat(7),
                    vec![file("document", 512 << 10)],
                ),
                dir(
                    "tiny",
                    (0..160)
                        .map(|i| file(&format!("file-{i:03}"), 4096))
                        .collect(),
                ),
                unknown,
            ],
        ));
        let path = PathBuf::from("/Users/demo");
        let folder = Folder {
            hierarchy: Some(root.clone()),
            root: path.clone(),
            path: path.clone(),
            state: ScanState::Complete,
            allocated: root.bytes,
            items: root.files + u64::from(root.dirs) - 1,
            errors: 1,
            rows: vec![],
            page: 0,
            pages: 1,
            cached: false,
            smaller_items: 0,
            smaller_bytes: 0,
            observed: std::time::SystemTime::now(),
        };
        let (sender, _) = mpsc::sync_channel(1);
        Disk {
            inspector_resize: Default::default(),
            volume: Some(gtop_disk::inspector::VolumeReadout::query_with(
                &path,
                |_| {
                    Ok((
                        "Data · /dev/disk3s5".into(),
                        gtop_disk::space::SpaceInfo {
                            total: 1 << 40,
                            free: 120 << 30,
                            available: 94 << 30,
                        },
                    ))
                },
            )),
            volume_query: Arc::new(gtop_disk::inspector::VolumeReadout::query),
            scan_bytes: Some(root.bytes),
            options: Default::default(),
            map: Default::default(),
            open: true,
            session: Some(ScanSession {
                root: path.clone(),
                generation: 1,
                force: false,
                options: Default::default(),
                outcome: Some(ScanState::Complete),
                view: crate::disk::ScanView {
                    path: folder.path.clone(),
                    page: 0,
                },
                activity: crate::disk::ScanActivity::Finished,
            }),
            navigation: 0,
            home: path,
            sender,
            mailbox: Arc::new(Mutex::new(Mailbox::default())),
            events: None,
            cancel: Arc::new(AtomicBool::new(false)),
            foreground: Arc::new(AtomicBool::new(false)),
            folder: Some(folder),
            selected: 0,
            confirmation: None,
            proposal_id: 0,
            sending: false,
            scroll: ScrollHandle::new(),
            row_children: Default::default(),
            status: "Complete · observed sizes".into(),
            status_kind: StatusKind::Message,
        }
    }
}
