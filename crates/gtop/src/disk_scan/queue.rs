use super::*;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{atomic::AtomicUsize, Arc, Condvar, Mutex},
};

const RECEIPT_WINDOW: usize = 64 * 1024 * 1024;
// Charged speculative receipts and pending route keys stay within this window plus one
// reservation per worker. The reducer's required directory may exceed the window.
const MAP_NODE_OVERHEAD: usize = 64;
const CACHE_FDS: usize = 32;
const REPORT_INTERVAL: Duration = Duration::from_millis(100);
pub(super) const DEFAULT_WORKERS: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Urgency {
    Background,
    Foreground,
}
impl Urgency {
    fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            let mut relative = 0;
            unsafe {
                libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative)
            };
            if class as u32 >= libc::qos_class_t::QOS_CLASS_USER_INITIATED as u32 {
                return Self::Foreground;
            }
        }
        Self::Background
    }
}

#[derive(Default)]
pub(super) struct Stats {
    pub(super) receipt_bytes: AtomicUsize,
    pub(super) peak_receipt_bytes: AtomicUsize,
    pub(super) live_fds: AtomicUsize,
    pub(super) peak_fds: AtomicUsize,
    pub(super) jobs: AtomicUsize,
    pub(super) effective_workers: AtomicUsize,
    pub(super) cache_capacity: AtomicUsize,
    pub(super) reopens: AtomicUsize,
    pub(super) cache_hits: AtomicUsize,
    pub(super) oversized: AtomicUsize,
    pub(super) abandons: AtomicUsize,
    pub(super) speculative_receipt_bytes: AtomicUsize,
    pub(super) peak_speculative_receipt_bytes: AtomicUsize,
    pub(super) required_over_window: AtomicUsize,
    pub(super) qos_mismatches: AtomicUsize,
    pub(super) foreground_workers: AtomicUsize,
    pub(super) background_workers: AtomicUsize,
    pub(super) restored_workers: AtomicUsize,
    pub(super) worker_nanos: std::sync::atomic::AtomicU64,
    pub(super) reducer_nanos: std::sync::atomic::AtomicU64,
    pub(super) admission_wait_nanos: std::sync::atomic::AtomicU64,
}
impl Stats {
    fn add_bytes(&self, bytes: usize) {
        let live = self.receipt_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak_receipt_bytes.fetch_max(live, Ordering::Relaxed);
    }
    fn add_speculative_bytes(&self, bytes: usize) {
        let live = self
            .speculative_receipt_bytes
            .fetch_add(bytes, Ordering::Relaxed)
            + bytes;
        self.peak_speculative_receipt_bytes
            .fetch_max(live, Ordering::Relaxed);
    }
    fn remove_bytes(&self, bytes: usize) {
        self.receipt_bytes.fetch_sub(bytes, Ordering::Relaxed);
    }
    fn remove_speculative_bytes(&self, bytes: usize) {
        self.speculative_receipt_bytes
            .fetch_sub(bytes, Ordering::Relaxed);
    }
    fn opened(&self) {
        let live = self.live_fds.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak_fds.fetch_max(live, Ordering::Relaxed);
    }
}

pub(super) struct Options {
    pub(super) workers: usize,
    pub(super) initial_urgency: Urgency,
    pub(super) window: usize,
    pub(super) stats: Arc<Stats>,
    #[cfg(test)]
    pub(super) hooks: TestHooks,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            workers: DEFAULT_WORKERS,
            initial_urgency: Urgency::current(),
            window: RECEIPT_WINDOW,
            stats: Arc::default(),
            #[cfg(test)]
            hooks: TestHooks::default(),
        }
    }
}
#[cfg(test)]
type RouteHook = Arc<dyn Fn(&[usize]) + Send + Sync>;
#[cfg(test)]
type PriorityHook = Arc<dyn Fn(usize, Urgency, bool) + Send + Sync>;
#[cfg(test)]
type TapeHook = Arc<dyn Fn(&[usize]) -> (Vec<Entry>, bool) + Send + Sync>;
#[cfg(test)]
#[derive(Clone, Default)]
pub(super) struct TestHooks {
    pub(super) before_open: Option<RouteHook>,
    pub(super) priority: Option<PriorityHook>,
    pub(super) cancel_after: Option<u64>,
    pub(super) after_entry: Option<RouteHook>,
    pub(super) completed: Option<RouteHook>,
    pub(super) abandoned: Option<RouteHook>,
    pub(super) needed: Option<RouteHook>,
    pub(super) tape: Option<TapeHook>,
    pub(super) spawn_failure: Option<Arc<dyn Fn(usize) -> bool + Send + Sync>>,
    pub(super) initial_links: Vec<(u64, u64)>,
}

struct Route {
    parent: Option<Arc<Route>>,
    name: std::ffi::OsString,
    expected: FileId,
    key: Vec<usize>,
}
struct PendingRoute {
    route: Arc<Route>,
    charge: usize,
    speculative: bool,
}
fn map_key_charge(key: &[usize]) -> usize {
    std::mem::size_of::<Vec<usize>>() + std::mem::size_of_val(key) + MAP_NODE_OVERHEAD
}
fn pending_route_charge(route: &Route) -> usize {
    std::mem::size_of::<Route>()
        + route.name.capacity()
        + route.key.capacity() * std::mem::size_of::<usize>()
        + 2 * std::mem::size_of::<usize>()
        + map_key_charge(&route.key)
}
impl Route {
    fn path(&self, root: &Path) -> PathBuf {
        let mut names = vec![];
        let mut route = self;
        while let Some(parent) = &route.parent {
            names.push(route.name.as_os_str());
            route = parent;
        }
        let mut path = root.to_path_buf();
        for name in names.into_iter().rev() {
            path.push(name);
        }
        path
    }
}

struct ObservedEntry {
    entry: Entry,
    child: Option<Arc<Route>>,
    cleanable: Option<&'static str>,
}
struct PlainRun {
    count: u64,
    allocated: u64,
    in_cloud: bool,
}
enum Observation {
    Plain(PlainRun),
    Entry(Box<ObservedEntry>),
}
#[derive(Clone, Copy)]
enum Outcome {
    End,
    ReadError,
    OpenError,
    Interrupted,
}
struct Receipt {
    observations: Vec<Observation>,
    outcome: Outcome,
    bytes: usize,
    speculative: bool,
    has_children: bool,
    children_published: bool,
    stats: Arc<Stats>,
}
impl Receipt {
    fn new(stats: &Arc<Stats>, speculative: bool) -> Self {
        Self {
            observations: vec![],
            outcome: Outcome::End,
            bytes: 0,
            speculative,
            has_children: false,
            children_published: false,
            stats: stats.clone(),
        }
    }
    fn push(&mut self, observation: Observation) {
        if matches!(&observation, Observation::Entry(observed) if observed.child.is_some()) {
            self.has_children = true;
        }
        let owned = match &observation {
            Observation::Plain(_) => 0,
            Observation::Entry(observed) => {
                std::mem::size_of::<ObservedEntry>()
                    + observed.entry.name.capacity()
                    + observed.child.as_ref().map_or(0, |child| {
                        std::mem::size_of::<Route>()
                            + child.name.capacity()
                            + child.key.capacity() * std::mem::size_of::<usize>()
                    })
            }
        };
        let previous = self.observations.capacity();
        self.observations.push(observation);
        let bytes =
            owned + (self.observations.capacity() - previous) * std::mem::size_of::<Observation>();
        self.bytes += bytes;
        self.stats.add_bytes(bytes);
        if self.speculative {
            self.stats.add_speculative_bytes(bytes);
        }
    }
    fn charge_finished(&mut self, key: &[usize]) {
        let bytes = std::mem::size_of::<Self>() + map_key_charge(key);
        self.bytes += bytes;
        self.stats.add_bytes(bytes);
        if self.speculative {
            self.stats.add_speculative_bytes(bytes);
        }
    }
    fn set_speculative(&mut self, speculative: bool) {
        if self.speculative == speculative {
            return;
        }
        if speculative {
            self.stats.add_speculative_bytes(self.bytes);
        } else {
            self.stats
                .speculative_receipt_bytes
                .fetch_sub(self.bytes, Ordering::Relaxed);
        }
        self.speculative = speculative;
    }
}
impl Drop for Receipt {
    fn drop(&mut self) {
        self.stats
            .receipt_bytes
            .fetch_sub(self.bytes, Ordering::Relaxed);
        if self.speculative {
            self.stats
                .speculative_receipt_bytes
                .fetch_sub(self.bytes, Ordering::Relaxed);
        }
    }
}

struct Control<'a> {
    cancel: &'a AtomicBool,
    stop: AtomicBool,
    foreground: AtomicBool,
}
impl Control<'_> {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.cancel.load(Ordering::Relaxed)
    }
    fn urgency(&self) -> Urgency {
        if self.foreground.load(Ordering::Acquire) {
            Urgency::Foreground
        } else {
            Urgency::Background
        }
    }
}
struct Priority<'a> {
    applied: Option<Urgency>,
    control: &'a Control<'a>,
    index: usize,
    stats: &'a Stats,
    #[cfg(test)]
    hooks: &'a TestHooks,
}
impl Priority<'_> {
    fn refresh(&mut self) {
        let urgency = self.control.urgency();
        if self.applied == Some(urgency) {
            return;
        }
        match urgency {
            Urgency::Background => background_priority(),
            Urgency::Foreground => foreground_priority(),
        }
        self.applied = Some(urgency);
        self.verify(urgency);
        #[cfg(test)]
        if let Some(hook) = &self.hooks.priority {
            hook(self.index, urgency, false);
        }
    }
}
impl Priority<'_> {
    fn verify(&self, urgency: Urgency) {
        let bits = if urgency == Urgency::Foreground {
            &self.stats.foreground_workers
        } else {
            &self.stats.background_workers
        };
        bits.fetch_or(1 << self.index, Ordering::Relaxed);
        #[cfg(target_os = "macos")]
        {
            let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            let mut relative = 0;
            let result = unsafe {
                libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative)
            };
            let expected = if urgency == Urgency::Foreground {
                libc::qos_class_t::QOS_CLASS_USER_INITIATED
            } else {
                libc::qos_class_t::QOS_CLASS_UTILITY
            };
            if result != 0 || class as u32 != expected as u32 {
                self.stats.qos_mismatches.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
impl Drop for Priority<'_> {
    fn drop(&mut self) {
        background_priority();
        self.verify(Urgency::Background);
        self.stats
            .restored_workers
            .fetch_or(1 << self.index, Ordering::Relaxed);
        #[cfg(test)]
        if let Some(hook) = &self.hooks.priority {
            hook(self.index, Urgency::Background, true);
        }
    }
}

struct ScanFd {
    fd: OwnedFd,
    stats: Arc<Stats>,
}
impl ScanFd {
    fn new(fd: OwnedFd, stats: &Arc<Stats>) -> Self {
        stats.opened();
        Self {
            fd,
            stats: stats.clone(),
        }
    }
}
impl Drop for ScanFd {
    fn drop(&mut self) {
        self.stats.live_fds.fetch_sub(1, Ordering::Relaxed);
    }
}
struct RootFdCount<'a>(&'a Stats);
impl Drop for RootFdCount<'_> {
    fn drop(&mut self) {
        self.0.live_fds.fetch_sub(1, Ordering::Relaxed);
    }
}
struct CachePermit {
    live: Arc<AtomicUsize>,
}
impl Drop for CachePermit {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::Relaxed);
    }
}
struct Cached {
    // Return the permit only after the descriptor has closed.
    directory: ScanFd,
    _permit: CachePermit,
}
struct CacheEntry {
    key: Vec<usize>,
    directory: Arc<Cached>,
}
struct Cache {
    entries: Mutex<VecDeque<CacheEntry>>,
    live: Arc<AtomicUsize>,
    capacity: usize,
}
impl Cache {
    fn ancestor(&self, route: &Route) -> Option<(usize, Arc<Cached>)> {
        let mut entries = self.entries.lock().unwrap();
        let position = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.key.len() < route.key.len() && route.key.starts_with(&entry.key)
            })
            .max_by_key(|(_, entry)| entry.key.len())
            .map(|(position, _)| position)?;
        let entry = entries.remove(position).unwrap();
        let found = (entry.key.len(), entry.directory.clone());
        entries.push_back(entry);
        Some(found)
    }
    fn retain(&self, route: &Route, directory: ScanFd) {
        loop {
            let evicted = {
                let mut entries = self.entries.lock().unwrap();
                if self.live.load(Ordering::Relaxed) < self.capacity {
                    self.live.fetch_add(1, Ordering::Relaxed);
                    entries.push_back(CacheEntry {
                        key: route.key.clone(),
                        directory: Arc::new(Cached {
                            directory,
                            _permit: CachePermit {
                                live: self.live.clone(),
                            },
                        }),
                    });
                    return;
                }
                entries.pop_front()
            };
            match evicted {
                Some(entry) => drop(entry),
                None => return,
            }
        }
    }
}

struct State {
    ready: BTreeMap<Vec<usize>, PendingRoute>,
    deferred: BTreeMap<Vec<usize>, PendingRoute>,
    finished: BTreeMap<Vec<usize>, Receipt>,
    active: BTreeSet<Vec<usize>>,
    needed: Option<Vec<usize>>,
    reserved: usize,
    stopping: bool,
}
struct Queue {
    state: Mutex<State>,
    wake: Condvar,
    reservation: usize,
    window: usize,
    stats: Arc<Stats>,
    background: Mutex<()>,
    #[cfg(test)]
    abandoned_hook: Option<RouteHook>,
    #[cfg(test)]
    needed_hook: Option<RouteHook>,
}
impl Queue {
    fn publish_locked(&self, state: &mut State, route: Arc<Route>, speculative: bool) {
        if !state.stopping
            && !state.active.contains(&route.key)
            && !state.ready.contains_key(&route.key)
            && !state.deferred.contains_key(&route.key)
            && !state.finished.contains_key(&route.key)
        {
            let charge = pending_route_charge(&route);
            self.stats.add_bytes(charge);
            if speculative {
                self.stats.add_speculative_bytes(charge);
            }
            state.ready.insert(
                route.key.clone(),
                PendingRoute {
                    route,
                    charge,
                    speculative,
                },
            );
            self.wake.notify_all();
        }
    }
    fn publish_required(&self, route: &Route, receipt: &mut Receipt) {
        let mut state = self.state.lock().unwrap();
        if state.needed.as_deref() != Some(route.key.as_slice()) {
            return;
        }
        receipt.set_speculative(false);
        publish_receipt_children(self, &mut state, receipt, false);
    }
    fn publish_required_child(&self, route: &Route, receipt: &mut Receipt, child: Arc<Route>) {
        let mut state = self.state.lock().unwrap();
        if state.needed.as_deref() != Some(route.key.as_slice()) {
            return;
        }
        receipt.set_speculative(false);
        if receipt.children_published {
            self.publish_locked(&mut state, child, false);
        } else {
            publish_receipt_children(self, &mut state, receipt, false);
        }
    }
    fn claim(&self, index: usize, priority: &mut Priority<'_>) -> Option<Arc<Route>> {
        let mut state = self.state.lock().unwrap();
        loop {
            priority.refresh();
            if state.stopping || priority.control.stopped() {
                return None;
            }
            if index == 0 || priority.control.urgency() == Urgency::Foreground {
                let required = state
                    .needed
                    .as_ref()
                    .filter(|key| {
                        state.ready.contains_key(*key) || state.deferred.contains_key(*key)
                    })
                    .cloned();
                let live = self.stats.receipt_bytes.load(Ordering::Relaxed);
                let speculative = live
                    .saturating_add(state.reserved)
                    .saturating_add(self.reservation)
                    <= self.window.saturating_sub(self.reservation);
                let required_over_window = required.is_some() && live > self.window;
                let key = required.or_else(|| {
                    speculative
                        .then(|| {
                            state
                                .ready
                                .keys()
                                .find(|key| !state.deferred.contains_key(*key))
                                .cloned()
                        })
                        .flatten()
                });
                if let Some(key) = key {
                    if required_over_window {
                        self.stats
                            .required_over_window
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    state.reserved += self.reservation;
                    let item = state
                        .ready
                        .remove(&key)
                        .or_else(|| state.deferred.remove(&key));
                    if let Some(item) = &item {
                        self.stats.remove_bytes(item.charge);
                        if item.speculative {
                            self.stats.remove_speculative_bytes(item.charge);
                        }
                        state.active.insert(item.route.key.clone());
                    }
                    return item.map(|item| item.route);
                }
            }
            let start = Instant::now();
            state = self.wake.wait_timeout(state, REPORT_INTERVAL).unwrap().0;
            self.stats
                .admission_wait_nanos
                .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
    }
    fn complete(&self, route: &Route, mut receipt: Receipt) {
        let mut state = self.state.lock().unwrap();
        state.reserved -= self.reservation;
        state.active.remove(&route.key);
        if !state.stopping {
            if state.needed.as_deref() == Some(route.key.as_slice()) {
                receipt.set_speculative(false);
            }
            let speculative = receipt.speculative;
            publish_receipt_children(self, &mut state, &mut receipt, speculative);
            receipt.charge_finished(&route.key);
            state.finished.insert(route.key.clone(), receipt);
        }
        self.wake.notify_all();
    }

    fn is_speculative(&self, route: &Route) -> bool {
        self.state.lock().unwrap().needed.as_deref() != Some(route.key.as_slice())
    }

    fn try_abandon(&self, route: &Arc<Route>, mut receipt: Receipt) -> Result<(), Receipt> {
        let mut state = self.state.lock().unwrap();
        if state.stopping {
            return Err(receipt);
        }
        if state.needed.as_deref() == Some(route.key.as_slice()) {
            receipt.set_speculative(false);
            publish_receipt_children(self, &mut state, &mut receipt, false);
            return Err(receipt);
        }
        drop(state);
        drop(receipt);
        self.stats.abandons.fetch_add(1, Ordering::Relaxed);
        #[cfg(test)]
        if let Some(hook) = &self.abandoned_hook {
            hook(&route.key);
        }
        Ok(())
    }

    fn defer(&self, route: &Arc<Route>) {
        let mut state = self.state.lock().unwrap();
        state.reserved -= self.reservation;
        state.active.remove(&route.key);
        if !state.stopping {
            let speculative = state.needed.as_deref() != Some(route.key.as_slice());
            let charge = pending_route_charge(route);
            self.stats.add_bytes(charge);
            if speculative {
                self.stats.add_speculative_bytes(charge);
            }
            state.deferred.insert(
                route.key.clone(),
                PendingRoute {
                    route: route.clone(),
                    charge,
                    speculative,
                },
            );
        }
        self.wake.notify_all();
    }
}

fn publish_receipt_children(
    queue: &Queue,
    state: &mut State,
    receipt: &mut Receipt,
    speculative: bool,
) {
    if receipt.children_published {
        return;
    }
    for observation in &receipt.observations {
        if let Observation::Entry(observed) = observation {
            if let Some(child) = &observed.child {
                queue.publish_locked(state, child.clone(), speculative);
            }
        }
    }
    receipt.children_published = true;
}

fn io_ready<'a>(
    queue: &'a Queue,
    priority: &mut Priority<'_>,
    permit: &mut Option<std::sync::MutexGuard<'a, ()>>,
) -> bool {
    loop {
        priority.refresh();
        if priority.control.stopped() {
            return false;
        }
        if priority.control.urgency() == Urgency::Foreground {
            if permit.take().is_some() {
                queue.wake.notify_all();
            }
            return true;
        }
        if permit.is_some() {
            return true;
        }
        match queue.background.try_lock() {
            Ok(guard) => {
                *permit = Some(guard);
                return true;
            }
            Err(std::sync::TryLockError::Poisoned(error)) => panic!("{error}"),
            Err(std::sync::TryLockError::WouldBlock) => {
                let state = queue.state.lock().unwrap();
                let _guard = queue.wake.wait_timeout(state, REPORT_INTERVAL).unwrap();
            }
        }
    }
}

fn open_job<'a>(
    root: &OwnedFd,
    route: &Route,
    cache: &Cache,
    priority: &mut Priority<'_>,
    options: &Options,
    queue: &'a Queue,
    permit: &mut Option<std::sync::MutexGuard<'a, ()>>,
) -> io::Result<ScanFd> {
    if !io_ready(queue, priority, permit) {
        return Err(io::Error::other("Scan cancelled"));
    }
    #[cfg(test)]
    if let Some(hook) = &options.hooks.before_open {
        hook(&route.key);
    }
    priority.refresh();
    if route.parent.is_none() {
        return open_child(root, std::ffi::OsStr::new("."), route.expected)
            .map(|fd| ScanFd::new(fd, &options.stats));
    }
    let ancestor = cache.ancestor(route);
    if ancestor.is_some() {
        options.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
    }
    let depth = ancestor.as_ref().map_or(0, |(depth, _)| *depth);
    let mut chain = vec![];
    let mut step = route;
    while step.parent.is_some() && step.key.len() > depth {
        chain.push(step);
        step = step.parent.as_deref().unwrap();
    }
    let mut current: Option<ScanFd> = None;
    for step in chain.into_iter().rev() {
        if priority.control.stopped() {
            return Err(io::Error::other("Scan cancelled"));
        }
        if !io_ready(queue, priority, permit) {
            return Err(io::Error::other("Scan cancelled"));
        }
        let parent = current
            .as_ref()
            .map(|fd| &fd.fd)
            .or_else(|| ancestor.as_ref().map(|(_, cached)| &cached.directory.fd))
            .unwrap_or(root);
        let child = ScanFd::new(
            open_child(parent, &step.name, step.expected)?,
            &options.stats,
        );
        options.stats.reopens.fetch_add(1, Ordering::Relaxed);
        current = Some(child);
    }
    Ok(current.unwrap())
}

fn observe_entries(
    directory: &OwnedFd,
    route: &Route,
    options: &Options,
    mut visit: impl FnMut(Entry) -> bool,
) -> io::Result<()> {
    #[cfg(test)]
    if let Some(tape) = &options.hooks.tape {
        let (entries, error) = tape(&route.key);
        for entry in entries {
            if !visit(entry) {
                return Ok(());
            }
        }
        return if error {
            Err(io::Error::other("Injected read failure"))
        } else {
            Ok(())
        };
    }
    #[cfg(not(test))]
    let _ = (route, options);
    entries(directory, &mut visit)
}

struct Reader<'a, 'b> {
    root: &'a Path,
    home: &'a Path,
    device: u64,
    queue: &'a Queue,
    priority: &'b mut Priority<'a>,
    options: &'a Options,
    permit: &'b mut Option<std::sync::MutexGuard<'a, ()>>,
}
impl Reader<'_, '_> {
    fn read(&mut self, directory: &OwnedFd, route: &Arc<Route>) -> Option<Receipt> {
        let Self {
            root,
            home,
            device,
            queue,
            priority,
            options,
            permit,
        } = self;
        let (root, home, device, queue, options) = (*root, *home, *device, *queue, *options);
        let mut receipt = Some(Receipt::new(&options.stats, queue.is_speculative(route)));
        let mut abandoned = false;
        let path = route.path(root);
        let mut ordinal = 0;
        let result = observe_entries(directory, route, options, |entry| {
            #[cfg(test)]
            if let Some(hook) = &options.hooks.after_entry {
                hook(&route.key);
            }
            if !io_ready(queue, priority, permit) {
                receipt.as_mut().unwrap().outcome = Outcome::Interrupted;
                return false;
            }
            if receipt
                .as_ref()
                .is_some_and(|receipt| receipt.speculative && receipt.has_children)
            {
                queue.publish_required(route, receipt.as_mut().unwrap());
            }
            let position = ordinal;
            ordinal += 1;
            if !route.key.is_empty()
                && !entry.directory
                && entry.links <= 1
                && !entry.error
                && entry.device == device
            {
                if let Some(Observation::Plain(run)) =
                    receipt.as_mut().unwrap().observations.last_mut()
                {
                    if run.allocated == entry.allocated && run.in_cloud == entry.in_cloud {
                        run.count += 1;
                        return true;
                    }
                }
                receipt.as_mut().unwrap().push(Observation::Plain(PlainRun {
                    count: 1,
                    allocated: entry.allocated,
                    in_cloud: entry.in_cloud,
                }));
                if receipt.as_ref().unwrap().speculative
                    && receipt.as_ref().unwrap().bytes > queue.reservation
                {
                    match queue.try_abandon(route, receipt.take().unwrap()) {
                        Ok(()) => {
                            abandoned = true;
                            return false;
                        }
                        Err(returned) => receipt = Some(returned),
                    }
                }
                return true;
            }
            let traversable =
                entry.directory && !entry.in_cloud && !entry.error && entry.device == device;
            let cleanable = if traversable {
                let candidate = matches!(
                    entry.name.as_bytes(),
                    b"Caches"
                        | b"DerivedData"
                        | b".npm"
                        | b"cache"
                        | b"store"
                        | b".Trash"
                        | b"node_modules"
                        | b"target"
                );
                if candidate {
                    let child_path = path.join(&entry.name);
                    cleanable_kind(&child_path, home).or_else(|| {
                        if entry.name == "target" && child_path.starts_with(home) {
                            options.stats.opened();
                            let found = local_manifest_while(directory, || {
                                io_ready(queue, priority, permit)
                            });
                            options.stats.live_fds.fetch_sub(1, Ordering::Relaxed);
                            found.then_some("Cargo build products")
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }
            } else {
                None
            };
            let child = if traversable && route.key.len() < 256 {
                let mut key = route.key.clone();
                key.push(position);
                Some(Arc::new(Route {
                    parent: Some(route.clone()),
                    name: entry.name.clone(),
                    expected: FileId {
                        device: entry.device,
                        inode: entry.inode,
                    },
                    key,
                }))
            } else {
                None
            };
            receipt
                .as_mut()
                .unwrap()
                .push(Observation::Entry(Box::new(ObservedEntry {
                    entry,
                    child: child.clone(),
                    cleanable,
                })));
            if let Some(child) = child {
                queue.publish_required_child(route, receipt.as_mut().unwrap(), child);
            }
            if receipt.as_ref().unwrap().speculative
                && receipt.as_ref().unwrap().bytes > queue.reservation
            {
                match queue.try_abandon(route, receipt.take().unwrap()) {
                    Ok(()) => {
                        abandoned = true;
                        return false;
                    }
                    Err(returned) => receipt = Some(returned),
                }
            }
            true
        });
        if abandoned {
            return None;
        }
        let mut receipt = receipt.unwrap();
        if result.is_err() {
            receipt.outcome = Outcome::ReadError;
        }
        if receipt.bytes > queue.reservation {
            options.stats.oversized.fetch_add(1, Ordering::Relaxed);
        }
        Some(receipt)
    }
}

struct Coordinator<'a, 'b> {
    queue: &'a Queue,
    control: &'a Control<'a>,
    options: &'a Options,
    root: &'a Path,
    progress: &'b mut dyn FnMut(&Tree),
    on_tick: &'b mut dyn FnMut() -> Urgency,
    last_tick: &'b mut Instant,
    last_report: Instant,
    reported: bool,
}
impl Coordinator<'_, '_> {
    fn service(&mut self, tree: &Tree, entered: bool) {
        let report = self.last_report.elapsed() >= REPORT_INTERVAL || (entered && !self.reported);
        if self.last_tick.elapsed() >= REPORT_INTERVAL && (entered || self.reported || report) {
            let urgency = (self.on_tick)();
            self.control
                .foreground
                .store(urgency == Urgency::Foreground, Ordering::Release);
            self.queue.wake.notify_all();
            *self.last_tick = Instant::now();
        }
        if report {
            (self.progress)(tree);
            self.last_report = Instant::now();
            self.reported = true;
        }
    }
    fn stopped(&self, reducer: &Reducer<'_>) -> bool {
        if self.control.stopped() || matches!(reducer.tree.state, ScanState::Failed(_)) {
            return true;
        }
        #[cfg(test)]
        if let Some(limit) = self.options.hooks.cancel_after {
            if reducer.tree.nodes[0].descendants >= limit {
                self.control.cancel.store(true, Ordering::Relaxed);
                return true;
            }
        }
        false
    }
    fn receive(&mut self, route: &Route, reducer: &Reducer<'_>) -> Option<Receipt> {
        loop {
            if self.stopped(reducer) {
                return None;
            }
            {
                let mut state = self.queue.state.lock().unwrap();
                if let Some(mut receipt) = state.finished.remove(&route.key) {
                    receipt.set_speculative(false);
                    state.needed = None;
                    return Some(receipt);
                }
                state.needed = Some(route.key.clone());
                self.queue.wake.notify_all();
                #[cfg(test)]
                if let Some(hook) = &self.queue.needed_hook {
                    hook(&route.key);
                }
                let mut remaining = REPORT_INTERVAL.saturating_sub(self.last_report.elapsed());
                if self.reported {
                    remaining =
                        remaining.min(REPORT_INTERVAL.saturating_sub(self.last_tick.elapsed()));
                }
                let _guard = self.queue.wake.wait_timeout(state, remaining).unwrap();
            }
            self.service(reducer.tree, false);
        }
    }
    fn replay(&mut self, route: &Route, reducer: &mut Reducer<'_>) -> io::Result<(u64, u64, u64)> {
        let Some(mut receipt) = self.receive(route, reducer) else {
            return Ok((0, 0, 0));
        };
        if matches!(receipt.outcome, Outcome::OpenError) {
            return Err(io::Error::other("Directory unavailable"));
        }
        let mut total = (0u64, 0u64, 0u64);
        let path = route.path(self.root);
        for observation in receipt.observations.drain(..) {
            if self.stopped(reducer) {
                break;
            }
            let start = Instant::now();
            match observation {
                Observation::Plain(run) => {
                    let count = run.count;
                    #[cfg(test)]
                    let count = self.options.hooks.cancel_after.map_or(count, |limit| {
                        count.min(limit.saturating_sub(reducer.tree.nodes[0].descendants))
                    });
                    #[cfg(test)]
                    if count < run.count {
                        self.control.cancel.store(true, Ordering::Relaxed);
                    }
                    reducer.plain_run(run.allocated, run.in_cloud, count, &mut total);
                    self.service(reducer.tree, true);
                }
                Observation::Entry(observed) => {
                    let Some(mut accepted) = reducer.enter(&path, observed.entry) else {
                        break;
                    };
                    self.service(reducer.tree, true);
                    if accepted.row.directory
                        && !accepted.row.in_cloud
                        && !accepted.row.skipped_volume
                        && !accepted.error
                    {
                        self.options
                            .stats
                            .reducer_nanos
                            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
                        let result = match observed.child {
                            Some(child) => self.replay(&child, reducer),
                            None => Err(io::Error::other("Directory depth limit reached")),
                        };
                        let start = Instant::now();
                        reducer.descended(&mut accepted, result);
                        accepted.row.cleanable = observed.cleanable;
                        reducer.finish(accepted, route.key.len(), &mut total);
                        self.options
                            .stats
                            .reducer_nanos
                            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
                        continue;
                    }
                    accepted.row.cleanable = observed.cleanable;
                    reducer.finish(accepted, route.key.len(), &mut total);
                }
            }
            self.options
                .stats
                .reducer_nanos
                .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        if matches!(receipt.outcome, Outcome::ReadError) && !self.stopped(reducer) {
            reducer.read_error(&mut total);
        }
        self.queue.wake.notify_all();
        Ok(total)
    }
}

fn descriptor_count() -> Option<usize> {
    #[cfg(target_os = "macos")]
    {
        let bytes = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                std::ptr::null_mut(),
                0,
            )
        };
        if bytes <= 0 {
            return None;
        }
        let mut buffer = vec![0u8; bytes as usize + 64 * std::mem::size_of::<libc::proc_fdinfo>()];
        let bytes = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDLISTFDS,
                0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as i32,
            )
        };
        (bytes > 0).then_some(bytes as usize / std::mem::size_of::<libc::proc_fdinfo>())
    }
    #[cfg(not(target_os = "macos"))]
    {
        fs::read_dir("/proc/self/fd")
            .ok()
            .map(|entries| entries.count())
    }
}

fn limits_for(requested: usize, limit: u64, used: usize) -> (usize, usize) {
    let available = (limit as usize)
        .saturating_sub(used.saturating_add(64 + 1))
        .min(4096);
    #[cfg(target_os = "macos")]
    let per_worker = 2;
    #[cfg(not(target_os = "macos"))]
    let per_worker = 3;
    let workers = requested.clamp(1, 14).min((available / per_worker).max(1));
    let cache = CACHE_FDS.min(available.saturating_sub(workers * per_worker));
    (workers, cache)
}

fn effective_limits(requested: usize) -> (usize, usize) {
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } == 0 {
        if let Some(used) = descriptor_count() {
            return limits_for(requested, unsafe { limit.assume_init() }.rlim_cur, used);
        }
    }
    (1, 0)
}

struct WorkerExit<'a> {
    control: &'a Control<'a>,
    queue: &'a Queue,
}
impl Drop for WorkerExit<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.control.stop.store(true, Ordering::Relaxed);
            self.queue.wake.notify_all();
        }
    }
}

// The scope joins after its closure returns, so every exit must wake workers first.
struct StopWorkers<'a> {
    control: &'a Control<'a>,
    queue: &'a Queue,
}
impl Drop for StopWorkers<'_> {
    fn drop(&mut self) {
        self.control.stop.store(true, Ordering::Relaxed);
        let mut state = self.queue.state.lock().unwrap();
        state.stopping = true;
        for item in state.ready.values().chain(state.deferred.values()) {
            self.queue.stats.remove_bytes(item.charge);
            if item.speculative {
                self.queue.stats.remove_speculative_bytes(item.charge);
            }
        }
        state.ready.clear();
        state.deferred.clear();
        state.finished.clear();
        self.queue.wake.notify_all();
    }
}

pub(super) struct Callbacks<'a> {
    pub(super) progress: &'a mut dyn FnMut(&Tree),
    pub(super) on_tick: &'a mut dyn FnMut() -> Urgency,
    pub(super) last_tick: &'a mut Instant,
}

pub(super) fn run(
    tree: &mut Tree,
    root_fd: &OwnedFd,
    home: &Path,
    cancel: &AtomicBool,
    callbacks: Callbacks<'_>,
    options: Options,
) {
    let (workers, cache_capacity) = effective_limits(options.workers);
    options
        .stats
        .effective_workers
        .store(workers, Ordering::Relaxed);
    options
        .stats
        .cache_capacity
        .store(cache_capacity, Ordering::Relaxed);
    let root_path = tree.root.clone();
    let root = Arc::new(Route {
        parent: None,
        name: std::ffi::OsString::new(),
        expected: tree.nodes[0].identity,
        key: vec![],
    });
    let control = Control {
        cancel,
        stop: AtomicBool::new(false),
        foreground: AtomicBool::new(options.initial_urgency == Urgency::Foreground),
    };
    let queue = Queue {
        state: Mutex::new(State {
            ready: BTreeMap::from([(
                vec![],
                PendingRoute {
                    route: root.clone(),
                    charge: pending_route_charge(&root),
                    speculative: false,
                },
            )]),
            deferred: BTreeMap::new(),
            finished: BTreeMap::new(),
            active: BTreeSet::new(),
            needed: Some(vec![]),
            reserved: 0,
            stopping: false,
        }),
        wake: Condvar::new(),
        reservation: (options.window / (workers + 2)).max(1),
        window: options.window,
        stats: options.stats.clone(),
        background: Mutex::new(()),
        #[cfg(test)]
        abandoned_hook: options.hooks.abandoned.clone(),
        #[cfg(test)]
        needed_hook: options.hooks.needed.clone(),
    };
    options.stats.add_bytes(pending_route_charge(&root));
    let cache = Cache {
        entries: Mutex::default(),
        live: Arc::default(),
        capacity: cache_capacity,
    };
    options.stats.opened();
    let _root_fd_count = RootFdCount(&options.stats);
    let device = tree.nodes[0].identity.device;
    std::thread::scope(|scope| {
        let _stop_workers = StopWorkers {
            control: &control,
            queue: &queue,
        };
        let mut started = 0;
        for index in 0..workers {
            let (queue, cache, control, options, root_path) =
                (&queue, &cache, &control, &options, &root_path);
            #[cfg(test)]
            let fail_spawn = options
                .hooks
                .spawn_failure
                .as_ref()
                .is_some_and(|hook| hook(index));
            #[cfg(not(test))]
            let fail_spawn = false;
            let worker = move || {
                let _exit = WorkerExit { control, queue };
                let mut priority = Priority {
                    applied: None,
                    control,
                    index,
                    stats: &options.stats,
                    #[cfg(test)]
                    hooks: &options.hooks,
                };
                priority.refresh();
                while let Some(route) = queue.claim(index, &mut priority) {
                    let start = Instant::now();
                    let mut permit = None;
                    let receipt = match open_job(
                        root_fd,
                        &route,
                        cache,
                        &mut priority,
                        options,
                        queue,
                        &mut permit,
                    ) {
                        Ok(directory) => {
                            let outcome = Reader {
                                root: root_path,
                                home,
                                device,
                                queue,
                                priority: &mut priority,
                                options,
                                permit: &mut permit,
                            }
                            .read(&directory.fd, &route);
                            let Some(receipt) = outcome else {
                                drop(directory);
                                drop(permit);
                                queue.defer(&route);
                                continue;
                            };
                            if !route.key.is_empty() && receipt.observations.iter().any(|observation| {
                                matches!(observation, Observation::Entry(observed) if observed.child.is_some())
                            }) {
                                cache.retain(&route, directory);
                            }
                            receipt
                        }
                        Err(_) => {
                            let mut receipt = Receipt::new(&options.stats, false);
                            receipt.outcome = Outcome::OpenError;
                            receipt
                        }
                    };
                    options.stats.jobs.fetch_add(1, Ordering::Relaxed);
                    options
                        .stats
                        .worker_nanos
                        .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    drop(permit);
                    #[cfg(test)]
                    if let Some(hook) = &options.hooks.completed {
                        hook(&route.key);
                    }
                    queue.complete(&route, receipt);
                }
            };
            let spawn = if fail_spawn {
                Err(io::Error::other("Injected worker spawn failure"))
            } else {
                std::thread::Builder::new().spawn_scoped(scope, worker)
            };
            match spawn {
                Ok(_) => started += 1,
                Err(error) if started == 0 => {
                    tree.state = ScanState::Failed(format!("Could not start scan worker: {error}"));
                    return;
                }
                Err(_) => break,
            }
        }
        let mut links = HashSet::new();
        #[cfg(test)]
        links.extend(options.hooks.initial_links.iter().copied());
        let mut reducer = Reducer {
            tree,
            device,
            links: &mut links,
        };
        let mut coordinator = Coordinator {
            queue: &queue,
            control: &control,
            options: &options,
            root: &root_path,
            progress: callbacks.progress,
            on_tick: callbacks.on_tick,
            last_tick: callbacks.last_tick,
            last_report: Instant::now(),
            reported: false,
        };
        if coordinator.replay(&root, &mut reducer).is_err() {
            reducer.tree.nodes[0].unreadable += 1;
        }
    });
    drop(cache);
    drop(queue);
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
