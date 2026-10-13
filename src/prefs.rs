//! Preferences, kept in a JSON file in the user's config folder
//! (~/Library/Application Support on macOS, ~/.config on Linux), so muting,
//! filters, the window's place and the rest survive relaunches.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub sound_enabled: bool,
    pub sound_needs_help: bool,
    pub sound_finished: bool,
    pub notify_enabled: bool,
    pub notify_needs_help: bool,
    pub notify_finished: bool,
    pub filter: String,
    pub session: Option<String>,
    pub columns: usize,
    pub rows: usize,
    /// The chat panel's height under the rooms, as its edge was last dragged.
    pub panel_height: f32,
    pub on_top: bool,
    /// The window's top-right corner, in screen points.
    pub top_right: Option<(f32, f32)>,
    pub create_kind: String,
    pub create_folder: String,
    /// Open Herdr in a terminal when the app starts and its server is down.
    pub open_herdr_at_start: bool,
    /// Text size in the panels, as a factor (1.0 = normal).
    pub text_scale: f32,
    /// Text size of the console (the agent's terminal), its own factor.
    #[serde(default = "one")]
    pub console_scale: f32,
    /// Kiro's monthly credits (its plan), for the HP viewer; 0 until picked.
    pub kiro_plan: u32,
    /// Claude's HP live from Anthropic with Claude Code's OAuth token; off,
    /// only the last /usage Claude Code cached in ~/.claude.json.
    pub claude_usage_live: bool,
    /// How tall full screen draws its rooms: "large", "medium" or "small".
    pub room_size: String,
    /// Full screen's guild HP shows each harness's bars; closed, one line.
    pub guild_hp_open: bool,
    /// Folders starred in the summon panel, offered first.
    pub favorite_folders: Vec<String>,
    /// Optional choices for a repo's emblem, by its key (`owner/repo`, or the
    /// folder's name): `{"someone/app": {"name": "App", "color": "yellow",
    /// "symbol": "quill", "kind": "game"}}`. Every repo gets one without it.
    pub projects: std::collections::BTreeMap<String, crate::identity::ProjectOverride>,
    /// Each repo's emblem as it was first worked out, by key, so it does not
    /// change between launches.
    pub emblems: std::collections::BTreeMap<String, crate::identity::SavedEmblem>,
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            sound_enabled: true,
            sound_needs_help: true,
            sound_finished: true,
            notify_enabled: true,
            notify_needs_help: true,
            notify_finished: false,
            filter: "all".into(),
            session: None,
            columns: 2,
            rows: 4,
            panel_height: crate::ui::PANEL_HEIGHT,
            on_top: true,
            top_right: None,
            create_kind: "claude".into(),
            create_folder: String::new(),
            open_herdr_at_start: false,
            text_scale: 1.0,
            console_scale: 1.0,
            kiro_plan: 0,
            claude_usage_live: false,
            guild_hp_open: false,
            room_size: "medium".into(),
            favorite_folders: vec![],
            projects: Default::default(),
            emblems: Default::default(),
            path: None,
        }
    }
}

fn one() -> f32 { 1.0 }

impl Prefs {
    pub fn default_path() -> PathBuf {
        let config = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
        let path = config.join("agents-pixel-dungeon").join("settings.json");
        // The settings from before the app was renamed come along, once.
        let old = config.join("herdr-pixel-dungeon").join("settings.json");
        if !path.exists() && old.exists() {
            if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
            let _ = std::fs::copy(&old, &path);
        }
        path
    }

    pub fn load(path: PathBuf) -> Prefs {
        let mut prefs = std::fs::read(&path).ok().and_then(|d| serde_json::from_slice::<Prefs>(&d).ok()).unwrap_or_default();
        prefs.columns = prefs.columns.max(1);
        prefs.rows = prefs.rows.max(1);
        prefs.panel_height = prefs.panel_height.max(crate::ui::MIN_PANEL);
        prefs.text_scale = prefs.text_scale.clamp(0.7, 1.6);
        prefs.console_scale = prefs.console_scale.clamp(0.6, 2.5);
        prefs.path = Some(path);
        prefs
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
        if let Ok(text) = serde_json::to_string_pretty(self) { let _ = std::fs::write(path, text); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guild_hp_starts_closed_and_remembers() {
        assert!(!Prefs::default().guild_hp_open);
        let old: Prefs = serde_json::from_str(r#"{"sound_enabled": true}"#).unwrap();
        assert!(!old.guild_hp_open, "settings from before keep it closed");
        let open: Prefs = serde_json::from_str(&serde_json::to_string(&Prefs { guild_hp_open: true, ..Prefs::default() }).unwrap()).unwrap();
        assert!(open.guild_hp_open);
    }
}
