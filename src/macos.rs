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
}
pub struct MacBackend {
    pub apps: Vec<App>,
    entries: HashMap<WindowId, Entry>,
    next: WindowId,
    watched: HashSet<WindowId>,
    pub focus_suspended: Arc<AtomicBool>,
}
fn check(e: AXError) -> Result<()> {
    if e == AXError::Success {
        Ok(())
    } else {
        Err(format!("Accessibility error {}", e.0))
    }
}
fn optional_attr(e: &AXUIElement, name: &str) -> Result<Option<CFRetained<CFType>>> {
    let mut value = std::ptr::null();
    let code =
        unsafe { e.copy_attribute_value(&CFString::from_str(name), NonNull::from(&mut value)) };
    let retained = NonNull::new(value.cast_mut()).map(|p| unsafe { CFRetained::from_raw(p) });
    if code == AXError::AttributeUnsupported || code == AXError::NoValue {
        return Ok(None);
    }
    check(code)?;
    Ok(retained)
}
fn attr(e: &AXUIElement, name: &str) -> Result<CFRetained<CFType>> {
    optional_attr(e, name)?.ok_or_else(|| format!("Missing {name}"))
}
fn optional_bool(e: &AXUIElement, name: &str) -> Result<bool> {
    match optional_attr(e, name)? {
        Some(v) => v
            .downcast::<CFBoolean>()
            .map(|b| b.value())
            .map_err(|_| format!("Invalid {name}")),
        None => Ok(false),
    }
}

fn string(e: &AXUIElement, name: &str) -> Option<String> {
    attr(e, name)
        .ok()?
        .downcast::<CFString>()
        .ok()
        .map(|s| s.to_string())
}
fn boolean(e: &AXUIElement, name: &str) -> Result<bool> {
    attr(e, name)?
        .downcast::<CFBoolean>()
        .map(|v| v.value())
        .map_err(|_| format!("Invalid {name}"))
}
fn set_bool(e: &AXUIElement, name: &str, value: bool) -> Result<()> {
    unsafe {
        check(e.set_attribute_value(&CFString::from_str(name), CFBoolean::new(value).as_ref()))
    }
}
fn settable(e: &AXUIElement, name: &str) -> bool {
    let mut b = 0;
    unsafe {
        e.is_attribute_settable(&CFString::from_str(name), NonNull::from(&mut b))
            == AXError::Success
            && b != 0
    }
}
fn array(e: &AXUIElement, name: &str) -> Result<Vec<CFRetained<AXUIElement>>> {
    let a = attr(e, name)?
        .downcast::<CFArray>()
        .map_err(|_| "Not an AX array")?;
    // CFArray is untyped; each member is checked before treating it as an AX element.
    let a = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(a) };
    Ok(a.iter()
        .filter_map(|v| v.downcast::<AXUIElement>().ok())
        .collect())
}
/// Read badge metadata exposed by the Dock, matching exact application paths.
/// No notification contents, private APIs, focus changes or app mutations.
pub fn read_dock_badges(
    dock_pid: i32,
    targets: &[(TabId, String)],
) -> Result<std::collections::BTreeMap<TabId, String>> {
    let mut badges = std::collections::BTreeMap::new();
    if targets.is_empty() {
        return Ok(badges);
    }
    if !unsafe { AXIsProcessTrusted() } {
        return Err("Accessibility unavailable".into());
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
        unsafe {
            check(element.set_messaging_timeout(0.2))?;
        }
        if let Some(url) =
            optional_attr(&element, "AXURL")?.and_then(|v| v.downcast::<CFURL>().ok())
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
                && let Some(label) = string(&element, "AXStatusLabel")
                && badge_text(&label).is_some()
            {
                let label: String = label.chars().filter(|c| !c.is_control()).take(64).collect();
                for (id, _) in matches {
                    badges.insert(*id, label.clone());
                }
            }
        } else if depth < 2
            && let Some(children) = optional_attr(&element, "AXChildren")?
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
fn state_of(e: &AXUIElement) -> Result<WindowState> {
    let p = attr(e, "AXPosition")?
        .downcast::<AXValue>()
        .map_err(|_| "Invalid position")?;
    let s = attr(e, "AXSize")?
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
        minimized: boolean(e, "AXMinimized")?,
        fullscreen: optional_bool(e, "AXFullScreen")?,
        modal: optional_bool(e, "AXModal")?
            || optional_attr(e, "AXSheets")?
                .is_some_and(|v| v.downcast::<CFArray>().is_ok_and(|a| !a.is_empty())),
    })
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
    fn entry(&self, id: WindowId) -> Result<&Entry> {
        self.entries.get(&id).ok_or("Window is disconnected".into())
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
        if !self.trusted() {
            return Err(
                "Grant AppDock Accessibility permission in System Settings, then Resume.".into(),
            );
        }
        let mut result = vec![];
        for app in &self.apps {
            let root = unsafe { AXUIElement::new_application(app.pid) };
            unsafe {
                check(root.set_messaging_timeout(1.0))?;
            }
            let Ok(windows) = array(&root, "AXWindows") else {
                continue;
            };
            for element in windows {
                let subrole = string(&element, "AXSubrole");
                if subrole.as_deref() != Some("AXStandardWindow") {
                    continue;
                }
                unsafe {
                    let _ = element.set_messaging_timeout(1.0);
                }
                let existing = self
                    .entries
                    .iter()
                    .find(|(_, e)| e.info.pid == app.pid && *e.element == *element)
                    .map(|(id, _)| *id);
                let id = existing.unwrap_or_else(|| {
                    let id = self.next;
                    self.next += 1;
                    id
                });
                let mut actions = std::ptr::null();
                let raise = unsafe {
                    if element.copy_action_names(NonNull::from(&mut actions)) == AXError::Success {
                        NonNull::new(actions.cast_mut())
                            .map(|p| CFRetained::<CFArray<CFString>>::from_raw(p.cast()))
                            .is_some_and(|a| a.iter().any(|s| s.to_string() == "AXRaise"))
                    } else {
                        false
                    }
                };
                let state = state_of(&element);
                let info = WindowInfo {
                    id,
                    pid: app.pid,
                    app: app.name.clone(),
                    title: string(&element, "AXTitle").unwrap_or_else(|| "Untitled window".into()),
                    identity: Identity {
                        bundle: app.bundle.clone(),
                        identifier: string(&element, "AXIdentifier").filter(|s| !s.is_empty()),
                    },
                    eligible: settable(&root, "AXFrontmost")
                        && raise
                        && ["AXPosition", "AXSize", "AXMinimized", "AXMain"]
                            .iter()
                            .all(|n| settable(&element, n))
                        && state.is_ok_and(|s| !s.fullscreen && !s.modal),
                };
                let number = self.entries.get(&id).and_then(|e| e.number);
                self.entries.insert(
                    id,
                    Entry {
                        element,
                        app: root.clone(),
                        info: info.clone(),
                        number,
                    },
                );
                result.push(info);
            }
        }
        Ok(result)
    }
    fn state(&self, id: WindowId) -> Result<WindowState> {
        state_of(&self.entry(id)?.element)
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
            check(e.set_attribute_value(&CFString::from_str("AXSize"), s.as_ref()))
                .map_err(|e| format!("Resize: {e}"))?;
            check(e.set_attribute_value(&CFString::from_str("AXPosition"), p.as_ref()))
                .map_err(|e| format!("Move: {e}"))?;
        }
        // Let asynchronous AX resizing settle and read back target-enforced size limits.
        let mut actual = self.state(id)?.frame;
        for _ in 0..4 {
            std::thread::sleep(std::time::Duration::from_millis(40));
            let next = self.state(id)?.frame;
            if next.near(actual) {
                actual = next;
                break;
            }
            actual = next;
        }
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
            check(
                self.entry(id)?
                    .element
                    .set_attribute_value(&CFString::from_str("AXPosition"), &value),
            )
        }
    }
    fn minimize(&mut self, id: WindowId, value: bool) -> Result<()> {
        if self.state(id)?.minimized == value {
            return Ok(());
        }
        set_bool(&self.entry(id)?.element, "AXMinimized", value)?;
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
    fn focus(&mut self, id: WindowId) -> Result<()> {
        if self.focus_suspended.load(Ordering::SeqCst) {
            return Ok(());
        }
        let e = self.entry(id)?;
        set_bool(&e.app, "AXFrontmost", true)?;
        if self.focus_suspended.load(Ordering::SeqCst) {
            return Ok(());
        }
        unsafe {
            check(e.element.perform_action(&CFString::from_str("AXRaise")))?;
        }
        if self.focus_suspended.load(Ordering::SeqCst) {
            return Ok(());
        }
        set_bool(&e.element, "AXMain", true)?;
        for _ in 0..30 {
            if self.focus_suspended.load(Ordering::SeqCst) {
                return Ok(());
            }
            if boolean(&e.app, "AXFrontmost")?
                && attr(&e.app, "AXFocusedWindow")?
                    .downcast::<AXUIElement>()
                    .is_ok_and(|focused| *focused == *e.element)
                && let Some(number) =
                    crate::window_tracking::focused_number(e.info.pid, self.state(id)?.frame)
                // AX focus and WindowServer ordering settle independently. Never
                // replace a known anchor with another tab's still-frontmost window.
                && e.number.is_none_or(|known| known == number)
                && !self.entries.iter().any(|(other_id, other)| {
                    *other_id != id && other.number == Some(number) && other.element != e.element
                })
            {
                self.entries.get_mut(&id).unwrap().number = Some(number);
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        Err("Target window did not receive keyboard focus".into())
    }
    fn watch(&mut self, id: WindowId, enabled: bool) {
        if enabled {
            self.watched.insert(id);
        } else {
            self.watched.remove(&id);
        }
    }
    fn events(&mut self) -> Vec<BackendEvent> {
        if !self.trusted() {
            return vec![BackendEvent::PermissionLost];
        }
        let mut events = vec![];
        let mut app_windows = HashMap::new();
        for id in self.watched.iter().copied().collect::<Vec<_>>() {
            let Some(e) = self.entries.get(&id) else {
                continue;
            };
            if !self.apps.iter().any(|app| app.pid == e.info.pid) {
                events.push(BackendEvent::Closed(id));
                continue;
            }
            // Closed AX objects can still return cached attributes. Confirm
            // membership once per process each poll; failed reads retain handles.
            let Ok(windows) = app_windows
                .entry(e.info.pid)
                .or_insert_with(|| array(&e.app, "AXWindows"))
            else {
                events.push(BackendEvent::Changed(id));
                continue;
            };
            if windows.iter().any(|w| **w == *e.element) {
                events.push(BackendEvent::Changed(id));
                continue;
            }
            // Some apps replace AX objects. Rebind only through an identifier
            // unique in both the live app and our watched entries, never title.
            let replacement = e.info.identity.identifier.as_ref().and_then(|identifier| {
                let candidates: Vec<_> = windows
                    .iter()
                    .filter(|w| string(w, "AXIdentifier").as_ref() == Some(identifier))
                    .collect();
                let owners = self
                    .watched
                    .iter()
                    .filter_map(|id| self.entries.get(id))
                    .filter(|other| {
                        other.info.pid == e.info.pid
                            && other.info.identity.identifier.as_ref() == Some(identifier)
                    })
                    .count();
                (candidates.len() == 1 && owners == 1).then(|| candidates[0].clone())
            });
            if let Some(element) = replacement {
                let entry = self.entries.get_mut(&id).unwrap();
                entry.element = element;
                entry.number = None;
                events.push(BackendEvent::Changed(id));
            } else {
                events.push(BackendEvent::Closed(id));
            }
        }
        for e in &events {
            if let BackendEvent::Closed(id) = e {
                self.entries.remove(id);
                self.watched.remove(id);
            }
        }
        events
    }
}
