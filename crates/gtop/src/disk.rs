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
};

enum Command {
    Scan(PathBuf, bool, Arc<AtomicBool>, u64),
    DropCache,
    Browse(PathBuf, usize, u64, u64, Arc<AtomicBool>),
    Trash(Confirmation, Arc<AtomicBool>, u64, u64, PathBuf, usize),
    Prepare(PathBuf, u64, disk_scan::FileId, u64, u64, u64),
}
#[derive(Default)]
struct Mailbox {
    proposal: Option<(u64, u64, Result<Confirmation, String>)>,
    folder: Option<(u64, u64, Folder)>,
    status: Option<String>,
    wake: Option<futures::channel::mpsc::Sender<()>>,
}
impl Mailbox {
    fn notify(&mut self) {
        if let Some(wake) = &mut self.wake {
            let _ = wake.try_send(());
        }
    }
}
pub(super) struct Disk {
    pub open: bool,
    generation: u64,
    navigation: u64,
    home: PathBuf,
    sender: mpsc::SyncSender<Command>,
    mailbox: Arc<Mutex<Mailbox>>,
    events: Option<futures::channel::mpsc::Receiver<()>>,
    cancel: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    pub folder: Option<Folder>,
    pub selected: usize,
    pub confirmation: Option<Confirmation>,
    proposal_id: u64,
    sending: bool,
    scroll: ScrollHandle,
    row_children: std::cell::RefCell<Vec<(PathBuf, usize)>>,
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
        let (wake, events) = futures::channel::mpsc::channel(1);
        let mailbox = Arc::new(Mutex::new(Mailbox {
            wake: Some(wake),
            ..Mailbox::default()
        }));
        let output = mailbox.clone();
        let worker_home = home.clone();
        let release = Arc::new(AtomicBool::new(false));
        let worker_release = release.clone();
        std::thread::Builder::new()
            .name("disk-scan".into())
            .spawn(move || worker(receiver, output, worker_home, worker_release))
            .expect("start disk worker");
        let mut disk = Self {
            open: true,
            generation: 0,
            navigation: 0,
            home: home.clone(),
            sender,
            mailbox,
            events: Some(events),
            cancel: Arc::new(AtomicBool::new(false)),
            release,
            folder: None,
            selected: 0,
            confirmation: None,
            proposal_id: 0,
            sending: false,
            scroll: ScrollHandle::new(),
            row_children: Default::default(),
            status: String::new(),
        };
        disk.start(home, false);
        disk
    }
    pub fn close(&mut self) {
        self.open = false;
        self.cancel.store(true, Ordering::Relaxed);
        self.release.store(true, Ordering::Release);
        self.generation += 1;
        self.navigation += 1;
        let _ = self.sender.try_send(Command::DropCache);
        self.folder = None;
        self.invalidate_proposal();
    }
    fn invalidate_proposal(&mut self) {
        if self.status == "Checking cleanup path…" {
            self.status.clear();
        }
        self.confirmation = None;
        self.proposal_id += 1;
    }
    fn select(&mut self, index: usize) {
        self.invalidate_proposal();
        self.selected = index;
    }
    fn apply_proposal(
        &mut self,
        generation: u64,
        proposal_id: u64,
        result: Result<Confirmation, String>,
    ) -> bool {
        if generation != self.generation || proposal_id != self.proposal_id || !self.open {
            return false;
        }
        match result {
            Ok(proposal) => {
                self.confirmation = Some(proposal);
                self.status.clear();
            }
            Err(error) => self.status = error,
        }
        true
    }
    fn accepted(&mut self, command: Command) -> bool {
        match self.sender.try_send(command) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) => {
                self.status = "Disk worker busy. Try again.".into();
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.status = "Disk worker unavailable. Reopen the application.".into();
                false
            }
        }
    }
    fn start(&mut self, root: PathBuf, force: bool) {
        let cancel = Arc::new(AtomicBool::new(false));
        let generation = self.generation + 1;
        if !self.accepted(Command::Scan(root, force, cancel.clone(), generation)) {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = cancel;
        self.invalidate_proposal();
        self.generation = generation;
        self.navigation = 0;
        self.selected = 0;
        self.folder = None;
    }
    fn browse(&mut self, path: PathBuf, page: usize) {
        let navigation = self.navigation + 1;
        let token = Arc::new(AtomicBool::new(false));
        if !self.accepted(Command::Browse(
            path,
            page,
            self.generation,
            navigation,
            token.clone(),
        )) {
            return;
        }
        self.invalidate_proposal();
        self.selected = 0;
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = token;
        self.navigation = navigation;
        self.scroll.set_offset(point(px(0.), px(0.)));
    }
    fn change_page(&mut self, forward: bool) {
        if let Some(folder) = &self.folder {
            let page = if forward {
                (folder.page + 1).min(folder.pages.saturating_sub(1))
            } else {
                folder.page.saturating_sub(1)
            };
            if page != folder.page {
                self.browse(folder.path.clone(), page);
            }
        }
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
    fn request_trash(&mut self, row: &disk_scan::Row) {
        if row.in_cloud {
            self.status = "Cloud items cannot be prepared for cleanup".into();
            return;
        }
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
        let proposal = self.proposal_id + 1;
        if !self.accepted(Command::Prepare(
            row.path.clone(),
            row.allocated,
            row.identity,
            self.generation,
            self.navigation,
            proposal,
        )) {
            return;
        }
        self.proposal_id = proposal;
        self.confirmation = None;
        self.status = "Checking cleanup path…".into();
    }

    fn confirm(&mut self, rendered_proposal: u64) {
        if rendered_proposal != self.proposal_id || !self.open || self.sending {
            return;
        }
        if let Some(confirmation) = self.confirmation.take() {
            let token = Arc::new(AtomicBool::new(false));
            match self.sender.try_send(Command::Trash(
                confirmation,
                token.clone(),
                self.generation,
                self.navigation,
                self.folder.as_ref().unwrap().path.clone(),
                self.folder.as_ref().unwrap().page,
            )) {
                Ok(()) => {
                    self.sending = true;
                    self.cancel = token;
                    self.status = "Moving item to Trash…".into();
                }
                Err(error) => {
                    self.status = match &error {
                        mpsc::TrySendError::Full(_) => "Disk worker busy. Try again.",
                        mpsc::TrySendError::Disconnected(_) => {
                            "Disk worker unavailable. Reopen the application."
                        }
                    }
                    .into();
                    let (mpsc::TrySendError::Full(Command::Trash(confirmation, ..))
                    | mpsc::TrySendError::Disconnected(Command::Trash(confirmation, ..))) = error
                    else {
                        unreachable!()
                    };
                    self.confirmation = Some(confirmation);
                }
            }
        }
    }
    pub fn key(&mut self, key: &str) -> bool {
        if !self.open {
            return false;
        }
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
                self.close();
            }
            "backspace" | "left" => self.up(),
            "down" | "j" => {
                let selected = (self.selected + 1).min(
                    self.folder
                        .as_ref()
                        .map_or(0, |f| f.rows.len().saturating_sub(1)),
                );
                if selected != self.selected {
                    self.select(selected);
                }
            }
            "up" | "k" => {
                let selected = self.selected.saturating_sub(1);
                if selected != self.selected {
                    self.select(selected);
                }
            }
            "enter" | "right" => {
                if let Some(row) = self
                    .folder
                    .as_ref()
                    .and_then(|f| f.rows.get(self.selected))
                    .filter(|n| n.directory && !n.skipped_volume && !n.in_cloud)
                {
                    self.enter(row.path.clone());
                }
            }
            "x" => {
                if let Some(row) = self
                    .folder
                    .as_ref()
                    .and_then(|f| f.rows.get(self.selected))
                    .cloned()
                {
                    self.request_trash(&row);
                }
            }
            "c" => self.cancel.store(true, Ordering::Relaxed),
            "r" => {
                if let Some(f) = &self.folder {
                    self.start(f.root.clone(), true);
                }
            }
            "pagedown" | "pageup" => self.change_page(key == "pagedown"),
            _ => return false,
        }
        if let Some(row) = self.folder.as_ref().and_then(|f| f.rows.get(self.selected)) {
            if let Some((_, child)) = self
                .row_children
                .borrow()
                .iter()
                .find(|(path, _)| *path == row.path)
            {
                self.scroll.scroll_to_item(*child);
            }
        }
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
    let mut mailbox = output.lock().unwrap();
    mailbox.folder = Some((generation, navigation, folder));
    mailbox.notify();
}
fn publish_browse(
    output: &Mutex<Mailbox>,
    tree: &Tree,
    path: &Path,
    page: usize,
    home: &Path,
    token: &AtomicBool,
    identity: (u64, u64),
) -> bool {
    let (generation, navigation) = identity;
    if path == tree.root || !path.starts_with(&tree.root) {
        publish(
            output, tree, &tree.root, page, false, generation, navigation,
        );
        return tree.state == ScanState::Complete && !token.load(Ordering::Relaxed);
    }
    let subtree = disk_scan::scan_beneath(&tree.root, path, home, token, |subtree| {
        let mut folder = subtree.folder(path, page, false);
        folder.root = tree.root.clone();
        let mut mailbox = output.lock().unwrap();
        mailbox.folder = Some((generation, navigation, folder));
        mailbox.notify();
    });
    let mut folder = subtree.folder(path, page, false);
    folder.root = tree.root.clone();
    let mut mailbox = output.lock().unwrap();
    mailbox.folder = Some((generation, navigation, folder));
    mailbox.notify();
    subtree.state == ScanState::Complete && !token.load(Ordering::Relaxed)
}
fn prepare_cleanup(
    completed: Option<(u64, u64)>,
    requested: (u64, u64),
    path: &Path,
    size: u64,
    identity: disk_scan::FileId,
    home: &Path,
) -> Result<Confirmation, String> {
    if completed != Some(requested) {
        return Err("Scan changed. Rescan before cleanup.".into());
    }
    Confirmation::new(path, size, identity, home).map_err(|e| e.to_string())
}
fn worker(
    receiver: mpsc::Receiver<Command>,
    output: Arc<Mutex<Mailbox>>,
    home: PathBuf,
    release: Arc<AtomicBool>,
) {
    worker_with_trash(receiver, output, home, release, |confirmation, home| {
        confirmation.trash(home)
    });
}
fn worker_with_trash(
    receiver: mpsc::Receiver<Command>,
    output: Arc<Mutex<Mailbox>>,
    home: PathBuf,
    release: Arc<AtomicBool>,
    mut trash: impl FnMut(Confirmation, &Path) -> std::io::Result<PathBuf>,
) {
    disk_scan::background_priority();
    let mut cache: HashMap<PathBuf, Tree> = HashMap::new();
    let mut active: Option<Tree> = None;
    let mut generation = 0;
    let mut completed = None;
    let mut queue = VecDeque::new();
    while let Some(command) = queue.pop_front().or_else(|| receiver.recv().ok()) {
        if release.swap(false, Ordering::AcqRel) {
            active = None;
            cache.clear();
            completed = None;
        }
        match command {
            Command::DropCache => {
                completed = None;
                active = None;
                cache.clear();
                let mut mailbox = output.lock().unwrap();
                mailbox.folder = None;
                mailbox.proposal = None;
            }
            Command::Scan(root, force, token, scan_generation) => {
                if token.load(Ordering::Relaxed) {
                    continue;
                }
                generation = scan_generation;
                if let Some(tree) = active.take().filter(|t| t.state == ScanState::Complete) {
                    cache.insert(tree.root.clone(), tree);
                }
                let browsing = root.clone();
                let page = 0;
                let navigation = 0;
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
                        completed = Some((generation, navigation));
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
                        queue.push_back(command);
                    }
                    publish(
                        &output, tree, &browsing, page, false, generation, navigation,
                    );
                });
                publish(
                    &output, &tree, &browsing, page, false, generation, navigation,
                );
                completed = (tree.state == ScanState::Complete && !token.load(Ordering::Relaxed))
                    .then_some((generation, navigation));
                active = Some(tree);
            }
            Command::Browse(path, p, g, n, token) => {
                if g == generation && !token.load(Ordering::Relaxed) {
                    if let Some(tree) = &active {
                        completed =
                            publish_browse(&output, tree, &path, p, &home, &token, (generation, n))
                                .then_some((generation, n));
                    }
                }
            }
            Command::Prepare(path, size, identity, g, n, proposal) => {
                let result = prepare_cleanup(completed, (g, n), &path, size, identity, &home);
                let mut mailbox = output.lock().unwrap();
                mailbox.proposal = Some((g, proposal, result));
                mailbox.notify();
            }
            Command::Trash(confirmation, token, g, n, path, p) => {
                let result = trash(confirmation, &home);
                let mut mailbox = output.lock().unwrap();
                let final_status = match result {
                    Ok(destination) => format!("Moved to {}.", destination.display()),
                    Err(e) => format!("Move to Trash failed: {e}"),
                };
                mailbox.status = Some(format!("{final_status} Rescanning to refresh sizes."));
                mailbox.notify();
                drop(mailbox);
                cache.clear();
                if g == generation {
                    if let Some(tree) = active.take() {
                        let root = tree.root.clone();
                        let browsing = path;
                        let page = p;
                        let navigation = n;
                        let refreshed = disk_scan::scan(&root, &home, &token, |tree| {
                            while let Ok(command) = receiver.try_recv() {
                                queue.push_back(command);
                            }
                            if browsing == tree.root || tree.state != ScanState::Complete {
                                publish(
                                    &output, tree, &browsing, page, false, generation, navigation,
                                );
                            }
                        });
                        completed = publish_browse(
                            &output,
                            &refreshed,
                            &browsing,
                            page,
                            &home,
                            &token,
                            (generation, navigation),
                        )
                        .then_some((generation, navigation));
                        active = Some(refreshed);
                        let mut mailbox = output.lock().unwrap();
                        mailbox.status = Some(final_status);
                        mailbox.notify();
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
            if disk.folder.is_none() {
                disk.start(disk.home.clone(), false);
            }
        } else {
            self.disk = Some(Disk::new());
            let mut events = self.disk.as_mut().unwrap().events.take().unwrap();
            cx.spawn(async move |entity, cx| {
                use futures::StreamExt;
                while events.next().await.is_some() {
                    if entity
                        .update(cx, |this, cx| {
                            if let Some(disk) = &mut this.disk {
                                let mut mailbox = disk.mailbox.lock().unwrap();
                                let folder = mailbox.folder.take();
                                let status = mailbox.status.take();
                                let proposal = mailbox.proposal.take();
                                drop(mailbox);
                                if let Some((g, id, result)) = proposal {
                                    if disk.apply_proposal(g, id, result) {
                                        cx.notify();
                                    }
                                }
                                if let Some(status) = status {
                                    disk.status = status;
                                    disk.sending = false;
                                    cx.notify();
                                }
                                if let Some((_, _, folder)) = folder.filter(|(g, n, _)| {
                                    disk.open && *g == disk.generation && *n == disk.navigation
                                }) {
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
            let mut header = vec![
                crate::rack::label(state, &t).into_any_element(),
                div()
                    .child(format!(
                        "{} · {} items",
                        bytes(folder.allocated),
                        folder.items
                    ))
                    .into_any_element(),
                div()
                    .text_size(px(10.))
                    .child("Attributed blocks · hard links counted once · one volume")
                    .into_any_element(),
                div()
                    .text_size(px(10.))
                    .child(folder.path.display().to_string())
                    .into_any_element(),
            ];
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
            header.push(crumbs.into_any_element());
            header.push(
                div()
                    .text_size(px(10.))
                    .child(format!(
                        "Observed {}s ago. Moving to Trash does not free space immediately.",
                        folder.observed.elapsed().unwrap_or_default().as_secs()
                    ))
                    .into_any_element(),
            );
            if folder.errors > 0 {
                header.push(div().text_color(rgb(t.base.hot)).text_size(px(11.)).child(format!("{} unreadable entries. Sizes are incomplete. Full Disk Access may allow more folders.", folder.errors)).into_any_element());
            }
            if folder.smaller_items > 0 {
                header.push(
                    div()
                        .child(format!(
                            "{} smaller items · {}. Open a folder to scan its contents.",
                            folder.smaller_items,
                            bytes(folder.smaller_bytes)
                        ))
                        .into_any_element(),
                );
            }
            let row_start = header.len();
            body = body.children(header);
            disk.row_children.borrow_mut().clear();
            for (index, row) in folder.rows.iter().enumerate() {
                disk.row_children
                    .borrow_mut()
                    .push((row.path.clone(), row_start + index));
                let path = row.path.clone();
                let directory = row.directory && !row.skipped_volume && !row.in_cloud;
                let share = row.allocated as f64 / folder.allocated.max(1) as f64;
                let suffix = if row.skipped_volume {
                    " · other volume"
                } else if row.in_cloud {
                    " · in cloud"
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
                        .debug_selector(move || format!("disk-row-{index}"))
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
                            disk.select(index);
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
                            .disabled(folder.page == 0)
                            .label("Prev")
                            .rack(&t)
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let d = this.disk.as_mut().unwrap();
                                d.change_page(false);
                                cx.notify();
                            })),
                    )
                    .child(format!("{} / {}", folder.page + 1, folder.pages))
                    .child(
                        Button::new("disk-next")
                            .disabled(folder.page + 1 >= folder.pages)
                            .label("Next")
                            .rack(&t)
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let d = this.disk.as_mut().unwrap();
                                d.change_page(true);
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
                let row = row.clone();
                body = body.child(
                    div()
                        .border_t_1()
                        .border_color(rgb(t.rule))
                        .py(px(6.))
                        .child(div().text_size(px(11.)).child(format!(
                            "{} · {}",
                            bytes(row.allocated),
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
                                    this.disk.as_mut().unwrap().request_trash(&row);
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
                    .debug_selector(|| "disk-body".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&disk.scroll),
            );

        if let Some(confirmation) = &disk.confirmation {
            let rendered_proposal = disk.proposal_id;
            panel=panel.child(div().id("disk-confirmation").role(Role::AlertDialog).p(px(12.)).border_1().border_color(rgb(t.base.hot))
                .child(format!("Move {} to Trash? Scanned size {}. Allocated size is a past observation. Incomplete folders may contain more. Trash retains disk space. Finder Put Back is unavailable for items gtop moves to Trash. To restore this item, move it from Trash to its original folder.",confirmation.path.display(),bytes(confirmation.allocated)))
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
                        this.disk.as_mut().unwrap().close();
                        cx.notify();
                    })),
            );
        Some(panel)
    }
}

#[cfg(test)]
impl Monitor {
    pub(super) fn prepare_disk_scroll_test(&mut self, home: PathBuf) {
        let mut disk = Disk::with_home(home);
        let start = std::time::Instant::now();
        loop {
            let value = disk.mailbox.lock().unwrap().folder.take();
            if let Some((_, _, folder)) = value {
                if folder.state == ScanState::Complete {
                    assert_eq!(folder.rows.len(), 128);
                    assert_eq!(folder.smaller_items, 32);
                    disk.folder = Some(folder);
                    break;
                }
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(10));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        disk.selected = 126;
        self.disk = Some(disk);
    }
    pub(super) fn disk_selected_for_test(&self) -> usize {
        self.disk.as_ref().unwrap().selected
    }
    pub(super) fn prepare_disk_draw_benchmark(&mut self) {
        let disk = Disk::new();
        std::thread::sleep(std::time::Duration::from_millis(250));
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
    use std::{
        fs,
        time::{Duration, Instant},
    };
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
                if (g, n) == (disk.generation, disk.navigation) && folder.state == expected {
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
    fn receive_proposal(disk: &Disk) -> (u64, u64, Result<Confirmation, String>) {
        let start = Instant::now();
        loop {
            if let Some(proposal) = disk.mailbox.lock().unwrap().proposal.take() {
                return proposal;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "worker timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn missing_trash_is_created_and_worker_refreshes_after_native_move() {
        use std::os::unix::fs::MetadataExt;
        let home = scratch();
        fs::create_dir_all(home.join("node_modules/pkg")).unwrap();
        fs::write(home.join("node_modules/pkg/index.js"), b"scratch fixture").unwrap();
        let expected = fs::metadata(home.join("node_modules")).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let folder = receive(&mut disk, ScanState::Complete);
        let row = folder
            .rows
            .iter()
            .find(|row| row.path.ends_with("node_modules"))
            .unwrap()
            .clone();
        disk.folder = Some(folder);
        disk.request_trash(&row);
        let (generation, proposal, result) = receive_proposal(&disk);
        assert!(disk.apply_proposal(generation, proposal, result));
        disk.confirm(disk.proposal_id);
        let folder = receive(&mut disk, ScanState::Complete);
        let start = Instant::now();
        let status = loop {
            if let Some(status) = disk
                .mailbox
                .lock()
                .unwrap()
                .status
                .clone()
                .filter(|s| !s.contains("Rescanning"))
            {
                break status;
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        };
        let destination = home.join(".Trash/node_modules");
        assert_eq!(status, format!("Moved to {}.", destination.display()));
        assert!(!folder.cleanable.iter().any(|item| item.path == row.path));
        assert!(!row.path.exists());
        let moved = fs::metadata(&destination).unwrap();
        assert_eq!((moved.dev(), moved.ino()), (expected.dev(), expected.ino()));
        assert_eq!(
            fs::read(destination.join("pkg/index.js")).unwrap(),
            b"scratch fixture"
        );
        disk.close();
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn boundary_page_commands_do_not_rescan() {
        let home = scratch();
        let child = home.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(child.join("one"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        disk.folder = Some(receive(&mut disk, ScanState::Complete));
        disk.enter(child.clone());
        disk.folder = Some(receive(&mut disk, ScanState::Complete));
        let navigation = disk.navigation;
        fs::write(child.join("two"), [2; 4096]).unwrap();
        for key in ["pagedown", "pageup"] {
            disk.key(key);
            assert_eq!(disk.navigation, navigation);
            assert_eq!(disk.folder.as_ref().unwrap().items, 1);
        }
        assert!(disk.mailbox.lock().unwrap().folder.is_none());
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn completed_subtree_can_prepare_after_root_cancellation() {
        let home = scratch();
        let child = home.join("child");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("file"), [1; 4096]).unwrap();
        let token = AtomicBool::new(false);
        let root = disk_scan::scan(&home, &home, &token, |_| {
            token.store(true, Ordering::Relaxed);
        });
        assert_eq!(root.state, ScanState::Cancelled);
        let output = Mutex::new(Mailbox::default());
        let completed = publish_browse(
            &output,
            &root,
            &child,
            0,
            &home,
            &AtomicBool::new(false),
            (7, 3),
        )
        .then_some((7, 3));
        let (_, _, folder) = output.lock().unwrap().folder.take().unwrap();
        assert_eq!(folder.state, ScanState::Complete);
        assert_eq!(folder.root, home);
        let row = folder.rows[0].clone();
        assert!(prepare_cleanup(
            completed,
            (7, 3),
            &row.path,
            row.allocated,
            row.identity,
            &home
        )
        .is_ok());
        for identity in [(7, 2), (6, 3)] {
            assert!(prepare_cleanup(
                completed,
                identity,
                &row.path,
                row.allocated,
                row.identity,
                &home
            )
            .is_err());
        }
        assert!(
            prepare_cleanup(None, (7, 3), &row.path, row.allocated, row.identity, &home).is_err()
        );
        let completed_root = publish_browse(
            &output,
            &root,
            &home,
            0,
            &home,
            &AtomicBool::new(false),
            (7, 4),
        );
        assert!(!completed_root);
        let mut disk = Disk::with_home(home.clone());
        let _ = receive(&mut disk, ScanState::Complete);
        let mut incomplete = folder;
        incomplete.state = ScanState::Cancelled;
        disk.folder = Some(incomplete);
        disk.request_trash(&row);
        assert!(disk.status.contains("Finish the scan"));
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn cloud_row_cannot_queue_cleanup_preparation() {
        let home = scratch();
        fs::write(home.join("local-item"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let mut folder = receive(&mut disk, ScanState::Complete);
        let mut cloud = folder.rows[0].clone();
        cloud.path = home.join("missing-cloud-item");
        cloud.in_cloud = true;
        folder.rows[0] = cloud;
        disk.folder = Some(folder);
        let (sender, receiver) = mpsc::sync_channel(1);
        disk.sender = sender;

        assert!(disk.key("x"));

        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(disk.mailbox.lock().unwrap().proposal.is_none());
        assert!(disk.confirmation.is_none());
        assert!(disk.status.contains("Cloud"));
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn close_full_queue_invalidates_replies_and_requests_release() {
        let home = scratch();
        let mut disk = Disk::with_home(home.clone());
        disk.folder = Some(receive(&mut disk, ScanState::Complete));
        let (sender, _receiver) = mpsc::sync_channel(1);
        disk.sender = sender;
        disk.browse(home.clone(), 0);
        let identity = (disk.generation, disk.navigation);
        let token = disk.cancel.clone();
        disk.close();
        assert!(disk.release.load(Ordering::Acquire));
        assert!(token.load(Ordering::Relaxed));
        assert_ne!(identity, (disk.generation, disk.navigation));
        assert!(disk.folder.is_none());
        disk.open = true;
        assert_ne!(identity, (disk.generation, disk.navigation));
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn cancelled_browse_is_superseded_by_fresh_navigation() {
        let home = scratch();
        fs::create_dir(home.join("child")).unwrap();
        fs::write(home.join("child/file"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let _ = receive(&mut disk, ScanState::Complete);
        disk.cancel.store(true, Ordering::Relaxed);
        disk.browse(home.join("child"), 0);
        let first = disk.cancel.clone();
        let child = receive(&mut disk, ScanState::Complete);
        assert_eq!(child.items, 1);
        for _ in 0..16 {
            disk.browse(home.join("child"), 0);
        }
        disk.browse(home.clone(), 0);
        assert!(first.load(Ordering::Relaxed));
        assert!(!disk.cancel.load(Ordering::Relaxed));
        let root = receive(&mut disk, ScanState::Complete);
        assert_eq!(root.path, home);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn full_queue_retains_confirmation_and_last_accepted_identity() {
        let home = scratch();
        let path = home.join("cache");
        fs::write(&path, [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let folder = receive(&mut disk, ScanState::Complete);
        let row = folder.rows[0].clone();
        disk.folder = Some(folder);
        let (sender, receiver) = mpsc::sync_channel(32);
        disk.sender = sender;
        for n in 0..32 {
            disk.browse(home.clone(), n);
        }
        let accepted = (disk.generation, disk.navigation, disk.proposal_id);
        disk.confirmation =
            Some(Confirmation::new(&path, row.allocated, row.identity, &home).unwrap());
        disk.start(home.clone(), true);
        assert_eq!(
            (disk.generation, disk.navigation, disk.proposal_id),
            accepted
        );
        assert!(disk.folder.is_some());
        disk.browse(home.join("unaccepted"), 0);
        assert_eq!(disk.navigation, accepted.1);
        disk.request_trash(&row);
        assert_eq!(disk.proposal_id, accepted.2);
        disk.confirm(disk.proposal_id);
        assert!(disk.confirmation.is_some());
        assert!(!disk.sending);
        assert!(disk.status.contains("busy"));
        for n in 1..=32 {
            assert!(
                matches!(receiver.recv().unwrap(), Command::Browse(_, _, _, navigation, _) if navigation == n)
            );
        }
        disk.confirm(disk.proposal_id);
        assert!(disk.confirmation.is_none());
        assert!(disk.sending);
        assert_eq!(disk.folder.as_ref().unwrap().state, ScanState::Complete);
        assert!(matches!(receiver.recv().unwrap(), Command::Trash(..)));
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn close_invalidates_proposals_and_hidden_enter_cannot_cleanup() {
        let home = scratch();
        fs::write(home.join("cache"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let folder = receive(&mut disk, ScanState::Complete);
        let row = folder.rows[0].clone();
        disk.folder = Some(folder);
        disk.request_trash(&row);
        let pending = disk.proposal_id;
        disk.close();
        assert!(disk.proposal_id > pending);
        assert!(disk.confirmation.is_none());
        assert!(!disk.key("enter"));
        disk.open = true;
        assert_ne!(disk.proposal_id, pending);
        disk.confirmation =
            Some(Confirmation::new(&row.path, row.allocated, row.identity, &home).unwrap());
        disk.close();
        disk.open = true;
        assert!(disk.confirmation.is_none());
        assert!(!disk.sending);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn row_click_rejects_pending_cleanup_reply() {
        let home = scratch();
        fs::write(home.join("a"), [1; 4096]).unwrap();
        fs::write(home.join("b"), [2; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let folder = receive(&mut disk, ScanState::Complete);
        let row = folder.rows[0].clone();
        let other_index = 1;
        disk.folder = Some(folder);
        disk.request_trash(&row);
        let (generation, proposal_id, result) = receive_proposal(&disk);
        disk.select(other_index);
        assert!(!disk.apply_proposal(generation, proposal_id, result));
        assert_eq!(disk.selected, other_index);
        assert!(disk.confirmation.is_none());
        assert!(!disk.sending);
        assert!(disk.status.is_empty());
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn keyboard_selection_rejects_pending_cleanup_reply() {
        let home = scratch();
        fs::write(home.join("a"), [1; 4096]).unwrap();
        fs::write(home.join("b"), [2; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let folder = receive(&mut disk, ScanState::Complete);
        let row = folder.rows[0].clone();
        disk.folder = Some(folder);
        disk.request_trash(&row);
        let (generation, proposal_id, result) = receive_proposal(&disk);
        disk.key("down");
        assert!(!disk.apply_proposal(generation, proposal_id, result));
        assert_eq!(disk.selected, 1);
        assert!(disk.confirmation.is_none());
        assert!(!disk.sending);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn cleanup_outcomes_refresh_browsed_folder_with_matching_identity() {
        for outcome in ["success", "failure", "unknown"] {
            let home = scratch();
            fs::create_dir_all(home.join("child")).unwrap();
            let path = home.join("child/cache");
            fs::write(&path, [1; 4096]).unwrap();
            let mut disk = Disk::with_home(home.clone());
            let _ = receive(&mut disk, ScanState::Complete);
            let (sender, receiver) = mpsc::sync_channel(32);
            disk.sender = sender;
            let output = disk.mailbox.clone();
            let worker_home = home.clone();
            let worker = std::thread::spawn(move || {
                worker_with_trash(
                    receiver,
                    output,
                    worker_home,
                    Arc::new(AtomicBool::new(false)),
                    |confirmation, _| {
                        let destination = confirmation.path.with_file_name("moved");
                        if outcome != "failure" {
                            fs::rename(&confirmation.path, &destination)?;
                        }
                        if outcome == "success" {
                            Ok(destination)
                        } else {
                            Err(std::io::Error::other(outcome))
                        }
                    },
                )
            });
            disk.start(home.clone(), true);
            let _ = receive(&mut disk, ScanState::Complete);
            disk.browse(home.join("child"), 0);
            let folder = receive(&mut disk, ScanState::Complete);
            let row = folder.rows[0].clone();
            disk.folder = Some(folder);
            disk.confirmation =
                Some(Confirmation::new(&row.path, row.allocated, row.identity, &home).unwrap());
            disk.confirm(disk.proposal_id);
            assert_eq!(disk.folder.as_ref().unwrap().state, ScanState::Complete);
            let folder = receive(&mut disk, ScanState::Complete);
            assert_eq!(folder.path, home.join("child"));
            assert_eq!(
                folder.rows[0].path.file_name().unwrap(),
                if outcome == "failure" {
                    "cache"
                } else {
                    "moved"
                }
            );
            let status = disk.mailbox.lock().unwrap().status.take().unwrap();
            assert!(!status.contains("Rescanning"), "{status}");
            drop(disk);
            worker.join().unwrap();
            fs::remove_dir_all(home).unwrap();
        }
    }
    #[test]
    fn mailbox_notifications_are_bounded_and_idle_has_no_wake() {
        use futures::{FutureExt, StreamExt};
        let (wake, mut events) = futures::channel::mpsc::channel(1);
        let mut mailbox = Mailbox {
            wake: Some(wake),
            ..Mailbox::default()
        };
        assert!(events.next().now_or_never().is_none());
        for _ in 0..1000 {
            mailbox.notify();
        }
        let mut count = 0;
        while events.next().now_or_never().is_some() {
            count += 1;
        }
        assert!(count > 0 && count <= 2);
        assert!(events.next().now_or_never().is_none());
        mailbox.notify();
        assert_eq!(events.next().now_or_never(), Some(Some(())));
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
        disk.request_trash(&row);
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
        assert!(disk.confirmation.is_some());
        assert!(!disk.sending);
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
