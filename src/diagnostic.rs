//! Read-only discovery, plus an explicitly invoked live docking smoke test.
use crate::{
    engine::Engine,
    macos::{App, MacBackend},
    model::*,
};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSWorkspace};
use objc2_foundation::MainThreadMarker;
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
        b.apps = apps;
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
        let mut e = Engine::new(b, Workspace::default());
        let mut ids = vec![];
        for bundle in ["com.hnc.Discord", "org.telegram.desktop"] {
            let matches: Vec<_> = windows
                .iter()
                .filter(|w| w.identity.bundle == bundle && w.eligible)
                .collect();
            if matches.len() != 1 {
                return Err(format!(
                    "Smoke test requires exactly one eligible window for {bundle}"
                ));
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
                    restoration = Err(format!(
                        "Window {id} restoration readback differs: {actual:?}"
                    ));
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
        backend.apps=vec![App{pid,name:"Disposable AppDock fixture".into(),bundle:"dev.appdock.fixture".into()}];
        let mut found=None;
        for _ in 0..50 {
            if let Ok(windows)=backend.discover() && let Some(w)=windows.into_iter().find(|w|w.eligible) {found=Some(w);break;}
            std::thread::sleep(Duration::from_millis(100));
        }
        let window=found.ok_or("Fixture did not expose an eligible window")?;
        let mut engine=Engine::new(backend,Workspace::default());
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
            if !actual.near(engine.area) || engine.paused.is_some() {return Err(format!("Fixture did not return to docking area: {actual:?}"));}
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
    }).join().map_err(|_|"Movement fixture worker panicked".to_string()).and_then(|r|r);
    // This child was created solely for this test and has no attached windows.
    let _ = child.kill();
    let _ = child.wait();
    outcome
}
