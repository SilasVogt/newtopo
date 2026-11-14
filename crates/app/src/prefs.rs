//! Per-user preferences (SPEC §5.7) and where they live on disk.

use crate::keymap::Keymap;
use newtopo_render::{OverlayColors, UpAxis};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const RADIAL_SLOTS: usize = 12;
pub const MAX_RECENT: usize = 8;

/// Config/data directories. `None` in screenshot/test mode so nothing on
/// the user's disk is read or written.
#[derive(Clone, Debug, Default)]
pub struct Paths {
    pub config: Option<PathBuf>,
    pub data: Option<PathBuf>,
}

impl Paths {
    pub fn user() -> Paths {
        // Override for tests and sandboxes: keep everything in one folder.
        if let Some(dir) = std::env::var_os("NEWTOPO_CONFIG_DIR") {
            let dir = PathBuf::from(dir);
            return Paths {
                data: Some(dir.join("data")),
                config: Some(dir),
            };
        }
        match directories::ProjectDirs::from("", "", "newtopo") {
            Some(d) => Paths {
                config: Some(d.config_dir().to_path_buf()),
                data: Some(d.data_dir().to_path_buf()),
            },
            None => Paths::default(),
        }
    }
    pub fn file(&self, name: &str) -> Option<PathBuf> {
        self.config.as_ref().map(|c| c.join(name))
    }
    pub fn themes_dir(&self) -> Option<PathBuf> {
        self.config.as_ref().map(|c| c.join("themes"))
    }
    /// Autosave for scenes that were never saved.
    pub fn untitled_autosave(&self) -> Option<PathBuf> {
        self.data
            .as_ref()
            .map(|d| d.join("untitled.ntopo.autosave"))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavPreset {
    #[default]
    Blender,
    Maya,
    Max,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatsShow {
    Totals,
    Selection,
    #[default]
    Both,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RadialStyle {
    #[default]
    FollowTheme,
    HighContrast,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsPrefs {
    pub show: StatsShow,
    pub verts: bool,
    pub edges: bool,
    pub faces: bool,
    pub tris: bool,
    pub quads: bool,
    pub ngons: bool,
    pub dormant: bool,
}

impl Default for StatsPrefs {
    fn default() -> Self {
        StatsPrefs {
            show: StatsShow::Both,
            verts: true,
            edges: true,
            faces: true,
            tris: true,
            quads: true,
            ngons: true,
            dormant: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportPrefs {
    pub heal: bool,
    pub sewing_tolerance: f64,
    pub units: String,
}

impl Default for ImportPrefs {
    fn default() -> Self {
        ImportPrefs {
            heal: true,
            sewing_tolerance: 1e-3,
            units: "mm".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BridgePrefs {
    pub token: String,
    pub ports: [u16; 3],
    pub keep_both: bool,
}

impl Default for BridgePrefs {
    fn default() -> Self {
        BridgePrefs {
            token: String::new(),
            ports: [47110, 47111, 47112],
            keep_both: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub nav: NavPreset,
    pub units: String,
    pub up: UpAxis,
    pub autosave_minutes: u32,
    pub theme: String,
    pub radial_style: RadialStyle,
    /// `None` = use the theme's overlay colours.
    pub overlay: Option<OverlayColors>,
    pub ui_scale: f32,
    pub stats: StatsPrefs,
    pub favourites: Vec<String>,
    pub radial: Vec<Option<String>>,
    pub recent: Vec<PathBuf>,
    pub import: ImportPrefs,
    pub export: newtopo_core::export::ExportOptions,
    pub export_format: newtopo_core::export::Format,
    pub selection_undo: bool,
    pub select_fill_edges: bool,
    pub bridge: BridgePrefs,
    pub show_symmetry_planes: bool,
    pub show_grid: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        let mut radial: Vec<Option<String>> = [
            "tool.connect",
            "tool.grid",
            "tool.slice",
            "tool.iso",
            "tool.cut",
            "tool.weld",
            "edit.collapse",
            "edit.remove_edge",
            "select.loop",
            "select.ring",
            "view.preview",
            "toggle.autofill",
        ]
        .iter()
        .map(|s| Some(s.to_string()))
        .collect();
        radial.resize(RADIAL_SLOTS, None);
        Prefs {
            nav: NavPreset::Blender,
            units: "mm".into(),
            up: UpAxis::Z,
            autosave_minutes: 5,
            theme: crate::theme::Theme::builtin_dark().name,
            radial_style: RadialStyle::FollowTheme,
            overlay: None,
            ui_scale: 1.0,
            stats: StatsPrefs::default(),
            favourites: vec![
                "file.import".into(),
                "file.export".into(),
                "view.preview".into(),
            ],
            radial,
            recent: Vec::new(),
            import: ImportPrefs::default(),
            export: Default::default(),
            export_format: newtopo_core::export::Format::Fbx,
            selection_undo: true,
            select_fill_edges: false,
            bridge: BridgePrefs::default(),
            show_symmetry_planes: true,
            show_grid: true,
        }
    }
}

pub fn load_json<T: for<'de> Deserialize<'de> + Default>(path: Option<&Path>) -> T {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| match serde_json::from_str(&s) {
            Ok(v) => Some(v),
            Err(e) => {
                log::warn!("ignoring unreadable settings file: {e}");
                None
            }
        })
        .unwrap_or_default()
}

pub fn save_json<T: Serialize>(path: Option<&Path>, value: &T) {
    let Some(path) = path else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(value) {
        Ok(s) => {
            // Write to a temp file and rename so a crash never leaves half a file.
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
        Err(e) => log::warn!("could not serialise settings: {e}"),
    }
}

impl Prefs {
    pub fn load(paths: &Paths) -> Prefs {
        let mut p: Prefs = load_json(paths.file("prefs.json").as_deref());
        p.radial.resize(RADIAL_SLOTS, None);
        if p.bridge.token.is_empty() {
            p.bridge.token = random_token();
        }
        p
    }

    pub fn save(&self, paths: &Paths) {
        save_json(paths.file("prefs.json").as_deref(), self);
    }

    pub fn push_recent(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }
}

pub fn load_keymap(paths: &Paths) -> Keymap {
    load_json(paths.file("keymap.json").as_deref())
}

pub fn save_keymap(paths: &Paths, k: &Keymap) {
    save_json(paths.file("keymap.json").as_deref(), k);
}

/// A per-install random token for the DCC bridge (SPEC §9.3). Uses the
/// standard library's randomly seeded hasher as the entropy source.
pub fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: Some(dir.path().to_path_buf()),
            data: None,
        };
        let mut p = Prefs::load(&paths);
        assert_eq!(p.radial.len(), RADIAL_SLOTS);
        assert_eq!(p.nav, NavPreset::Blender);
        assert_eq!(p.bridge.token.len(), 32);
        p.push_recent(Path::new("/a.ntopo"));
        p.push_recent(Path::new("/b.ntopo"));
        p.push_recent(Path::new("/a.ntopo"));
        assert_eq!(
            p.recent,
            vec![PathBuf::from("/a.ntopo"), PathBuf::from("/b.ntopo")]
        );
        p.save(&paths);
        let q = Prefs::load(&paths);
        assert_eq!(q, p);
    }

    #[test]
    fn recent_is_capped() {
        let mut p = Prefs::default();
        for i in 0..20 {
            p.push_recent(Path::new(&format!("/s{i}.ntopo")));
        }
        assert_eq!(p.recent.len(), MAX_RECENT);
    }
}
