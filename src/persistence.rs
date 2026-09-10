use crate::model::*;
use std::{
    fs,
    path::{Path, PathBuf},
};
pub fn path() -> PathBuf {
    std::env::var_os("APPDOCK_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME required"))
                .join("Library/Application Support/AppDock")
        })
        .join("workspace.json")
}
pub fn load(path: &Path) -> Result<Workspace> {
    let data = match fs::read(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Workspace::default()),
        Err(e) => return Err(e.to_string().into()),
    };
    let value: serde_json::Value = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    if value["version"].as_u64() != Some(1) {
        return Err("Unsupported workspace version; original file has been preserved.".into());
    }
    let workspace: Workspace = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if workspace.tabs.iter().any(|t| t.id > isize::MAX as u64)
        || !workspace.geometry.valid()
        || workspace
            .tabs
            .iter()
            .enumerate()
            .any(|(i, t)| workspace.tabs[..i].iter().any(|p| p.id == t.id))
    {
        return Err("Invalid workspace geometry or duplicate tab IDs; file preserved.".into());
    }
    if workspace.startup_apps.iter().enumerate().any(|(i, app)| {
        app.bundle.trim().is_empty()
            || app.bundle != app.bundle.trim()
            || workspace.startup_apps[..i]
                .iter()
                .any(|other| other.bundle == app.bundle)
    }) {
        return Err("Invalid or duplicate startup app rules; file preserved.".into());
    }
    Ok(workspace)
}
/// Each launch is a new attachment session. Retain preferences, never live tabs.
pub fn load_for_launch(path: &Path) -> Result<Workspace> {
    let mut workspace = load(path)?;
    if !workspace.tabs.is_empty() {
        workspace.tabs.clear();
        save(path, &workspace)?;
    }
    Ok(workspace)
}
pub fn save(path: &Path, workspace: &Workspace) -> Result<()> {
    let parent = path.parent().ok_or("Missing parent directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(workspace).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut file = fs::File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(temporary, path).map_err(|e| e.to_string().into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_version_is_not_overwritten() {
        let p = std::env::temp_dir().join(format!("appdock-version-{}.json", std::process::id()));
        fs::write(&p, b"{\"version\":999}").unwrap();
        assert!(load(&p).is_err());
        assert!(load_for_launch(&p).is_err());
        assert_eq!(fs::read(&p).unwrap(), b"{\"version\":999}");
        fs::remove_file(p).unwrap();
    }
    #[test]
    fn labels_order_and_shortcuts_round_trip() {
        let p = std::env::temp_dir().join(format!("appdock-roundtrip-{}.json", std::process::id()));
        let w = Workspace {
            tabs: vec![
                SavedTab {
                    id: 7,
                    name: "Discord - username 1".into(),
                    identity: Identity {
                        bundle: "discord".into(),
                        identifier: None,
                    },
                },
                SavedTab {
                    id: 2,
                    name: "Telegram".into(),
                    identity: Identity {
                        bundle: "telegram".into(),
                        identifier: None,
                    },
                },
            ],
            ..Workspace::default()
        };
        save(&p, &w).unwrap();
        let r = load(&p).unwrap();
        assert_eq!(r.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), vec![7, 2]);
        assert_eq!(r.tabs[0].name, w.tabs[0].name);
        fs::remove_file(p).unwrap();
    }
    #[test]
    fn fresh_launch_clears_even_reconnectable_tabs_and_preserves_preferences() {
        let p =
            std::env::temp_dir().join(format!("appdock-fresh-launch-{}.json", std::process::id()));
        let mut saved = Workspace::default();
        saved.geometry.x = 460.;
        saved.next_shortcut = "Control+Super+ArrowRight".into();
        saved.tabs.push(SavedTab {
            id: 3,
            name: "Terminal".into(),
            identity: Identity {
                bundle: "com.apple.Terminal".into(),
                identifier: Some("stable-window".into()),
            },
        });
        save(&p, &saved).unwrap();
        let fresh = load_for_launch(&p).unwrap();
        assert!(fresh.tabs.is_empty());
        assert_eq!(fresh.geometry, saved.geometry);
        assert_eq!(fresh.next_shortcut, saved.next_shortcut);
        assert!(load(&p).unwrap().tabs.is_empty());
        assert!(load_for_launch(&p).unwrap().tabs.is_empty());
        fs::remove_file(p).unwrap();
    }
    #[test]
    fn startup_rules_survive_release_and_fresh_launch_with_legacy_json_compatibility() {
        let p = std::env::temp_dir().join(format!("appdock-startup-{}.json", std::process::id()));
        let legacy = serde_json::to_value(Workspace::default()).unwrap();
        assert!(legacy.get("startup_apps").is_none());
        fs::write(&p, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let mut w = load_for_launch(&p).unwrap();
        assert!(w.startup_apps.is_empty());
        let rule = StartupApp {
            bundle: "com.example.app".into(),
            name: "Example".into(),
        };
        w.set_startup_app(rule.clone(), true);
        w.set_startup_app(rule.clone(), true);
        assert_eq!(w.startup_apps.len(), 1);
        w.tabs.push(SavedTab {
            id: 1,
            name: "Example".into(),
            identity: Identity {
                bundle: rule.bundle.clone(),
                identifier: None,
            },
        });
        save(&p, &w).unwrap();
        let fresh = load_for_launch(&p).unwrap();
        assert!(fresh.tabs.is_empty());
        assert_eq!(fresh.startup_apps, vec![rule.clone()]);
        w.set_startup_app(rule, false);
        save(&p, &w).unwrap();
        assert!(load_for_launch(&p).unwrap().startup_apps.is_empty());
        fs::remove_file(p).unwrap();
    }
}
