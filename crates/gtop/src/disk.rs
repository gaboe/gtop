use crate::{
    disk_scan::{self, Confirmation, Folder, ScanState, Tree},
    model::bytes,
    rack::RackButton,
    Monitor, SESSION_RETENTION,
};
use gpui::{prelude::*, *};
use gpui_component::{
    button::{Button, ButtonVariants},
    Disableable, Sizable,
};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Instant,
};

#[derive(Default)]
enum Retention {
    #[default]
    Open,
    Closed(Instant),
    Expired,
}
impl Retention {
    fn expire(&mut self, now: Instant) -> bool {
        if matches!(self, Self::Closed(deadline) if now >= *deadline) {
            *self = Self::Expired;
        }
        matches!(self, Self::Expired)
    }
    fn close(&mut self, deadline: Instant) {
        if !matches!(self, Self::Expired) {
            *self = Self::Closed(deadline);
        }
    }
    fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Closed(deadline) => Some(*deadline),
            _ => None,
        }
    }
}

fn cancel_expired_scan(retention: &RefCell<Retention>, token: &AtomicBool) {
    if retention.borrow_mut().expire(Instant::now()) {
        token.store(true, Ordering::Relaxed);
    }
}

fn service_retention(
    receiver: &mpsc::Receiver<Command>,
    queue: &RefCell<VecDeque<Command>>,
    retention: &RefCell<Retention>,
    output: &Mutex<Mailbox>,
    generation: u64,
    token: &AtomicBool,
    empty: bool,
) {
    let mut queue = queue.borrow_mut();
    while let Ok(command) = receiver.try_recv() {
        queue.push_back(command);
    }
    while let Some(command) = queue.front() {
        match command {
            Command::Close(deadline) => retention.borrow_mut().close(*deadline),
            Command::Reopen(now) => {
                let mut retention = retention.borrow_mut();
                let expired = retention.expire(*now);
                if !expired {
                    *retention = Retention::Open;
                }
                if expired || empty {
                    let mut mailbox = output.lock().unwrap();
                    mailbox.resume = Some(generation);
                    mailbox.notify();
                }
            }
            _ => break,
        }
        queue.pop_front();
    }
    cancel_expired_scan(retention, token);
}

enum Command {
    Scan(PathBuf, bool, Arc<AtomicBool>, u64),
    Close(Instant),
    Reopen(Instant),
    Browse(PathBuf, usize, u64, u64, Arc<AtomicBool>),
    Trash(Confirmation, Arc<AtomicBool>, u64, u64, PathBuf, usize),
    Prepare(PathBuf, u64, disk_scan::FileId, u64, u64, u64),
}
#[derive(Default)]
struct Mailbox {
    proposal: Option<(u64, u64, Result<Confirmation, String>)>,
    expired: Option<u64>,
    #[cfg(test)]
    scan_gate: Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>,
    outcome: Option<(u64, ScanState)>,
    resume: Option<u64>,
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
struct ScanSession {
    root: PathBuf,
    generation: u64,
    outcome: Option<ScanState>,
}

pub(super) struct Disk {
    pub open: bool,
    session: Option<ScanSession>,
    navigation: u64,
    home: PathBuf,
    sender: mpsc::SyncSender<Command>,
    mailbox: Arc<Mutex<Mailbox>>,
    events: Option<futures::channel::mpsc::Receiver<()>>,
    cancel: Arc<AtomicBool>,
    foreground: Arc<AtomicBool>,
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
    fn new(visible: bool) -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()));
        Self::with_home_and_visibility(home, visible)
    }
    #[cfg(test)]
    fn with_home(home: PathBuf) -> Self {
        Self::with_home_and_visibility(home, false)
    }
    fn with_home_and_visibility(home: PathBuf, visible: bool) -> Self {
        let (sender, receiver) = mpsc::sync_channel(32);
        let (wake, events) = futures::channel::mpsc::channel(1);
        let mailbox = Arc::new(Mutex::new(Mailbox {
            wake: Some(wake),
            ..Mailbox::default()
        }));
        let output = mailbox.clone();
        let worker_home = home.clone();
        let foreground = Arc::new(AtomicBool::new(visible));
        let worker_foreground = foreground.clone();
        std::thread::Builder::new()
            .name("disk-scan".into())
            .spawn(move || worker(receiver, output, worker_home, worker_foreground))
            .expect("start disk worker");
        let mut disk = Self {
            open: true,
            session: None,
            navigation: 0,
            home: home.clone(),
            sender,
            mailbox,
            events: Some(events),
            cancel: Arc::new(AtomicBool::new(false)),
            foreground,
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
    pub(super) fn set_visible(&self, visible: bool) {
        self.foreground
            .store(visible && self.open, Ordering::Release);
    }
    pub fn close(&mut self) {
        if !self.open {
            return;
        }
        if !self.accepted(Command::Close(Instant::now() + SESSION_RETENTION)) {
            return;
        }
        self.open = false;
        self.set_visible(false);
        self.invalidate_proposal();
    }
    fn reopen(&mut self, now: Instant, visible: bool) {
        if !self.accepted(Command::Reopen(now)) {
            return;
        }
        self.open = true;
        self.set_visible(visible);
        if self.folder.is_none()
            && !self
                .session
                .as_ref()
                .is_some_and(|session| session.outcome.is_none())
        {
            self.start_over();
        }
    }
    fn generation(&self) -> u64 {
        self.session
            .as_ref()
            .map_or(0, |session| session.generation)
    }
    fn apply_outcome(&mut self, generation: u64, outcome: ScanState) {
        if let Some(session) = self.session.as_mut().filter(|s| s.generation == generation) {
            session.outcome = Some(outcome);
        }
    }
    fn apply_expiry(&mut self, generation: u64) {
        if generation != self.generation() {
            return;
        }
        self.folder = None;
        self.invalidate_proposal();
        if self.open {
            self.start_over();
        }
    }
    fn consume_mailbox(&mut self) -> bool {
        let mut mailbox = self.mailbox.lock().unwrap();
        let expired = mailbox.expired.take();
        let folder = mailbox.folder.take();
        let status = mailbox.status.take();
        let proposal = mailbox.proposal.take();
        let outcome = mailbox.outcome.take();
        let resume = mailbox.resume.take();
        drop(mailbox);
        if let Some((generation, outcome)) = outcome {
            self.apply_outcome(generation, outcome);
        }
        if expired == Some(self.generation()) {
            self.apply_expiry(self.generation());
        } else if self.open && resume == Some(self.generation()) {
            self.start_over();
        }
        if let Some((g, id, result)) = proposal {
            self.apply_proposal(g, id, result);
        }
        if let Some(status) = status {
            self.status = status;
            self.sending = false;
        }
        if let Some((_, _, folder)) =
            folder.filter(|(g, n, _)| *g == self.generation() && *n == self.navigation)
        {
            self.selected = self.selected.min(folder.rows.len().saturating_sub(1));
            self.folder = Some(folder);
            return true;
        }
        false
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
        if generation != self.generation() || proposal_id != self.proposal_id || !self.open {
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
        let generation = self.generation() + 1;
        if !self.accepted(Command::Scan(
            root.clone(),
            force,
            cancel.clone(),
            generation,
        )) {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = cancel;
        self.session = Some(ScanSession {
            root,
            generation,
            outcome: None,
        });
        self.invalidate_proposal();
        self.navigation = 0;
        self.selected = 0;
        self.folder = None;
    }
    fn start_over(&mut self) {
        let root = self
            .session
            .as_ref()
            .map(|session| session.root.clone())
            .unwrap_or_else(|| self.home.clone());
        self.start(root, true);
    }
    fn browse(&mut self, path: PathBuf, page: usize) {
        let navigation = self.navigation + 1;
        let token = Arc::new(AtomicBool::new(false));
        if !self.accepted(Command::Browse(
            path,
            page,
            self.generation(),
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
            self.generation(),
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
                self.generation(),
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
                self.start_over();
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
    #[cfg(test)]
    if tree.state == ScanState::Scanning {
        let gate = output.lock().unwrap().scan_gate.take();
        if let Some((started, resume)) = gate {
            started.send(()).unwrap();
            resume.recv().unwrap();
        }
    }
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
    control: ScanControl<'_>,
) -> bool {
    let ScanControl {
        token,
        priority,
        identity,
        service,
    } = control;
    let (generation, navigation) = identity;
    if path == tree.root || !path.starts_with(&tree.root) {
        publish(
            output, tree, &tree.root, page, false, generation, navigation,
        );
        return tree.state == ScanState::Complete && !token.load(Ordering::Relaxed);
    }
    let mut published_complete = false;
    priority.update();
    let subtree = disk_scan::scan_beneath(
        &tree.root,
        path,
        home,
        token,
        |subtree| {
            service();
            published_complete |= subtree.state == ScanState::Complete;
            let mut folder = subtree.folder(path, page, false);
            folder.root = tree.root.clone();
            let mut mailbox = output.lock().unwrap();
            mailbox.folder = Some((generation, navigation, folder));
            mailbox.notify();
        },
        || {
            service();
            priority.update()
        },
    );
    priority.restore();
    if !published_complete {
        let mut folder = subtree.folder(path, page, false);
        folder.root = tree.root.clone();
        let mut mailbox = output.lock().unwrap();
        mailbox.folder = Some((generation, navigation, folder));
        mailbox.notify();
    }
    subtree.state == ScanState::Complete && !token.load(Ordering::Relaxed)
}
struct ScanControl<'a> {
    token: &'a AtomicBool,
    priority: &'a mut ScanPriority,
    identity: (u64, u64),
    service: &'a dyn Fn(),
}
struct ScanPriority {
    foreground: Arc<AtomicBool>,
    applied_foreground: bool,
}
impl ScanPriority {
    fn new(foreground: Arc<AtomicBool>) -> Self {
        disk_scan::background_priority();
        Self {
            foreground,
            applied_foreground: false,
        }
    }
    fn update(&mut self) -> disk_scan::ScanUrgency {
        let foreground = self.foreground.load(Ordering::Acquire);
        self.set(foreground);
        if foreground {
            disk_scan::ScanUrgency::Foreground
        } else {
            disk_scan::ScanUrgency::Background
        }
    }
    fn restore(&mut self) {
        self.set(false);
    }
    fn set(&mut self, foreground: bool) {
        if self.applied_foreground == foreground {
            return;
        }
        if foreground {
            disk_scan::foreground_priority();
        } else {
            disk_scan::background_priority();
        }
        self.applied_foreground = foreground;
    }
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
    foreground: Arc<AtomicBool>,
) {
    worker_with_trash(receiver, output, home, foreground, |confirmation, home| {
        confirmation.trash(home)
    });
}
fn worker_with_trash(
    receiver: mpsc::Receiver<Command>,
    output: Arc<Mutex<Mailbox>>,
    home: PathBuf,
    foreground: Arc<AtomicBool>,
    mut trash: impl FnMut(Confirmation, &Path) -> std::io::Result<PathBuf>,
) {
    let mut priority = ScanPriority::new(foreground);
    let mut cache: HashMap<PathBuf, Tree> = HashMap::new();
    let mut active: Option<Tree> = None;
    let mut generation = 0;
    let mut completed = None;
    let retention = RefCell::new(Retention::default());
    let queue = RefCell::new(VecDeque::new());
    loop {
        service_retention(
            &receiver,
            &queue,
            &retention,
            &output,
            generation,
            &AtomicBool::new(false),
            active.is_none(),
        );
        let expired = retention.borrow_mut().expire(Instant::now());
        if expired {
            active = None;
            cache.clear();
            completed = None;
            let mut mailbox = output.lock().unwrap();
            mailbox.folder = None;
            mailbox.proposal = None;
            mailbox.expired = Some(generation);
            mailbox.notify();
        }
        let command = if let Some(command) = queue.borrow_mut().pop_front() {
            command
        } else {
            let deadline = retention.borrow().deadline();
            let result = match deadline {
                Some(deadline) => {
                    receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                }
                None => receiver
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            };
            match result {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };
        match command {
            Command::Close(deadline) => {
                retention.borrow_mut().close(deadline);
            }
            Command::Reopen(now) => {
                let mut retained = retention.borrow_mut();
                let expired = retained.expire(now);
                if !expired {
                    *retained = Retention::Open;
                }
                if expired || active.is_none() {
                    let mut mailbox = output.lock().unwrap();
                    mailbox.resume = Some(generation);
                    mailbox.notify();
                }
            }
            Command::Scan(root, force, token, scan_generation) => {
                let expired = retention.borrow_mut().expire(Instant::now());
                if expired {
                    active = None;
                    cache.clear();
                }
                *retention.borrow_mut() = Retention::default();
                generation = scan_generation;
                if token.load(Ordering::Relaxed) {
                    active = None;
                    completed = None;
                    let mut mailbox = output.lock().unwrap();
                    mailbox.outcome = Some((generation, ScanState::Cancelled));
                    mailbox.notify();
                    continue;
                }
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
                        let mut mailbox = output.lock().unwrap();
                        mailbox.outcome = Some((generation, tree.state.clone()));
                        mailbox.notify();
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
                priority.update();
                let tree = disk_scan::scan_with_hook(
                    &root,
                    &home,
                    &token,
                    |tree| {
                        service_retention(
                            &receiver, &queue, &retention, &output, generation, &token, false,
                        );
                        publish(
                            &output, tree, &browsing, page, false, generation, navigation,
                        );
                    },
                    || {
                        service_retention(
                            &receiver, &queue, &retention, &output, generation, &token, false,
                        );
                        priority.update()
                    },
                );
                priority.restore();
                publish(
                    &output, &tree, &browsing, page, false, generation, navigation,
                );
                completed = (tree.state == ScanState::Complete && !token.load(Ordering::Relaxed))
                    .then_some((generation, navigation));
                let outcome = tree.state.clone();
                active = Some(tree);
                let mut mailbox = output.lock().unwrap();
                mailbox.outcome = Some((generation, outcome));
                mailbox.notify();
            }
            Command::Browse(path, p, g, n, token) => {
                if g == generation && !token.load(Ordering::Relaxed) {
                    if let Some(tree) = &active {
                        completed = publish_browse(
                            &output,
                            tree,
                            &path,
                            p,
                            &home,
                            ScanControl {
                                token: &token,
                                priority: &mut priority,
                                identity: (generation, n),
                                service: &|| {
                                    service_retention(
                                        &receiver, &queue, &retention, &output, generation, &token,
                                        false,
                                    )
                                },
                            },
                        )
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
                        priority.update();
                        let refreshed = disk_scan::scan_with_hook(
                            &root,
                            &home,
                            &token,
                            |tree| {
                                service_retention(
                                    &receiver, &queue, &retention, &output, generation, &token,
                                    false,
                                );
                                if browsing == tree.root || tree.state != ScanState::Complete {
                                    publish(
                                        &output, tree, &browsing, page, false, generation,
                                        navigation,
                                    );
                                }
                            },
                            || {
                                service_retention(
                                    &receiver, &queue, &retention, &output, generation, &token,
                                    false,
                                );
                                priority.update()
                            },
                        );
                        priority.restore();
                        completed = publish_browse(
                            &output,
                            &refreshed,
                            &browsing,
                            page,
                            &home,
                            ScanControl {
                                token: &token,
                                priority: &mut priority,
                                identity: (generation, navigation),
                                service: &|| {
                                    service_retention(
                                        &receiver, &queue, &retention, &output, generation, &token,
                                        false,
                                    )
                                },
                            },
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
        self.panel_resize.finish();
        self.moment = None;
        self.settings_open = false;
        if let Some(disk) = &mut self.disk {
            disk.reopen(Instant::now(), window.is_visible());
        } else {
            self.disk = Some(Disk::new(window.is_visible()));
            let mut events = self.disk.as_mut().unwrap().events.take().unwrap();
            cx.spawn(async move |entity, cx| {
                use futures::StreamExt;
                while events.next().await.is_some() {
                    if entity
                        .update(cx, |this, cx| {
                            if let Some(disk) = &mut this.disk {
                                disk.consume_mailbox();
                                cx.notify();
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
    pub(super) fn disk_panel(
        &self,
        panel_width: crate::details::DetailWidth,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let disk = self.disk.as_ref().filter(|d| d.open)?;
        let t = self.draft.unwrap_or(self.settings.base()).tokens();
        let mut body = div().flex().flex_col().gap(px(6.)).px(px(12.)).py(px(10.));
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
                current_path(folder.path.display().to_string()).into_any_element(),
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
                        .when(index == disk.selected, |d| {
                            d.bg(rgb(if t.light { t.selection } else { t.rule_hi }))
                        })
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
        body =
            body.child(div().text_size(px(10.)).child(
                "↑ ↓ select · Enter open · Backspace up · X trash · C cancel · R start over",
            ));
        let mut panel = div()
            .relative()
            .child(crate::rack::screws(&t, "disk"))
            .flex()
            .flex_col()
            .w(px(panel_width.pixels()))
            .flex_shrink_0()
            .h_full()
            .min_h_0()
            .bg(rgb(t.base.plate))
            .border_1()
            .border_color(rgb(t.rule_hi))
            .child(
                div()
                    .px(px(12.))
                    .py(px(10.))
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
                                    .label("Start over")
                                    .rack(&t)
                                    .small()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let d = this.disk.as_mut().unwrap();
                                        d.start_over();
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
                    .px(px(12.))
                    .py(px(10.))
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
                        this.panel_resize.finish();
                        cx.notify();
                    })),
            )
            .child(self.panel_resize_handle(cx))
            .child(self.panel_resize_events(cx));
        Some(panel)
    }
}

fn current_path(path: String) -> Stateful<Div> {
    div()
        .id("disk-current-path")
        .debug_selector(|| "disk-current-path".into())
        .flex_shrink_0()
        .text_size(px(10.))
        .min_w_0()
        .whitespace_nowrap()
        .truncate()
        .child(path)
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
        let disk = Disk::new(false);
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
    use std::{fs, time::Duration};

    struct CurrentPathLayout;

    impl Render for CurrentPathLayout {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(354.))
                .h(px(120.))
                .flex()
                .flex_col()
                .child(current_path(
                    "/a/long/current/folder/path/that/stays/visible".into(),
                ))
                .children((0..128).map(|index| {
                    div()
                        .h(px(22.))
                        .flex_shrink_0()
                        .child(format!("file-{index}"))
                }))
        }
    }

    #[gpui::test]
    async fn current_folder_path_keeps_one_visible_line(cx: &mut TestAppContext) {
        let (_, visual) = cx.add_window_view(|_, _| CurrentPathLayout);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let path = visual.debug_bounds("disk-current-path").unwrap();
        assert!(path.size.height >= px(12.), "path bounds: {path:?}");
    }

    fn scratch() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gtop-disk-service-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }
    #[test]
    fn scan_priority_tracks_visibility_and_defaults_to_utility() {
        fn current_qos_class() -> u32 {
            let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            let mut priority = 0;
            let result = unsafe {
                libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut priority)
            };
            assert_eq!(result, 0);
            class as u32
        }

        let foreground = Arc::new(AtomicBool::new(false));
        let mut priority = ScanPriority::new(foreground.clone());
        assert!(!priority.applied_foreground);
        assert_eq!(
            current_qos_class(),
            libc::qos_class_t::QOS_CLASS_UTILITY as u32
        );

        foreground.store(true, Ordering::Release);
        priority.update();
        assert_eq!(
            current_qos_class(),
            libc::qos_class_t::QOS_CLASS_USER_INITIATED as u32
        );

        foreground.store(false, Ordering::Release);
        priority.update();
        assert_eq!(
            current_qos_class(),
            libc::qos_class_t::QOS_CLASS_UTILITY as u32
        );
    }
    #[test]
    fn worker_without_visibility_scans_at_utility() {
        let home = scratch();
        fs::write(home.join("file"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        assert!(!disk.foreground.load(Ordering::Acquire));
        assert_eq!(
            receive(&mut disk, ScanState::Complete).state,
            ScanState::Complete
        );
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
    fn receive(disk: &mut Disk, expected: ScanState) -> Folder {
        let start = Instant::now();
        loop {
            if disk.consume_mailbox() {
                if let Some(folder) = &disk.folder {
                    if folder.state == expected {
                        return folder.clone();
                    }
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "worker timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn receive_cleanup_refresh(disk: &Disk) -> (String, Folder) {
        let start = Instant::now();
        loop {
            let mut mailbox = disk.mailbox.lock().unwrap();
            let status = mailbox.status.take();
            let Some(status) = status else {
                drop(mailbox);
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "cleanup timed out"
                );
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            if status.contains("Rescanning") {
                drop(mailbox);
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "cleanup timed out"
                );
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            let folder = mailbox.folder.take();
            drop(mailbox);
            let (generation, navigation, folder) = folder.expect("cleanup refresh folder");
            assert_eq!(
                (generation, navigation),
                (disk.generation(), disk.navigation)
            );
            return (status, folder);
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
        let (status, folder) = receive_cleanup_refresh(&disk);
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
        let foreground = Arc::new(AtomicBool::new(false));
        let mut priority = ScanPriority::new(foreground.clone());
        let browse_token = AtomicBool::new(false);
        let completed = publish_browse(
            &output,
            &root,
            &child,
            0,
            &home,
            ScanControl {
                token: &browse_token,
                priority: &mut priority,
                identity: (7, 3),
                service: &|| {},
            },
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
            ScanControl {
                token: &browse_token,
                priority: &mut priority,
                identity: (7, 4),
                service: &|| {},
            },
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
    fn full_queue_preserves_session_on_close_reopen_and_start_over() {
        let home = scratch();
        let mut disk = Disk::with_home(home.clone());
        disk.folder = Some(receive(&mut disk, ScanState::Complete));
        let (sender, receiver) = mpsc::sync_channel(1);
        disk.sender = sender;
        disk.browse(home.clone(), 0);
        let identity = (disk.generation(), disk.navigation);
        let folder = disk.folder.clone();
        let token = disk.cancel.clone();
        disk.close();
        assert!(disk.open);
        disk.reopen(Instant::now(), false);
        disk.key("r");
        assert!(Arc::ptr_eq(&token, &disk.cancel));
        assert_eq!(disk.session.as_ref().unwrap().root, home);
        assert_eq!(identity, (disk.generation(), disk.navigation));
        assert_eq!(disk.folder.as_ref().unwrap().items, folder.unwrap().items);
        assert!(disk.status.contains("busy"));
        assert!(matches!(receiver.try_recv(), Ok(Command::Browse(..))));
        let (sender, receiver) = mpsc::sync_channel(1);
        disk.sender = sender;
        disk.key("r");
        assert_ne!(identity.0, disk.generation());
        assert!(matches!(
            receiver.try_recv(),
            Ok(Command::Scan(_, true, _, _))
        ));
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn expired_reopen_with_full_queue_keeps_state_until_retry() {
        let home = scratch();
        let mut disk = Disk::with_home(home.clone());
        let folder = receive(&mut disk, ScanState::Complete);
        disk.close();
        let (sender, receiver) = mpsc::sync_channel(1);
        disk.sender = sender;
        disk.sender.send(Command::Close(Instant::now())).unwrap();
        let generation = disk.generation();
        let token = disk.cancel.clone();
        disk.reopen(Instant::now() + SESSION_RETENTION, false);
        assert!(!disk.open);
        assert_eq!(disk.generation(), generation);
        assert!(Arc::ptr_eq(&token, &disk.cancel));
        assert_eq!(disk.folder.as_ref().unwrap().root, folder.root);
        assert!(disk.status.contains("busy"));
        receiver.recv().unwrap();
        disk.reopen(Instant::now() + SESSION_RETENTION, false);
        assert!(disk.open);
        assert!(matches!(receiver.recv().unwrap(), Command::Reopen(_)));
        disk.key("r");
        assert!(matches!(
            receiver.recv().unwrap(),
            Command::Scan(_, true, _, _)
        ));
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn reopening_retries_a_cancelled_queued_scan() {
        let home = scratch();
        fs::write(home.join("one"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let _ = receive(&mut disk, ScanState::Complete);
        let (started, ready) = mpsc::sync_channel(1);
        let (resume, suspended) = mpsc::sync_channel(1);
        disk.mailbox.lock().unwrap().scan_gate = Some((started, suspended));
        disk.start(home.clone(), true);
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(disk.session.as_ref().unwrap().outcome.is_none());
        let data = home.join("data-volume");
        fs::create_dir(&data).unwrap();
        fs::write(data.join("data"), [1; 4096]).unwrap();
        disk.start(data.clone(), true);
        let queued_token = disk.cancel.clone();
        queued_token.store(true, Ordering::Relaxed);
        disk.close();
        disk.reopen(Instant::now(), false);
        resume.send(()).unwrap();
        let folder = receive(&mut disk, ScanState::Complete);
        assert_eq!(folder.root, data);
        assert_eq!(folder.items, 1);
        assert!(!Arc::ptr_eq(&queued_token, &disk.cancel));
        assert!(!disk.cancel.load(Ordering::Relaxed));
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn start_over_keeps_accepted_root_before_any_reply() {
        let home = scratch();
        let mut disk = Disk::with_home(home.clone());
        let _ = receive(&mut disk, ScanState::Complete);
        let (sender, receiver) = mpsc::sync_channel(4);
        disk.sender = sender;
        let data = home.join("data-volume");
        disk.start(data.clone(), true);
        let first = disk.generation();
        assert!(disk.folder.is_none());
        assert!(disk.key("r"));
        assert!(disk.generation() > first);
        assert!(
            matches!(receiver.recv().unwrap(), Command::Scan(root, true, _, _) if root == data)
        );
        assert!(
            matches!(receiver.recv().unwrap(), Command::Scan(root, true, _, _) if root == data)
        );
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn later_close_cannot_revive_an_expired_scan() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let now = Instant::now();
        sender.send(Command::Close(now)).unwrap();
        sender.send(Command::Reopen(now)).unwrap();
        sender
            .send(Command::Close(now + SESSION_RETENTION))
            .unwrap();
        let retention = RefCell::new(Retention::default());
        let queue = RefCell::new(VecDeque::new());
        let output = Mutex::new(Mailbox::default());
        let token = AtomicBool::new(false);
        service_retention(&receiver, &queue, &retention, &output, 7, &token, false);
        assert!(matches!(*retention.borrow(), Retention::Expired));
        assert!(token.load(Ordering::Relaxed));
        assert_eq!(output.lock().unwrap().resume, Some(7));
    }

    #[test]
    fn fresh_scan_resets_worker_expiry_before_its_first_callback() {
        let home = scratch();
        fs::write(home.join("one"), [1; 4096]).unwrap();
        let (sender, receiver) = mpsc::sync_channel(4);
        let (started, ready) = mpsc::sync_channel(1);
        let (resume, suspended) = mpsc::sync_channel(1);
        let output = Arc::new(Mutex::new(Mailbox {
            scan_gate: Some((started, suspended)),
            ..Mailbox::default()
        }));
        let mailbox = output.clone();
        let worker_home = home.clone();
        let token = Arc::new(AtomicBool::new(false));
        sender.send(Command::Close(Instant::now())).unwrap();
        sender
            .send(Command::Scan(home.clone(), true, token.clone(), 7))
            .unwrap();
        let worker = std::thread::spawn(move || {
            worker(
                receiver,
                output,
                worker_home,
                Arc::new(AtomicBool::new(false)),
            );
        });
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!token.load(Ordering::Relaxed));
        resume.send(()).unwrap();
        let start = Instant::now();
        loop {
            let mailbox = mailbox.lock().unwrap();
            if matches!(mailbox.outcome, Some((7, ScanState::Complete))) {
                assert_ne!(mailbox.expired, Some(7));
                assert_eq!(mailbox.folder.as_ref().unwrap().2.items, 1);
                break;
            }
            drop(mailbox);
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!token.load(Ordering::Relaxed));
        drop(sender);
        worker.join().unwrap();
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn start_over_scans_home_when_no_folder_is_available() {
        let home = scratch();
        fs::write(home.join("one"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let _ = receive(&mut disk, ScanState::Complete);
        disk.folder = None;
        assert!(disk.key("r"));
        assert_eq!(receive(&mut disk, ScanState::Complete).items, 1);
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
        let accepted = (disk.generation(), disk.navigation, disk.proposal_id);
        disk.confirmation =
            Some(Confirmation::new(&path, row.allocated, row.identity, &home).unwrap());
        disk.start(home.clone(), true);
        assert_eq!(
            (disk.generation(), disk.navigation, disk.proposal_id),
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
            let (status, folder) = receive_cleanup_refresh(&disk);
            assert!(!status.contains("Rescanning"), "{status}");
            assert_eq!(folder.path, home.join("child"));
            assert_eq!(
                folder.rows[0].path.file_name().unwrap(),
                if outcome == "failure" {
                    "cache"
                } else {
                    "moved"
                }
            );
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
    fn close_and_reopen_keep_completed_tree_and_start_over_refreshes() {
        let home = scratch();
        fs::write(home.join("one"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        disk.folder = Some(receive(&mut disk, ScanState::Complete));
        let generation = disk.generation();
        disk.close();
        fs::write(home.join("two"), [2; 4096]).unwrap();
        disk.reopen(Instant::now(), false);
        assert_eq!(disk.generation(), generation);
        assert_eq!(disk.folder.as_ref().unwrap().items, 1);
        assert_eq!(disk.folder.as_ref().unwrap().state, ScanState::Complete);
        assert!(!disk.cancel.load(Ordering::Relaxed));
        assert!(disk.key("r"));
        let refreshed = receive(&mut disk, ScanState::Complete);
        assert_eq!(refreshed.items, 2);
        assert!(!refreshed.cached);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn running_scan_survives_close_and_reopen_without_restarting() {
        let home = scratch();
        fs::write(home.join("one"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let _ = receive(&mut disk, ScanState::Complete);
        let (started, ready) = mpsc::sync_channel(1);
        let (resume, suspended) = mpsc::sync_channel(1);
        disk.mailbox.lock().unwrap().scan_gate = Some((started, suspended));
        disk.start(home.clone(), true);
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(disk.session.as_ref().unwrap().outcome.is_none());
        let generation = disk.generation();
        disk.close();
        assert!(!disk.cancel.load(Ordering::Relaxed));
        assert!(!disk.foreground.load(Ordering::Acquire));
        disk.reopen(Instant::now(), false);
        assert_eq!(disk.generation(), generation);
        resume.send(()).unwrap();
        assert_eq!(receive(&mut disk, ScanState::Complete).items, 1);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn closed_scan_completes_in_background_and_expiry_releases_cache() {
        let home = scratch();
        fs::write(home.join("one"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        let _ = receive(&mut disk, ScanState::Complete);
        let (started, ready) = mpsc::sync_channel(1);
        let (resume, suspended) = mpsc::sync_channel(1);
        disk.mailbox.lock().unwrap().scan_gate = Some((started, suspended));
        disk.start(home.clone(), true);
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        disk.close();
        resume.send(()).unwrap();
        disk.folder = Some(receive(&mut disk, ScanState::Complete));
        fs::write(home.join("two"), [2; 4096]).unwrap();
        disk.reopen(Instant::now() + SESSION_RETENTION, false);
        let fresh = receive(&mut disk, ScanState::Complete);
        assert_eq!(fresh.items, 2);
        assert!(!fresh.cached);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn expired_running_scan_is_cancelled_and_cache_is_released() {
        let home = scratch();
        for n in 0..256 {
            fs::create_dir(home.join(format!("dir-{n}"))).unwrap();
        }
        let mut disk = Disk::with_home(home.clone());
        let _ = receive(&mut disk, ScanState::Complete);
        let (started, ready) = mpsc::sync_channel(1);
        let (resume, suspended) = mpsc::sync_channel(1);
        disk.mailbox.lock().unwrap().scan_gate = Some((started, suspended));
        disk.start(home.clone(), true);
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        let token = disk.cancel.clone();
        disk.close();
        disk.sender.send(Command::Close(Instant::now())).unwrap();
        resume.send(()).unwrap();
        let start = Instant::now();
        while disk.mailbox.lock().unwrap().expired != Some(disk.generation()) {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(token.load(Ordering::Relaxed));
        disk.consume_mailbox();
        assert!(disk.folder.is_none());
        disk.reopen(Instant::now(), false);
        let fresh = receive(&mut disk, ScanState::Complete);
        assert!(!fresh.cached);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn worker_expires_closed_cache_without_reopening() {
        let home = scratch();
        fs::write(home.join("one"), [1; 4096]).unwrap();
        let mut disk = Disk::with_home(home.clone());
        disk.folder = Some(receive(&mut disk, ScanState::Complete));
        disk.close();
        disk.sender.send(Command::Close(Instant::now())).unwrap();

        let start = Instant::now();
        loop {
            let mailbox = disk.mailbox.lock().unwrap();
            if mailbox.expired == Some(disk.generation()) {
                assert!(mailbox.folder.is_none());
                break;
            }
            drop(mailbox);
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        fs::write(home.join("two"), [2; 4096]).unwrap();
        disk.reopen(Instant::now(), false);
        let fresh = receive(&mut disk, ScanState::Complete);
        assert_eq!(fresh.items, 2);
        assert!(!fresh.cached);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
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
        let generation = disk.generation();
        disk.browse(home.join("child"), 0);
        let folder = receive(&mut disk, ScanState::Complete);
        assert_eq!(folder.path, home.join("child"));
        assert_eq!(disk.navigation, 1);
        disk.start(home.clone(), true);
        assert!(disk.generation() > generation);
        assert_eq!(disk.navigation, 0);
        let _ = receive(&mut disk, ScanState::Complete);
        drop(disk);
        fs::remove_dir_all(home).unwrap();
    }
}
