#[cfg(target_os = "macos")]
mod appearance;
#[cfg(target_os = "macos")]
mod backdrop;
#[cfg(target_os = "macos")]
mod diagnostic;
mod engine;
mod error;
#[cfg(target_os = "macos")]
mod macos;
mod model;
mod native_ops;
mod persistence;
#[cfg(target_os = "macos")]
mod picker;
mod schedule;
#[cfg(target_os = "macos")]
mod ui;
#[cfg(target_os = "macos")]
mod window_tracking;
#[cfg(target_os = "macos")]
mod worker;
fn main() {
    #[cfg(target_os = "macos")]
    {
        let arg = std::env::args().nth(1);
        if matches!(
            arg.as_deref(),
            Some("--review-fixture" | "--review-targets")
        ) {
            let result = if arg.as_deref() == Some("--review-targets") {
                ui::fixtures::review_targets()
            } else {
                ui::fixtures::review(&std::env::args().nth(2).unwrap_or_default())
            };
            if let Err(e) = result {
                eprintln!("{e}");
                std::process::exit(if e.kind == model::ErrorKind::Permission {
                    2
                } else {
                    1
                });
            }
            return;
        }
        if matches!(
            arg.as_deref(),
            Some(
                "--movement-fixture"
                    | "--overlay-fixture"
                    | "--overlay-targets"
                    | "--restoration-fixture"
            )
        ) {
            let result = if matches!(
                arg.as_deref(),
                Some("--overlay-fixture" | "--restoration-fixture")
            ) {
                diagnostic::overlay_fixture(arg.as_deref() == Some("--restoration-fixture"))
            } else if arg.as_deref() == Some("--overlay-targets") {
                diagnostic::overlay_targets()
            } else {
                diagnostic::movement_fixture()
            };
            if let Err(e) = result {
                eprintln!("{e}");
                std::process::exit(1);
            }
        } else if matches!(
            arg.as_deref(),
            Some("--diagnose" | "--smoke" | "--diagnose-badges")
        ) {
            let result = if arg.as_deref() == Some("--diagnose-badges") {
                diagnostic::badges()
            } else {
                diagnostic::run(arg.as_deref() == Some("--smoke"))
            };
            if let Err(e) = result {
                eprintln!("{e}");
                std::process::exit(1);
            }
        } else {
            if matches!(
                arg.as_deref(),
                Some(
                    "--ui-smoke"
                        | "--surface-smoke"
                        | "--dock-smoke"
                        | "--picker-smoke"
                        | "--tracking-smoke"
                        | "--disconnected-smoke"
                        | "--rename-smoke"
                        | "--design-smoke"
                        | "--cleanup-smoke"
                        | "--frame-smoke"
                        | "--pointer-smoke"
                )
            ) && std::env::var_os("APPDOCK_DATA_DIR").is_none()
            {
                eprintln!("UI smoke requires isolated APPDOCK_DATA_DIR");
                std::process::exit(1);
            }
            let path = persistence::path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("Cannot create AppDock data directory");
            }
            let lock = std::fs::File::create(path.with_extension("lock"))
                .expect("Cannot open AppDock lock");
            if lock.try_lock().is_err() {
                eprintln!("AppDock is already running for this workspace.");
                std::process::exit(1);
            }
            if matches!(arg.as_deref(), Some("--dock-smoke" | "--picker-smoke"))
                && !persistence::load(&path).is_ok_and(|w| w.tabs.is_empty())
            {
                eprintln!("Docking smoke requires an empty valid workspace");
                std::process::exit(1);
            }
            ui::run();
        }
    }
    #[cfg(not(target_os = "macos"))]
    eprintln!("AppDock currently requires macOS.");
}
