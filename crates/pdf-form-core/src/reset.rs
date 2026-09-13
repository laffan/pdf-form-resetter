//! Resetting fields, and only fields.
//!
//! Two rules shape everything here:
//!
//! 1. A reset means what PDF 32000-1 12.7.5.3 (ResetForm) says it means: the
//!    field takes its default value /DV, or has no value if it has none. It is
//!    not "delete the field", and it is not "flatten".
//! 2. The write is an *incremental update* (7.5.6): the original bytes are
//!    copied through untouched and the handful of changed objects are appended
//!    with a new cross-reference section. Nothing else in the file is
//!    rewritten, re-compressed or re-ordered, and the previous revision stays
//!    in the file.

use std::fs;
use std::io::Write;
use std::path::Path;

use lopdf::{Document, IncrementalDocument, Object, ObjectId};
use serde::Serialize;

use crate::form::{
    display_value, encode_id, read_form, AcroFormLocation, AppearanceRef, FieldKind, Form,
};
use crate::Error;

/// A single object-level change. Collected first, applied second, so that
/// planning can read the document while applying holds it mutably.
#[derive(Debug, Clone)]
enum Edit {
    /// Set /V to the field's default, or remove it. Also drops /I and /RV,
    /// which cache the same value.
    Value { object: ObjectId, to: Option<Object> },
    /// Point a button widget at the appearance state matching the new value.
    AppearanceState { object: ObjectId, state: Vec<u8> },
    /// Empty a text or choice widget's appearance stream so the old text
    /// stops being drawn. The stream object and its dictionary survive; only
    /// its content becomes empty.
    BlankAppearance(AppearanceRef),
    /// Ask the viewer to regenerate appearances, needed only when a reset
    /// puts a non-empty default value back into a text or choice field.
    NeedAppearances(AcroFormLocation),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetReport {
    /// Names of the fields that were reset, in the order requested.
    pub fields: Vec<String>,
    /// Indirect objects rewritten in the appended revision.
    pub objects_changed: usize,
    /// Whether /NeedAppearances had to be turned on.
    pub need_appearances_set: bool,
    /// Size of the appended revision.
    pub bytes_appended: usize,
}

pub struct ResetOutput {
    pub bytes: Vec<u8>,
    pub report: ResetReport,
}

impl std::fmt::Debug for ResetOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResetOutput")
            .field("bytes", &format_args!("{} bytes", self.bytes.len()))
            .field("report", &self.report)
            .finish()
    }
}

/// Reset the given fields in `bytes`, returning the new file.
pub fn reset_bytes(bytes: Vec<u8>, field_ids: &[ObjectId]) -> Result<ResetOutput, Error> {
    let doc = Document::load_mem(&bytes)?;
    if doc.is_encrypted() {
        return Err(Error::Encrypted);
    }
    let form = read_form(&doc)?;

    let mut edits = Vec::new();
    let mut names = Vec::new();
    for id in field_ids {
        let index = form
            .field_ids
            .iter()
            .position(|refs| refs.field == *id)
            .ok_or_else(|| Error::UnknownField(encode_id(*id)))?;
        let field = &form.fields[index];
        if !field.resettable {
            return Err(Error::NotResettable {
                name: field.name.clone(),
                reason: field.note.clone().unwrap_or_default(),
            });
        }
        plan_field(&form, index, &mut edits)?;
        names.push(field.name.clone());
    }

    let need_appearances_set = edits
        .iter()
        .any(|edit| matches!(edit, Edit::NeedAppearances(_)));
    let original_len = bytes.len();
    let mut incremental = IncrementalDocument::create_from(bytes, doc);
    let objects_changed = apply(&mut incremental, &edits)?;

    let mut out = Vec::new();
    incremental.save_to(&mut out)?;

    // The point of the incremental write is that the original file is still
    // there, byte for byte. Check it rather than trust it.
    if out.len() < original_len
        || out[..original_len] != incremental.get_prev_documents_bytes()[..original_len]
    {
        return Err(Error::OriginalNotPreserved);
    }

    Ok(ResetOutput {
        report: ResetReport {
            fields: names,
            objects_changed,
            need_appearances_set,
            bytes_appended: out.len() - original_len,
        },
        bytes: out,
    })
}

/// Reset the given fields in the file at `path`, in place.
///
/// The new revision is written to a temporary file in the same directory and
/// renamed over the original, so an interrupted write cannot leave a
/// half-updated PDF behind.
pub fn reset_file(path: &Path, field_ids: &[ObjectId]) -> Result<ResetReport, Error> {
    let bytes = fs::read(path)?;
    let output = reset_bytes(bytes, field_ids)?;

    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.pdf".to_string());
    let temp_path = directory.join(format!(".{file_name}.reset-tmp"));

    {
        let mut temp = fs::File::create(&temp_path)?;
        temp.write_all(&output.bytes)?;
        temp.sync_all()?;
    }
    if let Ok(metadata) = fs::metadata(path) {
        let _ = fs::set_permissions(&temp_path, metadata.permissions());
    }
    fs::rename(&temp_path, path)?;

    Ok(output.report)
}

fn plan_field(form: &Form, index: usize, edits: &mut Vec<Edit>) -> Result<(), Error> {
    let refs = &form.field_ids[index];
    let target = refs.default_value.clone();

    edits.push(Edit::Value {
        object: refs.value_owner,
        to: target.clone(),
    });

    match refs.kind {
        FieldKind::Radio | FieldKind::Checkbox => {
            let wanted = target.as_ref().and_then(|value| match value {
                Object::Name(name) => Some(name.clone()),
                _ => None,
            });
            for widget in &refs.widgets {
                // A widget turns on only if the value names *its* export
                // state; every other widget in the group goes to /Off.
                let state = match (&wanted, &widget.on_state) {
                    (Some(value), Some(on)) if value == on => on.clone(),
                    _ => b"Off".to_vec(),
                };
                edits.push(Edit::AppearanceState {
                    object: widget.id,
                    state,
                });
            }
        }
        FieldKind::Text | FieldKind::Combo | FieldKind::List => {
            let restores_text = target
                .as_ref()
                .is_some_and(|value| !display_value(value).is_empty());
            if restores_text {
                let location = form.location.ok_or(Error::NoForm)?;
                edits.push(Edit::NeedAppearances(location));
            } else {
                for widget in &refs.widgets {
                    if let Some(appearance) = widget.normal_appearance {
                        edits.push(Edit::BlankAppearance(appearance));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn apply(incremental: &mut IncrementalDocument, edits: &[Edit]) -> Result<usize, Error> {
    let mut touched = std::collections::BTreeSet::new();

    for edit in edits {
        match edit {
            Edit::Value { object, to } => {
                let dict = clone_dict(incremental, *object)?;
                match to {
                    Some(value) => dict.set("V", value.clone()),
                    None => {
                        dict.remove(b"V");
                    }
                }
                // /I (selection indices) and /RV (rich text) mirror /V and
                // would otherwise contradict it.
                dict.remove(b"I");
                dict.remove(b"RV");
                touched.insert(*object);
            }
            Edit::AppearanceState { object, state } => {
                let dict = clone_dict(incremental, *object)?;
                dict.set("AS", Object::Name(state.clone()));
                touched.insert(*object);
            }
            Edit::BlankAppearance(appearance) => {
                let object = match appearance {
                    AppearanceRef::Stream(id) => {
                        incremental.opt_clone_object_to_new_document(*id)?;
                        incremental
                            .new_document
                            .get_object_mut(*id)?
                            .as_stream_mut()?
                            .set_plain_content(Vec::new());
                        *id
                    }
                    AppearanceRef::InlineInWidget(id) => {
                        clone_dict(incremental, *id)?
                            .get_mut(b"AP")?
                            .as_dict_mut()?
                            .get_mut(b"N")?
                            .as_stream_mut()?
                            .set_plain_content(Vec::new());
                        *id
                    }
                    AppearanceRef::InlineInApDict(id) => {
                        clone_dict(incremental, *id)?
                            .get_mut(b"N")?
                            .as_stream_mut()?
                            .set_plain_content(Vec::new());
                        *id
                    }
                };
                touched.insert(object);
            }
            Edit::NeedAppearances(location) => {
                let object = match location {
                    AcroFormLocation::Indirect(id) => {
                        clone_dict(incremental, *id)?.set("NeedAppearances", Object::Boolean(true));
                        *id
                    }
                    AcroFormLocation::InCatalog(id) => {
                        clone_dict(incremental, *id)?
                            .get_mut(b"AcroForm")?
                            .as_dict_mut()?
                            .set("NeedAppearances", Object::Boolean(true));
                        *id
                    }
                };
                touched.insert(object);
            }
        }
    }

    Ok(touched.len())
}

/// Copy an object out of the previous revision so it can be edited in the
/// appended one, and hand back its dictionary.
fn clone_dict(
    incremental: &mut IncrementalDocument,
    id: ObjectId,
) -> Result<&mut lopdf::Dictionary, Error> {
    incremental.opt_clone_object_to_new_document(id)?;
    Ok(incremental.new_document.get_dictionary_mut(id)?)
}
