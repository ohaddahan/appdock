//! Startup rules are resolved once against a completed discovery. They never
//! reconnect saved handles, launch apps, or reattach windows after release.
use crate::model::*;
use std::collections::VecDeque;

pub struct Startup {
    apps: Option<Vec<StartupApp>>,
    pending: VecDeque<WindowId>,
    notes: Vec<String>,
}
impl Startup {
    pub fn new(apps: Vec<StartupApp>) -> Self {
        Self {
            apps: (!apps.is_empty()).then_some(apps),
            pending: VecDeque::new(),
            notes: vec![],
        }
    }
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn next(&mut self) -> Option<WindowId> {
        self.pending.pop_front()
    }
    pub fn cancel(&mut self) {
        self.apps = None;
        self.pending.clear();
        self.notes.clear();
    }
    pub fn resolve(&mut self, windows: &[WindowInfo], occupied: &[WindowId]) {
        let Some(apps) = self.apps.take() else { return };
        for app in apps {
            let mut unavailable = false;
            for window in windows
                .iter()
                .filter(|w| w.identity.bundle == app.bundle)
                .filter(|w| !occupied.contains(&w.id))
            {
                if !window.eligible {
                    unavailable = true;
                } else if !self.pending.contains(&window.id) {
                    self.pending.push_back(window.id);
                }
            }
            if unavailable {
                self.notes.push(format!(
                    "Some {} windows are not ready for docking. Use Add App when they are available.",
                    app.name
                ));
            }
        }
    }
    pub fn notice(&mut self) -> Option<String> {
        if self.apps.is_none() && self.pending.is_empty() && !self.notes.is_empty() {
            Some(std::mem::take(&mut self.notes).join("\n"))
        } else {
            None
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn rule(bundle: &str) -> StartupApp {
        StartupApp {
            bundle: bundle.into(),
            name: bundle.into(),
        }
    }
    fn window(id: WindowId, bundle: &str) -> WindowInfo {
        WindowInfo {
            id,
            pid: 1,
            app: bundle.into(),
            title: "Duplicate title".into(),
            identity: Identity {
                bundle: bundle.into(),
                identifier: None,
            },
            eligible: true,
            minimized: false,
        }
    }
    #[test]
    fn startup_matches_exact_apps_once_and_includes_minimized_windows() {
        let mut state = Startup::new(vec![rule("wanted"), rule("missing")]);
        let wanted = WindowInfo {
            minimized: true,
            ..window(2, "wanted")
        };
        state.resolve(&[window(1, "unrelated"), wanted.clone()], &[]);
        assert_eq!(state.next(), Some(2));
        assert_eq!(state.next(), None);
        state.resolve(&[wanted], &[]);
        assert_eq!(state.next(), None);
    }
    #[test]
    fn startup_adds_all_eligible_windows_including_minimized_and_skips_only_occupied() {
        let mut state = Startup::new(vec![rule("many"), rule("blocked"), rule("attached")]);
        state.resolve(
            &[
                window(1, "many"),
                WindowInfo {
                    minimized: true,
                    ..window(2, "many")
                },
                WindowInfo {
                    eligible: false,
                    ..window(3, "blocked")
                },
                window(4, "attached"),
                window(5, "attached"),
                window(5, "attached"),
                WindowInfo {
                    eligible: false,
                    ..window(6, "many")
                },
            ],
            &[4],
        );
        assert_eq!(state.next(), Some(1));
        assert_eq!(state.next(), Some(2));
        assert_eq!(state.next(), Some(5));
        assert!(!state.has_pending());
        let notice = state.notice().unwrap();
        assert!(notice.contains("not ready"));
        assert!(notice.contains("many"));
        assert!(notice.contains("blocked"));
        assert!(!notice.contains("attached"));
        assert!(state.notice().is_none());
    }
    #[test]
    fn manual_interaction_cancels_waiting_and_queued_startup_work() {
        for resolve_first in [false, true] {
            let mut state = Startup::new(vec![rule("wanted")]);
            if resolve_first {
                state.resolve(&[window(1, "wanted")], &[]);
            }
            state.cancel();
            state.resolve(&[window(1, "wanted")], &[]);
            assert_eq!(state.next(), None);
            assert!(state.notice().is_none());
        }
    }
}
