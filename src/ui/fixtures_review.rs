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
            "Minimized",
            "Startup",
            "Animations",
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
    if case == "Animations" {
        return animation_case(backend, &targets);
    }
    if case == "Minimized" {
        for target in &targets {
            backend.minimize(target.id, true)?;
        }
        backend.probe_minimized_fixture(pid)?;
        // Simulate launching AppDock after minimization: no cached handles.
        let mut backend = MacBackend::new();
        backend.update_apps(vec![App {
            pid,
            name: "Review fixture".into(),
            bundle: "dev.appdock.review".into(),
        }]);
        let minimized = backend.discover()?;
        println!(
            "Minimized discovery: {} windows, {} eligible",
            minimized.len(),
            minimized.iter().filter(|w| w.eligible).count()
        );
        require(
            minimized.len() == targets.len() && minimized.iter().all(|w| w.eligible),
            "Minimized windows disappeared from eligible picker results",
        )?;
        let mut engine = Engine::new(backend, Workspace::default());
        let window = &minimized[0];
        let id = engine.attach(None, window)?;
        require(
            engine.live[&id].original.minimized,
            "Original minimized state was not captured",
        )?;
        engine.switch(id)?;
        require(
            !engine.backend.state(window.id)?.minimized,
            "Added window stayed minimized",
        )?;
        engine.release(id)?;
        require(
            engine.backend.state(window.id)?.minimized,
            "Release lost original minimized state",
        )?;
        return Ok(());
    }
    if case == "Startup" {
        return startup_case(pid, dir, backend);
    }
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

fn animation_case(mut backend: MacBackend, targets: &[WindowInfo]) -> Result<()> {
    for target in targets {
        backend.minimize(target.id, true)?;
    }
    let mut baseline = Engine::new(
        backend,
        Workspace {
            keep_apps_open_on_close: false,
            ..Workspace::default()
        },
    );
    let start = Instant::now();
    for target in targets {
        let id = baseline.attach(None, target)?;
        baseline.switch(id)?;
    }
    let sequential_startup = start.elapsed();
    let start = Instant::now();
    baseline.restore_all()?;
    let original_close = start.elapsed();
    require(
        targets
            .iter()
            .all(|w| baseline.backend.state(w.id).is_ok_and(|s| s.minimized)),
        "Baseline did not restore minimization",
    )?;
    let mut quiet = Engine::new(
        baseline.backend,
        Workspace {
            keep_apps_open_on_close: true,
            ..Workspace::default()
        },
    );
    let start = Instant::now();
    let ids = targets
        .iter()
        .map(|target| quiet.attach_startup(target, || true))
        .collect::<Result<Vec<_>>>()?;
    let quiet_startup = start.elapsed();
    require(
        quiet.live.len() == targets.len() && quiet.selected == Some(ids[0]),
        "Quiet startup did not register all tabs",
    )?;
    for (index, target) in targets.iter().enumerate() {
        require(
            quiet.backend.state(target.id)?.minimized == (index != 0),
            "Startup restored an inactive window",
        )?;
    }
    // Opening a different tab is an explicit user action; only then restore it.
    for id in ids.iter().skip(1) {
        quiet.switch(*id)?;
    }
    let originals = quiet
        .live
        .values()
        .map(|a| (a.window, a.original.frame))
        .collect::<Vec<_>>();
    let start = Instant::now();
    quiet.restore_all()?;
    let keep_open_close = start.elapsed();
    for (window, frame) in &originals {
        let state = quiet.backend.state(*window)?;
        require(
            !state.minimized && state.frame.near(*frame),
            "Keep-open close did not preserve visibility and restore geometry",
        )?;
    }
    let mut reopened = Engine::new(
        quiet.backend,
        Workspace {
            keep_apps_open_on_close: true,
            ..Workspace::default()
        },
    );
    let start = Instant::now();
    for target in targets {
        reopened.attach_startup(target, || true)?;
    }
    let quiet_reopen = start.elapsed();
    require(
        targets
            .iter()
            .all(|w| reopened.backend.state(w.id).is_ok_and(|s| !s.minimized)),
        "Reopen unexpectedly minimized an app",
    )?;
    reopened.restore_all()?;
    println!(
        "Three-window timing, ms: sequential_startup={} original_close={} quiet_startup={} keep_open_close={} quiet_reopen={}",
        sequential_startup.as_millis(),
        original_close.as_millis(),
        quiet_startup.as_millis(),
        keep_open_close.as_millis(),
        quiet_reopen.as_millis()
    );
    println!(
        "All tabs registered; only selected startup window restored; keep-open close preserved all visible windows and their original geometry"
    );
    Ok(())
}

fn startup_case(pid: i32, dir: &Path, mut backend: MacBackend) -> Result<()> {
    send_target(dir, "single")?;
    let windows = backend.discover()?;
    require(windows.len() == 1, "Startup fixture needs one window")?;
    let window = windows[0].id;
    backend.minimize(window, true)?;
    let rule = StartupApp {
        bundle: "dev.appdock.review".into(),
        name: "Review fixture".into(),
    };
    let area = Rect {
        x: 380.,
        y: 220.,
        width: 1100.,
        height: 720.,
    };
    let mut saved = Workspace {
        geometry: area,
        keep_apps_open_on_close: false, // Verify opt-out still restores minimization.
        ..Workspace::default()
    };
    saved.set_startup_app(rule.clone(), true);
    persistence::save(&persistence::path(), &saved)?;
    let launch = |workspace: Workspace| {
        let client = worker::start(workspace);
        client.send(Command::Resize(area, area));
        client.send(Command::Apps(vec![App {
            pid,
            name: "Review fixture".into(),
            bundle: "dev.appdock.review".into(),
        }]));
        client.send(Command::Discover);
        client
    };
    let client = launch(persistence::load_for_launch(&persistence::path())?);
    let attached = wait_snapshot(&client, |s| s.live.len() == 1 && s.selected.is_some())?;
    require(
        !backend.state(window)?.minimized,
        "Startup did not restore minimized window",
    )?;
    client.send(Command::Release(attached.live[0].0));
    wait_snapshot(&client, |s| s.live.is_empty() && s.restore_pending == 0)?;
    require(
        backend.state(window)?.minimized,
        "Startup release did not restore original minimization",
    )?;
    client.send(Command::Discover);
    std::thread::sleep(Duration::from_millis(200));
    require(
        client.snapshot.lock().unwrap().live.is_empty(),
        "Periodic discovery reattached a released startup window",
    )?;
    client.send(Command::Quit);
    wait_snapshot(&client, |s| s.stopped)?;
    let next = persistence::load_for_launch(&persistence::path())?;
    require(
        next.startup_apps == vec![rule.clone()],
        "Release/quit lost startup preference",
    )?;
    let client = launch(next);
    wait_snapshot(&client, |s| s.live.len() == 1 && s.selected.is_some())?;
    let before_settings = backend.state(window)?;
    client.send(Command::ResetStartupApps);
    let reset = wait_snapshot(&client, |s| s.workspace.startup_apps.is_empty())?;
    require(
        reset.live.len() == 1 && reset.selected.is_some(),
        "Reset released a current attachment",
    )?;
    require(
        backend.state(window)? == before_settings,
        "Reset changed a docked window",
    )?;
    require(
        persistence::load(&persistence::path())?
            .startup_apps
            .is_empty(),
        "Reset was not saved",
    )?;
    client.send(Command::SaveStartupApps);
    let saved = wait_snapshot(&client, |s| s.workspace.startup_apps == vec![rule.clone()])?;
    require(
        saved.live.len() == 1 && backend.state(window)? == before_settings,
        "Saving app choices changed a docked window",
    )?;
    require(
        persistence::load(&persistence::path())?.startup_apps == vec![rule.clone()],
        "Current apps were not saved",
    )?;
    println!(
        "Save Current Apps and Reset Saved App Choices persisted without changing live windows"
    );
    client.send(Command::SetStartupApp(rule, false));
    wait_snapshot(&client, |s| s.workspace.startup_apps.is_empty())?;
    client.send(Command::Quit);
    wait_snapshot(&client, |s| s.stopped)?;
    let client = launch(persistence::load_for_launch(&persistence::path())?);
    wait_snapshot(&client, |s| s.windows.len() == 1 && s.trusted)?;
    require(
        client.snapshot.lock().unwrap().live.is_empty(),
        "Disabled startup rule still attached a window",
    )?;
    client.send(Command::Quit);
    wait_snapshot(&client, |s| s.stopped)?;
    require(
        backend.state(window)?.minimized,
        "Final startup fixture restoration changed original state",
    )?;
    println!(
        "Startup config passed: restored minimized app on two launches, retained preference through release/quit, skipped rediscovery, and honored disabling"
    );
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
        let row = bitmap.pixelsHigh() - ((4. * scale) as isize).max(1);
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
                "single" => {
                    windows[1].close();
                    windows[2].close();
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
