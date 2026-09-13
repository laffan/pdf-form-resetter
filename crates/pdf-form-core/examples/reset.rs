//! A command-line way into the same core the app uses. Handy for scripting,
//! and for checking a result with a different PDF implementation.
//!
//!     cargo run -p pdf-form-core --example reset -- list form.pdf
//!     cargo run -p pdf-form-core --example reset -- reset form.pdf choice
//!     cargo run -p pdf-form-core --example reset -- clear form.pdf fullname

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pdf_form_core::{decode_id, read_form_bytes, reset_file, ResetMode};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let path = args.next().map(PathBuf::from);
    let names: Vec<String> = args.collect();

    let Some(path) = path else {
        eprintln!("usage: reset <list|reset|clear> <file.pdf> [field name or id...]");
        return ExitCode::FAILURE;
    };

    match run(&command, &path, &names) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: &str, path: &Path, names: &[String]) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let form = read_form_bytes(&bytes).map_err(|error| error.to_string())?;

    if command == "list" {
        for field in &form.fields {
            let value = field.value.as_deref().unwrap_or("");
            let default = field
                .default_value
                .as_deref()
                .map(|default| format!(", default {default:?}"))
                .unwrap_or_default();
            println!(
                "{:<6} {:<22} {:<12} {value:?}{default}{}",
                field.id,
                field.name,
                field.kind_label,
                if field.is_set { "  [set]" } else { "" }
            );
        }
        return Ok(());
    }

    let mode = match command {
        "reset" => ResetMode::Default,
        "clear" => ResetMode::Clear,
        other => return Err(format!("unknown command {other:?}")),
    };

    let ids = names
        .iter()
        .map(|name| {
            form.fields
                .iter()
                .find(|field| &field.name == name || &field.id == name)
                .and_then(|field| decode_id(&field.id))
                .ok_or_else(|| format!("no field called {name:?}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Err("name at least one field".into());
    }

    let report = reset_file(path, &ids, mode).map_err(|error| error.to_string())?;
    println!(
        "{} {:?}: {} object(s) rewritten, {} bytes appended",
        if mode == ResetMode::Clear {
            "cleared"
        } else {
            "reset"
        },
        report.fields,
        report.objects_changed,
        report.bytes_appended
    );
    Ok(())
}
