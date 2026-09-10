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
        Err(e) => return Err(e.to_string()),
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
    fs::rename(temporary, path).map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_version_is_not_overwritten() {
        let p = std::env::temp_dir().join(format!("appdock-version-{}.json", std::process::id()));
        fs::write(&p, b"{\"version\":999}").unwrap();
        assert!(load(&p).is_err());
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
}
