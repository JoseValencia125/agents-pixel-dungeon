//! Global Codex CLI defaults. Preserve unrelated TOML and reject stale edits.
use crate::l10n::tr;
use std::path::{Path, PathBuf};
use toml_edit::{value, Document};

#[derive(Clone, Debug, PartialEq)]
struct Settings {
    sandbox: String,
    approval: String,
    reviewer: String,
    search: String,
    network: bool,
}
impl Settings {
    fn read(doc: &Document) -> Self {
        let string = |key: &str, fallback: &str| doc.get(key).and_then(|v| v.as_str()).unwrap_or(fallback).to_string();
        Self {
            sandbox: string("sandbox_mode", "workspace-write"),
            approval: string("approval_policy", "on-request"),
            reviewer: string("approvals_reviewer", "user"),
            search: string("web_search", "cached"),
            network: doc.get("sandbox_workspace_write").and_then(|v| v.get("network_access")).and_then(|v| v.as_bool()).unwrap_or(false),
        }
    }
    fn apply(&self, doc: &mut Document) {
        doc["sandbox_mode"] = value(&self.sandbox);
        doc["approval_policy"] = value(&self.approval);
        doc["approvals_reviewer"] = value(&self.reviewer);
        doc["web_search"] = value(&self.search);
        doc["sandbox_workspace_write"]["network_access"] = value(self.network);
    }
}
fn read(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}
fn parse(text: &str) -> Result<Document, String> {
    let doc = text.parse::<Document>().map_err(|e| e.to_string())?;
    // A named permissions profile takes precedence over the legacy sandbox knobs.
    // Do not silently replace a custom policy or a granular approval table.
    if doc.get("default_permissions").is_some() || doc.get("approval_policy").is_some_and(|v| v.as_str().is_none()) {
        return Err(tr("Esta configuración usa una política personalizada. Edítala en el archivo."));
    }
    if doc.get("sandbox_workspace_write").is_some_and(|v| !v.is_table() && !v.is_inline_table()) {
        return Err(tr("La sección sandbox_workspace_write no es una tabla."));
    }
    Ok(doc)
}
fn save(path: &Path, original: &str, settings: &Settings) -> Result<String, String> {
    if read(path)? != original { return Err(tr("El archivo cambió. Cierra y vuelve a abrir este panel.")); }
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(tr("El archivo es un enlace simbólico. Edítalo directamente."));
    }
    let mut doc = parse(original)?;
    settings.apply(&mut doc);
    let text = doc.to_string();
    let parent = path.parent().ok_or("Missing parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(".config.toml.dungeon-{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        use std::io::Write;
        if let Ok(meta) = std::fs::metadata(path) { file.set_permissions(meta.permissions()).map_err(|e| e.to_string())?; }
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        if read(path)? != original { return Err(tr("El archivo cambió. Cierra y vuelve a abrir este panel.")); }
        std::fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&temp); }
    result?;
    Ok(text)
}

pub struct Editor {
    path: PathBuf,
    original: String,
    settings: Settings,
    error: Option<String>,
    note: Option<String>,
}
impl Editor {
    pub fn load() -> Self {
        let home = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| crate::herdr::home_dir().join(".codex"));
        let path = home.join("config.toml");
        let loaded = read(&path).and_then(|text| Ok((parse(&text)?, text)));
        match loaded {
            Ok((doc, original)) => Self { path, original, settings: Settings::read(&doc), error: None, note: None },
            Err(error) => Self { path, original: String::new(), settings: Settings::read(&Document::new()), error: Some(error), note: None },
        }
    }
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let mut open = true;
        let mut close = false;
        egui::Window::new(tr("Configuraciones de Codex")).id(egui::Id::new("codex-settings"))
            .open(&mut open).default_width(470.0).resizable(true).vscroll(true).show(ctx, |ui| {
                ui.label(tr("Valores globales para nuevas sesiones de Codex CLI."));
                ui.label(self.path.display().to_string());
                ui.label(tr("Las opciones del proyecto, del lanzador o de la organización pueden prevalecer."));
                ui.separator();
                if let Some(error) = &self.error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
                ui.add_enabled_ui(self.error.is_none(), |ui| {
                    choice(ui, "Archivos y carpetas", &mut self.settings.sandbox, &[
                        ("read-only", "Solo lectura"), ("workspace-write", "Escribir en el proyecto"), ("danger-full-access", "Acceso completo al equipo"),
                    ]);
                    choice(ui, "Aprobaciones", &mut self.settings.approval, &[("on-request", "Cuando sea necesario"), ("never", "Nunca preguntar (bloquear lo no permitido)")]);
                    ui.add_enabled_ui(self.settings.approval != "never", |ui| {
                        choice(ui, "Quién revisa", &mut self.settings.reviewer, &[("user", "Yo"), ("auto_review", "Revisión automática")]);
                    });
                    choice(ui, "Búsqueda web", &mut self.settings.search, &[("disabled", "Desactivada"), ("cached", "Resultados en caché"), ("indexed", "Acceso limitado por el índice"), ("live", "En vivo")]);
                    ui.add_enabled_ui(self.settings.sandbox == "workspace-write", |ui| {
                        ui.checkbox(&mut self.settings.network, tr("Permitir internet a los comandos (GitHub, descargas)"));
                    });
                    ui.separator();
                    if self.settings.sandbox == "danger-full-access" {
                        ui.colored_label(egui::Color32::YELLOW, tr("Acceso completo permite modificar o borrar archivos fuera del proyecto."));
                    } else if self.settings.sandbox == "workspace-write" {
                        ui.label(tr("El proyecto y las carpetas temporales son editables; .git sigue protegido."));
                        if self.settings.approval == "never" {
                            ui.label(tr("Crear commits puede fallar porque no se permite salir del sandbox."));
                        } else if self.settings.reviewer == "auto_review" {
                            ui.label(tr("La revisión automática puede autorizar acciones fuera del proyecto; no es un límite estricto."));
                        }
                    }
                    ui.label(tr("GitHub necesita autenticación. Guardar no reinicia agentes activos."));
                    if ui.button(tr("Guardar")).clicked() {
                        self.note = Some(match save(&self.path, &self.original, &self.settings) {
                            Ok(text) => { self.original = text; tr("Guardado. Se aplicará a nuevas sesiones de Codex.") }
                            Err(e) => format!("{}: {e}", tr("No se pudo guardar")),
                        });
                    }
                });
                if let Some(note) = &self.note { ui.label(note); }
                if ui.button(tr("Cerrar")).clicked() { close = true; }
            });
        open && !close
    }
}
fn choice(ui: &mut egui::Ui, label: &str, selected: &mut String, choices: &[(&str, &str)]) {
    ui.horizontal(|ui| {
        ui.label(tr(label));
        let shown = choices.iter().find(|(key, _)| *key == selected).map(|(_, text)| tr(text)).unwrap_or_else(|| selected.clone());
        egui::ComboBox::from_id_salt(label).selected_text(shown).show_ui(ui, |ui| {
            for (key, text) in choices { ui.selectable_value(selected, key.to_string(), tr(text)); }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_other_settings_and_round_trips() {
        let original = "# keep me\nmodel = 'example'\n[projects.'/tmp/game']\ntrust_level = 'trusted'\n[sandbox_workspace_write]\nwritable_roots = ['/tmp/extra']\n";
        let mut doc = parse(original).unwrap();
        let mut settings = Settings::read(&doc);
        settings.network = true;
        settings.reviewer = "auto_review".into();
        settings.apply(&mut doc);
        let text = doc.to_string();
        assert!(text.contains("# keep me") && text.contains("trust_level = 'trusted'") && text.contains("writable_roots = ['/tmp/extra']"));
        assert_eq!(Settings::read(&parse(&text).unwrap()), settings);
    }
    #[test]
    fn rejects_invalid_and_custom_policies() {
        for text in ["[broken", "approval_policy = { granular = {} }", "default_permissions = 'custom'", "sandbox_workspace_write = false"] { assert!(parse(text).is_err()); }
    }
    #[test]
    fn writes_atomically_and_rejects_stale_editor() {
        let path = std::env::temp_dir().join(format!("codex-settings-test-{}", std::process::id())).join("config.toml");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let settings = Settings::read(&Document::new());
        let saved = save(&path, "", &settings).unwrap();
        assert_eq!(read(&path).unwrap(), saved);
        std::fs::write(&path, "model = 'changed'\n").unwrap();
        assert!(save(&path, &saved, &settings).is_err());
        assert_eq!(read(&path).unwrap(), "model = 'changed'\n");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
