use crate::{
    disk_scan::{self, Confirmation, Folder, ScanState, Tree},
    model::bytes,
    rack::RackButton,
    Monitor,
};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants},
    Disableable, Sizable,
};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

enum Command {
    Scan(PathBuf, bool, Arc<AtomicBool>, u64),
    Browse(PathBuf, usize, u64, u64),
    Trash(Confirmation, Arc<AtomicBool>),
    Prepare(PathBuf, u64, disk_scan::FileId, u64, u64),
}
#[derive(Default)]
struct Mailbox {
    proposal: Option<(u64, u64, Result<Confirmation, String>)>,
    folder: Option<(u64, u64, Folder)>,
    status: Option<String>,
}
pub(super) struct Disk {
    pub open: bool,
    generation: u64,
    navigation: u64,
    home: PathBuf,
    sender: mpsc::SyncSender<Command>,
    mailbox: Arc<Mutex<Mailbox>>,
    cancel: Arc<AtomicBool>,
    pub folder: Option<Folder>,
    pub selected: usize,
    pub confirmation: Option<Confirmation>,
    proposal_id: u64,
    sending: bool,
    scroll: ScrollHandle,
    status: String,
}
impl Drop for Disk {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Disk {
    fn new() -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()));
        Self::with_home(home)
    }
    fn with_home(home: PathBuf) -> Self {
        let (sender, receiver) = mpsc::sync_channel(32);
        let mailbox = Arc::new(Mutex::new(Mailbox::default()));
        let output = mailbox.clone();
        let worker_home = home.clone();
        std::thread::Builder::new()
            .name("disk-scan".into())
            .spawn(move || worker(receiver, output, worker_home))
            .expect("start disk worker");
        let mut disk = Self {
            open: true,
            generation: 0,
            navigation: 0,
            home: home.clone(),
            sender,
            mailbox,
            cancel: Arc::new(AtomicBool::new(false)),
            folder: None,
            selected: 0,
            confirmation: None,
            proposal_id: 0,
            sending: false,
            scroll: ScrollHandle::new(),
            status: String::new(),
        };
        disk.start(home, false);
        disk
    }
    fn start(&mut self, root: PathBuf, force: bool) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.confirmation = None;
        self.proposal_id += 1;
        self.generation += 1;
        self.navigation = 0;
        self.selected = 0;
        self.folder = None;
        let _ = self.sender.try_send(Command::Scan(
            root,
            force,
            self.cancel.clone(),
            self.generation,
        ));
    }
    fn browse(&mut self, path: PathBuf, page: usize) {
        self.confirmation = None;
        self.proposal_id += 1;
        self.selected = 0;
        self.navigation += 1;
        self.scroll.set_offset(point(px(0.), px(0.)));
        let _ = self.sender.try_send(Command::Browse(
            path,
            page,
            self.generation,
            self.navigation,
        ));
    }
    fn enter(&mut self, path: PathBuf) {
        self.browse(path, 0);
    }
    fn up(&mut self) {
        if let Some(folder) = self.folder.as_ref() {
            if folder.path != folder.root {
                if let Some(parent) = folder.path.parent() {
                    self.enter(parent.into());
                }
            }
        }
    }
    fn request_trash(&mut self, path: &Path, allocated: u64, identity: disk_scan::FileId) {
        if self.sending {
            return;
        }
        if !self
            .folder
            .as_ref()
            .is_some_and(|f| f.state == ScanState::Complete)
        {
            self.status = "Finish the scan before cleanup".into();
            return;
        }
        self.proposal_id += 1;
        self.confirmation = None;
        self.status = "Checking cleanup path…".into();
        let _ = self.sender.try_send(Command::Prepare(
            path.into(),
            allocated,
            identity,
            self.generation,
            self.proposal_id,
        ));
    }

    fn confirm(&mut self, rendered_proposal: u64) {
        if rendered_proposal != self.proposal_id {
            return;
        }
        if let Some(confirmation) = self.confirmation.take() {
            self.sending = true;
            self.cancel = Arc::new(AtomicBool::new(false));
            if let Some(folder) = &mut self.folder {
                folder.state = ScanState::Scanning;
            }
            self.status = "Moving item to Trash…".into();
            let _ = self
                .sender
                .try_send(Command::Trash(confirmation, self.cancel.clone()));
        }
    }
    pub fn key(&mut self, key: &str) -> bool {
        if self.confirmation.is_some() {
            match key {
                "y" | "enter" => self.confirm(self.proposal_id),
                "n" | "escape" => self.confirmation = None,
                _ => {}
            }
            return true;
        }
        match key {
            "escape" => {
                self.open = false;
                self.proposal_id += 1;
            }
            "backspace" | "left" => self.up(),
            "down" | "j" => {
                self.selected = (self.selected + 1).min(
                    self.folder
                        .as_ref()
                        .map_or(0, |f| f.rows.len().saturating_sub(1)),
                )
            }
            "up" | "k" => self.selected = self.selected.saturating_sub(1),
            "enter" | "right" => {
                if let Some(row) = self
                    .folder
                    .as_ref()
                    .and_then(|f| f.rows.get(self.selected))
                    .filter(|n| n.directory && !n.skipped_volume)
                {
                    self.enter(row.path.clone());
                }
            }
            "x" => {
                if let Some(row) = self.folder.as_ref().and_then(|f| f.rows.get(self.selected)) {
                    self.request_trash(&row.path.clone(), row.allocated, row.identity);
                }
            }
            "c" => self.cancel.store(true, Ordering::Relaxed),
            "r" => {
                if let Some(f) = &self.folder {
                    self.start(f.root.clone(), true);
                }
            }
            "pagedown" | "pageup" => {
                if let Some(f) = &self.folder {
                    self.browse(
                        f.path.clone(),
                        if key == "pagedown" {
                            (f.page + 1).min(f.pages - 1)
                        } else {
                            f.page.saturating_sub(1)
                        },
                    );
                }
            }
            _ => return false,
        }
        self.scroll.scroll_to_item(
            self.selected + 6 + usize::from(self.folder.as_ref().is_some_and(|f| f.errors > 0)),
        );
        true
    }
}
fn publish(
    output: &Mutex<Mailbox>,
    tree: &Tree,
    path: &Path,
    page: usize,
    cached: bool,
    generation: u64,
    navigation: u64,
) {
    let folder = tree.folder(path, page, cached);
    output.lock().unwrap().folder = Some((generation, navigation, folder));
}
fn worker(receiver: mpsc::Receiver<Command>, output: Arc<Mutex<Mailbox>>, home: PathBuf) {
    let mut cache: HashMap<PathBuf, Tree> = HashMap::new();
    let mut active: Option<Tree> = None;
    let mut generation = 0;
    let mut queue = VecDeque::new();
    while let Some(command) = queue.pop_front().or_else(|| receiver.recv().ok()) {
        match command {
            Command::Scan(root, force, token, scan_generation) => {
                generation = scan_generation;
                if let Some(tree) = active.take().filter(|t| t.state == ScanState::Complete) {
                    cache.insert(tree.nodes[0].path.clone(), tree);
                }
                let mut browsing = root.clone();
                let mut page = 0;
                let mut navigation = 0;
                if !force {
                    if let Some(tree) = cache.remove(&root).filter(|t| {
                        std::fs::symlink_metadata(&root).is_ok_and(|m| {
                            use std::os::unix::fs::MetadataExt;
                            t.nodes[0].identity
                                == (disk_scan::FileId {
                                    device: m.dev(),
                                    inode: m.ino(),
                                })
                        })
                    }) {
                        publish(
                            &output, &tree, &browsing, page, true, generation, navigation,
                        );
                        active = Some(tree);
                        continue;
                    }
                }
                cache.remove(&root);
                while cache.len() > 1 {
                    let key = cache.keys().next().unwrap().clone();
                    cache.remove(&key);
                }
                let tree = disk_scan::scan(&root, &home, &token, |tree| {
                    while let Ok(command) = receiver.try_recv() {
                        match command {
                            Command::Browse(path, p, g, n) if g == generation => {
                                browsing = path;
                                page = p;
                                navigation = n;
                            }
                            Command::Browse(..) => {}
                            other => queue.push_back(other),
                        }
                    }
                    publish(
                        &output, tree, &browsing, page, false, generation, navigation,
                    );
                });
                publish(
                    &output, &tree, &browsing, page, false, generation, navigation,
                );
                active = Some(tree);
            }
            Command::Browse(path, p, g, n) => {
                if g == generation {
                    if let Some(tree) = &active {
                        publish(&output, tree, &path, p, true, generation, n);
                    }
                }
            }
            Command::Prepare(path, size, identity, g, proposal) => {
                let result = if g == generation
                    && active
                        .as_ref()
                        .is_some_and(|t| t.state == ScanState::Complete)
                {
                    Confirmation::new(&path, size, identity, &home).map_err(|e| e.to_string())
                } else {
                    Err("Scan changed. Rescan before cleanup.".into())
                };
                output.lock().unwrap().proposal = Some((g, proposal, result));
            }
            Command::Trash(confirmation, token) => {
                let result = confirmation.trash(&home);
                let succeeded = result.is_ok();
                output.lock().unwrap().status = Some(match result {
                    Ok(destination) => format!(
                        "Moved to {}. Rescanning to refresh sizes.",
                        destination.display()
                    ),
                    Err(e) => format!("Move to Trash failed: {e}"),
                });
                if succeeded {
                    cache.clear();
                    if let Some(tree) = active.take() {
                        queue.push_front(Command::Scan(
                            tree.nodes[0].path.clone(),
                            true,
                            token,
                            generation,
                        ));
                    }
                }
            }
        }
    }
}
impl Monitor {
    pub(super) fn open_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.details = None;
        self.moment = None;
        self.settings_open = false;
        if let Some(disk) = &mut self.disk {
            disk.open = true;
        } else {
            self.disk = Some(Disk::new());
            cx.spawn(async move |entity, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if entity
                    .update(cx, |this, cx| {
                        if let Some(disk) = &mut this.disk {
                            let mut mailbox = disk.mailbox.lock().unwrap();
                            let folder = mailbox.folder.take();
                            let status = mailbox.status.take();
                            let proposal = mailbox.proposal.take();
                            drop(mailbox);
                            if let Some((g, id, result)) = proposal {
                                if g == disk.generation && id == disk.proposal_id && disk.open {
                                    match result {
                                        Ok(proposal) => {
                                            disk.confirmation = Some(proposal);
                                            disk.status.clear();
                                        }
                                        Err(e) => disk.status = e,
                                    }
                                    cx.notify();
                                }
                            }
                            if let Some(status) = status {
                                disk.status = status;
                                disk.sending = false;
                                cx.notify();
                            }
                            if let Some((_, _, folder)) = folder
                                .filter(|(g, n, _)| *g == disk.generation && *n == disk.navigation)
                            {
                                disk.selected =
                                    disk.selected.min(folder.rows.len().saturating_sub(1));
                                disk.folder = Some(folder);
                                if disk.open {
                                    cx.notify();
                                }
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            })
            .detach();
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }
    pub(super) fn disk_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let disk = self.disk.as_ref().filter(|d| d.open)?;
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let mut body = div().flex().flex_col().gap(px(6.)).p(px(14.));
        if let Some(folder) = &disk.folder {
            let state = match &folder.state {
                ScanState::Scanning => "Scanning · partial sizes".into(),
                ScanState::Complete => {
                    if folder.cached {
                        "Complete · session cache".into()
                    } else {
                        "Complete".into()
                    }
                }
                ScanState::Cancelled => "Cancelled · partial sizes".into(),
                ScanState::Failed(e) => format!("Scan failed: {e}"),
            };
            body = body
                .child(crate::rack::label(state, &t))
                .child(format!(
                    "{} · {} items",
                    bytes(folder.allocated),
                    folder.items
                ))
                .child(
                    div()
                        .text_size(px(10.))
                        .child("Attributed blocks · hard links counted once · one volume"),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .child(folder.path.display().to_string()),
                );
            let mut crumbs = div().flex().flex_wrap().gap(px(4.));
            let mut ancestors: Vec<_> = folder
                .path
                .ancestors()
                .take_while(|p| p.starts_with(&folder.root))
                .collect();
            ancestors.reverse();
            for (i, path) in ancestors.into_iter().enumerate() {
                let path = path.to_path_buf();
                let name = if path == folder.root {
                    "Root".into()
                } else {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                };
                crumbs = crumbs.child(
                    Button::new(("disk-crumb", i))
                        .label(name)
                        .rack(&t)
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.disk.as_mut().unwrap().enter(path.clone());
                            cx.notify();
                        })),
                );
            }
            body = body
                .child(crumbs)
                .child(div().text_size(px(10.)).child(format!(
                    "Observed {}s ago. Moving to Trash does not free space immediately.",
                    folder.observed.elapsed().unwrap_or_default().as_secs()
                )));
            if folder.errors > 0 {
                body = body.child(div().text_color(rgb(t.base.hot)).text_size(px(11.)).child(format!("{} unreadable entries. Sizes are incomplete. Full Disk Access may allow more folders.",folder.errors)));
            }
            for (index, row) in folder.rows.iter().enumerate() {
                let path = row.path.clone();
                let directory = row.directory && !row.skipped_volume;
                let share = row.allocated as f64 / folder.allocated.max(1) as f64;
                let suffix = if row.skipped_volume {
                    " · other volume"
                } else if row.unreadable > 0 {
                    " · unreadable"
                } else if row.shared {
                    " · counted elsewhere"
                } else if row.directory {
                    "/"
                } else {
                    ""
                };
                body = body.child(
                    div()
                        .id(("disk-row", index))
                        .cursor_pointer()
                        .p(px(6.))
                        .border_b_1()
                        .border_color(rgb(t.rule))
                        .when(index == disk.selected, |d| d.bg(rgb(t.rule_hi)))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .gap(px(8.))
                                .child(div().min_w_0().truncate().child(format!(
                                    "{}{suffix}",
                                    row.path.file_name().unwrap_or_default().to_string_lossy()
                                )))
                                .child(if row.unreadable > 0 {
                                    format!("{} + unknown", bytes(row.allocated))
                                } else {
                                    bytes(row.allocated)
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(t.silk_dim))
                                .child(format!("{:.1}% · {} items", share * 100., row.descendants)),
                        )
                        .child(
                            div().h(px(3.)).w_full().bg(rgb(t.seg_off)).child(
                                div()
                                    .h_full()
                                    .w(relative(share.clamp(0., 1.) as f32))
                                    .bg(rgb(t.base.led)),
                            ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let disk = this.disk.as_mut().unwrap();
                            disk.confirmation = None;
                            disk.selected = index;
                            if directory {
                                disk.enter(path.clone());
                            }
                            cx.notify();
                        })),
                );
            }
            body = body.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        Button::new("disk-previous")
                            .label("Prev")
                            .rack(&t)
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let d = this.disk.as_mut().unwrap();
                                if let Some(f) = &d.folder {
                                    d.browse(f.path.clone(), f.page.saturating_sub(1));
                                }
                                cx.notify();
                            })),
                    )
                    .child(format!("{} / {}", folder.page + 1, folder.pages))
                    .child(
                        Button::new("disk-next")
                            .label("Next")
                            .rack(&t)
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let d = this.disk.as_mut().unwrap();
                                if let Some(f) = &d.folder {
                                    d.browse(f.path.clone(), (f.page + 1).min(f.pages - 1));
                                }
                                cx.notify();
                            })),
                    ),
            );
            body = body.child(crate::rack::label(
                "CLEANABLE · LARGEST 24 · REVIEW FIRST",
                &t,
            ));
            if folder.cleanable.is_empty() {
                body = body.child("No known caches found yet");
            }
            for (i, row) in folder.cleanable.iter().enumerate() {
                let path = row.path.clone();
                let allocated = row.allocated;
                let identity = row.identity;
                body = body.child(
                    div()
                        .border_t_1()
                        .border_color(rgb(t.rule))
                        .py(px(6.))
                        .child(div().text_size(px(11.)).child(format!(
                            "{} · {}",
                            bytes(allocated),
                            row.path.display()
                        )))
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(t.silk_dim))
                                .child(row.cleanable.unwrap()),
                        )
                        .child(
                            Button::new(("disk-clean", i))
                                .label("Review cleanup")
                                .rack(&t)
                                .small()
                                .disabled(row.path.starts_with(disk.home.join(".Trash")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.disk
                                        .as_mut()
                                        .unwrap()
                                        .request_trash(&path, allocated, identity);
                                    cx.notify();
                                })),
                        ),
                );
            }
        } else {
            body = body.child("Starting disk scan…");
        }
        body = body.child(
            div()
                .text_size(px(10.))
                .child("↑ ↓ select · Enter open · Backspace up · X trash · C cancel · R rescan"),
        );
        let mut panel = div()
            .relative()
            .child(crate::rack::screws(&t))
            .flex()
            .flex_col()
            .w(px(380.))
            .flex_shrink_0()
            .h_full()
            .min_h_0()
            .bg(rgb(t.base.plate))
            .border_1()
            .border_color(rgb(t.rule_hi))
            .child(
                div()
                    .p(px(18.))
                    .border_b_1()
                    .border_color(rgb(t.rule))
                    .child(crate::rack::label("DISK SPACE", &t))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(5.))
                            .child(
                                Button::new("disk-home")
                                    .label("Home")
                                    .rack(&t)
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let d = this.disk.as_mut().unwrap();
                                        d.start(d.home.clone(), false);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("disk-system")
                                    .label("Data volume")
                                    .rack(&t)
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.disk
                                            .as_mut()
                                            .unwrap()
                                            .start(disk_scan::system_root(), false);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("disk-cancel")
                                    .label("Cancel")
                                    .rack(&t)
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.disk
                                            .as_ref()
                                            .unwrap()
                                            .cancel
                                            .store(true, Ordering::Relaxed);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("disk-rescan")
                                    .label("Rescan")
                                    .rack(&t)
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let d = this.disk.as_mut().unwrap();
                                        if let Some(f) = &d.folder {
                                            d.start(f.root.clone(), true);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                body.id("disk-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&disk.scroll),
            );

        if let Some(confirmation) = &disk.confirmation {
            let rendered_proposal = disk.proposal_id;
            panel=panel.child(div().id("disk-confirmation").role(Role::AlertDialog).p(px(12.)).border_1().border_color(rgb(t.base.hot))
                .child(format!("Move {} to Trash? Scanned size {}. Allocated size is a past observation. Incomplete folders may contain more. Trash retains disk space.",confirmation.path.display(),bytes(confirmation.allocated)))
                .child(Button::new("disk-confirm").label("Move to Trash · Y").rack(&t).danger().small().on_click(cx.listener(move |this,_,_,cx| { this.disk.as_mut().unwrap().confirm(rendered_proposal); cx.notify(); })))
                .child(Button::new("disk-keep").label("Keep · N").rack(&t).small().on_click(cx.listener(|this,_,_,cx| { this.disk.as_mut().unwrap().confirmation=None; cx.notify(); }))));
        }
        panel = panel
            .child(
                div()
                    .id("disk-status")
                    .role(Role::Status)
                    .p(px(10.))
                    .text_size(px(10.))
                    .child(disk.status.clone()),
            )
            .child(
                Button::new("disk-close")
                    .label("Close · Esc")
                    .rack(&t)
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.disk.as_mut().unwrap().open = false;
                        cx.notify();
                    })),
            );
        Some(panel)
    }
}

#[cfg(test)]
impl Monitor {
    pub(super) fn prepare_disk_draw_benchmark(&mut self) {
        let disk = Disk::new();
        std::thread::sleep(Duration::from_millis(250));
        let folder = disk
            .mailbox
            .lock()
            .unwrap()
            .folder
            .take()
            .expect("scan must publish progress")
            .2;
        assert_eq!(
            folder.state,
            ScanState::Scanning,
            "benchmark requires a pending real scan"
        );
        assert!(folder.items > 0);
        let mut disk = disk;
        disk.folder = Some(folder);
        self.disk = Some(disk);
    }
    pub(super) fn assert_disk_scan_pending(&self) {
        let disk = self.disk.as_ref().unwrap();
        if let Some((_, _, folder)) = &disk.mailbox.lock().unwrap().folder {
            assert_eq!(
                folder.state,
                ScanState::Scanning,
                "scan finished during benchmark"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    use std::{fs, time::Instant};
    fn scratch() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gtop-disk-service-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }
    fn receive(disk: &mut Disk, expected: ScanState) -> Folder {
        let start = Instant::now();
        loop {
            let value = disk.mailbox.lock().unwrap().folder.take();
            if let Some((g, n, folder)) = value {
                assert_eq!((g, n), (disk.generation, disk.navigation));
                if folder.state == expected {
                    return folder;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "worker timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn completed_scan_is_cached_until_explicit_rescan() {
        let home = scratch();
        fs::write(home.join("one"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let first = receive(&mut disk, ScanState::Complete);
        assert_eq!(first.items, 1);
        assert!(!first.cached);
        fs::write(home.join("two"), [2; 4096]).unwrap();
        disk.start(home.clone(), false);
        let cached = receive(&mut disk, ScanState::Complete);
        assert_eq!(cached.items, 1);
        assert!(cached.cached);
        disk.start(home.clone(), true);
        let refreshed = receive(&mut disk, ScanState::Complete);
        assert_eq!(refreshed.items, 2);
        assert!(!refreshed.cached);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn confirmation_requires_separate_event_and_only_matching_proposal_consumes_once() {
        let home = scratch();
        let path = home.join("cache");
        fs::write(&path, [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let folder = receive(&mut disk, ScanState::Complete);
        let row = folder.rows[0].clone();
        disk.folder = Some(folder);
        disk.confirm(0);
        assert!(!disk.sending);
        disk.request_trash(&path, row.allocated, row.identity);
        let start = Instant::now();
        let proposal = loop {
            if let Some((_, _, result)) = disk.mailbox.lock().unwrap().proposal.take() {
                break result.unwrap();
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        };
        disk.confirmation = Some(proposal);
        disk.confirm(disk.proposal_id + 1);
        assert!(disk.confirmation.is_some());
        // Use a disconnected sender so this test never moves any data.
        let (sender, receiver) = mpsc::sync_channel(32);
        drop(receiver);
        disk.sender = sender;
        disk.confirm(disk.proposal_id);
        assert!(disk.confirmation.is_none());
        assert!(disk.sending);
        disk.sending = false;
        disk.confirm(disk.proposal_id);
        assert!(!disk.sending);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn stale_navigation_and_generation_replies_are_identifiable() {
        let home = scratch();
        fs::create_dir(home.join("child")).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let _ = receive(&mut disk, ScanState::Complete);
        let generation = disk.generation;
        disk.browse(home.join("child"), 0);
        let folder = receive(&mut disk, ScanState::Complete);
        assert_eq!(folder.path, home.join("child"));
        assert_eq!(disk.navigation, 1);
        disk.start(home.clone(), true);
        assert!(disk.generation > generation);
        assert_eq!(disk.navigation, 0);
        let _ = receive(&mut disk, ScanState::Complete);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
}
