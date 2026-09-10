use crate::model::*;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Attachment {
    pub window: WindowId,
    pub original: WindowState,
    pub expected: WindowState,
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
        }
    }
    pub fn attach(&mut self, tab: Option<TabId>, window: &WindowInfo) -> Result<TabId> {
        if !self.backend.trusted() {
            return Err("Enable Accessibility in System Settings, then Resume.".into());
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
                name: format!("{} — {}", window.app, window.title),
                identity: window.identity.clone(),
            });
        }
        // A new explicit attachment supersedes any old detached recovery for this window.
        self.released
            .retain(|_, a| !self.backend.same_window(a.window, window.id));
        self.live.insert(
            id,
            Attachment {
                window: window.id,
                original,
                expected: original,
            },
        );
        self.backend.watch(window.id, true);
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
            return Err("Enable Accessibility, then Resume.".into());
        }
        if let Some(reason) = &self.paused {
            return Err(format!("Docking paused: {reason}. Select Resume."));
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
        // Prepare the next window before hiding the previous one. Roll back on failure.
        let prepared: Result<Rect> = (|| {
            self.backend.minimize(next.window, false)?;
            let frame = self.backend.set_frame(next.window, self.area)?;
            if !current() {
                return Err("Superseded by a newer selection".into());
            }
            self.backend.focus(next.window)?;
            Ok(frame)
        })();
        let frame = match prepared {
            Ok(f) => f,
            Err(e) => {
                let rollback = self.backend.restore(next.window, before);
                return Err(format!("Switch failed: {e}; rollback: {rollback:?}"));
            }
        };
        if !current() {
            self.backend.restore(next.window, before)?;
            return Ok(());
        }
        if let Some(old) = self
            .selected
            .filter(|old| *old != id)
            .and_then(|old| self.live.get(&old))
            .cloned()
        {
            let old_state = match self.backend.state(old.window) {
                Ok(s) => s,
                Err(e) => {
                    let rollback = self.backend.restore(next.window, before);
                    return Err(format!(
                        "Previous window unavailable: {e}; rollback: {rollback:?}"
                    ));
                }
            };
            if old_state.modal || old_state.fullscreen {
                let _ = self.backend.restore(next.window, before);
                self.paused = Some("Previous window has a dialog or is fullscreen".into());
                return Err("Previous window cannot be hidden safely.".into());
            }
            if let Err(e) = self.backend.minimize(old.window, true) {
                let rollback = self.backend.restore(next.window, before);
                let _ = self.backend.focus(old.window);
                return Err(format!(
                    "Could not hide previous window: {e}; rollback: {rollback:?}"
                ));
            }
            if let Some(a) = self.live.values_mut().find(|a| a.window == old.window) {
                a.expected.minimized = true;
            }
        }
        self.selected = Some(id);
        if let Some(a) = self.live.get_mut(&id) {
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
        if let Some(a) = self.selected.and_then(|id| self.live.get_mut(&id)) {
            let state = self.backend.state(a.window)?;
            if state.fullscreen || state.modal {
                self.paused = Some("Fullscreen or dialog is active".into());
                return Ok(());
            }
            if !state.frame.near(a.expected.frame) || state.minimized != a.expected.minimized {
                // The observer restores the target after the user releases the mouse.
                return Ok(());
            }
            a.expected.frame = self.backend.set_frame(a.window, area)?;
        }
        Ok(())
    }
    /// Detaching is unconditional. Failed restoration must never re-dock a released window.
    pub fn detach(&mut self, id: TabId) {
        if let Some(a) = self.live.remove(&id) {
            self.backend.watch(a.window, false);
            self.released.insert(id, a);
        }
        self.workspace.tabs.retain(|t| t.id != id);
        if self.selected == Some(id) {
            self.selected = None;
        }
    }
    pub fn restore_released(&mut self, id: TabId) -> Result<()> {
        if let Some(a) = self.released.get(&id) {
            let result = self.backend.restore(a.window, a.original);
            self.backend.watch(a.window, false);
            result.map_err(|e|format!("Window released; original state could not be restored: {e}. Use AppDock → Retry restoration."))?;
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
            Err(errors.join("\n"))
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
        if let Some(a) = self.selected.and_then(|id| self.live.get_mut(&id)) {
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
        for (&id, a) in &self.live {
            if let Err(e) = self.backend.restore(a.window, a.original) {
                errors.push(format!("Tab {id}: {e}"));
            } else {
                self.backend.watch(a.window, false);
                restored.push(id);
            }
        }
        for id in restored {
            self.live.remove(&id);
            if self.selected == Some(id) {
                self.selected = None;
            }
        }
        if errors.is_empty() {
            self.live.clear();
            self.selected = None;
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
    pub fn observe(&mut self) {
        for event in self.backend.events() {
            match event {
                BackendEvent::PermissionLost => {
                    self.paused = Some("Accessibility permission was revoked".into())
                }
                BackendEvent::Closed(w) => {
                    let ids: Vec<_> = self
                        .live
                        .iter()
                        .filter(|(_, a)| a.window == w)
                        .map(|(id, _)| *id)
                        .collect();
                    for id in ids {
                        self.live.remove(&id);
                        if self.selected == Some(id) {
                            self.selected = None;
                        }
                    }
                }
                BackendEvent::Changed(w) => {
                    if let Some((&id, a)) = self.live.iter().find(|(_, a)| a.window == w) {
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
                                let target = if self.selected == Some(id) {
                                    self.area
                                } else {
                                    expected.frame
                                };
                                match self.backend.set_frame(w, target) {
                                    Ok(frame) => {
                                        self.live.get_mut(&id).unwrap().expected.frame = frame
                                    }
                                    Err(e) => self.paused = Some(e),
                                }
                            }
                            Err(e) if !self.pointer_down => self.paused = Some(e),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    pub fn resume(&mut self) -> Result<()> {
        if !self.backend.trusted() {
            return Err("Accessibility permission is still unavailable.".into());
        }
        for a in self.live.values_mut() {
            a.expected = self.backend.state(a.window)?;
        }
        self.paused = None;
        if let Some(id) = self.selected {
            self.switch(id)?;
        }
        Ok(())
    }
    pub fn reconnect(&mut self, windows: &[WindowInfo]) {
        let saved = self.workspace.tabs.clone();
        for tab in &saved {
            if saved
                .iter()
                .filter(|other| other.identity == tab.identity)
                .count()
                == 1
                && !self.live.contains_key(&tab.id)
                && let Some(id) = reconnect(tab, windows)
                && let Some(w) = windows.iter().find(|w| w.id == id)
            {
                let _ = self.attach(Some(tab.id), w);
            }
        }
        // Reconnection binds handles only; the user explicitly selects a tab to dock it.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        states: HashMap<WindowId, WindowState>,
        aliases: HashMap<WindowId, WindowId>,
        moves: usize,
        resizes: usize,
        fail_focus: bool,
        fail_minimize: bool,
        events: Vec<BackendEvent>,
    }
    impl WindowBackend for Fake {
        fn same_window(&self, a: WindowId, b: WindowId) -> bool {
            self.aliases.get(&a).copied().unwrap_or(a) == self.aliases.get(&b).copied().unwrap_or(b)
        }
        fn trusted(&self) -> bool {
            true
        }
        fn discover(&mut self) -> Result<Vec<WindowInfo>> {
            Ok(vec![])
        }
        fn state(&self, id: WindowId) -> Result<WindowState> {
            self.states.get(&id).copied().ok_or("closed".into())
        }
        fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> Result<()> {
            self.moves += 1;
            let state = self.states.get_mut(&id).unwrap();
            state.frame.x = x;
            state.frame.y = y;
            Ok(())
        }
        fn set_frame(&mut self, id: WindowId, f: Rect) -> Result<Rect> {
            self.resizes += 1;
            self.states.get_mut(&id).unwrap().frame = f;
            Ok(f)
        }
        fn minimize(&mut self, id: WindowId, v: bool) -> Result<()> {
            if v && self.fail_minimize {
                return Err("denied".into());
            }
            self.states.get_mut(&id).unwrap().minimized = v;
            Ok(())
        }
        fn focus(&mut self, _: WindowId) -> Result<()> {
            if self.fail_focus {
                Err("timeout".into())
            } else {
                Ok(())
            }
        }
        fn events(&mut self) -> Vec<BackendEvent> {
            std::mem::take(&mut self.events)
        }
    }
    fn fixture() -> Engine<Fake> {
        let mut e = Engine::new(Fake::default(), Workspace::default());
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
                },
            )
            .unwrap();
        }
        e
    }
    #[test]
    fn switching_duplicate_titles_hides_only_previous_handle() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.switch(2).unwrap();
        assert!(e.backend.states[&1].minimized);
        assert!(!e.backend.states[&2].minimized);
        assert_eq!(e.selected, Some(2));
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
    fn minimize_failure_rolls_back_next_geometry() {
        let mut e = fixture();
        e.switch(1).unwrap();
        let before = e.backend.states[&2];
        e.area.x = 400.;
        e.backend.fail_minimize = true;
        assert!(e.switch(2).is_err());
        assert_eq!(e.backend.states[&2], before);
        assert_eq!(e.selected, Some(1));
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
    fn closed_target_keeps_disconnected_tab() {
        let mut e = fixture();
        e.switch(1).unwrap();
        e.backend.events.push(BackendEvent::Closed(1));
        e.observe();
        assert!(!e.live.contains_key(&1));
        assert_eq!(e.workspace.tabs.len(), 2);
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
    fn duplicate_saved_identities_require_explicit_selection() {
        let mut e = fixture();
        e.live.clear();
        for t in &mut e.workspace.tabs {
            t.identity.identifier = Some("same-id".into());
        }
        let w = WindowInfo {
            id: 1,
            pid: 10,
            app: "same".into(),
            title: "same".into(),
            identity: e.workspace.tabs[0].identity.clone(),
            eligible: true,
        };
        e.reconnect(&[w]);
        assert!(e.live.is_empty());
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
}
