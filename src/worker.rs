use crate::{
    engine::Engine,
    macos::{App, MacBackend},
    model::*,
    persistence,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct Snapshot {
    pub workspace: Workspace,
    pub selected: Option<TabId>,
    pub live: Vec<(TabId, WindowId)>,
    pub windows: Vec<WindowInfo>,
    pub occupied: Vec<WindowId>,
    pub status: String,
    pub trusted: bool,
    pub paused: bool,
    pub stopped: bool,
    pub quitting: bool,
    pub close_attempt: u64,
    pub restore_pending: usize,
    pub backdrop: Option<(u32, Vec<Rect>)>,
    pub selected_frame: Option<Rect>,
    pub editing_ack: u64,
    pub area: Rect,
    pub stacked_windows: Vec<(TabId, u32, i32)>,
    pub badges: std::collections::BTreeMap<TabId, String>,
}
pub enum Command {
    Apps(Vec<App>),
    Discover,
    Attach(Option<TabId>, WindowId),
    Switch(TabId, u64),
    Resize(Rect, Rect),
    Rename(TabId, String),
    Reorder(TabId, usize),
    Release(TabId),
    RetryRestoration,
    Pause,
    Resume,
    RequestPermission,
    Raise,
    Quit,
    CancelQuit,
    EditingBarrier(u64),
    ReadBadges(i32, Vec<(TabId, WindowId, String)>),
}
#[derive(Default)]
struct Mailbox {
    queue: Mutex<VecDeque<Command>>,
    wake: Condvar,
}
impl Mailbox {
    fn push(&self, c: Command) {
        let mut q = self.queue.lock().unwrap();
        match c {
            Command::ReadBadges(..) => {
                q.retain(|c| !matches!(c, Command::ReadBadges(..)));
                q.push_back(c);
            }
            Command::EditingBarrier(_) => {
                q.retain(|c| {
                    !matches!(
                        c,
                        Command::Raise | Command::Switch(..) | Command::EditingBarrier(_)
                    )
                });
                q.push_front(c);
            }
            Command::Resize(..) => {
                q.retain(|c| !matches!(c, Command::Resize(..)));
                let index = q
                    .iter()
                    .take_while(|c| {
                        matches!(
                            c,
                            Command::Quit
                                | Command::Pause
                                | Command::Release(_)
                                | Command::EditingBarrier(_)
                        )
                    })
                    .count();
                q.insert(index, c);
            }
            Command::Quit | Command::Pause | Command::Release(_) => {
                q.retain(|c| {
                    !matches!(
                        c,
                        Command::Resize(..) | Command::Switch(..) | Command::Raise
                    )
                });
                q.push_front(c);
            }
            Command::Apps(..) => {
                q.retain(|c| !matches!(c, Command::Apps(..)));
                q.push_back(c);
            }
            Command::Switch(..) => {
                q.retain(|c| !matches!(c, Command::Switch(..)));
                q.push_back(c);
            }
            Command::Raise => {
                if !q.iter().any(|c| matches!(c, Command::Raise)) {
                    q.push_back(c);
                }
            }
            _ => q.push_back(c),
        }
        self.wake.notify_one();
    }
    fn receive(&self, timeout: Duration) -> Option<Command> {
        let q = self.queue.lock().unwrap();
        let (mut q, _) = self
            .wake
            .wait_timeout_while(q, timeout, |q| q.is_empty())
            .unwrap();
        q.pop_front()
    }
}
#[derive(Clone)]
pub struct Client {
    mailbox: Arc<Mailbox>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    generation: Arc<AtomicU64>,
    pointer_down: Arc<AtomicBool>,
    focus_suspended: Arc<AtomicBool>,
    requested_tab: Arc<Mutex<Option<TabId>>>,
}
impl Client {
    pub fn set_text_editing(&self, editing: bool) -> u64 {
        self.focus_suspended.store(editing, Ordering::SeqCst);
        let epoch = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.requested_tab.lock().unwrap().take();
        self.send(Command::EditingBarrier(epoch));
        epoch
    }
    pub fn set_pointer_down(&self, down: bool) {
        self.pointer_down.store(down, Ordering::Relaxed);
    }
    pub fn send(&self, c: Command) {
        if matches!(c, Command::Pause | Command::Quit | Command::Release(_)) {
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.requested_tab.lock().unwrap().take();
        }
        self.mailbox.push(c);
    }
    pub fn switch(&self, id: TabId) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.requested_tab.lock().unwrap() = Some(id);
        self.send(Command::Switch(id, generation));
    }
    pub fn is_switching_to(&self, id: TabId) -> bool {
        *self.requested_tab.lock().unwrap() == Some(id)
    }
}
pub fn start(workspace: Workspace) -> Client {
    let snapshot = Snapshot {
        workspace: workspace.clone(),
        selected: None,
        live: vec![],
        windows: vec![],
        occupied: vec![],
        status: "Add an existing window to begin.".into(),
        trusted: false,
        paused: false,
        stopped: false,
        quitting: false,
        close_attempt: 0,
        restore_pending: 0,
        backdrop: None,
        selected_frame: None,
        editing_ack: 0,
        area: workspace.geometry,
        stacked_windows: vec![],
        badges: std::collections::BTreeMap::new(),
    };
    let client = Client {
        mailbox: Arc::new(Mailbox::default()),
        snapshot: Arc::new(Mutex::new(snapshot)),
        generation: Arc::new(AtomicU64::new(0)),
        pointer_down: Arc::new(AtomicBool::new(false)),
        focus_suspended: Arc::new(AtomicBool::new(false)),
        requested_tab: Arc::new(Mutex::new(None)),
    };
    let c = client.clone();
    thread::spawn(move || {
        let mut backend = MacBackend::new();
        backend.focus_suspended = c.focus_suspended.clone();
        let mut engine = Engine::new(backend, workspace);
        let mut editing_ack = 0;
        let mut badges = std::collections::BTreeMap::new();
        let mut windows = vec![];
        let mut status = "Add an existing window to begin.".to_string();
        let mut quitting = false;
        let mut close_attempt = 0;
        let mut next_observe = Instant::now();
        let mut save_after: Option<Instant> = None;
        loop {
            let deadline = save_after.map_or(next_observe, |t| t.min(next_observe));
            let command = c
                .mailbox
                .receive(deadline.saturating_duration_since(Instant::now()));
            let completing_switch = match &command {
                Some(Command::Switch(id, generation)) => Some((*id, *generation)),
                _ => None,
            };
            let mut dirty = false;
            let mut stopped = false;
            let result: Result<()> = match command {
                Some(Command::ReadBadges(pid, targets)) => {
                    if !c.pointer_down.load(Ordering::Relaxed)
                        && !c.focus_suspended.load(Ordering::SeqCst)
                    {
                        let targets: Vec<_> = targets
                            .into_iter()
                            .filter(|(id, window, _)| {
                                engine.live.get(id).is_some_and(|a| a.window == *window)
                            })
                            .map(|(id, _, path)| (id, path))
                            .collect();
                        badges = crate::macos::read_dock_badges(pid, &targets).unwrap_or_default();
                    }
                    Ok(())
                }
                Some(Command::EditingBarrier(epoch)) => {
                    editing_ack = epoch;
                    Ok(())
                }
                Some(Command::Apps(apps)) => {
                    engine.backend.apps = apps;
                    Ok(())
                }
                Some(Command::Discover) => engine.backend.discover().map(|w| {
                    windows = w;
                }),
                Some(Command::Attach(tab, id)) => {
                    dirty = true;
                    match windows.iter().find(|w| w.id == id) {
                        Some(w) => engine.attach(tab, w).and_then(|id| engine.switch(id)),
                        None => Err("Window list changed; refresh the picker.".into()),
                    }
                }
                Some(Command::Switch(id, generation))
                    if generation == c.generation.load(Ordering::SeqCst) =>
                {
                    engine.switch_current(id, || generation == c.generation.load(Ordering::SeqCst))
                }
                Some(Command::Resize(area, geometry)) => {
                    save_after = Some(Instant::now() + Duration::from_millis(250));
                    next_observe = Instant::now() + Duration::from_millis(100);
                    engine.workspace.geometry = geometry;
                    engine.follow_workspace(area)
                }
                Some(Command::Rename(id, name)) => {
                    dirty = true;
                    if let Some(t) = engine.workspace.tabs.iter_mut().find(|t| t.id == id) {
                        t.name = name;
                    }
                    Ok(())
                }
                Some(Command::Reorder(id, index)) => {
                    dirty = true;
                    if let Some(from) = engine.workspace.tabs.iter().position(|t| t.id == id) {
                        let tab = engine.workspace.tabs.remove(from);
                        let target = index.min(engine.workspace.tabs.len());
                        engine.workspace.tabs.insert(target, tab);
                    }
                    Ok(())
                }
                Some(Command::Release(id)) => {
                    dirty = true;
                    engine.detach(id);
                    // Publish detached state before any slow restoration IPC.
                    {
                        let mut view = c.snapshot.lock().unwrap();
                        view.workspace = engine.workspace.clone();
                        view.selected = engine.selected;
                        view.backdrop = None;
                        view.selected_frame = None;
                        view.stacked_windows.retain(|(tab, _, _)| *tab != id);
                        view.badges.remove(&id);
                        badges.remove(&id);
                        view.live.retain(|(tab, _)| *tab != id);
                        view.restore_pending = engine.released.len();
                        view.status = "Window released; restoring original state…".into();
                    }
                    engine.restore_released(id)
                }
                Some(Command::RetryRestoration) => engine.retry_released(),
                Some(Command::Pause) => {
                    engine.paused = Some("Desktop changed".into());
                    Ok(())
                }
                Some(Command::Resume) => engine.resume(),
                Some(Command::RequestPermission) => {
                    crate::macos::request_permission();
                    Ok(())
                }
                Some(Command::Raise) => {
                    if engine.paused.is_none() {
                        if let Some(a) = engine.selected.and_then(|id| engine.live.get(&id)) {
                            engine.backend.focus(a.window)
                        } else {
                            Ok(())
                        }
                    } else {
                        Ok(())
                    }
                }
                Some(Command::Quit) => {
                    dirty = true;
                    quitting = true;
                    close_attempt += 1;
                    engine.observe();
                    match engine.restore_all() {
                        Ok(()) => {
                            stopped = true;
                            Ok(())
                        }
                        Err(e) => Err(format!(
                            "Cannot finish closing: {e}. Resolve the window-control error, then Retry close."
                        )),
                    }
                }
                Some(Command::CancelQuit) => {
                    quitting = false;
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(e) = result {
                status = e;
            } else if dirty {
                status = "Ready".into();
            }
            let notes = std::mem::take(&mut engine.restoration_notes);
            if !notes.is_empty() {
                for note in &notes {
                    eprintln!("{note}");
                }
                if engine.released.is_empty() && !quitting {
                    status = notes.join("\n");
                }
            }
            if !stopped && !quitting && Instant::now() >= next_observe {
                next_observe = Instant::now() + Duration::from_millis(250);
                engine.pointer_down = c.pointer_down.load(Ordering::Relaxed);
                let tabs_before = engine.workspace.tabs.len();
                engine.observe();
                dirty |= engine.workspace.tabs.len() != tabs_before;
            }
            if dirty || save_after.is_some_and(|t| Instant::now() >= t) {
                save_after = None;
                if let Err(e) = persistence::save(&persistence::path(), &engine.workspace) {
                    status = format!("Could not save workspace: {e}");
                }
            }
            if next_observe <= Instant::now() {
                next_observe = Instant::now() + Duration::from_millis(250);
            }
            badges.retain(|id, _| engine.live.contains_key(id));
            *c.snapshot.lock().unwrap() = Snapshot {
                workspace: engine.workspace.clone(),
                selected: engine.selected,
                live: engine.live.iter().map(|(id, a)| (*id, a.window)).collect(),
                windows: windows.clone(),
                occupied: windows
                    .iter()
                    .filter(|w| {
                        engine
                            .live
                            .values()
                            .any(|a| engine.backend.same_window(a.window, w.id))
                    })
                    .map(|w| w.id)
                    .collect(),
                status: if !engine.released.is_empty() {
                    format!(
                        "{} released window(s) await restoration. AppDock → Retry restoration. {}",
                        engine.released.len(),
                        status
                    )
                } else if quitting {
                    status.clone()
                } else {
                    engine
                        .paused
                        .as_ref()
                        .map(|p| format!("Paused: {p}. Select Resume."))
                        .unwrap_or_else(|| status.clone())
                },
                trusted: engine.backend.trusted(),
                paused: engine.paused.is_some(),
                stopped,
                quitting,
                close_attempt,
                restore_pending: engine.released.len(),
                editing_ack,
                area: engine.area,
                badges: badges.clone(),
                stacked_windows: engine
                    .live
                    .iter()
                    .filter_map(|(tab, a)| {
                        engine
                            .backend
                            .stacking_identity(a.window)
                            .map(|(number, pid)| (*tab, number, pid))
                    })
                    .collect(),
                selected_frame: engine
                    .selected
                    .and_then(|id| engine.live.get(&id))
                    .map(|a| a.expected.frame),
                backdrop: engine
                    .selected
                    .and_then(|id| engine.live.get(&id))
                    .and_then(|a| engine.backend.window_number(a.window))
                    .map(|number| {
                        (
                            number,
                            engine
                                .live
                                .values()
                                .filter(|a| a.docked)
                                .map(|a| a.expected.frame)
                                .collect(),
                        )
                    }),
            };
            if let Some((id, generation)) = completing_switch {
                let mut requested = c.requested_tab.lock().unwrap();
                if *requested == Some(id) && generation == c.generation.load(Ordering::SeqCst) {
                    requested.take();
                }
            }
            if stopped {
                break;
            }
        }
    });
    client
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_barrier_cancels_pending_focus_and_precedes_geometry() {
        let m = Mailbox::default();
        m.push(Command::Raise);
        m.push(Command::Switch(1, 1));
        m.push(Command::EditingBarrier(2));
        let r = Rect::default();
        m.push(Command::Resize(r, r));
        assert!(matches!(
            m.receive(Duration::ZERO),
            Some(Command::EditingBarrier(2))
        ));
        assert!(matches!(
            m.receive(Duration::ZERO),
            Some(Command::Resize(..))
        ));
        assert!(m.receive(Duration::ZERO).is_none());
    }
    #[test]
    fn newest_geometry_is_delivered_ahead_of_background_work() {
        let m = Mailbox::default();
        m.push(Command::Discover);
        let mut r = Rect::default();
        m.push(Command::Resize(r, r));
        r.x = 800.;
        m.push(Command::Resize(r, r));
        assert!(matches!(m.receive(Duration::ZERO),Some(Command::Resize(a,_)) if a.x==800.));
        assert!(matches!(m.receive(Duration::ZERO), Some(Command::Discover)));
        assert!(m.receive(Duration::ZERO).is_none());
    }
    #[test]
    fn release_discards_stale_movement_switch_and_raise() {
        let m = Mailbox::default();
        let r = Rect::default();
        m.push(Command::Resize(r, r));
        m.push(Command::Switch(1, 1));
        m.push(Command::Raise);
        m.push(Command::Release(1));
        assert!(matches!(
            m.receive(Duration::ZERO),
            Some(Command::Release(1))
        ));
        assert!(m.receive(Duration::ZERO).is_none());
    }
    #[test]
    fn waiting_worker_is_notified_by_a_new_command() {
        let m = Arc::new(Mailbox::default());
        let target = m.clone();
        let waiter = thread::spawn(move || target.receive(Duration::from_secs(1)));
        m.push(Command::Release(9));
        assert!(matches!(waiter.join().unwrap(), Some(Command::Release(9))));
    }
}
