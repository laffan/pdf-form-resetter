//! Reading the AcroForm field tree.
//!
//! The unit this tool works in is the *terminal field*: the node in the
//! AcroForm tree that actually carries a value. For a radio button group that
//! is the single field the individual buttons hang off, which is why selecting
//! one and resetting it clears the whole group — the behaviour a PDF viewer
//! gives you no way to reach.

use std::collections::{HashMap, HashSet};

use lopdf::{decode_text_string, Dictionary, Document, Object, ObjectId};
use serde::Serialize;

use crate::Error;

/// Field flags (/Ff), PDF 32000-1 table 227/228/230. Bit *n* is `1 << (n - 1)`.
const FF_READ_ONLY: i64 = 1 << 0;
const FF_RADIO: i64 = 1 << 15;
const FF_PUSHBUTTON: i64 = 1 << 16;
const FF_COMBO: i64 = 1 << 17;

/// Guards against a /Kids cycle in a malformed file.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    Radio,
    Checkbox,
    PushButton,
    Text,
    Combo,
    List,
    Signature,
    Unknown,
}

impl FieldKind {
    pub fn label(self) -> &'static str {
        match self {
            FieldKind::Radio => "radio group",
            FieldKind::Checkbox => "checkbox",
            FieldKind::PushButton => "push button",
            FieldKind::Text => "text field",
            FieldKind::Combo => "dropdown",
            FieldKind::List => "list box",
            FieldKind::Signature => "signature",
            FieldKind::Unknown => "field",
        }
    }

    fn is_button(self) -> bool {
        matches!(
            self,
            FieldKind::Radio | FieldKind::Checkbox | FieldKind::PushButton
        )
    }
}

/// One on-page rectangle belonging to a field. A radio group has one per
/// button; most other fields have exactly one.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Widget {
    /// Object id, serialised as `"num gen"`.
    pub id: String,
    /// 0-based page index, or `None` when the widget is not on any page's
    /// /Annots array (a damaged file; it is then not clickable in the UI).
    pub page_index: Option<usize>,
    /// `[x0, y0, x1, y1]` in PDF user space, lower-left origin, normalised so
    /// that x0 <= x1 and y0 <= y1. The frontend maps this through pdf.js's
    /// viewport so page rotation and a non-zero MediaBox origin are handled
    /// in one place.
    pub rect: Option<[f32; 4]>,
    /// For a button widget, the /AP /N state that means "on" (the export
    /// value of this particular radio button or checkbox).
    pub on_state: Option<String>,
    /// The widget's current /AS appearance state.
    pub appearance_state: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    /// Object id of the field dictionary, serialised as `"num gen"`. Used as
    /// the handle for `reset`; stable for a given revision of the file and
    /// unambiguous where two fields share a name.
    pub id: String,
    /// Fully qualified field name (`parent.child`), `""` when unnamed.
    pub name: String,
    pub kind: FieldKind,
    /// Current value, rendered for display.
    pub value: Option<String>,
    /// Value a reset restores (the field's /DV), when it has one.
    pub default_value: Option<String>,
    /// True when the field currently holds a value — what the UI highlights,
    /// and what you would want to clear.
    pub has_value: bool,
    /// True when a reset to the default would change something. A field whose
    /// /DV equals its current value is filled but not "set".
    pub is_set: bool,
    /// False for fields this tool refuses to touch (push buttons hold no
    /// value; signatures carry bytes a reset cannot honestly rewrite).
    pub resettable: bool,
    /// Why not, when `resettable` is false.
    pub note: Option<String>,
    pub read_only: bool,
    /// True when /V lives on an ancestor shared with sibling fields, so a
    /// reset necessarily clears those siblings too.
    pub shared_value: bool,
    pub widgets: Vec<Widget>,
}

/// Where the AcroForm dictionary lives, so /NeedAppearances can be set on it.
#[derive(Debug, Clone, Copy)]
pub enum AcroFormLocation {
    /// The usual case: /AcroForm is an indirect reference.
    Indirect(ObjectId),
    /// The dictionary is inline in the catalog, so the catalog is the object
    /// that has to be rewritten.
    InCatalog(ObjectId),
}

#[derive(Debug, Clone)]
pub struct Form {
    pub fields: Vec<Field>,
    pub page_count: usize,
    pub need_appearances: bool,
    pub location: Option<AcroFormLocation>,
    /// Parsed object ids, parallel to `fields`, kept out of the serialised
    /// model.
    pub(crate) field_ids: Vec<FieldRefs>,
}

/// The object ids a reset needs, resolved while walking the tree.
#[derive(Debug, Clone)]
pub(crate) struct FieldRefs {
    pub(crate) field: ObjectId,
    /// Object that actually carries /V (the field, or an ancestor).
    pub(crate) value_owner: ObjectId,
    pub(crate) kind: FieldKind,
    pub(crate) current_value: Option<Object>,
    pub(crate) default_value: Option<Object>,
    pub(crate) widgets: Vec<WidgetRefs>,
}

#[derive(Debug, Clone)]
pub(crate) struct WidgetRefs {
    pub(crate) id: ObjectId,
    pub(crate) on_state: Option<Vec<u8>>,
    /// Where this widget's normal appearance stream lives, if any.
    pub(crate) normal_appearance: Option<AppearanceRef>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum AppearanceRef {
    /// /AP /N is an indirect reference to a stream.
    Stream(ObjectId),
    /// /AP is a dictionary inline in the widget, holding /N as a stream.
    InlineInWidget(ObjectId),
    /// /AP is an indirect dictionary holding /N as a stream.
    InlineInApDict(ObjectId),
}

/// Read the form out of a PDF held in memory.
pub fn read_form_bytes(bytes: &[u8]) -> Result<Form, Error> {
    read_form(&Document::load_mem(bytes)?)
}

pub fn read_form(doc: &Document) -> Result<Form, Error> {
    let pages = doc.get_pages();
    let page_of = widget_pages(doc, &pages);

    let catalog_id = doc
        .trailer
        .get(b"Root")
        .map_err(|_| Error::NoCatalog)?
        .as_reference()
        .map_err(|_| Error::NoCatalog)?;
    let catalog = doc
        .get_dictionary(catalog_id)
        .map_err(|_| Error::NoCatalog)?;

    let (acro, location) = match catalog.get(b"AcroForm") {
        Ok(Object::Reference(id)) => (
            doc.get_dictionary(*id).map_err(|_| Error::NoForm)?,
            AcroFormLocation::Indirect(*id),
        ),
        Ok(Object::Dictionary(dict)) => (dict, AcroFormLocation::InCatalog(catalog_id)),
        _ => {
            return Ok(Form {
                fields: Vec::new(),
                page_count: pages.len(),
                need_appearances: false,
                location: None,
                field_ids: Vec::new(),
            })
        }
    };

    let need_appearances = acro
        .get(b"NeedAppearances")
        .and_then(|o| o.as_bool())
        .unwrap_or(false);

    let mut walker = Walker {
        doc,
        page_of,
        fields: Vec::new(),
        refs: Vec::new(),
        seen: HashSet::new(),
    };

    if let Ok(roots) = acro.get(b"Fields").and_then(|o| deref(doc, o)?.as_array()) {
        for entry in roots.clone() {
            if let Ok(id) = entry.as_reference() {
                walker.walk(id, &Inherited::default(), None, 0);
            }
        }
    }

    Ok(Form {
        fields: walker.fields,
        page_count: pages.len(),
        need_appearances,
        location: Some(location),
        field_ids: walker.refs,
    })
}

/// Map every widget annotation to the page it sits on. /Annots is the
/// authoritative link; a widget's own /P entry is optional and often absent.
fn widget_pages(
    doc: &Document,
    pages: &std::collections::BTreeMap<u32, ObjectId>,
) -> HashMap<ObjectId, usize> {
    let mut map = HashMap::new();
    for (index, page_id) in pages.values().enumerate() {
        let Ok(page) = doc.get_dictionary(*page_id) else {
            continue;
        };
        let Ok(annots) = page.get(b"Annots").and_then(|o| deref(doc, o)?.as_array()) else {
            continue;
        };
        for annot in annots {
            if let Ok(id) = annot.as_reference() {
                map.entry(id).or_insert(index);
            }
        }
    }
    map
}

/// Entries a terminal field can inherit from its ancestors (PDF 32000-1 12.7.3.1).
#[derive(Debug, Clone, Default)]
struct Inherited {
    field_type: Option<Vec<u8>>,
    flags: i64,
    /// Value and the object it was found on.
    value: Option<(ObjectId, Object)>,
    default_value: Option<Object>,
}

struct Walker<'a> {
    doc: &'a Document,
    page_of: HashMap<ObjectId, usize>,
    fields: Vec<Field>,
    refs: Vec<FieldRefs>,
    seen: HashSet<ObjectId>,
}

impl Walker<'_> {
    fn walk(&mut self, id: ObjectId, parent: &Inherited, prefix: Option<&str>, depth: usize) {
        if depth > MAX_DEPTH || !self.seen.insert(id) {
            return;
        }
        let Ok(dict) = self.doc.get_dictionary(id) else {
            return;
        };

        let mut state = parent.clone();
        if let Ok(ft) = dict.get(b"FT").and_then(|o| o.as_name()) {
            state.field_type = Some(ft.to_vec());
        }
        if let Ok(ff) = dict.get(b"Ff").and_then(|o| o.as_i64()) {
            state.flags = ff;
        }
        if let Ok(v) = dict.get(b"V") {
            state.value = Some((id, deref_owned(self.doc, v)));
        }
        if let Ok(dv) = dict.get(b"DV") {
            state.default_value = Some(deref_owned(self.doc, dv));
        }

        let name = match dict.get(b"T").and_then(decode_text_string) {
            Ok(part) => match prefix {
                Some(p) if !p.is_empty() => format!("{p}.{part}"),
                _ => part,
            },
            Err(_) => prefix.unwrap_or_default().to_string(),
        };

        // Split /Kids into child *fields* and this field's own widget
        // annotations. A kid with no /T and no /Kids of its own is a widget
        // that was kept separate from the field dictionary.
        let (child_fields, widget_kids) = self.split_kids(dict);

        for child in &child_fields {
            self.walk(*child, &state, Some(&name), depth + 1);
        }

        if state.field_type.is_none() {
            return;
        }
        // A node with child fields is not itself terminal, even if it carries
        // an inheritable /FT or /V.
        if !child_fields.is_empty() {
            return;
        }

        let widget_ids = if widget_kids.is_empty() {
            // Field and widget merged into one dictionary.
            if dict.has(b"Rect") || dict.has(b"AP") {
                vec![id]
            } else {
                Vec::new()
            }
        } else {
            widget_kids
        };

        self.emit(id, name, &state, widget_ids);
    }

    fn split_kids(&self, dict: &Dictionary) -> (Vec<ObjectId>, Vec<ObjectId>) {
        let mut fields = Vec::new();
        let mut widgets = Vec::new();
        let Ok(kids) = dict
            .get(b"Kids")
            .and_then(|o| deref(self.doc, o)?.as_array())
        else {
            return (fields, widgets);
        };
        for kid in kids {
            let Ok(kid_id) = kid.as_reference() else {
                continue;
            };
            let Ok(kid_dict) = self.doc.get_dictionary(kid_id) else {
                continue;
            };
            if kid_dict.has(b"T") || kid_dict.has(b"Kids") {
                fields.push(kid_id);
            } else {
                widgets.push(kid_id);
            }
        }
        (fields, widgets)
    }

    fn emit(&mut self, id: ObjectId, name: String, state: &Inherited, widget_ids: Vec<ObjectId>) {
        let kind = classify(state.field_type.as_deref(), state.flags);
        let value = state.value.as_ref().map(|(_, v)| v.clone());
        let value_owner = state.value.as_ref().map(|(owner, _)| *owner).unwrap_or(id);

        let mut widget_refs = Vec::new();
        let mut widgets = Vec::new();
        for widget_id in widget_ids {
            let Ok(widget) = self.doc.get_dictionary(widget_id) else {
                continue;
            };
            let on_state = kind.is_button().then(|| self.on_state(widget)).flatten();
            let appearance_state = widget
                .get(b"AS")
                .and_then(|o| o.as_name())
                .ok()
                .map(|n| String::from_utf8_lossy(n).into_owned());
            widgets.push(Widget {
                id: encode_id(widget_id),
                page_index: self.page_of.get(&widget_id).copied(),
                rect: rect_of(self.doc, widget),
                on_state: on_state
                    .as_ref()
                    .map(|s| String::from_utf8_lossy(s).into_owned()),
                appearance_state,
            });
            widget_refs.push(WidgetRefs {
                id: widget_id,
                on_state,
                normal_appearance: self.normal_appearance(widget_id, widget),
            });
        }

        let (resettable, note) = match kind {
            FieldKind::PushButton => (false, Some("A push button holds no value.".into())),
            FieldKind::Signature => (
                false,
                Some("Clearing a signature field is outside what this tool does.".into()),
            ),
            _ => (true, None),
        };

        let is_set = resettable
            && differs_from_reset(value.as_ref(), state.default_value.as_ref(), kind, &widgets);
        let has_value = resettable
            && (value.as_ref().is_some_and(|v| !is_empty_value(v))
                || widgets.iter().any(|w| {
                    w.appearance_state
                        .as_ref()
                        .is_some_and(|state| state != "Off")
                }));

        self.fields.push(Field {
            id: encode_id(id),
            name,
            kind,
            value: value.as_ref().map(display_value),
            default_value: state.default_value.as_ref().map(display_value),
            has_value,
            is_set,
            resettable,
            note,
            read_only: state.flags & FF_READ_ONLY != 0,
            shared_value: value_owner != id,
            widgets,
        });
        self.refs.push(FieldRefs {
            field: id,
            value_owner,
            kind,
            current_value: value,
            default_value: state.default_value.clone(),
            widgets: widget_refs,
        });
    }

    /// The /AP /N key that is not /Off — the export value of this button.
    fn on_state(&self, widget: &Dictionary) -> Option<Vec<u8>> {
        let ap = widget
            .get(b"AP")
            .ok()
            .and_then(|o| deref(self.doc, o).ok())?;
        let normal = ap.as_dict().ok()?.get(b"N").ok()?;
        let states = deref(self.doc, normal).ok()?.as_dict().ok()?;
        states
            .iter()
            .map(|(key, _)| key.clone())
            .find(|key| key.as_slice() != b"Off")
    }

    /// Where the widget's normal appearance stream lives, if it has one. A
    /// button's /AP /N is a dictionary of states rather than a stream, so this
    /// is `None` for them and their appearances are never rewritten.
    fn normal_appearance(&self, widget_id: ObjectId, widget: &Dictionary) -> Option<AppearanceRef> {
        let ap_entry = widget.get(b"AP").ok()?;
        let ap = deref(self.doc, ap_entry).ok()?.as_dict().ok()?;
        match ap.get(b"N").ok()? {
            Object::Reference(id) => Some(AppearanceRef::Stream(*id)),
            Object::Stream(_) => Some(match ap_entry {
                Object::Reference(ap_id) => AppearanceRef::InlineInApDict(*ap_id),
                _ => AppearanceRef::InlineInWidget(widget_id),
            }),
            _ => None,
        }
    }
}

fn classify(field_type: Option<&[u8]>, flags: i64) -> FieldKind {
    match field_type {
        Some(b"Btn") => {
            if flags & FF_PUSHBUTTON != 0 {
                FieldKind::PushButton
            } else if flags & FF_RADIO != 0 {
                FieldKind::Radio
            } else {
                FieldKind::Checkbox
            }
        }
        Some(b"Tx") => FieldKind::Text,
        Some(b"Ch") => {
            if flags & FF_COMBO != 0 {
                FieldKind::Combo
            } else {
                FieldKind::List
            }
        }
        Some(b"Sig") => FieldKind::Signature,
        _ => FieldKind::Unknown,
    }
}

/// Would a reset change anything? For buttons this also looks at the widgets'
/// /AS, because a stale appearance state is exactly the kind of leftover this
/// tool exists to clear.
fn differs_from_reset(
    value: Option<&Object>,
    default: Option<&Object>,
    kind: FieldKind,
    widgets: &[Widget],
) -> bool {
    let value_differs = match (value, default) {
        (None, None) => false,
        (Some(v), None) => !is_empty_value(v),
        (None, Some(d)) => !is_empty_value(d),
        (Some(v), Some(d)) => display_value(v) != display_value(d),
    };
    if value_differs {
        return true;
    }
    if kind.is_button() {
        let target = match (value, default) {
            (_, Some(d)) => display_value(d),
            (Some(v), None) => display_value(v),
            (None, None) => "Off".to_string(),
        };
        let target = if target.is_empty() {
            "Off".to_string()
        } else {
            target
        };
        return widgets
            .iter()
            .any(|w| match (&w.appearance_state, &w.on_state) {
                // A widget is stale when it shows "on" but is not the chosen one.
                (Some(state), Some(on)) => state == on && *state != target,
                (Some(state), None) => state.as_str() != "Off" && *state != target,
                _ => false,
            });
    }
    false
}

fn is_empty_value(obj: &Object) -> bool {
    match obj {
        Object::Null => true,
        Object::Name(name) => name.as_slice() == b"Off",
        Object::String(bytes, _) => bytes.is_empty(),
        Object::Array(items) => items.is_empty(),
        _ => false,
    }
}

pub(crate) fn display_value(obj: &Object) -> String {
    match obj {
        Object::Name(name) => String::from_utf8_lossy(name).into_owned(),
        Object::String(..) => decode_text_string(obj).unwrap_or_default(),
        Object::Integer(n) => n.to_string(),
        Object::Real(n) => n.to_string(),
        Object::Boolean(b) => b.to_string(),
        Object::Array(items) => items
            .iter()
            .map(display_value)
            .collect::<Vec<_>>()
            .join(", "),
        Object::Null => String::new(),
        _ => String::new(),
    }
}

fn rect_of(doc: &Document, widget: &Dictionary) -> Option<[f32; 4]> {
    let rect = widget.get(b"Rect").ok().and_then(|o| deref(doc, o).ok())?;
    let values = rect.as_array().ok()?;
    if values.len() != 4 {
        return None;
    }
    let mut out = [0f32; 4];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = deref(doc, value).ok()?.as_float().ok()?;
    }
    Some([
        out[0].min(out[2]),
        out[1].min(out[3]),
        out[0].max(out[2]),
        out[1].max(out[3]),
    ])
}

fn deref<'a>(doc: &'a Document, obj: &'a Object) -> Result<&'a Object, lopdf::Error> {
    doc.dereference(obj).map(|(_, resolved)| resolved)
}

fn deref_owned(doc: &Document, obj: &Object) -> Object {
    deref(doc, obj).cloned().unwrap_or(Object::Null)
}

pub fn encode_id(id: ObjectId) -> String {
    format!("{} {}", id.0, id.1)
}

pub fn decode_id(text: &str) -> Option<ObjectId> {
    let (num, generation) = text.split_once(' ')?;
    Some((num.parse().ok()?, generation.parse().ok()?))
}
