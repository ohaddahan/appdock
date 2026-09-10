//! Isolated review cases. Only the child PID supplied here enters MacBackend.
use super::*;
use crate::{engine::Engine, macos::MacBackend};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn data_dir() -> Result<PathBuf> {
    let path = std::env::var_os("APPDOCK_DATA_DIR")
        .ok_or("Review fixtures require isolated APPDOCK_DATA_DIR")?;
    let path = PathBuf::from(path);
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}
fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn pump(app: &NSApplication, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        let until = objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.005);
        if let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&until),
            unsafe { objc2_foundation::NSDefaultRunLoopMode },
            true,
        ) {
            app.sendEvent(&event);
        }
        app.updateWindows();
    }
}
pub(crate) fn run(case: &str) -> Result<()> {
    let dir = data_dir()?;
    require(
        !dir.join("workspace.json").exists(),
        "Review fixtures require a fresh isolated directory without workspace.json",
    )?;
    require(
        [
            "prerequisite",
            "A1",
            "A2",
            "A3",
            "A4",
            "A5",
            "A6",
            "D3",
            "D4",
            "D5",
            "D6",
        ]
        .contains(&case),
        "Unknown review case",
    )?;
    let trusted = unsafe { objc2_application_services::AXIsProcessTrusted() };
    println!("review case={case} accessibility={trusted}");
    if !trusted {
        return Err(BackendError::new(
            ErrorKind::Permission,
            "PREREQUISITE FAILURE: Accessibility permission unavailable for this launch context",
        ));
    }
    if case == "prerequisite" {
        return Ok(());
    }
    let m = MainThreadMarker::new().ok_or("Main thread required")?;
    let app = NSApplication::sharedApplication(m);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.finishLaunching();
    if case == "D6" {
        return backdrop_lifetime(m, &app);
    }
    if case == "A2" {
        return rendered_selection(m, &app);
    }
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg("--review-targets")
            .env("APPDOCK_DATA_DIR", &dir)
            .spawn()
            .map_err(|e| e.to_string())?,
    );
    let pid = child.0.id() as i32;
    let case = case.to_owned();
    let worker = std::thread::spawn(move || backend_case(&case, pid, &dir));
    while !worker.is_finished() {
        let _ = child.0.try_wait(); // Reap our exited target before the OS liveness probe.
        pump(&app, Duration::from_millis(10));
    }
    worker.join().map_err(|_| "Review worker panicked")?
}
fn send_target(dir: &Path, op: &str) -> Result<()> {
    let _ = std::fs::remove_file(dir.join("ack"));
    std::fs::write(dir.join("command.tmp"), op).map_err(|e| e.to_string())?;
    std::fs::rename(dir.join("command.tmp"), dir.join("command")).map_err(|e| e.to_string())?;
    let start = Instant::now();
    while !dir.join("ack").exists() {
        if start.elapsed() > Duration::from_secs(3) {
            return Err("Target command timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
fn backend_case(case: &str, pid: i32, dir: &Path) -> Result<()> {
    let mut backend = MacBackend::new();
    backend.update_apps(vec![App {
        pid,
        name: "Review fixture".into(),
        bundle: "dev.appdock.review".into(),
    }]);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut targets = loop {
        let windows = backend.discover()?;
        if windows.len() == 3 {
            break windows;
        }
        if Instant::now() >= deadline {
            return Err("Expected three disposable windows".into());
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    targets.sort_by(|a, b| a.title.cmp(&b.title));
    require(
        targets.iter().all(|w| w.pid == pid && w.eligible),
        "Fixture windows are not eligible",
    )?;
    if case == "A3" || case == "A6" {
        return worker_case(case, pid, backend, &targets);
    }
    let mut engine = Engine::new(backend, Workspace::default());
    let ids = targets
        .iter()
        .map(|w| engine.attach(None, w))
        .collect::<Result<Vec<_>>>()?;
    if case == "A5" || case == "D5" {
        let mut calls = 0;
        // Cancellation is checked between each individual AX request.
        let count = Cell::new(0);
        let scan = engine.backend.discover_current(|| {
            let n = count.get();
            count.set(n + 1);
            n < 8
        });
        require(
            scan.is_err_and(|e| e.kind == ErrorKind::Cancelled),
            "Native discovery did not cancel",
        )?;
        for id in &ids {
            engine.backend.check_window(engine.live[id].window)?;
            calls += 1;
        }
        println!(
            "Cancelled scan retained {calls} attached handles; checkpoints={}",
            count.get()
        );
        engine.backend.update_apps(vec![]);
        require(
            engine.backend.discover()?.is_empty(),
            "Removed app still appeared in discovery",
        )?;
        for id in &ids {
            engine.backend.check_window(engine.live[id].window)?;
        }
        engine.backend.update_apps(vec![App {
            pid,
            name: "Review fixture".into(),
            bundle: "dev.appdock.review".into(),
        }]);
        require(
            engine.backend.discover()?.len() == 3,
            "Re-added app missing",
        )?;
        return engine.restore_all();
    }
    if case == "D3" {
        let window = engine.live[&ids[0]].window;
        send_target(dir, "delay")?;
        let before = engine.backend.state(window)?.frame;
        let requested = Rect {
            x: before.x + 110.,
            width: before.width + 90.,
            ..before
        };
        let start = Instant::now();
        let actual = engine.backend.set_frame(window, requested)?;
        let elapsed = start.elapsed();
        std::thread::sleep(Duration::from_millis(200));
        let final_frame = engine.backend.state(window)?.frame;
        println!(
            "D3 before={before:?} request={requested:?} returned={actual:?} elapsed_ms={} final={final_frame:?}",
            elapsed.as_millis()
        );
        require(
            dir.join("delayed-commit").exists(),
            "PREREQUISITE FAILURE: native delay hook did not execute",
        )?;
        require(
            actual.near(final_frame) && !actual.near(before),
            "D3 premature native settling success",
        )?;
        send_target(dir, "immediate")?;
        return engine.restore_all();
    }
    engine.area.x += 180.;
    engine.switch(ids[0])?;
    engine.switch(ids[1])?;
    if case == "A1" {
        // A deliberately invalid restoration snapshot forces a real restoration
        // error. The child then closes, while the stale AX object remains cached.
        engine.live.get_mut(&ids[0]).unwrap().original.frame.width = 1.;
        require(engine.release(ids[0]).is_err(), "Expected failed release")?;
        send_target(dir, "close")?;
        // Preflight can learn closure before the engine consumes an event.
        let closed_window = targets[0].id;
        require(
            engine
                .backend
                .check_window(closed_window)
                .is_err_and(|e| e.kind == ErrorKind::Closed),
            "Native closure was not confirmed",
        )?;
        require(
            engine
                .backend
                .check_window(closed_window)
                .is_err_and(|e| e.kind == ErrorKind::Closed),
            "Confirmed closure was lost after preflight",
        )?;
        engine.observe();
        engine.retry_released()?;
        require(
            engine.released.is_empty(),
            "Closed child window retained recovery",
        )?;
        engine.live.get_mut(&ids[1]).unwrap().original.frame.width = 1.;
        require(
            engine.release(ids[1]).is_err(),
            "Expected second failed release",
        )?;
        send_target(dir, "exit")?;
        // Wait for process exit to be observable (not merely a queued request).
        std::thread::sleep(Duration::from_millis(150));
        engine.restore_all()?;
        return require(
            engine.released.is_empty() && engine.live.is_empty(),
            "Exited process blocked quit",
        );
    }
    if case == "A4" {
        let untouched = engine.backend.state(engine.live[&ids[2]].window)?;
        engine.backend.minimize(engine.live[&ids[0]].window, true)?;
        engine.observe();
        require(
            engine.paused.is_some(),
            "Minimization did not pause globally",
        )?;
        engine.resize(Rect {
            x: 460.,
            y: 260.,
            width: 500.,
            height: 350.,
        })?;
        engine.resume()?;
        for id in &ids[..2] {
            let state = engine.backend.state(engine.live[id].window)?;
            require(
                state.frame.near(engine.live[id].expected.frame)
                    && state.frame.x == engine.area.x
                    && !state.minimized,
                "Resume left an inactive window behind",
            )?;
        }
        require(
            engine.backend.state(engine.live[&ids[2]].window)? == untouched,
            "Resume moved never-docked attachment",
        )?;
    } else if case == "A3" {
        // Real worker integration is covered separately by the frame smoke;
        // here verify native released handles do not follow later geometry.
        let window = engine.live[&ids[0]].window;
        engine.release(ids[0])?;
        let released = engine.backend.state(window)?;
        engine.resize(Rect {
            x: 540.,
            width: 1100.,
            ..engine.area
        })?;
        require(
            engine.backend.state(window)? == released,
            "Released window moved",
        )?;
    } else if case == "D4" || case == "A6" {
        let window = engine.live[&ids[0]].window;
        engine
            .backend
            .focus_suspended
            .store(true, std::sync::atomic::Ordering::SeqCst);
        require(
            engine
                .backend
                .focus(window)
                .is_err_and(|e| e.kind == ErrorKind::Cancelled),
            "Editing did not suspend native focus",
        )?;
        engine
            .backend
            .focus_suspended
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
    engine.restore_all()
}
fn wait_snapshot(client: &Client, predicate: impl Fn(&Snapshot) -> bool) -> Result<Snapshot> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = client.snapshot.lock().unwrap().clone();
        if predicate(&snapshot) {
            return Ok(snapshot);
        }
        if Instant::now() >= deadline {
            return Err(format!("Worker fixture timed out: {}", snapshot.status).into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn worker_case(case: &str, pid: i32, backend: MacBackend, targets: &[WindowInfo]) -> Result<()> {
    let client = worker::start(Workspace::default());
    client.send(Command::Apps(vec![App {
        pid,
        name: "Worker fixture".into(),
        bundle: "dev.appdock.review".into(),
    }]));
    client.send(Command::Discover);
    let discovered = wait_snapshot(&client, |s| s.windows.len() == 3)?.windows;
    for (index, window) in discovered.iter().enumerate() {
        client.send(Command::Attach(None, window.id));
        wait_snapshot(&client, |s| {
            s.live.len() == index + 1 && s.selected == s.workspace.tabs.last().map(|t| t.id)
        })?;
    }
    let snapshot = client.snapshot.lock().unwrap().clone();
    let tabs = &snapshot.workspace.tabs;
    if case == "A6" {
        client.switch(tabs[0].id);
        wait_snapshot(&client, |s| {
            s.selected == Some(tabs[0].id) && !client.is_switching_to(tabs[0].id)
        })?;
        require(
            client.navigate(tabs, Some(tabs[0].id), false) == Some(tabs[1].id),
            "First navigation step lost",
        )?;
        require(
            client.navigate(tabs, Some(tabs[0].id), false) == Some(tabs[2].id),
            "Second navigation step lost",
        )?;
        wait_snapshot(&client, |s| {
            s.selected == Some(tabs[2].id) && !client.is_switching_to(tabs[2].id)
        })?;
        println!("A6 two unacknowledged next requests reached third tab");
    } else {
        let area = Rect {
            x: 440.,
            y: 220.,
            width: 1100.,
            height: 700.,
        };
        client.send(Command::Resize(area, area));
        client.send(Command::Release(tabs[0].id));
        client.send(Command::Release(tabs[1].id));
        let remaining = wait_snapshot(&client, |s| {
            s.live.len() == 1 && s.area == area && s.restore_pending == 0
        })?;
        let window = targets
            .iter()
            .find(|w| w.title == discovered[2].title)
            .unwrap();
        require(
            backend.state(window.id)?.frame.near(area),
            "Resize then multiple releases lost remaining geometry",
        )?;
        let paused = Rect {
            x: 560.,
            width: 1200.,
            ..area
        };
        client.send(Command::Resize(paused, paused));
        client.send(Command::Pause);
        wait_snapshot(&client, |s| s.paused && s.workspace.geometry == paused)?;
        client.send(Command::Resume);
        wait_snapshot(&client, |s| {
            !s.paused && s.selected_frame.is_some_and(|r| r.near(paused))
        })?;
        require(remaining.live.len() == 1, "Released windows reattached")?;
        let final_geometry = Rect {
            x: 600.,
            width: 1300.,
            ..area
        };
        client.send(Command::Resize(final_geometry, final_geometry));
        client.send(Command::Quit);
        wait_snapshot(&client, |s| s.stopped)?;
        require(
            persistence::load(&persistence::path())?.geometry == final_geometry,
            "Quit lost final saved geometry",
        )?;
        println!("A3 native mailbox release/pause/resume/quit saved latest geometry");
        return Ok(());
    }
    client.send(Command::Quit);
    wait_snapshot(&client, |s| s.stopped)?;
    Ok(())
}

fn backdrop_lifetime(m: MainThreadMarker, app: &NSApplication) -> Result<()> {
    let anchor = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(m),
            rect(160., 160., 400., 300.),
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        anchor.setReleasedWhenClosed(false);
    }
    anchor.makeKeyAndOrderFront(None);
    pump(app, Duration::from_millis(50));
    let backdrop = crate::backdrop::Backdrop::new(m);
    let frame = crate::window_tracking::frame(anchor.windowNumber()).ok_or("No fixture anchor")?;
    let primary = NSScreen::screens(m)
        .firstObject()
        .ok_or("No display")?
        .frame()
        .size
        .height;
    backdrop.place(anchor.windowNumber() as u32, None, &[frame], primary);
    let number = backdrop.number();
    let visible =
        || crate::window_tracking::stack().is_some_and(|s| s.iter().any(|w| w.number == number));
    pump(app, Duration::from_millis(30));
    require(visible(), "Backdrop was never visible")?;
    let clone = backdrop.clone();
    drop(backdrop);
    pump(app, Duration::from_millis(30));
    require(visible(), "Dropping one clone hid the backdrop")?;
    drop(clone);
    pump(app, Duration::from_millis(80));
    println!("D6 final_owner_visible={}", visible());
    let result = require(!visible(), "D6 backdrop survives final owner destruction");
    anchor.orderOut(None);
    result
}
fn rendered_selection(m: MainThreadMarker, app: &NSApplication) -> Result<()> {
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(m),
            rect(160., 160., 500., 100.),
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    let view = window.contentView().ok_or("No view")?;
    let a = TabButton::new(m, rect(0., 0., 160., 32.), TabIndication::Active, None);
    let b = TabButton::new(m, rect(160., 0., 160., 32.), TabIndication::Inactive, None);
    a.setTitle(&NSString::from_str("A"));
    b.setTitle(&NSString::from_str("B"));
    view.addSubview(&a);
    view.addSubview(&b);
    window.makeKeyAndOrderFront(None);
    let editor = RenameEditor {
        id: 1,
        field: NSTextField::new(m),
    };
    for selected in [2, 1, 2] {
        for (id, button) in [(1, &a), (2, &b)] {
            button.set_indication(tab_indication(id, true, Some(selected), Some(1)));
            require(
                button.ivars().active.get() == (id == selected),
                "Rendered selected tab differs from confirmed selection",
            )?;
        }
        require(editor.id == 1, "Rename target changed with selection")?;
        pump(app, Duration::from_millis(15));
        let bounds = view.bounds();
        let bitmap = view
            .bitmapImageRepForCachingDisplayInRect(bounds)
            .ok_or("No bitmap")?;
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &bitmap);
        // Active underline and inactive surface must differ in the rendered view.
        let scale = bitmap.pixelsWide() as f64 / bounds.size.width;
        let row = bitmap.pixelsHigh() - (scale as isize).max(1);
        let left = bitmap
            .colorAtX_y((80. * scale) as isize, row)
            .ok_or("No left pixel")?;
        let right = bitmap
            .colorAtX_y((240. * scale) as isize, row)
            .ok_or("No right pixel")?;
        require(
            left != right,
            "Rendered tab indicators are indistinguishable",
        )?;
    }
    a.set_indication(tab_indication(1, false, Some(2), Some(1)));
    require(
        a.ivars().action.get() && !a.ivars().active.get(),
        "Disconnected action selection uses live indicator",
    )?;
    window.orderOut(None);
    Ok(())
}

#[derive(Default)]
struct TargetIvars {
    delayed: Cell<bool>,
    pending: Cell<Option<(NSRect, Instant)>>,
}
define_class!(
    #[unsafe(super=NSWindow)] #[thread_kind=MainThreadOnly] #[ivars=TargetIvars]
    struct ReviewWindow;
    unsafe impl NSObjectProtocol for ReviewWindow {}
    impl ReviewWindow {
        #[unsafe(method(setFrame:display:))]
        fn delayed_frame(&self,frame:NSRect,display:bool) {
            if self.ivars().delayed.get() {
                println!("D3 target queued frame {frame:?}");
                let current=self.frame();
                let mut merged=frame;
                if let Some((pending,_))=self.ivars().pending.get() {
                    if frame.size==current.size {merged.size=pending.size;}
                    if frame.origin==current.origin {merged.origin=pending.origin;}
                }
                self.ivars().pending.set(Some((merged,Instant::now()+Duration::from_millis(65))));
            } else {unsafe {let _:()=msg_send![super(self),setFrame:frame,display:display];}}
        }

    }
);
pub(crate) fn targets() -> Result<()> {
    let dir = data_dir()?;
    let m = MainThreadMarker::new().ok_or("Main thread required")?;
    let app = NSApplication::sharedApplication(m);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.finishLaunching();
    let mut windows = vec![];
    for i in 0..3 {
        let w: Retained<ReviewWindow> = unsafe {
            msg_send![super(ReviewWindow::alloc(m).set_ivars(TargetIvars::default())),initWithContentRect:rect(140.+i as f64*60.,180.,800.,500.),styleMask:NSWindowStyleMask::Titled|NSWindowStyleMask::Resizable|NSWindowStyleMask::Miniaturizable,backing:NSBackingStoreType::Buffered,defer:false]
        };
        unsafe {
            w.setReleasedWhenClosed(false);
            w.setContentMinSize(NSSize::new(600., 400.));
        }
        w.setTitle(&NSString::from_str(&format!("Disposable review {i}")));
        w.makeKeyAndOrderFront(None);
        windows.push(w);
    }
    loop {
        pump(&app, Duration::from_millis(5));
        for w in &windows {
            if w.ivars()
                .pending
                .get()
                .is_some_and(|(_, due)| Instant::now() >= due)
            {
                let (frame, _) = w.ivars().pending.take().unwrap();
                w.ivars().delayed.set(false);
                w.setFrame_display(frame, true);
                w.ivars().delayed.set(true);
                std::fs::write(dir.join("delayed-commit"), "ok").map_err(|e| e.to_string())?;
                println!("D3 target committed delayed frame {frame:?}");
            }
        }
        if let Ok(op) = std::fs::read_to_string(dir.join("command")) {
            std::fs::remove_file(dir.join("command")).map_err(|e| e.to_string())?;
            match op.as_str() {
                "delay" => {
                    for w in &windows {
                        w.ivars().delayed.set(true);
                    }
                }
                "immediate" => {
                    for w in &windows {
                        w.ivars().delayed.set(false);
                    }
                }
                "close" => {
                    windows[0].close();
                }
                "exit" => {
                    std::fs::write(dir.join("ack"), "ok").map_err(|e| e.to_string())?;
                    return Ok(());
                }
                _ => return Err("Unknown target command".into()),
            }
            std::fs::write(dir.join("ack"), "ok").map_err(|e| e.to_string())?;
        }
    }
}
