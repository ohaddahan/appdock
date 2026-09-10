use crate::model::*;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Attachment {
    pub window: WindowId,
    pub original: WindowState,
    pub expected: WindowState,
    pub docked: bool,
    /// Startup registration has not attempted to control this window yet.
    pub deferred_startup: bool,
}
pub struct Engine<B: WindowBackend> {
    pub backend: B,
    pub workspace: Workspace,
    pub live: HashMap<TabId, Attachment>,
    pub released: HashMap<TabId, Attachment>,
    pub selected: Option<TabId>,
    pub paused: Option<String>,
    pub area: Rect,
    pub pointer_down: bool,
    pub restoration_notes: Vec<String>,
}
impl<B: WindowBackend> Engine<B> {
    pub fn new(backend: B, workspace: Workspace) -> Self {
        let area = workspace.geometry;
        Self {
            backend,
            workspace,
            live: HashMap::new(),
            released: HashMap::new(),
            selected: None,
            paused: None,
            area,
            pointer_down: false,
            restoration_notes: vec![],
        }
    }
    pub fn attach(&mut self, tab: Option<TabId>, window: &WindowInfo) -> Result<TabId> {
        if !self.backend.trusted() {
            return Err(BackendError::new(
                ErrorKind::Permission,
                "Enable Accessibility in System Settings, then Resume.",
            ));
        }
        if !window.eligible {
            return Err("Window does not support moving, resizing, minimizing and raising.".into());
        }
        if self
            .live
            .values()
            .any(|a| self.backend.same_window(a.window, window.id))
        {
            return Err("This window already belongs to a tab.".into());
        }
        if tab.is_none()
            && self.workspace.tabs.iter().any(|t| {
                !self.live.contains_key(&t.id) && t.identity.bundle == window.identity.bundle
            })
        {
            return Err("This app has a disconnected tab. Use Replace window on that tab, or release it before adding another window.".into());
        }
        if let Some(id) = tab
            && (self.live.contains_key(&id) || !self.workspace.tabs.iter().any(|t| t.id == id))
        {
            return Err("Replacement tab is no longer disconnected.".into());
        }
        let original = self.backend.state(window.id)?;
        if original.fullscreen || original.modal || !original.frame.valid() {
            return Err("Leave fullscreen and close dialogs before attaching.".into());
        }
        let id = match tab {
            Some(id) => id,
            None => self
                .workspace
                .tabs
                .iter()
                .map(|t| t.id)
                .chain(self.released.keys().copied())
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .filter(|id| *id <= isize::MAX as u64)
                .ok_or("Tab ID space exhausted")?,
        };
        if let Some(t) = self.workspace.tabs.iter_mut().find(|t| t.id == id) {
            t.identity = window.identity.clone();
        } else {
            self.workspace.tabs.push(SavedTab {
                id,
                name: window.app.clone(),
                identity: window.identity.clone(),
            });
        }
        // A new explicit attachment supersedes any old detached recovery for this window.
        let superseded: Vec<_> = self
            .released
            .iter()
            .filter(|(_, a)| self.backend.same_window(a.window, window.id))
            .map(|(id, a)| (*id, a.window))
            .collect();
        for (id, old_window) in superseded {
            self.released.remove(&id);
            self.backend.watch(old_window, false);
        }
        self.live.insert(
            id,
            Attachment {
                window: window.id,
                original,
                expected: original,
                docked: false,
                deferred_startup: false,
            },
        );
        self.backend.watch(window.id, true);
        Ok(id)
    }
    /// Save the current live apps in tab order, once per bundle. Preferences do
    /// not contain live handles and changing them never changes attachments.
    pub fn save_startup_apps(&mut self, windows: &[WindowInfo]) {
        let mut apps = Vec::<StartupApp>::new();
        for tab in &self.workspace.tabs {
            let Some(attachment) = self.live.get(&tab.id) else {
                continue;
            };
            if tab.identity.bundle.is_empty()
                || apps.iter().any(|app| app.bundle == tab.identity.bundle)
            {
                continue;
            }
            let name = windows
                .iter()
                .find(|window| window.id == attachment.window)
                .map_or(&tab.name, |window| &window.app)
                .clone();
            apps.push(StartupApp {
                bundle: tab.identity.bundle.clone(),
                name,
            });
        }
        self.workspace.startup_apps = apps;
    }
    pub fn reset_startup_apps(&mut self) {
        self.workspace.startup_apps.clear();
    }
    /// Register startup tabs without restoring and focusing every window in turn.
    /// Untouched windows retain their original state until explicitly selected.
    pub fn attach_startup(
        &mut self,
        window: &WindowInfo,
        current: impl Fn() -> bool,
    ) -> Result<TabId> {
        if !current() {
            return Err(BackendError::cancelled());
        }
        let id = self.attach(None, window)?;
        self.live.get_mut(&id).unwrap().deferred_startup = true;
        if self.selected.is_none() {
            self.switch_current(id, current)?;
        }
        Ok(id)
    }
    pub fn switch(&mut self, id: TabId) -> Result<()> {
        self.switch_current(id, || true)
    }
    pub fn switch_current(&mut self, id: TabId, current: impl Fn() -> bool) -> Result<()> {
        if !current() {
            return Ok(());
        }
        if !self.backend.trusted() {
            self.paused = Some("Accessibility permission is unavailable".into());
            return Err(BackendError::new(
                ErrorKind::Permission,
                "Enable Accessibility, then Resume.",
            ));
        }
        if let Some(reason) = &self.paused {
            return Err(format!("Docking paused: {reason}. Select Resume.").into());
        }
        let next = self
            .live
            .get(&id)
            .ok_or("Disconnected tab: select Replace window")?
            .clone();
        let before = self.backend.state(next.window)?;
        if before.fullscreen || before.modal {
            self.paused = Some("Fullscreen or dialog is active".into());
            return Err("Close the dialog or leave fullscreen, then Resume.".into());
        }
        // Validate the previous window before changing focus or geometry.
        if let Some(old) = self
            .selected
            .filter(|old| *old != id)
            .and_then(|old| self.live.get(&old))
        {
            let state = self.backend.state(old.window)?;
            if state.modal || state.fullscreen {
                self.paused = Some("Previous window has a dialog or is fullscreen".into());
                return Err("Close the dialog or leave fullscreen, then Resume.".into());
            }
        }
        // The UI orders an opaque backdrop below this exact focused window.
        // Inactive tabs remain open; only an already minimized target needs restoring.
        // From this point even a failed preparation may need restoration.
        self.live.get_mut(&id).unwrap().deferred_startup = false;
        let prepared: Result<Rect> = (|| {
            if before.minimized {
                self.backend.minimize(next.window, false)?;
                self.backend.validate_restored_window(next.window)?;
            }
            let frame = self.backend.set_frame(next.window, self.area)?;
            if !current() {
                return Err(BackendError::cancelled());
            }
            self.backend.focus(next.window)?;
            Ok(frame)
        })();
        let frame = match prepared {
            Ok(f) => f,
            Err(e) => {
                let rollback = self.backend.restore(next.window, before);
                if let Some(old) = self.selected.and_then(|id| self.live.get(&id)) {
                    let _ = self.backend.focus(old.window);
                }
                let mut error = e.context("Switch failed");
                if let Err(rollback) = rollback {
                    error.causes.push(rollback.context("Rollback"));
                }
                return Err(error);
            }
        };
        if !current() {
            self.backend.restore(next.window, before)?;
            if let Some(old) = self.selected.and_then(|id| self.live.get(&id)) {
                let _ = self.backend.focus(old.window);
            }
            return Ok(());
        }
        self.selected = Some(id);
        if let Some(a) = self.live.get_mut(&id) {
            a.docked = true;
            a.expected = WindowState {
                frame,
                minimized: false,
                ..before
            };
        }
        Ok(())
    }
    pub fn resize(&mut self, area: Rect) -> Result<()> {
        self.area = area;
        if self.paused.is_some() {
            return Ok(());
        }
        for a in self.live.values_mut().filter(|a| a.docked) {
            let state = self.backend.state(a.window)?;
            if state.fullscreen || state.modal {
                self.paused = Some("Fullscreen or dialog is active".into());
                return Ok(());
            }
            if !state.frame.near(a.expected.frame) || state.minimized != a.expected.minimized {
                // The observer restores the target after the user releases the mouse.
                continue;
            }
            a.expected.frame = self.backend.set_frame(a.window, area)?;
        }
        Ok(())
    }
    /// Detaching is unconditional. Failed restoration must never re-dock a released window.
    pub fn detach(&mut self, id: TabId) {
        if let Some(a) = self.live.remove(&id) {
            if a.deferred_startup {
                self.backend.watch(a.window, false);
            } else {
                // Lifecycle observation remains read-only for detached windows.
                self.released.insert(id, a);
            }
        }
        self.workspace.tabs.retain(|t| t.id != id);
        if self.selected == Some(id) {
            self.selected = None;
        }
    }
    pub fn restore_released(&mut self, id: TabId) -> Result<()> {
        if let Some(a) = self.released.get(&id).cloned() {
            match self.backend.check_window(a.window) {
                Err(e) if e.kind == ErrorKind::Closed => {
                    self.backend.watch(a.window, false);
                    self.released.remove(&id);
                    return Ok(());
                }
                Err(e) => return Err(e.context("Released window still awaits restoration")),
                Ok(()) => {}
            }
            let restored = self.backend.restore_for_release(a.window, a.original)
                .map_err(|e| e.context("Window released; original state could not be restored. Use AppDock → Retry restoration"))?;
            self.backend.watch(a.window, false);
            self.record_restoration(id, restored);
            self.released.remove(&id);
        }
        Ok(())
    }
    pub fn release(&mut self, id: TabId) -> Result<()> {
        self.detach(id);
        self.restore_released(id)
    }
    pub fn retry_released(&mut self) -> Result<()> {
        let ids: Vec<_> = self.released.keys().copied().collect();
        let errors: Vec<_> = ids
            .into_iter()
            .filter_map(|id| self.restore_released(id).err())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(BackendError::multiple(errors))
        }
    }
    pub fn follow_workspace(&mut self, area: Rect) -> Result<()> {
        let previous = self.area;
        if previous.width != area.width || previous.height != area.height {
            return self.resize(area);
        }
        self.area = area;
        if self.paused.is_some() {
            return Ok(());
        }
        for a in self.live.values_mut().filter(|a| a.docked) {
            // Position-only AX writes preserve app-enforced size limits and avoid
            // resize IPC, settling sleeps, and repeated full-state reads per pixel.
            self.backend.move_window(a.window, area.x, area.y)?;
            a.expected.frame.x = area.x;
            a.expected.frame.y = area.y;
        }
        Ok(())
    }
    pub fn restore_all(&mut self) -> Result<()> {
        let mut errors: Vec<_> = self.retry_released().err().into_iter().collect();
        let mut restored = vec![];
        let mut outcomes = vec![];
        for (&id, a) in &self.live {
            if a.deferred_startup {
                self.backend.watch(a.window, false);
                restored.push(id);
                continue;
            }
            match self.backend.check_window(a.window).and_then(|()| {
                let mut target = a.original;
                if self.workspace.keep_apps_open_on_close && a.docked {
                    // Keep current visibility (including a user-minimized
                    // window), but still return to the original geometry.
                    target.minimized = self.backend.state(a.window)?.minimized;
                }
                self.backend.restore_for_release(a.window, target)
            }) {
                Err(e) if e.kind == ErrorKind::Closed => {
                    self.backend.watch(a.window, false);
                    restored.push(id);
                }
                Err(e) => errors.push(e.context(format!("Tab {id}"))),
                Ok(outcome) => {
                    self.backend.watch(a.window, false);
                    restored.push(id);
                    outcomes.push((id, outcome));
                }
            }
        }
        for (id, outcome) in outcomes {
            self.record_restoration(id, outcome);
        }
        for id in restored {
            self.live.remove(&id);
            self.workspace.tabs.retain(|t| t.id != id);
            if self.selected == Some(id) {
                self.selected = None;
            }
        }
        if errors.is_empty() {
            self.live.clear();
            self.selected = None;
            Ok(())
        } else {
            Err(BackendError::multiple(errors))
        }
    }
    fn record_restoration(&mut self, id: TabId, outcome: Restoration) {
        if let Restoration::Adjusted { requested, actual } = outcome {
            self.restoration_notes.push(format!(
                "Tab {id} released at the available window size and position. Original: {requested:?}; actual: {actual:?}"
            ));
        }
    }
    pub fn observe(&mut self) {
        for event in self.backend.events() {
            match event {
                BackendEvent::PermissionLost => {
                    self.paused = Some("Accessibility permission was revoked".into())
                }
                BackendEvent::Closed(w) => {
                    self.released.retain(|_, a| a.window != w);
                    self.backend.watch(w, false);
                    let ids: Vec<_> = self
                        .live
                        .iter()
                        .filter(|(_, a)| a.window == w)
                        .map(|(id, _)| *id)
                        .collect();
                    for id in ids {
                        self.live.remove(&id);
                        self.workspace.tabs.retain(|t| t.id != id);
                        self.backend.watch(w, false);
                        if self.selected == Some(id) {
                            self.selected = None;
                        }
                    }
                }
                BackendEvent::Changed(w) => {
                    if let Some((&id, a)) =
                        self.live.iter().find(|(_, a)| a.window == w && a.docked)
                    {
                        let expected = a.expected;
                        match self.backend.state(w) {
                            Ok(state)
                                if state.fullscreen
                                    || state.modal
                                    || state.minimized != expected.minimized =>
                            {
                                self.paused = Some(
                                    "Target minimized or entered fullscreen/dialog state".into(),
                                );
                            }
                            Ok(state)
                                if !state.frame.near(expected.frame)
                                    && self.paused.is_none()
                                    && !self.pointer_down =>
                            {
                                // Movement does not release ownership. Wait until the mouse is
                                // released, then put the selected window back in the workspace.
                                let target = if a.docked { self.area } else { expected.frame };
                                match self.backend.set_frame(w, target) {
                                    Ok(frame) => {
                                        self.live.get_mut(&id).unwrap().expected.frame = frame
                                    }
                                    Err(e) => self.paused = Some(e.to_string()),
                                }
                            }
                            Err(e) if !self.pointer_down => self.paused = Some(e.to_string()),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    pub fn resume(&mut self) -> Result<()> {
        // Remain paused even after a partial failure; successful changes update
        // expected state immediately, while original restoration snapshots survive.
        self.paused = Some("Resuming docking".into());
        let result = (|| {
            if !self.backend.trusted() {
                return Err(BackendError::new(
                    ErrorKind::Permission,
                    "Accessibility permission is still unavailable",
                ));
            }
            let mut ids: Vec<_> = self
                .live
                .iter()
                .filter(|(_, a)| a.docked)
                .map(|(id, _)| *id)
                .collect();
            ids.sort_unstable();
            let mut states = HashMap::new();
            for id in &ids {
                let window = self.live[id].window;
                self.backend.check_window(window)?;
                let state = self.backend.state(window)?;
                if state.fullscreen || state.modal {
                    return Err("Leave fullscreen and close dialogs before resuming".into());
                }
                states.insert(*id, state);
            }
            for id in ids {
                let a = self.live.get_mut(&id).unwrap();
                if states[&id].minimized {
                    self.backend.minimize(a.window, false)?;
                    a.expected.minimized = false;
                }
                a.expected.frame = self.backend.set_frame(a.window, self.area)?;
                a.expected.minimized = false;
                a.expected.fullscreen = false;
                a.expected.modal = false;
            }
            if let Some(a) = self
                .selected
                .and_then(|id| self.live.get(&id))
                .filter(|a| a.docked)
            {
                self.backend.focus(a.window)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.paused = None;
                Ok(())
            }
            Err(e) => {
                self.paused = Some(e.to_string());
                Err(e)
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[derive(Default)]
    pub(crate) struct Fake {
        pub(crate) states: HashMap<WindowId, WindowState>,
        aliases: HashMap<WindowId, WindowId>,
        moves: usize,
        resizes: usize,
        fail_focus: bool,
        fail_minimize: bool,
        minimize_calls: Vec<(WindowId, bool)>,
        focused: Option<WindowId>,
        events: Vec<BackendEvent>,
        minimum_width: Option<f64>,
        fail_resize: bool,
        fail_resize_window: Option<WindowId>,
        permission_lost: bool,
        lifecycle_error: Option<BackendError>,
        resize_error: Option<BackendError>,
        fail_restored_validation: bool,
        watched: std::collections::HashSet<WindowId>,
    }
    impl WindowBackend for Fake {
        fn same_window(&self, a: WindowId, b: WindowId) -> bool {
            self.aliases.get(&a).copied().unwrap_or(a) == self.aliases.get(&b).copied().unwrap_or(b)
        }
        fn trusted(&self) -> bool {
            !self.permission_lost
        }
        fn watch(&mut self, id: WindowId, enabled: bool) {
            if enabled {
                self.watched.insert(id);
            } else {
                self.watched.remove(&id);
            }
        }
        fn check_window(&mut self, id: WindowId) -> Result<()> {
            if let Some(error) = &self.lifecycle_error {
                return Err(error.clone());
            }
            self.states
                .get(&self.aliases.get(&id).copied().unwrap_or(id))
                .map(|_| ())
                .ok_or_else(|| BackendError::new(ErrorKind::Closed, "Confirmed process exit"))
        }
        fn discover(&mut self) -> Result<Vec<WindowInfo>> {
            Ok(vec![])
        }
        fn state(&self, id: WindowId) -> Result<WindowState> {
            self.states
                .get(&self.aliases.get(&id).copied().unwrap_or(id))
                .copied()
                .ok_or("state read failed".into())
        }
        fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> Result<()> {
            self.moves += 1;
            let state = self
                .states
                .get_mut(&self.aliases.get(&id).copied().unwrap_or(id))
                .unwrap();
            state.frame.x = x;
            state.frame.y = y;
            Ok(())
        }
        fn set_frame(&mut self, id: WindowId, mut f: Rect) -> Result<Rect> {
            self.resizes += 1;
            if let Some(error) = &self.resize_error {
                return Err(error.clone());
            }
            if self.fail_resize || self.fail_resize_window == Some(id) {
                return Err("Resize denied".into());
            }
            if let Some(minimum) = self.minimum_width {
                f.width = f.width.max(minimum);
            }
            self.states
                .get_mut(&self.aliases.get(&id).copied().unwrap_or(id))
                .unwrap()
                .frame = f;
            Ok(f)
        }
        fn minimize(&mut self, id: WindowId, v: bool) -> Result<()> {
            self.minimize_calls.push((id, v));
            if v && self.fail_minimize {
                return Err("denied".into());
            }
            self.states
                .get_mut(&self.aliases.get(&id).copied().unwrap_or(id))
                .unwrap()
                .minimized = v;
            Ok(())
        }
        fn validate_restored_window(&self, _id: WindowId) -> Result<()> {
            if self.fail_restored_validation {
                Err("Restored target is not a dockable standard window".into())
            } else {
                Ok(())
            }
        }
        fn focus(&mut self, id: WindowId) -> Result<()> {
            if self.fail_focus {
                Err("timeout".into())
            } else {
                self.focused = Some(id);
                Ok(())
            }
        }
        fn events(&mut self) -> Vec<BackendEvent> {
            std::mem::take(&mut self.events)
        }
    }
    pub(crate) fn fixture() -> Engine<Fake> {
        // Legacy restoration tests exercise the explicit original-state policy.
        let mut e = Engine::new(
            Fake::default(),
            Workspace {
                keep_apps_open_on_close: false,
                ..Workspace::default()
            },
        );
        for id in 1..=2 {
            e.backend.states.insert(
                id,
                WindowState {
                    frame: Rect::default(),
                    minimized: false,
                    fullscreen: false,
                    modal: false,
                },
            );
            e.attach(
                None,
                &WindowInfo {
                    id,
                    pid: 10,
                    app: "same".into(),
                    title: "duplicate".into(),
                    identity: Identity {
                        bundle: "same".into(),
                        identifier: None,
                    },
                    eligible: true,
                    minimized: false,
                },
            )
            .unwrap();
        }
        e
    }
    #[test]
    fn switching_duplicate_titles_focuses_exact_handle_without_minimizing() {
        let mut e = fixture();
        assert!(e.workspace.tabs.iter().all(|tab| tab.name == "same"));
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        assert!(!e.backend.states[&1].minimized);
        assert!(!e.backend.states[&2].minimized);
        assert_eq!(e.selected, Some(2));
        assert_eq!(e.backend.focused, Some(2));
        assert!(e.backend.minimize_calls.is_empty());
    }
    #[test]
    fn focus_failure_retains_previous_selection() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.backend.fail_focus = true;
        assert!(e.switch(2).is_err());
        assert_eq!(e.selected, Some(1));
        assert!(!e.backend.states[&1].minimized);
    }
    #[test]
    fn switching_does_not_require_minimizing_previous_window() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.area.x = 400.;
        e.backend.fail_minimize = true;
        e.switch(2).unwrap();
        assert_eq!(e.backend.states[&2].frame, e.area);
        assert_eq!(e.selected, Some(2));
        assert!(e.backend.minimize_calls.is_empty());
    }
    #[test]
    fn initially_minimized_target_is_restored_once_then_stays_open() {
        let mut e = fixture();
        e.backend.states.get_mut(&2).unwrap().minimized = true;
        e.switch(2).unwrap();
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        assert_eq!(e.backend.minimize_calls, vec![(2, false)]);
    }
    #[test]
    fn inactive_tabs_follow_workspace_but_unselected_attachments_do_not() {
        let mut e = fixture();
        let original = e.backend.states[&2];
        e.switch(1).unwrap();
        e.follow_workspace(Rect { x: 300., ..e.area }).unwrap();
        assert_eq!(e.backend.states[&2], original);
        e.switch(2).unwrap();
        e.follow_workspace(Rect { x: -400., ..e.area }).unwrap();
        assert!(
            e.backend
                .states
                .values()
                .all(|s| s.frame.x == -400. && !s.minimized)
        );
        e.resize(Rect {
            width: 1200.,
            ..e.area
        })
        .unwrap();
        assert!(e.backend.states.values().all(|s| s.frame == e.area));
    }
    #[test]
    fn previous_dialog_blocks_switch_before_any_window_changes() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.backend.states.get_mut(&1).unwrap().modal = true;
        let before = e.backend.states.clone();
        assert!(e.switch(2).is_err());
        assert_eq!(e.backend.states, before);
        assert_eq!(e.backend.focused, Some(1));
    }
    #[test]
    fn release_restores_original_state() {
        let mut e = fixture();
        let before = e.backend.states[&1];
        e.area.x = 400.;
        e.switch(1).unwrap();
        e.release(1).unwrap();
        assert_eq!(e.backend.states[&1], before);
        assert!(!e.live.contains_key(&1));
    }
    #[test]
    fn confirmed_closed_target_removes_its_tab() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.backend.events.push(BackendEvent::Closed(1));
        e.observe();
        assert!(!e.live.contains_key(&1));
        assert_eq!(
            e.workspace.tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(e.selected, None);
    }
    #[test]
    fn moved_window_returns_after_mouse_release_and_keeps_ownership() {
        let mut e = fixture();
        e.switch(1).unwrap();
        let original = e.live[&1].original;
        e.pointer_down = true;
        e.backend.states.get_mut(&1).unwrap().frame.x += 300.;
        let moved = e.backend.states[&1].frame;
        e.backend.events.push(BackendEvent::Changed(1));
        e.observe();
        assert_eq!(e.backend.states[&1].frame, moved);
        assert!(e.paused.is_none());
        e.pointer_down = false;
        e.backend.events.push(BackendEvent::Changed(1));
        e.observe();
        assert_eq!(e.backend.states[&1].frame, e.area);
        assert_eq!(e.live[&1].original, original);
        assert_eq!(e.selected, Some(1));
        assert_eq!(e.workspace.tabs.len(), 2);
    }
    #[test]
    fn exit_restores_every_attached_window() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        e.restore_all().unwrap();
        assert!(e.backend.states.values().all(|s| !s.minimized));
        assert!(e.live.is_empty());
        assert!(e.workspace.tabs.is_empty());
    }
    #[test]
    fn close_accepts_constrained_geometry_and_restores_minimized_state() {
        let mut e = fixture();
        e.area.x += 200.;
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        e.live.get_mut(&1).unwrap().original.minimized = true;
        e.backend.minimum_width = Some(1200.);
        e.restore_all().unwrap();
        assert!(e.live.is_empty());
        assert!(e.released.is_empty());
        assert!(e.workspace.tabs.is_empty());
        assert_eq!(e.restoration_notes.len(), 2);
        assert!(e.backend.states[&1].minimized);
        assert_eq!(e.backend.states[&1].frame.width, 1200.);
    }
    #[test]
    fn release_accepts_constrained_geometry_but_rollback_remains_strict() {
        let mut e = fixture();
        let original = e.backend.states[&1];
        e.area.x += 200.;
        e.switch(1).unwrap();
        e.backend.minimum_width = Some(1200.);
        assert!(e.backend.restore(1, original).is_err());
        e.release(1).unwrap();
        assert!(!e.live.contains_key(&1));
        assert!(!e.released.contains_key(&1));
        assert_eq!(e.restoration_notes.len(), 1);
    }
    #[test]
    fn actual_window_control_failure_keeps_retry_snapshot() {
        let mut e = fixture();
        e.area.x += 200.;
        e.switch(1).unwrap();
        e.backend.fail_resize = true;
        assert!(e.restore_all().is_err());
        assert!(e.live.contains_key(&1));
        assert!(e.workspace.tabs.iter().any(|t| t.id == 1));
        assert!(e.restoration_notes.is_empty());
        e.backend.fail_resize = false;
        e.restore_all().unwrap();
        assert!(e.workspace.tabs.is_empty());
    }
    #[test]
    fn unchanged_window_needs_no_restore_writes() {
        let mut e = fixture();
        e.backend.fail_resize = true;
        e.restore_all().unwrap();
        assert_eq!(e.backend.resizes, 0);
        assert!(e.backend.minimize_calls.is_empty());
    }
    #[test]
    fn stale_switch_does_not_touch_windows() {
        let mut e = fixture();
        e.switch(1).unwrap();
        let before = e.backend.states.clone();
        e.switch_current(2, || false).unwrap();
        assert_eq!(e.selected, Some(1));
        assert_eq!(e.backend.states, before);
    }
    #[test]
    fn superseded_preparation_rolls_back_without_hiding_previous() {
        let mut e = fixture();
        e.switch(1).unwrap();
        let before = e.backend.states.clone();
        let calls = std::cell::Cell::new(0);
        assert!(
            e.switch_current(2, || {
                let n = calls.get();
                calls.set(n + 1);
                n == 0
            })
            .is_err()
        );
        assert_eq!(e.selected, Some(1));
        assert_eq!(e.backend.states, before);
    }
    #[test]
    fn failed_release_keeps_original_snapshot_for_retry() {
        let mut e = fixture();
        e.live.get_mut(&1).unwrap().original.minimized = true;
        e.backend.fail_minimize = true;
        assert!(e.release(1).is_err());
        assert!(!e.live.contains_key(&1));
        assert!(!e.workspace.tabs.iter().any(|t| t.id == 1));
        assert!(e.released.contains_key(&1));
        e.backend.fail_minimize = false;
        e.retry_released().unwrap();
        assert!(e.backend.states[&1].minimized);
    }
    #[test]
    fn permission_loss_pauses_and_keeps_attachments() {
        let mut e = fixture();
        e.backend.events.push(BackendEvent::PermissionLost);
        e.observe();
        assert!(e.paused.is_some());
        assert_eq!(e.live.len(), 2);
        assert!(e.switch(1).is_err());
    }
    #[test]
    fn fullscreen_never_gets_silently_restored_or_switched() {
        let mut e = fixture();
        e.backend.states.get_mut(&1).unwrap().fullscreen = true;
        assert!(e.switch(1).is_err());
        assert!(e.release(1).is_err());
        assert!(!e.live.contains_key(&1));
        assert!(e.released.contains_key(&1));
    }
    #[test]
    fn resize_defers_manual_move_correction_to_the_observer() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.backend.states.get_mut(&1).unwrap().frame.x = 777.;
        e.resize(Rect {
            x: 500.,
            ..Rect::default()
        })
        .unwrap();
        assert_eq!(e.backend.states[&1].frame.x, 777.);
        assert!(e.paused.is_none());
        e.backend.events.push(BackendEvent::Changed(1));
        e.observe();
        assert_eq!(e.backend.states[&1].frame, e.area);
    }
    #[test]
    fn rediscovered_alias_cannot_attach_a_second_time() {
        let mut e = fixture();
        e.backend.aliases.insert(99, 1);
        let w = WindowInfo {
            id: 99,
            pid: 10,
            app: "same".into(),
            title: "new title".into(),
            identity: e.workspace.tabs[0].identity.clone(),
            eligible: true,
            minimized: false,
        };
        assert!(e.attach(None, &w).is_err());
        assert_eq!(e.workspace.tabs.len(), 2);
    }
    #[test]
    fn disconnected_app_requires_replace_instead_of_duplicate_add() {
        let mut e = fixture();
        e.live.remove(&1);
        let w = WindowInfo {
            id: 1,
            pid: 10,
            app: "same".into(),
            title: "new title".into(),
            identity: e.workspace.tabs[0].identity.clone(),
            eligible: true,
            minimized: false,
        };
        assert!(e.attach(None, &w).is_err());
        e.attach(Some(1), &w).unwrap();
        assert_eq!(e.workspace.tabs.len(), 2);
    }
    #[test]
    fn released_window_is_not_docked_again_after_restoration_failure() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.live.get_mut(&1).unwrap().original.minimized = true;
        e.backend.fail_minimize = true;
        assert!(e.release(1).is_err());
        assert_eq!(e.selected, None);
        e.backend.states.get_mut(&1).unwrap().frame.x = 900.;
        e.backend.events.push(BackendEvent::Changed(1));
        e.observe();
        assert_eq!(e.backend.states[&1].frame.x, 900.);
        assert!(!e.workspace.tabs.iter().any(|t| t.id == 1));
    }
    #[test]
    fn manager_drag_uses_position_only_and_preserves_actual_size() {
        let mut e = fixture();
        e.switch(1).unwrap();
        let resized = e.backend.resizes;
        for x in 200..220 {
            e.follow_workspace(Rect {
                x: x as f64,
                ..e.area
            })
            .unwrap();
        }
        assert_eq!(e.backend.moves, 20);
        assert_eq!(e.backend.resizes, resized);
        assert_eq!(e.backend.states[&1].frame.x, 219.);
        assert_eq!(e.backend.states[&1].frame.width, Rect::default().width);
    }
    #[test]
    fn a1_failed_release_then_confirmed_close_allows_quit() {
        let mut e = fixture();
        e.area.x += 200.;
        e.switch(1).unwrap();
        e.backend.fail_resize = true;
        assert!(e.release(1).is_err());
        e.backend.events.push(BackendEvent::Closed(1));
        e.observe();
        assert!(e.released.is_empty());
        e.backend.fail_resize = false;
        e.restore_all().unwrap();
    }
    #[test]
    fn a4_resume_moves_all_previously_docked_windows() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        let originals: Vec<_> = (1..=2).map(|id| e.live[&id].original).collect();
        e.paused = Some("test".into());
        e.resize(Rect {
            x: 800.,
            width: 1100.,
            ..e.area
        })
        .unwrap();
        e.backend.states.get_mut(&1).unwrap().minimized = true;
        e.resume().unwrap();
        for id in 1..=2 {
            assert_eq!(e.backend.states[&id].frame, e.area);
            assert!(!e.backend.states[&id].minimized);
            assert_eq!(e.live[&id].original, originals[id as usize - 1]);
        }
        assert_eq!(e.backend.focused, Some(2));
    }
    #[test]
    fn a4_resume_preflights_inactive_dialog_before_any_changes() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        e.paused = Some("test".into());
        e.backend.states.get_mut(&1).unwrap().modal = true;
        let before = e.backend.states.clone();
        assert!(e.resume().is_err());
        assert!(e.paused.is_some());
        assert_eq!(e.backend.states, before);
    }

    #[test]
    fn a1_failed_release_process_exit_and_transient_errors() {
        for kind in [ErrorKind::Communication, ErrorKind::Permission] {
            let mut e = fixture();
            e.area.x += 200.;
            e.switch(1).unwrap();
            e.backend.fail_resize = true;
            assert!(e.release(1).is_err());
            assert!(e.backend.watched.contains(&1));
            let original = e.released[&1].original;
            e.backend.lifecycle_error = Some(BackendError::new(kind, "transient"));
            assert_eq!(e.retry_released().unwrap_err().causes[0].kind, kind);
            assert_eq!(e.released[&1].original, original);
            e.backend.lifecycle_error = None;
            e.backend.states.remove(&1);
            e.backend.fail_resize = false;
            e.restore_all().unwrap();
            assert!(e.released.is_empty());
        }
    }
    #[test]
    fn a4_partial_resume_records_progress_and_can_retry() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        let original = e.live[&1].original;
        e.paused = Some("test".into());
        e.area.x = 900.;
        e.backend.minimum_width = Some(1300.);
        e.backend.fail_resize_window = Some(2);
        assert!(e.resume().is_err());
        assert!(e.paused.is_some());
        assert_eq!(e.live[&1].expected.frame.x, 900.);
        assert_eq!(e.live[&1].expected.frame.width, 1300.);
        assert_eq!(e.live[&1].original, original);
        e.backend.fail_resize_window = None;
        e.resume().unwrap();
        assert!(e.paused.is_none());
        assert!(
            e.live
                .values()
                .all(|a| a.expected.frame.x == 900. && a.expected.frame.width == 1300.)
        );
    }
    #[test]
    fn a4_no_selection_and_never_docked_attachments() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.selected = None;
        let untouched = e.backend.states[&2];
        e.paused = Some("test".into());
        e.area.x = -700.;
        e.resume().unwrap();
        assert_eq!(e.backend.states[&1].frame.x, -700.);
        assert_eq!(e.backend.states[&2], untouched);
        e.backend.states.get_mut(&2).unwrap().frame.x = 777.;
        e.backend.events.push(BackendEvent::Changed(2));
        e.observe();
        assert_eq!(e.backend.states[&2].frame.x, 777.);
    }
    #[test]
    fn a4_permission_and_fullscreen_failures_keep_global_pause() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.backend.permission_lost = true;
        assert_eq!(e.resume().unwrap_err().kind, ErrorKind::Permission);
        assert!(e.paused.is_some());
        e.backend.permission_lost = false;
        e.backend.states.get_mut(&1).unwrap().fullscreen = true;
        assert!(e.resume().is_err());
        assert!(e.paused.is_some());
    }
    #[test]
    fn d2_minimizing_one_window_pauses_all_docking_until_full_resume() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        e.backend.states.get_mut(&1).unwrap().minimized = true;
        e.backend.events.push(BackendEvent::Changed(1));
        e.observe();
        assert!(e.paused.is_some());
        let before = e.backend.states.clone();
        e.follow_workspace(Rect { x: 999., ..e.area }).unwrap();
        assert_eq!(e.backend.states, before);
        e.resume().unwrap();
        assert!(
            e.backend
                .states
                .values()
                .all(|s| s.frame == e.area && !s.minimized)
        );
    }
    #[test]
    fn a1_replacement_handle_restores_original_detached_snapshot() {
        let mut e = fixture();
        let original = e.live[&1].original;
        e.area.x = 500.;
        e.switch(1).unwrap();
        e.backend.fail_resize = true;
        assert!(e.release(1).is_err());
        let replacement = e.backend.states.remove(&1).unwrap();
        e.backend.states.insert(99, replacement);
        e.backend.aliases.insert(1, 99);
        e.backend.fail_resize = false;
        e.retry_released().unwrap();
        assert_eq!(e.backend.states[&99], original);
        assert!(!e.live.contains_key(&1));
        assert!(e.released.is_empty());
    }
    #[test]
    fn d3_unsettled_release_retains_snapshot_until_successful_retry() {
        let mut e = fixture();
        let original = e.live[&1].original;
        e.area.x = 500.;
        e.switch(1).unwrap();
        e.backend.resize_error = Some(BackendError::new(
            ErrorKind::UnsettledGeometry,
            "scripted oscillation",
        ));
        assert_eq!(e.release(1).unwrap_err().kind, ErrorKind::UnsettledGeometry);
        assert_eq!(e.released[&1].original, original);
        e.backend.resize_error = None;
        e.retry_released().unwrap();
        assert!(e.released.is_empty());
        assert_eq!(e.backend.states[&1], original);
    }
    #[test]
    fn minimized_attach_revalidates_before_movement_and_rolls_back_on_failure() {
        let mut e = fixture();
        e.switch(1).unwrap();
        let before = WindowState {
            minimized: true,
            ..e.backend.states[&2]
        };
        e.backend.states.insert(2, before);
        e.live.get_mut(&2).unwrap().original = before;
        e.backend.fail_restored_validation = true;
        assert!(e.switch(2).is_err());
        assert_eq!(e.selected, Some(1));
        assert_eq!(e.backend.states[&2], before);
        assert!(!e.live[&2].docked);
        assert_eq!(e.live[&2].original, before);
        e.backend.fail_restored_validation = false;
        e.switch(2).unwrap();
        assert!(!e.backend.states[&2].minimized);
        e.release(2).unwrap();
        assert_eq!(e.backend.states[&2], before);
    }
    #[test]
    fn save_and_reset_startup_choices_do_not_change_live_windows_or_other_preferences() {
        let mut e = fixture();
        e.workspace.keep_apps_open_on_close = true;
        e.switch(1).unwrap();
        e.workspace.startup_apps.push(StartupApp {
            bundle: "old.app".into(),
            name: "Old".into(),
        });
        let states = e.backend.states.clone();
        let geometry = e.workspace.geometry;
        let selected = e.selected;
        let shortcuts = (
            e.workspace.next_shortcut.clone(),
            e.workspace.previous_shortcut.clone(),
        );
        e.save_startup_apps(&[]);
        assert_eq!(
            e.workspace.startup_apps,
            vec![StartupApp {
                bundle: "same".into(),
                name: "same".into()
            }]
        );
        e.reset_startup_apps();
        assert!(e.workspace.startup_apps.is_empty());
        assert!(e.workspace.keep_apps_open_on_close);
        assert_eq!(e.backend.states, states);
        assert_eq!(e.live.len(), 2);
        assert_eq!(e.workspace.tabs.len(), 2);
        assert_eq!(e.selected, selected);
        assert_eq!(e.workspace.geometry, geometry);
        assert_eq!(
            (
                e.workspace.next_shortcut.clone(),
                e.workspace.previous_shortcut.clone()
            ),
            shortcuts
        );
    }
    #[test]
    fn quiet_startup_registers_all_tabs_but_only_restores_first_selected_window() {
        let mut e = fixture();
        e.live.clear();
        e.workspace.tabs.clear();
        for state in e.backend.states.values_mut() {
            state.minimized = true;
        }
        for id in 1..=2 {
            let window = WindowInfo {
                id,
                pid: 10,
                app: format!("app{id}"),
                title: "Fixture".into(),
                identity: Identity {
                    bundle: format!("app{id}"),
                    identifier: None,
                },
                eligible: true,
                minimized: true,
            };
            e.attach_startup(&window, || true).unwrap();
        }
        assert_eq!(e.live.len(), 2);
        assert_eq!(e.selected, Some(1));
        assert_eq!(e.backend.minimize_calls, vec![(1, false)]);
        assert!(e.live[&1].docked);
        assert!(!e.live[&2].docked);
        assert!(e.backend.states[&2].minimized);
        e.switch(2).unwrap();
        assert_eq!(e.backend.minimize_calls, vec![(1, false), (2, false)]);
        assert!(e.live.values().all(|a| a.original.minimized));
    }
    #[test]
    fn keep_open_on_close_preserves_visibility_but_restores_geometry() {
        let mut e = fixture();
        for a in e.live.values_mut() {
            a.original.minimized = true;
        }
        e.area.x += 300.;
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        e.workspace.keep_apps_open_on_close = true;
        let calls = e.backend.minimize_calls.len();
        e.restore_all().unwrap();
        assert_eq!(e.backend.minimize_calls.len(), calls);
        assert!(
            e.backend
                .states
                .values()
                .all(|s| !s.minimized && s.frame == Rect::default())
        );
    }
    #[test]
    fn keep_open_policy_does_not_change_explicit_release_or_original_retry_snapshots() {
        let mut e = fixture();
        e.live.get_mut(&1).unwrap().original.minimized = true;
        e.area.x += 300.;
        e.switch(1).unwrap();
        e.workspace.keep_apps_open_on_close = true;
        e.backend.fail_resize = true;
        assert!(e.restore_all().is_err());
        assert!(e.live[&1].original.minimized);
        e.backend.fail_resize = false;
        e.release(1).unwrap();
        assert!(e.backend.states[&1].minimized);
    }
    #[test]
    fn visibility_only_restoration_does_not_rewrite_geometry() {
        let mut e = fixture();
        e.live.get_mut(&1).unwrap().original.minimized = true;
        e.release(1).unwrap();
        assert_eq!(e.backend.resizes, 0);
        assert_eq!(e.backend.minimize_calls, vec![(1, true)]);
    }
    #[test]
    fn never_selected_startup_tabs_are_left_untouched_on_release_and_close() {
        for release in [false, true] {
            let mut e = fixture();
            e.live.clear();
            e.workspace.tabs.clear();
            for state in e.backend.states.values_mut() {
                state.minimized = true;
            }
            for id in 1..=2 {
                let window = WindowInfo {
                    id,
                    pid: 10,
                    app: format!("app{id}"),
                    title: "Fixture".into(),
                    identity: Identity {
                        bundle: format!("app{id}"),
                        identifier: None,
                    },
                    eligible: true,
                    minimized: true,
                };
                e.attach_startup(&window, || true).unwrap();
            }
            let external = WindowState {
                frame: Rect {
                    x: 999.,
                    ..Rect::default()
                },
                minimized: false,
                fullscreen: false,
                modal: false,
            };
            e.backend.states.insert(2, external);
            if release {
                e.release(2).unwrap();
            }
            e.restore_all().unwrap();
            assert_eq!(e.backend.states[&2], external);
            assert!(!e.backend.minimize_calls.iter().any(|(id, _)| *id == 2));
        }
    }
}
