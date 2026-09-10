//! Read-only discovery, plus an explicitly invoked live docking smoke test.
use crate::{
    engine::Engine,
    macos::{App, MacBackend},
    model::*,
};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSWorkspace};
use objc2_foundation::MainThreadMarker;
/// Read only app names and Dock badge metadata for the user's messaging apps.
pub fn badges() -> Result<()> {
    let m = MainThreadMarker::new().ok_or("Main thread required")?;
    let _ = NSApplication::sharedApplication(m);
    let dock = objc2_app_kit::NSRunningApplication::runningApplicationsWithBundleIdentifier(
        &objc2_foundation::NSString::from_str("com.apple.dock"),
    )
    .firstObject()
    .ok_or("Dock unavailable")?
    .processIdentifier();
    let apps: Vec<_> = NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter_map(|app| {
            let name = app.localizedName()?.to_string();
            if !["Discord", "Telegram Lite", "WhatsApp", "Spotify"].contains(&name.as_str()) {
                return None;
            }
            let path = app
                .bundleURL()
                .or_else(|| app.executableURL())?
                .path()?
                .to_string();
            Some((name, path))
        })
        .collect();
    std::thread::spawn(move || {
        let targets = apps
            .iter()
            .enumerate()
            .map(|(i, (_, path))| (i as u64 + 1, path.clone()))
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let badges = crate::macos::read_dock_badges(dock, &targets)?;
        for (i, (name, _)) in apps.iter().enumerate() {
            println!("{name}: Dock badge {:?}", badges.get(&(i as u64 + 1)));
        }
        println!(
            "Dock badge scan completed in {} ms",
            started.elapsed().as_millis()
        );
        Ok(())
    })
    .join()
    .map_err(|_| "Badge diagnostic panicked".to_string())?
}
/// Read-only candidate diagnostics. Omit window titles and message contents.
pub fn windows() -> Result<()> {
    let _ = MainThreadMarker::new().ok_or("Main thread required")?;
    let requested: Vec<_> = std::env::args()
        .skip(2)
        .map(|name| name.to_lowercase())
        .collect();
    if requested.is_empty() {
        return Err("Usage: --diagnose-windows <app name or bundle> [...]".into());
    }
    let apps: Vec<_> = NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
        .filter_map(|app| {
            let name = app.localizedName()?.to_string();
            let bundle = app
                .bundleIdentifier()
                .map(|s| s.to_string())
                .unwrap_or_default();
            requested
                .iter()
                .any(|value| *value == name.to_lowercase() || *value == bundle.to_lowercase())
                .then_some(App {
                    pid: app.processIdentifier(),
                    name,
                    bundle,
                })
        })
        .collect();
    std::thread::spawn(move || -> Result<()> {
        let mut backend = MacBackend::new();
        println!("Accessibility trusted: {}", backend.trusted());
        if !backend.trusted() {
            return Err(BackendError::new(
                ErrorKind::Permission,
                "Accessibility unavailable",
            ));
        }
        for app in apps {
            let name = app.name.clone();
            let pid = app.pid;
            backend.update_apps(vec![app]);
            let windows = backend.discover()?;
            println!(
                "{name} pid={pid}: {} candidate windows, {} eligible",
                windows.len(),
                windows.iter().filter(|w| w.eligible).count()
            );
            for window in windows {
                println!(
                    "window {}: minimized={} eligible={} lifecycle={:?}",
                    window.id,
                    window.minimized,
                    window.eligible,
                    backend.check_window(window.id)
                );
            }
        }
        Ok(())
    })
    .join()
    .map_err(|_| "Window diagnostics panicked")?
}

pub fn run(smoke: bool) -> Result<()> {
    let m = MainThreadMarker::new().ok_or("Main thread required")?;
    let _ = NSApplication::sharedApplication(m);
    let apps: Vec<_> = NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|a| {
            a.activationPolicy() == NSApplicationActivationPolicy::Regular
                && a.processIdentifier() != std::process::id() as i32
        })
        .map(|a| App {
            pid: a.processIdentifier(),
            name: a.localizedName().map(|s| s.to_string()).unwrap_or_default(),
            bundle: a
                .bundleIdentifier()
                .map(|s| s.to_string())
                .unwrap_or_default(),
        })
        .collect();
    std::thread::spawn(move || {
        let mut b = MacBackend::new();
        b.update_apps(apps);
        println!("Accessibility trusted: {}", b.trusted());
        let windows = b.discover()?;
        for bundle in ["com.hnc.Discord", "org.telegram.desktop"] {
            let matches: Vec<_> = windows
                .iter()
                .filter(|w| w.identity.bundle == bundle)
                .collect();
            for w in &matches {
                println!("window {} state: {:?}", w.id, b.state(w.id));
            }
            println!(
                "{bundle}: {} standard windows, {} eligible",
                matches.len(),
                matches.iter().filter(|w| w.eligible).count()
            );
        }
        if !smoke {
            return Ok(());
        }
        let mut e = Engine::new(
            b,
            Workspace {
                keep_apps_open_on_close: false,
                ..Workspace::default()
            },
        );
        let mut ids = vec![];
        for bundle in ["com.hnc.Discord", "org.telegram.desktop"] {
            let matches: Vec<_> = windows
                .iter()
                .filter(|w| w.identity.bundle == bundle && w.eligible)
                .collect();
            if matches.len() != 1 {
                return Err(format!(
                    "Smoke test requires exactly one eligible window for {bundle}"
                )
                .into());
            }
            ids.push(e.attach(None, matches[0])?);
        }
        for (id, a) in &e.live {
            println!("Original tab {id}: {:?}", a.original);
        }
        let originals: Vec<_> = e.live.values().map(|a| (a.window, a.original)).collect();
        let exercise = (|| {
            for cycle in 0..6 {
                for &id in &ids {
                    e.switch(id)?;
                    let a = &e.live[&id];
                    if e.backend.state(a.window)?.minimized {
                        return Err("Selected window stayed minimized".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                println!("Switch cycle {} passed", cycle + 1);
            }
            e.resize(Rect {
                x: 160.,
                y: 180.,
                width: 1100.,
                height: 700.,
            })?;
            Ok(())
        })();
        let mut restoration = e.restore_all();
        for _ in 0..5 {
            if restoration.is_ok() {
                break;
            }
            eprintln!("Restore retry: {restoration:?}");
            std::thread::sleep(std::time::Duration::from_secs(1));
            restoration = e.restore_all();
        }
        if restoration.is_ok() {
            for (id, original) in originals {
                let actual = e.backend.state(id)?;
                if !actual.frame.near(original.frame) || actual.minimized != original.minimized {
                    restoration =
                        Err(format!("Window {id} restoration readback differs: {actual:?}").into());
                }
            }
        }
        println!("Restoration readback: {restoration:?}");
        exercise.and(restoration)
    })
    .join()
    .map_err(|_| "Diagnostic worker panicked".to_string())?
}

/// Exercise a disposable AppDock window, never a user's managed application.
pub fn movement_fixture() -> Result<()> {
    use std::{
        process::{Command, Stdio},
        time::Duration,
    };
    let m = MainThreadMarker::new().ok_or("Main thread required")?;
    let _ = NSApplication::sharedApplication(m);
    let dir = std::env::temp_dir().join(format!("appdock-movement-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--ui-smoke")
        .env("APPDOCK_DATA_DIR", &dir)
        .env("APPDOCK_SMOKE_TICKS", "400")
        .env("APPDOCK_SMOKE_SCREENSHOT", dir.join("fixture.png"))
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let pid = child.id() as i32;
    let outcome=std::thread::spawn(move || {
        let mut backend=MacBackend::new();
        backend.update_apps(vec![App{pid,name:"Disposable AppDock fixture".into(),bundle:"dev.appdock.fixture".into()}]);
        let mut found=None;
        for _ in 0..50 {
            if let Ok(windows)=backend.discover() && let Some(w)=windows.into_iter().find(|w|w.eligible) {found=Some(w);break;}
            std::thread::sleep(Duration::from_millis(100));
        }
        let window=found.ok_or("Fixture did not expose an eligible window")?;
        let mut engine=Engine::new(backend,Workspace {keep_apps_open_on_close:false,..Workspace::default()});
        let id=engine.attach(None,&window)?;
        let exercise=(|| {
            engine.switch(id)?;
            engine.pointer_down=true;
            let moved=engine.backend.set_frame(window.id,Rect{x:520.,y:240.,..engine.area})?;
            engine.observe();
            if !engine.backend.state(window.id)?.frame.near(moved) { return Err("Moved window changed before mouse release".into()); }
            let rediscovered=engine.backend.discover()?.into_iter().find(|w|engine.backend.same_window(w.id,window.id)).ok_or("Movement lost window identity")?;
            if engine.attach(None,&rediscovered).is_ok() || engine.workspace.tabs.len()!=1 { return Err("Movement permitted duplicate attachment".into()); }
            engine.pointer_down=false;
            engine.observe();
            let actual=engine.backend.state(window.id)?.frame;
            if !actual.near(engine.area) || engine.paused.is_some() {return Err(format!("Fixture did not return to docking area: {actual:?}").into());}
            let started=std::time::Instant::now();
            for step in 0..20 {engine.follow_workspace(Rect{x:180.+step as f64*3.,..engine.area})?;}
            let elapsed=started.elapsed();
            let mut aligned=false;
            for _ in 0..20 {
                if engine.backend.state(window.id)?.frame.near(engine.area){aligned=true;break;}
                std::thread::sleep(Duration::from_millis(10));
            }
            if !aligned{return Err("Position-only follow did not reach the final frame".into());}
            println!("Native position-only follow: 20 updates in {:.1} ms ({:.2} ms/update), final frame verified",elapsed.as_secs_f64()*1000.,elapsed.as_secs_f64()*50.);
            println!("Native movement regression passed: drag retained ownership, duplicate rejected, window returned");
            Ok(())
        })();
        let original=engine.live[&id].original;
        let restored=engine.release(id).and_then(|_| {
            let state=engine.backend.state(window.id)?;
            if state.frame.near(original.frame) && state.minimized==original.minimized { Ok(()) } else { Err("Fixture restoration readback failed".into()) }
        });
        println!("Fixture restoration: {restored:?}");
        exercise.and(restored)
    }).join().map_err(|_|BackendError::from("Movement fixture worker panicked")).and_then(|r|r);
    // This child was created solely for this test and has no attached windows.
    let _ = child.kill();
    let _ = child.wait();
    outcome
}

/// Two disposable same-process, same-title windows exercise the real AX backend
/// and main-thread cover. No existing user windows are discovered or attached.
pub fn overlay_targets() -> Result<()> {
    use objc2::MainThreadOnly;
    use objc2_app_kit::*;
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    let m = MainThreadMarker::new().ok_or("Main thread required")?;
    let app = NSApplication::sharedApplication(m);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let mut windows = Vec::new();
    for offset in [0., 80.] {
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(m),
                NSRect::new(NSPoint::new(140. + offset, 180.), NSSize::new(800., 500.)),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
            window.setContentMinSize(NSSize::new(600., 400.));
        }
        window.setTitle(&NSString::from_str("Disposable duplicate title"));
        window.makeKeyAndOrderFront(None);
        windows.push(window);
    }
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    app.run();
    drop(windows);
    Ok(())
}

pub fn overlay_fixture(constrained_restore: bool) -> Result<()> {
    use objc2::MainThreadOnly;
    use objc2_app_kit::*;
    use objc2_foundation::{NSDate, NSPoint, NSRect, NSSize, NSString};
    use std::{
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };
    let m = MainThreadMarker::new().ok_or("Main thread required")?;
    let app = NSApplication::sharedApplication(m);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.finishLaunching();
    let controls = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(m),
            NSRect::new(NSPoint::new(120., 760.), NSSize::new(500., 64.)),
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        controls.setReleasedWhenClosed(false);
    }
    controls.setTitle(&NSString::from_str("Disposable AppDock controls"));
    let backdrop = crate::backdrop::Backdrop::new(m);
    let primary = NSScreen::screens(m)
        .firstObject()
        .ok_or("No screen")?
        .frame()
        .size
        .height;
    // AX calls to another process are dispatched to its main thread. Calling
    // our own AppKit AX setters from the backend thread would violate AppKit.
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--overlay-targets")
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let pid = child.id() as i32;
    let (tx, rx) = mpsc::channel::<(u32, Vec<Rect>, Option<u32>)>();
    let (ack_tx, ack_rx) = mpsc::channel::<Result<()>>();
    let worker = std::thread::spawn(move || -> Result<()> {
        let mut backend = MacBackend::new();
        backend.update_apps(vec![App {
            pid,
            name: "Fixture".into(),
            bundle: "dev.appdock.fixture".into(),
        }]);
        let mut targets = Vec::new();
        for _ in 0..50 {
            targets = backend.discover()?;
            if targets.len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if targets.len() != 2 || targets.iter().any(|w| !w.eligible) {
            return Err(format!(
                "Expected two eligible disposable windows, found {}",
                targets.len()
            )
            .into());
        }
        let mut engine = Engine::new(
            backend,
            Workspace {
                keep_apps_open_on_close: false,
                ..Workspace::default()
            },
        );
        let mut ids = Vec::new();
        for target in &targets {
            ids.push(engine.attach(None, target)?);
        }
        let originals: Vec<_> = engine
            .live
            .values()
            .map(|a| (a.window, a.original))
            .collect();
        let exercise = (|| {
            for cycle in 0..12 {
                let id = ids[cycle % 2];
                engine.switch(id)?;
                if cycle == 5 {
                    engine.follow_workspace(Rect {
                        x: 340.,
                        ..engine.area
                    })?;
                }
                if cycle == 7 {
                    engine.resize(Rect {
                        width: 1100.,
                        height: 680.,
                        ..engine.area
                    })?;
                }
                let selected = engine
                    .backend
                    .window_number(engine.live[&id].window)
                    .ok_or("Missing selected stacking anchor")?;
                let other = engine
                    .backend
                    .window_number(engine.live[&ids[(cycle + 1) % 2]].window);
                let frames = engine
                    .live
                    .values()
                    .filter(|a| a.docked)
                    .map(|a| a.expected.frame)
                    .collect();
                tx.send((selected, frames, other))
                    .map_err(|e| e.to_string())?;
                ack_rx
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|e| e.to_string())??;
                for a in engine.live.values() {
                    if engine.backend.state(a.window)?.minimized {
                        return Err("Switch minimized a fixture window".into());
                    }
                }
            }
            Ok(())
        })();
        if constrained_restore {
            // Deliberately request a saved frame the native app cannot accept.
            for a in engine.live.values_mut() {
                a.original.frame.width = 100.;
                a.original.frame.height = 100.;
            }
        }
        let restored = engine.restore_all().and_then(|_| {
            if !engine.workspace.tabs.is_empty()
                || !engine.live.is_empty()
                || !engine.released.is_empty()
            {
                return Err("Restoration left stale tabs or recovery snapshots".into());
            }
            if constrained_restore {
                if engine.restoration_notes.len() != originals.len() {
                    return Err("Expected nonblocking notes for native geometry constraints".into());
                }
                for (id, before) in &originals {
                    engine.backend.restore(*id, *before)?;
                }
            }
            for (id, before) in originals {
                let after = engine.backend.state(id)?;
                if !after.frame.near(before.frame) || after.minimized != before.minimized {
                    return Err("Overlay fixture original-state restoration failed".into());
                }
            }
            Ok(())
        });
        exercise.and(restored)
    });
    let pump = |seconds| {
        let until = NSDate::dateWithTimeIntervalSinceNow(seconds);
        while until.timeIntervalSinceNow() > 0. {
            if let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                unsafe { objc2_foundation::NSDefaultRunLoopMode },
                true,
            ) {
                app.sendEvent(&event);
            }
        }
    };
    let mut presentations = 0;
    while !worker.is_finished() {
        pump(0.01);
        if let Ok((selected, frames, other)) = rx.try_recv() {
            backdrop.place(selected, None, &frames, primary);
            presentations += 1;
            let test_controls = presentations == 4 || presentations == 10;
            if test_controls {
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
                controls.makeKeyAndOrderFront(None);
                pump(0.1);
                // Same callback-safe shared handle as the UI's key-window delegate.
                backdrop.clone().keep_below_selected();
            }
            pump(0.05);
            let result = (|| {
                let stack = crate::window_tracking::stack().ok_or("No WindowServer stack")?;
                if test_controls && !controls.isKeyWindow() {
                    return Err("Backdrop stole keyboard focus from the controls".into());
                }
                let position = |id| {
                    stack
                        .iter()
                        .position(|w| w.number == id)
                        .ok_or_else(|| format!("Window {id} missing from stack (selected {selected}, cover {}, other {other:?})", backdrop.number()))
                };
                let active = position(selected)?;
                let cover = position(backdrop.number())?;
                if active >= cover || other.is_some_and(|id| position(id).is_ok_and(|p| p <= cover))
                {
                    return Err(format!(
                        "Incorrect overlay order: selected {selected} at {active}, cover {} at {cover}, other {other:?}",
                        backdrop.number()
                    ).into());
                }
                Ok(())
            })();
            let _ = ack_tx.send(result);
        }
    }
    backdrop.hide();
    controls.orderOut(None);
    let _ = child.kill();
    let _ = child.wait();
    worker
        .join()
        .map_err(|_| "Overlay fixture worker panicked")??;
    println!(
        "Native overlay fixture passed: 12 same-app duplicate-title switches, active above opaque cover above inactive, controls keep keyboard focus, no minimization, movement/resizing and original-state restoration"
    );
    if constrained_restore {
        println!(
            "Native constrained-restoration regression passed: unavailable original geometry did not block close; tabs and recovery state cleared"
        );
    }
    Ok(())
}
