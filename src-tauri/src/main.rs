// Hide the console window on Windows in a release build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The app is a thin shell. Every decision about what a reset means lives in
//! `pdf-form-core`; this file only moves data between that crate and the
//! window.

use std::fs;
use std::path::{Path, PathBuf};

use pdf_form_core::{decode_id, read_form_bytes, reset_file, Field, ResetMode, ResetReport};
use serde::Serialize;
use tauri::ipc::Response;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentModel {
    path: String,
    file_name: String,
    page_count: usize,
    /// Set by a previous reset that restored a non-empty default value.
    need_appearances: bool,
    /// Whether the file can be written back to.
    writable: bool,
    fields: Vec<Field>,
}

/// A path given on the command line, so the app can be a handler for "Open
/// with" as well as for its own file dialog.
struct StartupPath(Option<String>);

#[tauri::command]
fn startup_path(path: tauri::State<'_, StartupPath>) -> Option<String> {
    path.0.clone()
}

/// Read the form structure. Nothing is written and nothing is cached: the file
/// on disk stays the single source of truth, so the window always shows what
/// another program would see.
#[tauri::command]
fn open_pdf(path: String) -> Result<DocumentModel, String> {
    let path = verify(&path)?;
    let bytes = fs::read(&path).map_err(|error| format!("could not read the file: {error}"))?;
    let form = read_form_bytes(&bytes).map_err(|error| error.to_string())?;

    Ok(DocumentModel {
        file_name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        page_count: form.page_count,
        need_appearances: form.need_appearances,
        writable: !fs::metadata(&path)
            .map(|meta| meta.permissions().readonly())
            .unwrap_or(false),
        fields: form.fields,
        path: path.to_string_lossy().into_owned(),
    })
}

/// Hand the raw file to the window for pdf.js to render. Returned as a
/// `Response` so the bytes travel as an ArrayBuffer rather than a JSON array.
#[tauri::command]
fn pdf_bytes(path: String) -> Result<Response, String> {
    let path = verify(&path)?;
    fs::read(&path)
        .map(Response::new)
        .map_err(|error| format!("could not read the file: {error}"))
}

#[tauri::command]
fn reset_fields(
    path: String,
    field_ids: Vec<String>,
    mode: ResetMode,
) -> Result<ResetReport, String> {
    let path = verify(&path)?;
    let ids = field_ids
        .iter()
        .map(|id| decode_id(id).ok_or_else(|| format!("not a field id: {id}")))
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Err("nothing selected".into());
    }
    reset_file(&path, &ids, mode).map_err(|error| error.to_string())
}

/// The window only ever passes back a path the user picked in the file dialog
/// or dropped on the window, but the path still has to name a readable file
/// before anything touches it.
fn verify(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    Ok(path.to_path_buf())
}

fn main() {
    let startup = std::env::args().nth(1).filter(|argument| {
        !argument.starts_with('-') && Path::new(argument).extension().is_some()
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(StartupPath(startup))
        .invoke_handler(tauri::generate_handler![
            open_pdf,
            pdf_bytes,
            reset_fields,
            startup_path
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the app");
}
