use super::*;
use gtop_disk::{
    tree::{Metric, Node},
    treemap::{self, LayoutOptions, Rect, Tile, TileKind},
};
use std::rc::Rc;

#[derive(Default)]
pub(super) struct MapState {
    selected: Option<TileKind>,
    hovered: Option<TileKind>,
    frame: Rc<RefCell<MapFrame>>,
    list_scroll: UniformListScrollHandle,
    listing: RefCell<Option<Listing>>,
}

struct Listing {
    root: Arc<Node>,
    entries: Rc<Vec<Entry>>,
}

#[derive(Default)]
struct MapFrame {
    origin: Point<Pixels>,
    size: Size<Pixels>,
    root: Option<Arc<Node>>,
    tiles: Vec<Tile>,
}

impl MapState {
    fn entries(&self, root: &Arc<Node>) -> Rc<Vec<Entry>> {
        let mut listing = self.listing.borrow_mut();
        if let Some(cached) = &*listing {
            if Arc::ptr_eq(&cached.root, root) {
                return cached.entries.clone();
            }
        }
        let entries = Rc::new(entries(root));
        *listing = Some(Listing {
            root: root.clone(),
            entries: entries.clone(),
        });
        entries
    }
}

struct Entry {
    kind: TileKind,
    label: String,
    depth: usize,
}

fn entries(root: &Node) -> Vec<Entry> {
    fn visit(node: &Node, crumbs: &mut Vec<usize>, out: &mut Vec<Entry>) {
        let mut children: Vec<_> = node
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| child.bytes > 0)
            .collect();
        children.sort_by_key(|(_, child)| std::cmp::Reverse(child.bytes));
        let kept = children.len().min(LayoutOptions::default().max_children);
        for (rank, (index, child)) in children.iter().enumerate() {
            if rank == kept {
                out.push(Entry {
                    kind: TileKind::Others {
                        crumbs: crumbs.clone(),
                        count: children.len() - kept,
                    },
                    label: format!(
                        "Other · {} items · {}",
                        children.len() - kept,
                        bytes(children[kept..].iter().map(|(_, node)| node.bytes).sum())
                    ),
                    depth: crumbs.len(),
                });
            }

            crumbs.push(*index);
            out.push(Entry {
                kind: TileKind::Node {
                    crumbs: crumbs.clone(),
                },
                label: format!("{} · {}{}", child.name, bytes(child.bytes), coverage(child)),
                depth: crumbs.len() - 1,
            });
            if rank < kept && crumbs.len() < LayoutOptions::default().max_depth as usize {
                visit(child, crumbs, out);
            }
            crumbs.pop();
        }
        for (index, child) in node
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| child.bytes == 0)
        {
            let mut path = crumbs.clone();
            path.push(index);
            out.push(Entry {
                kind: TileKind::Node { crumbs: path },
                label: format!("{} · 0 B{}", child.name, coverage(child)),
                depth: crumbs.len(),
            });
        }
    }
    let mut out = Vec::new();
    visit(root, &mut Vec::new(), &mut out);
    out
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
        self.invalidate_proposal();
        self.map.selected = Some(kind);
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
        match key {
            "down" | "j" | "up" | "k" => {
                let entries = self.map.entries(&root);
                let selected =
                    self.map.selected.as_ref().and_then(|selected| {
                        entries.iter().position(|entry| &entry.kind == selected)
                    });
                let next = match (selected, matches!(key, "down" | "j")) {
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
            "enter" | "right" => {
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
    pub(crate) fn treemap_workspace(&self, _window: &Window, cx: &mut Context<Self>) -> Div {
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
                        treemap::hit(
                            &frame.tiles,
                            (event.position.x - frame.origin.x).as_f32(),
                            (event.position.y - frame.origin.y).as_f32(),
                        )
                        .map(|tile| tile.kind.clone())
                    };
                    if let Some(kind) = hit {
                        disk.map_select(kind);
                    }
                    window.focus(&this.focus, cx);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let disk = this.disk.as_mut().unwrap();
                let hit = {
                    let frame = disk.map.frame.borrow();
                    treemap::hit(
                        &frame.tiles,
                        (event.position.x - frame.origin.x).as_f32(),
                        (event.position.y - frame.origin.y).as_f32(),
                    )
                    .map(|tile| tile.kind.clone())
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
                        if !same_root || frame.size != bounds.size {
                            frame.tiles = hierarchy.as_ref().map_or_else(Vec::new, |root| {
                                treemap::layout(
                                    root,
                                    &[],
                                    Rect::new(
                                        0.,
                                        0.,
                                        bounds.size.width.as_f32(),
                                        bounds.size.height.as_f32(),
                                    ),
                                    Metric::Bytes,
                                    &LayoutOptions::default(),
                                )
                            });
                            frame.size = bounds.size;
                            frame.root = hierarchy.clone();
                        }
                        frame.origin = bounds.origin;
                        (hierarchy.clone(), frame.tiles.clone())
                    },
                    move |bounds, (root, tiles), window, cx| {
                        let Some(root) = root else { return };
                        for tile in tiles {
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
                            window.paint_quad(quad(
                                rect,
                                Corners::default(),
                                rgb(if selected.as_ref() == Some(&tile.kind) {
                                    t.selection
                                } else {
                                    t.base.plate
                                }),
                                Edges::all(px(1.)),
                                rgb(edge),
                                BorderStyle::Solid,
                            ));
                            let (name, size_text) = match &tile.kind {
                                TileKind::Node { crumbs } => root.resolve(crumbs).map_or(
                                    (String::new(), String::new()),
                                    |node| {
                                        (
                                            format!("{}{}", node.name, coverage(node)),
                                            bytes(node.bytes),
                                        )
                                    },
                                ),
                                TileKind::Others { count, .. } => {
                                    (format!("Other · {count} items"), String::new())
                                }
                            };
                            let band = tile.header.unwrap_or(tile.rect);
                            let mask = Bounds::new(
                                bounds.origin + point(px(band.x), px(band.y)),
                                size(px(band.w), px(band.h)),
                            );
                            let minimum_height = if tile.header.is_some() { 12. } else { 16. };
                            if mask.size.width < px(44.) || mask.size.height < px(minimum_height) {
                                continue;
                            }
                            let size_line = tile.header.map(|_| {
                                window.text_system().shape_line(
                                    size_text.clone().into(),
                                    px(11.),
                                    &[TextRun {
                                        len: size_text.len(),
                                        font: font("JetBrains Mono"),
                                        color: rgb(t.base.silk).into(),
                                        ..Default::default()
                                    }],
                                    None,
                                )
                            });
                            let label = if tile.header.is_some() {
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
                                    color: rgb(t.base.silk).into(),
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
                                        mask.origin + point(px(4.), px(1.)),
                                        px(17.),
                                        TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    );
                                },
                            );
                            if let Some(line) = size_line {
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
                            let rect = Bounds::new(
                                bounds.origin + point(px(tile.rect.x), px(tile.rect.y)),
                                size(px(tile.rect.w), px(tile.rect.h)),
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
            || "Starting disk scan…".into(),
            |folder| folder.path.display().to_string(),
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
                let label = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned();
                crumbs = crumbs.child(
                    Button::new(("map-crumb", index))
                        .label(label)
                        .rack(&t)
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.disk.as_mut().unwrap().enter(path.clone());
                            window.focus(&this.focus, cx);
                            cx.notify();
                        })),
                );
            }
        }
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
        div().flex().size_full().gap(px(8.)).min_w_0()
            .child(div().flex().flex_col().flex_1().min_w_0().min_h_0().gap(px(8.))
                .child(div().bg(rgb(t.base.plate)).border_1().border_color(rgb(t.rule_hi)).p(px(12.))
                    .child(crate::rack::label("TREEMAP · ALLOCATED BLOCKS · DEPTH 4", &t))
                    .child(current_path(path)).child(crumbs))
                .child(canvas)
                .child(div().h(px(20.)).overflow_hidden().text_size(px(10.)).child(hover))
                .child(div().bg(rgb(t.base.plate)).border_1().border_color(rgb(t.rule)).p(px(8.))
                    .child("D Top · ↑ ↓ select · Enter open · Backspace up · X Trash")
                    .child(div().text_size(px(10.)).child("Observed sizes. Unknown space has no tile area. Hover informs; click selects."))))
            .children(self.disk_panel(crate::details::DetailWidth::new(360.), cx))
    }

    pub(super) fn treemap_inspector(&self, cx: &mut Context<Self>) -> Div {
        let disk = self.disk.as_ref().unwrap();
        let folder = disk.folder.as_ref().unwrap();
        let root = folder.hierarchy.as_ref().unwrap();
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let mut selection = div()
            .flex_shrink_0()
            .py(px(8.))
            .child(crate::rack::label("SELECTION", &t));
        if let Some(row) = disk.map_row() {
            selection = selection
                .child(
                    div()
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
                            .child(entry.label.clone())
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
            .child(crate::rack::label("FOLDERS AND FILES", &t))
            .child(
                div()
                    .id("map-folder-list")
                    .debug_selector(|| "map-folder-list".into())
                    .role(Role::List)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(list),
            )
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
            visual.simulate_keystrokes("down");
        }
        visual.update(|window, cx| {
            assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let list = visual.debug_bounds("map-folder-list").unwrap();
        let other = visual.debug_bounds("map-entry-other").unwrap();
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
                visual.simulate_keystrokes("down");
                visual.update(|window, cx| {
                    assert_eq!(window.focused(cx), Some(monitor.read(cx).focus.clone()))
                });
            }
            monitor.read_with(visual, |monitor, _| {
                assert!(monitor.disk.as_ref().unwrap().map_row().is_none())
            });
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let other = visual.debug_bounds("map-entry-other").unwrap();
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
                    visual.simulate_keystrokes("down");
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
                visual.simulate_keystrokes("up");
            }
            let directory_index = omitted
                .iter()
                .position(|item| item == &path.join("omitted-dir"))
                .unwrap();
            for _ in 0..directory_index {
                visual.simulate_keystrokes("down");
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
                    visual.simulate_keystrokes("down");
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
                    visual.simulate_keystrokes("down");
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
            let mut disk = snapshot_disk();
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
                .save(output.join(format!("treemap-{width:.0}x{height:.0}.png")))
                .unwrap();
        }
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
            cleanable: vec![],
            page: 0,
            pages: 1,
            cached: false,
            smaller_items: 0,
            smaller_bytes: 0,
            observed: std::time::SystemTime::now(),
        };
        let (sender, _) = mpsc::sync_channel(1);
        Disk {
            map: Default::default(),
            open: true,
            session: Some(ScanSession {
                root: path.clone(),
                generation: 1,
                outcome: Some(ScanState::Complete),
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
        }
    }
}
