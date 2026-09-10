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
    AutoAttach(WindowId),
    SetStartupApp(StartupApp, bool),
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
#[derive(Clone, Copy, Debug)]
struct GeometryRequest {
    area: Rect,
    geometry: Rect,
    revision: u64,
}
#[derive(Default)]
struct MailboxState {
    queue: VecDeque<Command>,
    geometry: Option<GeometryRequest>,
    revision: u64,
}
#[derive(Default)]
struct Mailbox {
    state: Mutex<MailboxState>,
    wake: Condvar,
    interruption: AtomicU64,
}
impl Mailbox {
    fn push(&self, c: Command) {
        let mut state = self.state.lock().unwrap();
        if let Command::Resize(area, geometry) = &c {
            state.revision += 1;
            state.geometry = Some(GeometryRequest {
                area: *area,
                geometry: *geometry,
                revision: state.revision,
            });
        }
        if matches!(
            c,
            Command::Quit | Command::Pause | Command::Release(_) | Command::EditingBarrier(_)
        ) {
            self.interruption.fetch_add(1, Ordering::SeqCst);
        }
        let q = &mut state.queue;
        match c {
            Command::Discover => {
                q.retain(|c| !matches!(c, Command::Discover));
                q.push_back(c);
            }
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
        let state = self.state.lock().unwrap();
        let (mut state, _) = self
            .wake
            .wait_timeout_while(state, timeout, |state| state.queue.is_empty())
            .unwrap();
        state.queue.pop_front()
    }
    fn geometry(&self) -> Option<GeometryRequest> {
        self.state.lock().unwrap().geometry
    }
    fn priority_pending(&self) -> bool {
        self.state.lock().unwrap().queue.iter().any(|c| {
            matches!(
                c,
                Command::Quit | Command::Pause | Command::Release(_) | Command::EditingBarrier(_)
            )
        })
    }
}
#[derive(Clone)]
pub struct Client {
    mailbox: Arc<Mailbox>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    generation: Arc<AtomicU64>,
    pointer_down: Arc<AtomicBool>,
    focus_suspended: Arc<AtomicBool>,
    requested_tab: Arc<Mutex<Option<(TabId, u64)>>>,
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
        if matches!(
            c,
            Command::Pause
                | Command::Quit
                | Command::Release(_)
                | Command::Attach(..)
                | Command::SetStartupApp(..)
        ) {
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.requested_tab.lock().unwrap().take();
        }
        self.mailbox.push(c);
    }
    pub fn switch(&self, id: TabId) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.requested_tab.lock().unwrap() = Some((id, generation));
        self.send(Command::Switch(id, generation));
    }
    pub fn navigate(
        &self,
        tabs: &[SavedTab],
        selected: Option<TabId>,
        previous: bool,
    ) -> Option<TabId> {
        let mut requested = self.requested_tab.lock().unwrap();
        let target = navigation_target(tabs, requested.map(|(id, _)| id), selected, previous)?;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *requested = Some((target, generation));
        self.mailbox.push(Command::Switch(target, generation));
        Some(target)
    }
    fn acknowledge_switch(&self, id: TabId, generation: u64) {
        let mut requested = self.requested_tab.lock().unwrap();
        if *requested == Some((id, generation)) {
            requested.take();
        }
    }
    pub fn is_switching_to(&self, id: TabId) -> bool {
        self.requested_tab
            .lock()
            .unwrap()
            .is_some_and(|(tab, _)| tab == id)
    }
}
fn client(workspace: Workspace) -> Client {
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
    Client {
        mailbox: Arc::new(Mailbox::default()),
        snapshot: Arc::new(Mutex::new(snapshot)),
        generation: Arc::new(AtomicU64::new(0)),
        pointer_down: Arc::new(AtomicBool::new(false)),
        focus_suspended: Arc::new(AtomicBool::new(false)),
        requested_tab: Arc::new(Mutex::new(None)),
    }
}
pub fn start(workspace: Workspace) -> Client {
    let client = client(workspace.clone());
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
        let mut geometry_revision = 0;
        let mut startup = crate::startup::Startup::new(engine.workspace.startup_apps.clone());
        let mut discovery_complete = false;
        loop {
            let deadline = save_after.map_or(next_observe, |t| t.min(next_observe));
            let command = c
                .mailbox
                .receive(if startup.has_pending() {
                    Duration::ZERO
                } else {
                    deadline.saturating_duration_since(Instant::now())
                })
                .or_else(|| startup.next().map(Command::AutoAttach));
            if matches!(
                command,
                Some(
                    Command::Attach(..)
                        | Command::SetStartupApp(..)
                        | Command::Switch(..)
                        | Command::Release(_)
                        | Command::Pause
                        | Command::Quit
                        | Command::EditingBarrier(_)
                )
            ) {
                startup.cancel();
            }
            let completing_switch = match &command {
                Some(Command::Switch(id, generation)) => Some((*id, *generation)),
                _ => None,
            };
            let mut dirty = false;
            let geometry = c.mailbox.geometry();
            if let Some(request) = geometry.filter(|request| request.revision > geometry_revision) {
                engine.workspace.geometry = request.geometry;
                geometry_revision = request.revision;
                save_after = Some(Instant::now() + Duration::from_millis(250));
            }
            let mut stopped = false;
            let result: Result<()> = match command {
                Some(
                    Command::Attach(..)
                    | Command::AutoAttach(_)
                    | Command::Switch(..)
                    | Command::Raise
                    | Command::Resume,
                ) if quitting => Ok(()),
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
                    engine.backend.update_apps(apps);
                    Ok(())
                }
                Some(Command::Discover) => {
                    let epoch = c.mailbox.interruption.load(Ordering::SeqCst);
                    engine
                        .backend
                        .discover_current(|| {
                            epoch == c.mailbox.interruption.load(Ordering::SeqCst)
                                && !c.mailbox.priority_pending()
                        })
                        .map(|w| {
                            windows = w;
                            discovery_complete = true;
                        })
                }
                Some(Command::SetStartupApp(app, enabled)) => {
                    dirty = true;
                    engine.workspace.set_startup_app(app, enabled);
                    Ok(())
                }
                Some(Command::AutoAttach(id)) => {
                    let epoch = c.generation.load(Ordering::SeqCst);
                    let current = || {
                        epoch == c.generation.load(Ordering::SeqCst)
                            && !c.focus_suspended.load(Ordering::SeqCst)
                            && !c.pointer_down.load(Ordering::Relaxed)
                            && !c.mailbox.priority_pending()
                    };
                    if engine.paused.is_none() && current() {
                        match windows.iter().find(|w| w.id == id) {
                            Some(window)
                                if !engine
                                    .live
                                    .values()
                                    .any(|a| engine.backend.same_window(a.window, id)) =>
                            {
                                dirty = true;
                                engine
                                    .attach(None, window)
                                    .and_then(|tab| engine.switch_current(tab, current))
                            }
                            _ => Ok(()),
                        }
                    } else {
                        startup.cancel();
                        Ok(())
                    }
                }
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
                Some(Command::Resize(..)) => {
                    next_observe = Instant::now() + Duration::from_millis(100);
                    apply_geometry(&mut engine, geometry, !quitting)
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
                    let restored = engine.restore_released(id);
                    let moved = apply_geometry(&mut engine, geometry, !quitting);
                    restored.and(moved)
                }
                Some(Command::RetryRestoration) => engine.retry_released(),
                Some(Command::Pause) => {
                    engine.paused = Some("Desktop changed".into());
                    apply_geometry(&mut engine, geometry, false)
                }
                Some(Command::Resume) => {
                    apply_geometry(&mut engine, geometry, false).and_then(|()| engine.resume())
                }
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
                    let _ = apply_geometry(&mut engine, geometry, false);
                    match engine.restore_all() {
                        Ok(()) => {
                            stopped = true;
                            Ok(())
                        }
                        Err(e) => Err(e.context("Cannot finish closing. Resolve the window-control error, then Retry close")),
                    }
                }
                Some(Command::CancelQuit) => {
                    quitting = false;
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(e) = result {
                if e.kind != ErrorKind::Cancelled {
                    status = e.to_string();
                }
            } else if dirty {
                status = "Ready".into();
            }
            if discovery_complete
                && geometry.is_some()
                && !quitting
                && engine.paused.is_none()
                && !c.focus_suspended.load(Ordering::SeqCst)
                && !c.pointer_down.load(Ordering::Relaxed)
            {
                startup.resolve(
                    &windows,
                    &engine.live.values().map(|a| a.window).collect::<Vec<_>>(),
                );
            }
            if let Some(notice) = startup.notice() {
                status = notice;
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
            if !stopped
                && !quitting
                && !c.mailbox.priority_pending()
                && Instant::now() >= next_observe
            {
                next_observe = Instant::now() + Duration::from_millis(250);
                engine.pointer_down = c.pointer_down.load(Ordering::Relaxed);
                let tabs_before = engine.workspace.tabs.len();
                engine.observe();
                dirty |= engine.workspace.tabs.len() != tabs_before;
            }
            if let Some(request) = c
                .mailbox
                .geometry()
                .filter(|request| request.revision > geometry_revision)
            {
                geometry_revision = request.revision;
                engine.workspace.geometry = request.geometry;
                if quitting || engine.paused.is_some() {
                    engine.area = request.area;
                }
                dirty = true;
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
                c.acknowledge_switch(id, generation);
            }
            if stopped {
                break;
            }
        }
    });
    client
}

/// Geometry persists independently of a cancellable movement command. Reading an
/// older revision never consumes a newer request in the mailbox.
fn apply_geometry<B: WindowBackend>(
    engine: &mut Engine<B>,
    request: Option<GeometryRequest>,
    move_windows: bool,
) -> Result<()> {
    if let Some(request) = request {
        engine.workspace.geometry = request.geometry;
        if move_windows {
            engine.follow_workspace(request.area)?;
        } else {
            engine.area = request.area;
        }
    }
    Ok(())
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
    #[test]
    fn a3_resize_release_multiple_releases_pause_resume_and_quit_save_latest() {
        use crate::engine::tests::fixture;
        let m = Mailbox::default();
        let mut e = fixture();
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        let old = e.backend.states[&1];
        let r = Rect {
            x: 700.,
            width: 1200.,
            ..Rect::default()
        };
        m.push(Command::Resize(r, r));
        m.push(Command::Release(1));
        assert!(matches!(
            m.receive(Duration::ZERO),
            Some(Command::Release(1))
        ));
        e.release(1).unwrap();
        let released = e.backend.states[&1];
        apply_geometry(&mut e, m.geometry(), true).unwrap();
        assert_eq!(e.backend.states[&1], released);
        assert_eq!(e.backend.states[&2].frame, r);
        assert_eq!(e.workspace.geometry, r);
        assert_eq!(old, released);
        let r2 = Rect {
            x: -300.,
            width: 1400.,
            ..r
        };
        m.push(Command::Resize(r2, r2));
        m.push(Command::Pause);
        e.paused = Some("test".into());
        apply_geometry(&mut e, m.geometry(), false).unwrap();
        assert_eq!(e.backend.states[&2].frame, r);
        e.resume().unwrap();
        assert_eq!(e.backend.states[&2].frame, r2);
        m.push(Command::Release(2));
        e.release(2).unwrap();
        let before_quit = e.backend.states.clone();
        m.push(Command::Resize(r, r));
        m.push(Command::Quit);
        apply_geometry(&mut e, m.geometry(), false).unwrap();
        e.restore_all().unwrap();
        assert_eq!(e.backend.states, before_quit);
        let path = std::env::temp_dir().join(format!("appdock-a3-{}.json", std::process::id()));
        persistence::save(&path, &e.workspace).unwrap();
        assert_eq!(persistence::load(&path).unwrap().geometry, r);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn a3_old_geometry_acknowledgement_cannot_erase_concurrent_request() {
        let m = Mailbox::default();
        let mut e = crate::engine::tests::fixture();
        let old = Rect::default();
        let new = Rect { x: 900., ..old };
        m.push(Command::Resize(old, old));
        let processing = m.geometry();
        m.push(Command::Resize(new, new));
        apply_geometry(&mut e, processing, true).unwrap();
        assert_eq!(m.geometry().unwrap().geometry, new);
        assert!(m.geometry().unwrap().revision > processing.unwrap().revision);
        apply_geometry(&mut e, m.geometry(), true).unwrap();
        assert_eq!(e.workspace.geometry, new);
    }
    #[test]
    fn a5_discovery_coalesces_and_priority_interrupts_current_call() {
        let m = Mailbox::default();
        m.push(Command::Discover);
        m.push(Command::Discover);
        assert!(matches!(m.receive(Duration::ZERO), Some(Command::Discover)));
        assert!(m.receive(Duration::ZERO).is_none());
        let epoch = m.interruption.load(Ordering::SeqCst);
        let result = crate::native_ops::prepared_request(
            || epoch == m.interruption.load(Ordering::SeqCst),
            || Ok(()),
            || {
                m.push(Command::Discover);
                m.push(Command::Release(1));
                m.push(Command::Quit);
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().kind, ErrorKind::Cancelled);
        assert!(matches!(m.receive(Duration::ZERO), Some(Command::Quit)));
        assert!(matches!(
            m.receive(Duration::ZERO),
            Some(Command::Release(1))
        ));
        assert!(matches!(m.receive(Duration::ZERO), Some(Command::Discover)));
    }
    #[test]
    fn a6_bursts_mixed_directions_wraparound_and_stale_acknowledgements() {
        let workspace = crate::engine::tests::fixture().workspace;
        let c = client(workspace.clone());
        let tabs = &workspace.tabs;
        assert_eq!(c.navigate(tabs, Some(1), false), Some(2));
        let first = c.generation.load(Ordering::SeqCst);
        assert_eq!(c.navigate(tabs, Some(1), false), Some(1));
        assert_eq!(c.navigate(tabs, Some(1), true), Some(2));
        c.acknowledge_switch(2, first);
        assert!(c.is_switching_to(2));
        let last = c.generation.load(Ordering::SeqCst);
        c.acknowledge_switch(2, last);
        assert!(!c.is_switching_to(2));
        assert_eq!(c.navigate(tabs, Some(1), false), Some(2)); // failed switch falls back to confirmed
        c.set_text_editing(true);
        assert!(!c.is_switching_to(2));
        assert!(matches!(
            c.mailbox.receive(Duration::ZERO),
            Some(Command::EditingBarrier(_))
        ));
    }
}
