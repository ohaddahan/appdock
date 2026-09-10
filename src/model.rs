use serde::{Deserialize, Serialize};

pub type WindowId = u64;
pub type TabId = u64;
pub use crate::error::{BackendError, ErrorKind};
pub type Result<T> = std::result::Result<T, BackendError>;

/// Compact presentation of an app-wide Dock badge. Unknown text is a dot,
/// never a guessed unread count. Empty labels and an explicit zero are clear.
pub fn badge_text(label: &str) -> Option<String> {
    if label.is_empty() {
        return None;
    }
    let label = label.trim();
    let more = label.ends_with('+');
    match label.strip_suffix('+').unwrap_or(label).parse::<u64>() {
        Ok(0) if !more => None,
        Ok(n) if n > 99 => Some("99+".into()),
        Ok(n) if more => Some(format!("{n}+")),
        Ok(n) => Some(n.to_string()),
        Err(_) => Some("•".into()),
    }
}

/// Global desktop points, with the origin at the primary display's top left (AX).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Rect {
    pub fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width >= 100.0
            && self.height >= 100.0
    }
    pub fn near(self, other: Self) -> bool {
        (self.x - other.x).abs() < 3.0
            && (self.y - other.y).abs() < 3.0
            && (self.width - other.width).abs() < 3.0
            && (self.height - other.height).abs() < 3.0
    }
    pub fn from_cocoa(x: f64, y: f64, width: f64, height: f64, primary_height: f64) -> Self {
        Self {
            x,
            y: primary_height - y - height,
            width,
            height,
        }
    }
}
impl Default for Rect {
    fn default() -> Self {
        Self {
            x: 120.0,
            y: 120.0,
            width: 1000.0,
            height: 740.0,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub bundle: String,
    pub identifier: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedTab {
    pub id: TabId,
    pub name: String,
    pub identity: Identity,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupApp {
    pub bundle: String,
    pub name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub version: u32,
    pub geometry: Rect,
    pub tabs: Vec<SavedTab>,
    pub next_shortcut: String,
    pub previous_shortcut: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub startup_apps: Vec<StartupApp>,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: 1,
            geometry: Rect::default(),
            tabs: vec![],
            next_shortcut: "Control+Alt+Super+ArrowRight".into(),
            previous_shortcut: "Control+Alt+Super+ArrowLeft".into(),
            startup_apps: vec![],
        }
    }
}
impl Workspace {
    pub fn set_startup_app(&mut self, app: StartupApp, enabled: bool) {
        self.startup_apps.retain(|rule| rule.bundle != app.bundle);
        if enabled && !app.bundle.trim().is_empty() {
            self.startup_apps.push(app);
        }
    }
}
#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub id: WindowId,
    pub pid: i32,
    pub app: String,
    pub title: String,
    pub identity: Identity,
    pub eligible: bool,
    pub minimized: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowState {
    pub frame: Rect,
    pub minimized: bool,
    pub fullscreen: bool,
    pub modal: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Restoration {
    Exact,
    Adjusted { requested: Rect, actual: Rect },
}
#[derive(Clone, Debug)]
pub enum BackendEvent {
    Closed(WindowId),
    Changed(WindowId),
    PermissionLost,
}
/// Implementations own live handles. Saved identities are never live handles.
pub trait WindowBackend {
    fn trusted(&self) -> bool;
    fn same_window(&self, a: WindowId, b: WindowId) -> bool {
        a == b
    }
    fn discover(&mut self) -> Result<Vec<WindowInfo>>;
    /// Positive closure requires lifecycle evidence; a failed state read is insufficient.
    fn check_window(&mut self, id: WindowId) -> Result<()> {
        self.state(id).map(|_| ())
    }
    fn state(&self, id: WindowId) -> Result<WindowState>;
    fn set_frame(&mut self, id: WindowId, frame: Rect) -> Result<Rect>;
    fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> Result<()> {
        let old = self.state(id)?.frame;
        self.set_frame(id, Rect { x, y, ..old }).map(|_| ())
    }
    fn minimize(&mut self, id: WindowId, value: bool) -> Result<()>;
    fn focus(&mut self, id: WindowId) -> Result<()>;
    /// Minimized windows can temporarily omit normal docking capabilities.
    /// Validate after restoring, before moving or focusing the target.
    fn validate_restored_window(&self, id: WindowId) -> Result<()> {
        let state = self.state(id)?;
        if state.minimized || state.fullscreen || state.modal {
            return Err("Window cannot be docked after restoring".into());
        }
        Ok(())
    }
    fn events(&mut self) -> Vec<BackendEvent>;
    fn watch(&mut self, _id: WindowId, _enabled: bool) {}
    fn restore(&mut self, id: WindowId, state: WindowState) -> Result<()> {
        match self.restore_for_release(id, state)? {
            Restoration::Exact => Ok(()),
            Restoration::Adjusted { requested, actual } => Err(format!(
                "Window geometry differs after rollback: requested {requested:?}, actual {actual:?}"
            )
            .into()),
        }
    }
    /// Release accepts the frame the app/desktop permits. An exact-coordinate
    /// mismatch must not trap the manager in a retry-close loop.
    fn restore_for_release(&mut self, id: WindowId, state: WindowState) -> Result<Restoration> {
        let current = self.state(id)?;
        if current.fullscreen || current.modal {
            return Err("Leave fullscreen and close dialogs before restoring this window".into());
        }
        if current.frame.near(state.frame) && current.minimized == state.minimized {
            return Ok(Restoration::Exact);
        }
        if current.minimized {
            self.minimize(id, false)?;
        }
        self.set_frame(id, state.frame)?;
        if state.minimized {
            self.minimize(id, true)?;
        }
        let actual = self.state(id)?;
        if actual.minimized != state.minimized
            || actual.fullscreen
            || actual.modal
            || !actual.frame.valid()
        {
            return Err(
                "Window did not accept its original state; retry after resolving the interruption"
                    .into(),
            );
        }
        Ok(if actual.frame.near(state.frame) {
            Restoration::Exact
        } else {
            Restoration::Adjusted {
                requested: state.frame,
                actual: actual.frame,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dock_badges_keep_counts_separate_from_unknown_status() {
        assert_eq!(badge_text("7").as_deref(), Some("7"));
        assert_eq!(badge_text("100").as_deref(), Some("99+"));
        assert_eq!(badge_text("99+").as_deref(), Some("99+"));
        assert_eq!(badge_text("7+").as_deref(), Some("7+"));
        assert_eq!(badge_text("•").as_deref(), Some("•"));
        assert_eq!(badge_text("attention").as_deref(), Some("•"));
        assert_eq!(badge_text(" ").as_deref(), Some("•"));
        assert_eq!(badge_text("0"), None);
        assert_eq!(badge_text(""), None);
    }
    #[test]
    fn cocoa_conversion_handles_displays_above_and_left() {
        assert_eq!(
            Rect::from_cocoa(-800.0, 1000.0, 700.0, 600.0, 900.0),
            Rect {
                x: -800.0,
                y: -700.0,
                width: 700.0,
                height: 600.0
            }
        );
    }
}

/// UI selection includes disconnected saved tabs; backend selection only has a live window.
pub fn action_tab(
    editing: Option<TabId>,
    selected: Option<TabId>,
    tabs: &[SavedTab],
) -> Option<TabId> {
    editing
        .filter(|id| tabs.iter().any(|t| t.id == *id))
        .or_else(|| selected.filter(|id| tabs.iter().any(|t| t.id == *id)))
        .or_else(|| tabs.first().map(|t| t.id))
}

#[cfg(test)]
mod action_tests {
    use super::*;
    fn tab(id: TabId) -> SavedTab {
        SavedTab {
            id,
            name: "Disconnected".into(),
            identity: Identity {
                bundle: "fixture".into(),
                identifier: None,
            },
        }
    }
    #[test]
    fn disconnected_only_tab_can_be_released_without_activation() {
        assert_eq!(action_tab(None, None, &[tab(7)]), Some(7));
    }
    #[test]
    fn stale_selection_falls_back_to_an_existing_tab() {
        assert_eq!(action_tab(Some(9), None, &[tab(7)]), Some(7));
    }
    #[test]
    fn explicitly_selected_disconnected_tab_wins_over_live_selection() {
        assert_eq!(action_tab(Some(7), Some(8), &[tab(7), tab(8)]), Some(7));
    }
}

/// Explicit disconnected action selection survives live selection updates. Live
/// action targets follow confirmed activation; a rename editor owns its own ID.
pub fn selection_for_actions(
    action: Option<TabId>,
    selected: Option<TabId>,
    tabs: &[SavedTab],
    live: &[(TabId, WindowId)],
) -> Option<TabId> {
    let disconnected = action.filter(|id| !live.iter().any(|(tab, _)| tab == id));
    action_tab(disconnected, selected, tabs)
}

/// Navigate from intent until acknowledged. Resolve IDs against the current tab
/// order so removals/reorders cannot turn an old index into a different target.
pub fn navigation_target(
    tabs: &[SavedTab],
    requested: Option<TabId>,
    selected: Option<TabId>,
    previous: bool,
) -> Option<TabId> {
    if tabs.is_empty() {
        return None;
    }
    let index_of = |id| tabs.iter().position(|t| Some(t.id) == id);
    let index = index_of(requested).or_else(|| index_of(selected));
    let next = match index {
        None => {
            if previous {
                tabs.len() - 1
            } else {
                0
            }
        }
        Some(i) if previous => (i + tabs.len() - 1) % tabs.len(),
        Some(i) => (i + 1) % tabs.len(),
    };
    Some(tabs[next].id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabIndication {
    Active,
    Action,
    Inactive,
}
pub fn tab_indication(
    id: TabId,
    connected: bool,
    selected: Option<TabId>,
    action: Option<TabId>,
) -> TabIndication {
    if connected && selected == Some(id) {
        TabIndication::Active
    } else if !connected && action == Some(id) {
        TabIndication::Action
    } else {
        TabIndication::Inactive
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    fn tabs(ids: &[TabId]) -> Vec<SavedTab> {
        ids.iter()
            .map(|id| SavedTab {
                id: *id,
                name: "fixture".into(),
                identity: Identity {
                    bundle: "fixture".into(),
                    identifier: None,
                },
            })
            .collect()
    }
    #[test]
    fn a2_attach_replace_failed_switch_and_direct_activation_indications() {
        for (selected, active) in [(Some(2), 2), (Some(1), 1), (Some(3), 3)] {
            for id in 1..=3 {
                assert_eq!(
                    tab_indication(id, true, selected, Some(1)),
                    if id == active {
                        TabIndication::Active
                    } else {
                        TabIndication::Inactive
                    }
                );
            }
        }
        assert_eq!(
            tab_indication(1, false, Some(2), Some(1)),
            TabIndication::Action
        );
        assert_eq!(
            tab_indication(2, true, Some(2), Some(1)),
            TabIndication::Active
        );
        assert_eq!(action_tab(Some(1), Some(2), &tabs(&[2])), Some(2));
    }
    #[test]
    fn a6_reordered_removed_empty_single_and_invalid_requests() {
        assert_eq!(
            navigation_target(&tabs(&[3, 1, 2]), Some(1), Some(3), false),
            Some(2)
        );
        assert_eq!(
            navigation_target(&tabs(&[3, 2]), Some(1), Some(3), false),
            Some(2)
        );
        assert_eq!(navigation_target(&[], Some(1), Some(3), false), None);
        assert_eq!(navigation_target(&tabs(&[1]), Some(1), None, true), Some(1));
        assert_eq!(
            navigation_target(&tabs(&[2, 3]), Some(9), Some(8), false),
            Some(2)
        );
        assert_eq!(navigation_target(&tabs(&[2, 3]), None, None, true), Some(3));
    }
    #[test]
    fn a2_actions_follow_live_activation_and_preserve_disconnected_target() {
        let tabs = tabs(&[1, 2, 3]);
        assert_eq!(
            selection_for_actions(Some(1), Some(2), &tabs, &[(1, 10), (2, 20)]),
            Some(2)
        );
        assert_eq!(
            selection_for_actions(Some(3), Some(2), &tabs, &[(1, 10), (2, 20)]),
            Some(3)
        );
        assert_eq!(
            selection_for_actions(Some(2), Some(1), &tabs, &[(1, 10), (2, 20)]),
            Some(1)
        );
    }
}

/// AppKit can report ordinary minimized windows as AXDialog. Only admit that
/// transient subrole when actually minimized; revalidate after restoring.
pub fn discoverable_window(role: Option<&str>, subrole: Option<&str>, minimized: bool) -> bool {
    role == Some("AXWindow")
        && (subrole == Some("AXStandardWindow") || (minimized && subrole == Some("AXDialog")))
}
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowCapabilities {
    pub frontmost: bool,
    pub minimize: bool,
    pub position: bool,
    pub size: bool,
    pub main: bool,
    pub raise: bool,
}
impl WindowCapabilities {
    pub fn can_attach(self, state: WindowState) -> bool {
        state.frame.valid()
            && !state.fullscreen
            && !state.modal
            && self.frontmost
            && self.minimize
            && (state.minimized || (self.position && self.size && self.main && self.raise))
    }
}
#[cfg(test)]
mod minimized_tests {
    use super::*;
    #[test]
    fn minimized_standard_windows_can_have_a_dialog_subrole_and_deferred_controls() {
        let state = WindowState {
            frame: Rect::default(),
            minimized: true,
            fullscreen: false,
            modal: false,
        };
        let caps = WindowCapabilities {
            frontmost: true,
            minimize: true,
            ..Default::default()
        };
        assert!(discoverable_window(
            Some("AXWindow"),
            Some("AXDialog"),
            true
        ));
        assert!(caps.can_attach(state));
        assert!(!caps.can_attach(WindowState {
            minimized: false,
            ..state
        }));
        assert!(!caps.can_attach(WindowState {
            modal: true,
            ..state
        }));
        assert!(!caps.can_attach(WindowState {
            fullscreen: true,
            ..state
        }));
        assert!(!discoverable_window(
            Some("AXWindow"),
            Some("AXDialog"),
            false
        ));
        assert!(!discoverable_window(
            Some("AXSheet"),
            Some("AXDialog"),
            true
        ));
    }
}
