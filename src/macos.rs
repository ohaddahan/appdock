//! All Accessibility IPC stays on the worker thread; no AppKit objects cross it.
use crate::model::*;
use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFRetained, CFString, CFType, CFURL, CFURLPathStyle,
};
use objc2_core_foundation::{CGPoint, CGSize};
use std::{
    collections::{HashMap, HashSet},
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone, Debug)]
pub struct App {
    pub pid: i32,
    pub name: String,
    pub bundle: String,
}
struct Entry {
    element: CFRetained<AXUIElement>,
    app: CFRetained<AXUIElement>,
    info: WindowInfo,
    number: Option<u32>,
    closed: bool,
}
pub struct MacBackend {
    apps: Vec<App>,
    entries: HashMap<WindowId, Entry>,
    next: WindowId,
    watched: HashSet<WindowId>,
    pub focus_suspended: Arc<AtomicBool>,
}
fn check(e: AXError) -> Result<()> {
    if e == AXError::Success {
        Ok(())
    } else {
        Err(BackendError {
            ax_code: Some(e.0),
            ..BackendError::new(
                if e == AXError::APIDisabled {
                    ErrorKind::Permission
                } else {
                    ErrorKind::Communication
                },
                "Accessibility request",
            )
        })
    }
}
struct Ax<'a> {
    current: &'a dyn Fn() -> bool,
    timeout: f32,
}
const AX: Ax<'static> = Ax {
    current: &|| true,
    timeout: 1.0,
};
impl Ax<'_> {
    fn checkpoint(&self) -> Result<()> {
        if (self.current)() {
            Ok(())
        } else {
            Err(BackendError::cancelled())
        }
    }
    fn request<T>(
        &self,
        e: &AXUIElement,
        context: &str,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        crate::native_ops::prepared_request(
            self.current,
            || unsafe {
                check(e.set_messaging_timeout(self.timeout))
                    .map_err(|e| e.context("Set object messaging timeout"))
            },
            operation,
        )
        .map_err(|e| e.context(context))
    }
    fn capabilities(
        &self,
        root: &AXUIElement,
        element: &AXUIElement,
        strict: bool,
    ) -> Result<WindowCapabilities> {
        let mut actions = std::ptr::null();
        let result = self.request(element, "Copy actions", || unsafe {
            check(element.copy_action_names(NonNull::from(&mut actions)))
        });
        let actions = NonNull::new(actions.cast_mut())
            .map(|p| unsafe { CFRetained::<CFArray<CFString>>::from_raw(p.cast()) });
        let available = if strict {
            result?;
            true
        } else {
            crate::native_ops::optional(result)?.is_some()
        };
        let raise =
            available && actions.is_some_and(|a| a.iter().any(|s| s.to_string() == "AXRaise"));
        let settable = |e: &AXUIElement, name| -> Result<bool> {
            let result = self.settable(e, name);
            if strict {
                result
            } else {
                Ok(crate::native_ops::optional(result)?.unwrap_or(false))
            }
        };
        Ok(WindowCapabilities {
            frontmost: settable(root, "AXFrontmost")?,
            minimize: settable(element, "AXMinimized")?,
            position: settable(element, "AXPosition")?,
            size: settable(element, "AXSize")?,
            main: settable(element, "AXMain")?,
            raise,
        })
    }
    fn optional_attr(&self, e: &AXUIElement, name: &str) -> Result<Option<CFRetained<CFType>>> {
        let mut value = std::ptr::null();
        let code = self.request(e, name, || unsafe {
            let code = e.copy_attribute_value(&CFString::from_str(name), NonNull::from(&mut value));
            if code == AXError::AttributeUnsupported || code == AXError::NoValue {
                Ok(code)
            } else {
                check(code).map(|()| code)
            }
        });
        let retained = NonNull::new(value.cast_mut()).map(|p| unsafe { CFRetained::from_raw(p) });
        let code = code?;
        if code == AXError::AttributeUnsupported || code == AXError::NoValue {
            return Ok(None);
        }
        check(code)?;
        Ok(retained)
    }
    fn attr(&self, e: &AXUIElement, name: &str) -> Result<CFRetained<CFType>> {
        self.optional_attr(e, name)?
            .ok_or_else(|| BackendError::from(format!("Missing {name}")))
    }
    fn optional_bool(&self, e: &AXUIElement, name: &str) -> Result<bool> {
        match self.optional_attr(e, name)? {
            Some(v) => v
                .downcast::<CFBoolean>()
                .map(|b| b.value())
                .map_err(|_| BackendError::from(format!("Invalid {name}"))),
            None => Ok(false),
        }
    }

    fn string(&self, e: &AXUIElement, name: &str) -> Result<Option<String>> {
        self.optional_attr(e, name)?
            .map(|value| {
                value
                    .downcast::<CFString>()
                    .map(|s| s.to_string())
                    .map_err(|_| BackendError::from(format!("Invalid {name}")))
            })
            .transpose()
    }
    fn boolean(&self, e: &AXUIElement, name: &str) -> Result<bool> {
        self.attr(e, name)?
            .downcast::<CFBoolean>()
            .map(|v| v.value())
            .map_err(|_| BackendError::from(format!("Invalid {name}")))
    }
    fn set_bool(&self, e: &AXUIElement, name: &str, value: bool) -> Result<()> {
        self.request(e, name, || unsafe {
            check(e.set_attribute_value(&CFString::from_str(name), CFBoolean::new(value).as_ref()))
        })
    }
    fn settable(&self, e: &AXUIElement, name: &str) -> Result<bool> {
        let mut b = 0;
        self.request(e, name, || unsafe {
            check(e.is_attribute_settable(&CFString::from_str(name), NonNull::from(&mut b)))
        })?;
        Ok(b != 0)
    }
    fn array_value(value: CFRetained<CFType>) -> Result<Vec<CFRetained<AXUIElement>>> {
        let array = value.downcast::<CFArray>().map_err(|_| "Not an AX array")?;
        let array = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(array) };
        array
            .iter()
            .map(|value| {
                value.downcast::<AXUIElement>().map_err(|_| {
                    BackendError::new(ErrorKind::Communication, "Invalid AX array member")
                })
            })
            .collect()
    }
    fn optional_array(
        &self,
        element: &AXUIElement,
        name: &str,
    ) -> Result<Option<Vec<CFRetained<AXUIElement>>>> {
        self.optional_attr(element, name)?
            .map(Self::array_value)
            .transpose()
    }
    fn array(&self, element: &AXUIElement, name: &str) -> Result<Vec<CFRetained<AXUIElement>>> {
        self.optional_array(element, name)?
            .ok_or_else(|| format!("Missing {name}").into())
    }
    fn windows(&self, root: &AXUIElement) -> Result<Vec<CFRetained<AXUIElement>>> {
        let windows = self.optional_array(root, "AXWindows")?;
        let children = self.optional_array(root, "AXChildren")?;
        crate::native_ops::merge_window_sources(
            windows,
            children,
            |child| {
                let role = self.string(child, "AXRole")?.ok_or_else(|| {
                    BackendError::new(ErrorKind::Communication, "Application child has no AXRole")
                })?;
                Ok(role == "AXWindow")
            },
            |a, b| **a == **b,
        )
    }
}
/// Read badge metadata exposed by the Dock, matching exact application paths.
/// No notification contents, private APIs, focus changes or app mutations.
pub fn read_dock_badges(
    dock_pid: i32,
    targets: &[(TabId, String)],
) -> Result<std::collections::BTreeMap<TabId, String>> {
    let ax = Ax { timeout: 0.2, ..AX };
    let mut badges = std::collections::BTreeMap::new();
    if targets.is_empty() {
        return Ok(badges);
    }
    if !unsafe { AXIsProcessTrusted() } {
        return Err(BackendError::new(
            ErrorKind::Permission,
            "Accessibility unavailable",
        ));
    }
    let root = unsafe { AXUIElement::new_application(dock_pid) };
    let started = std::time::Instant::now();
    let mut pending = vec![(root, 0)];
    let mut visited = 0;
    while let Some((element, depth)) = pending.pop() {
        if visited >= 128 || started.elapsed() > std::time::Duration::from_millis(250) {
            return Err("Dock badge scan timed out".into());
        }
        visited += 1;
        if let Some(url) = ax
            .optional_attr(&element, "AXURL")?
            .and_then(|v| v.downcast::<CFURL>().ok())
        {
            if !url
                .scheme()
                .is_some_and(|scheme| scheme.to_string() == "file")
            {
                continue;
            }
            let Some(path) = url
                .file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle)
                .map(|s| s.to_string())
            else {
                continue;
            };
            let matches: Vec<_> = targets
                .iter()
                .filter(|(_, candidate)| {
                    candidate.trim_end_matches('/') == path.trim_end_matches('/')
                })
                .collect();
            if !matches.is_empty()
                && let Some(label) = ax.string(&element, "AXStatusLabel")?
                && badge_text(&label).is_some()
            {
                let label: String = label.chars().filter(|c| !c.is_control()).take(64).collect();
                for (id, _) in matches {
                    badges.insert(*id, label.clone());
                }
            }
        } else if depth < 2
            && let Some(children) = ax.optional_attr(&element, "AXChildren")?
        {
            let children = children
                .downcast::<CFArray>()
                .map_err(|_| "Invalid Dock children")?;
            // AXChildren is an array of Accessibility elements; check each value.
            let children = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(children) };
            pending.extend(
                children
                    .iter()
                    .filter_map(|child| child.downcast::<AXUIElement>().ok())
                    .map(|child| (child, depth + 1)),
            );
        }
    }
    Ok(badges)
}
impl Ax<'_> {
    fn state(&self, e: &AXUIElement) -> Result<WindowState> {
        let p = self
            .attr(e, "AXPosition")?
            .downcast::<AXValue>()
            .map_err(|_| "Invalid position")?;
        let s = self
            .attr(e, "AXSize")?
            .downcast::<AXValue>()
            .map_err(|_| "Invalid size")?;
        let mut point = CGPoint::default();
        let mut size = CGSize::default();
        unsafe {
            if !p.value(AXValueType::CGPoint, NonNull::from(&mut point).cast())
                || !s.value(AXValueType::CGSize, NonNull::from(&mut size).cast())
            {
                return Err("Invalid window geometry".into());
            }
        }
        Ok(WindowState {
            frame: Rect {
                x: point.x,
                y: point.y,
                width: size.width,
                height: size.height,
            },
            minimized: self.boolean(e, "AXMinimized")?,
            fullscreen: self.optional_bool(e, "AXFullScreen")?,
            modal: self.optional_bool(e, "AXModal")?
                || self
                    .optional_attr(e, "AXSheets")?
                    .is_some_and(|v| v.downcast::<CFArray>().is_ok_and(|a| !a.is_empty())),
        })
    }
}
pub fn request_permission() {
    let key = CFString::from_str("AXTrustedCheckOptionPrompt");
    let options =
        objc2_core_foundation::CFDictionary::from_slices(&[&*key], &[CFBoolean::new(true)]);
    // The dictionary has CF string keys and boolean values, as required by this API.
    let options =
        unsafe { CFRetained::cast_unchecked::<objc2_core_foundation::CFDictionary>(options) };
    unsafe {
        objc2_application_services::AXIsProcessTrustedWithOptions(Some(&options));
    }
}
impl MacBackend {
    pub fn new() -> Self {
        Self {
            apps: vec![],
            entries: HashMap::new(),
            next: 1,
            watched: HashSet::new(),
            focus_suspended: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn update_apps(&mut self, apps: Vec<App>) {
        self.apps = apps;
    }
    pub fn probe_minimized_fixture(&self, pid: i32) -> Result<()> {
        let root = unsafe { AXUIElement::new_application(pid) };
        let windows = AX.array(&root, "AXWindows")?;
        println!("AXWindows count={}", windows.len());
        for element in windows {
            println!(
                "role={:?} subrole={:?} minimized={:?} state={:?}",
                AX.string(&element, "AXRole"),
                AX.string(&element, "AXSubrole"),
                AX.boolean(&element, "AXMinimized"),
                AX.state(&element)
            );
            for attribute in ["AXPosition", "AXSize", "AXMinimized", "AXMain"] {
                println!(
                    "{attribute} settable={:?}",
                    AX.settable(&element, attribute)
                );
            }
        }
        Ok(())
    }
    pub fn discover_current(&mut self, current: impl Fn() -> bool) -> Result<Vec<WindowInfo>> {
        if !self.trusted() {
            return Err(BackendError::new(
                ErrorKind::Permission,
                "Grant AppDock Accessibility permission in System Settings, then Resume",
            ));
        }
        let ax = Ax {
            current: &current,
            ..AX
        };
        let mut found = vec![];
        let mut staged = vec![];
        let mut next = self.next;
        for app in &self.apps {
            ax.checkpoint()?;
            let root = unsafe { AXUIElement::new_application(app.pid) };
            let Some(windows) = crate::native_ops::optional(ax.windows(&root))? else {
                continue;
            };
            for element in windows {
                ax.checkpoint()?;
                let role = crate::native_ops::optional(ax.string(&element, "AXRole"))?.flatten();
                if role.as_deref() != Some("AXWindow") {
                    continue;
                }
                let subrole =
                    crate::native_ops::optional(ax.string(&element, "AXSubrole"))?.flatten();
                let state = crate::native_ops::optional(ax.state(&element))?;
                let minimized = state.is_some_and(|s| s.minimized);
                if !discoverable_window(role.as_deref(), subrole.as_deref(), minimized) {
                    continue;
                }
                let existing = self
                    .entries
                    .iter()
                    .find(|(_, e)| e.info.pid == app.pid && *e.element == *element)
                    .map(|(id, _)| *id);
                let id = existing.unwrap_or_else(|| {
                    let id = next;
                    next += 1;
                    id
                });
                let capabilities = ax.capabilities(&root, &element, false)?;
                let info = WindowInfo {
                    id,
                    pid: app.pid,
                    app: app.name.clone(),
                    title: crate::native_ops::optional(ax.string(&element, "AXTitle"))?
                        .flatten()
                        .unwrap_or_else(|| "Untitled window".into()),
                    identity: Identity {
                        bundle: app.bundle.clone(),
                        identifier: crate::native_ops::optional(
                            ax.string(&element, "AXIdentifier"),
                        )?
                        .flatten()
                        .filter(|s| !s.is_empty()),
                    },
                    eligible: state.is_some_and(|s| capabilities.can_attach(s)),
                    minimized,
                };
                let number = self.entries.get(&id).and_then(|e| e.number);
                staged.push((
                    id,
                    Entry {
                        element,
                        app: root.clone(),
                        info: info.clone(),
                        number,
                        closed: false,
                    },
                ));
                found.push(info);
            }
        }
        ax.checkpoint()?;
        self.next = next;
        self.entries.extend(staged);
        let found_ids: HashSet<_> = found.iter().map(|w| w.id).collect();
        self.entries
            .retain(|id, _| self.watched.contains(id) || found_ids.contains(id));
        Ok(found)
    }
    fn entry(&self, id: WindowId) -> Result<&Entry> {
        let entry = self.entries.get(&id).ok_or("Window is disconnected")?;
        if entry.closed {
            return Err(BackendError::new(
                ErrorKind::Closed,
                "Window closure already confirmed",
            ));
        }
        Ok(entry)
    }
    pub fn window_number(&self, id: WindowId) -> Option<u32> {
        self.entries.get(&id)?.number
    }
    pub fn stacking_identity(&self, id: WindowId) -> Option<(u32, i32)> {
        let entry = self.entries.get(&id)?;
        Some((entry.number?, entry.info.pid))
    }
}
impl WindowBackend for MacBackend {
    fn same_window(&self, a: WindowId, b: WindowId) -> bool {
        a == b
            || self
                .entries
                .get(&a)
                .zip(self.entries.get(&b))
                .is_some_and(|(a, b)| a.info.pid == b.info.pid && a.element == b.element)
    }
    fn trusted(&self) -> bool {
        unsafe { AXIsProcessTrusted() }
    }
    fn discover(&mut self) -> Result<Vec<WindowInfo>> {
        self.discover_current(|| true)
    }
    fn state(&self, id: WindowId) -> Result<WindowState> {
        AX.state(&self.entry(id)?.element)
    }
    fn set_frame(&mut self, id: WindowId, frame: Rect) -> Result<Rect> {
        if !frame.valid() {
            return Err("Invalid docking area".into());
        }
        let e = &self.entry(id)?.element;
        let point = CGPoint {
            x: frame.x,
            y: frame.y,
        };
        let size = CGSize {
            width: frame.width,
            height: frame.height,
        };
        unsafe {
            let p = AXValue::new(AXValueType::CGPoint, NonNull::from(&point).cast())
                .ok_or("Cannot encode position")?;
            let s = AXValue::new(AXValueType::CGSize, NonNull::from(&size).cast())
                .ok_or("Cannot encode size")?;
            AX.request(e, "Set AXSize", || {
                check(e.set_attribute_value(&CFString::from_str("AXSize"), s.as_ref()))
            })
            .map_err(|e| e.context("Resize"))?;
            AX.request(e, "Set AXPosition", || {
                check(e.set_attribute_value(&CFString::from_str("AXPosition"), p.as_ref()))
            })
            .map_err(|e| e.context("Move"))?;
        }
        // Let asynchronous AX resizing settle and read back target-enforced size limits.
        let actual = crate::native_ops::settle_frame(
            || self.state(id).map(|state| state.frame),
            || std::thread::sleep(std::time::Duration::from_millis(40)),
        )?;
        self.watched.insert(id);
        // AX reports the actual clamped size: never repeatedly force an unsupported size.
        Ok(actual)
    }
    fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> Result<()> {
        if !x.is_finite() || !y.is_finite() {
            return Err("Invalid position".into());
        }
        let point = CGPoint { x, y };
        unsafe {
            let value = AXValue::new(AXValueType::CGPoint, NonNull::from(&point).cast())
                .ok_or("Cannot encode position")?;
            let element = &self.entry(id)?.element;
            AX.request(element, "Set AXPosition", || {
                check(element.set_attribute_value(&CFString::from_str("AXPosition"), &value))
            })
        }
    }
    fn minimize(&mut self, id: WindowId, value: bool) -> Result<()> {
        if self.state(id)?.minimized == value {
            return Ok(());
        }
        AX.set_bool(&self.entry(id)?.element, "AXMinimized", value)?;
        for attempt in 0..30 {
            if self.state(id)?.minimized == value {
                break;
            }
            if attempt == 29 {
                return Err("Window did not accept minimization change".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        self.watched.insert(id);
        Ok(())
    }
    fn validate_restored_window(&self, id: WindowId, current: &dyn Fn() -> bool) -> Result<()> {
        let entry = self.entry(id)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(900);
        let active = || current() && !self.focus_suspended.load(Ordering::SeqCst);
        let ax = Ax {
            current: &active,
            ..AX
        };
        crate::native_ops::wait_for_window_ready(
            active,
            || {
                let state = ax.state(&entry.element)?;
                if state.fullscreen || state.modal {
                    return Err(
                        "Close dialogs and leave fullscreen before docking this restored window"
                            .into(),
                    );
                }
                if state.minimized {
                    return Ok(false);
                }
                let role = ax.string(&entry.element, "AXRole")?;
                let subrole = ax.string(&entry.element, "AXSubrole")?;
                if !discoverable_window(role.as_deref(), subrole.as_deref(), false) {
                    return Ok(false);
                }
                Ok(ax
                    .capabilities(&entry.app, &entry.element, true)?
                    .can_attach(state))
            },
            || std::time::Instant::now() >= deadline,
            || std::thread::sleep(std::time::Duration::from_millis(30)),
        )
    }
    fn focus(&mut self, id: WindowId) -> Result<()> {
        let e = self.entry(id)?;
        let number = crate::native_ops::focus_sequence(
            || self.focus_suspended.load(Ordering::SeqCst),
            |step| match step {
                0 => AX.set_bool(&e.app, "AXFrontmost", true),
                1 => AX.request(&e.element, "AXRaise", || unsafe {
                    check(e.element.perform_action(&CFString::from_str("AXRaise")))
                }),
                _ => AX.set_bool(&e.element, "AXMain", true),
            },
            || {
                if AX.boolean(&e.app, "AXFrontmost")?
                    && AX
                        .attr(&e.app, "AXFocusedWindow")?
                        .downcast::<AXUIElement>()
                        .is_ok_and(|focused| *focused == *e.element)
                    && let Some(number) =
                        crate::window_tracking::focused_number(e.info.pid, self.state(id)?.frame)
                    && e.number.is_none_or(|known| known == number)
                    && !self.entries.iter().any(|(other_id, other)| {
                        *other_id != id
                            && other.number == Some(number)
                            && other.element != e.element
                    })
                {
                    Ok(Some(number))
                } else {
                    Ok(None)
                }
            },
            || std::thread::sleep(std::time::Duration::from_millis(30)),
        )?;
        if let Some(number) = number {
            self.entries.get_mut(&id).unwrap().number = Some(number);
        }
        Ok(())
    }
    fn watch(&mut self, id: WindowId, enabled: bool) {
        if enabled {
            self.watched.insert(id);
        } else {
            self.watched.remove(&id);
        }
    }
    fn check_window(&mut self, id: WindowId) -> Result<()> {
        self.refresh_window(id, &mut HashMap::new())
    }
    fn events(&mut self) -> Vec<BackendEvent> {
        if !self.trusted() {
            return vec![BackendEvent::PermissionLost];
        }
        let mut app_windows = HashMap::new();
        self.watched
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|id| match self.refresh_window(id, &mut app_windows) {
                Err(e) if e.kind == ErrorKind::Closed => BackendEvent::Closed(id),
                Err(e) if e.kind == ErrorKind::Permission => BackendEvent::PermissionLost,
                _ => BackendEvent::Changed(id),
            })
            .collect()
    }
}

impl MacBackend {
    fn refresh_window(
        &mut self,
        id: WindowId,
        app_windows: &mut HashMap<i32, Result<Vec<CFRetained<AXUIElement>>>>,
    ) -> Result<()> {
        let e = self.entry(id)?;
        // App-list snapshots may lag. Only the OS process probe or successful
        // combined AX window-list membership can confirm closure.
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
            fn __error() -> *mut i32;
        }
        const ESRCH: i32 = 3; // Darwin: no such process. Other probe failures retain recovery.
        let exited = unsafe { kill(e.info.pid, 0) == -1 && *__error() == ESRCH };
        let membership = if exited {
            crate::native_ops::Membership::Closed
        } else {
            if !self.trusted() {
                return Err(BackendError::new(
                    ErrorKind::Permission,
                    "Accessibility unavailable",
                ));
            }
            let windows = app_windows
                .entry(e.info.pid)
                .or_insert_with(|| AX.windows(&e.app))
                .as_ref()
                .map_err(Clone::clone)?;
            let exact = windows.iter().position(|w| **w == *e.element);
            let owners = self
                .watched
                .iter()
                .filter_map(|id| self.entries.get(id))
                .filter(|other| {
                    other.info.pid == e.info.pid
                        && other.info.identity.identifier == e.info.identity.identifier
                })
                .count();
            crate::native_ops::membership(
                exact,
                e.info.identity.identifier.as_deref(),
                owners,
                windows.iter().map(|w| AX.string(w, "AXIdentifier")),
            )?
        };
        match membership {
            crate::native_ops::Membership::Closed => {
                // A preflight caller may not remove its attachment immediately.
                // Preserve confirmed closure until the engine acknowledges it.
                self.entries.get_mut(&id).unwrap().closed = true;
                Err(BackendError::new(
                    ErrorKind::Closed,
                    "Window closure confirmed",
                ))
            }
            crate::native_ops::Membership::Present(index) => {
                let element = app_windows[&e.info.pid].as_ref().unwrap()[index].clone();
                let entry = self.entries.get_mut(&id).unwrap();
                if entry.element != element {
                    entry.number = None;
                }
                entry.element = element;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn m2_ax_failures_preserve_codes_and_never_establish_closure() {
        for code in [AXError::CannotComplete, AXError::InvalidUIElement] {
            let error = check(code).unwrap_err().context("AXWindows");
            assert_eq!(error.kind, ErrorKind::Communication);
            assert_eq!(error.ax_code, Some(code.0));
            assert!(error.to_string().contains("AXWindows"));
        }
        assert_eq!(
            check(AXError::APIDisabled).unwrap_err().kind,
            ErrorKind::Permission
        );
    }
    #[test]
    fn a1_confirmed_closure_survives_preflight_until_engine_acknowledgement() {
        let mut backend = MacBackend::new();
        let root = unsafe { AXUIElement::new_application(std::process::id() as i32) };
        backend.entries.insert(
            1,
            Entry {
                element: root.clone(),
                app: root,
                number: None,
                closed: true,
                info: WindowInfo {
                    id: 1,
                    pid: std::process::id() as i32,
                    app: "fixture".into(),
                    title: "fixture".into(),
                    identity: Identity {
                        bundle: "fixture".into(),
                        identifier: None,
                    },
                    eligible: true,
                    minimized: false,
                },
            },
        );
        backend.watch(1, true);
        for _ in 0..2 {
            assert_eq!(backend.check_window(1).unwrap_err().kind, ErrorKind::Closed);
        }
        assert!(backend.watched.contains(&1));
        backend.watch(1, false);
        assert!(!backend.watched.contains(&1));
    }
}
