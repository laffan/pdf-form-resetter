//! The behaviour that matters: the selected field is cleared, and nothing else
//! in the file moves.

use std::collections::BTreeSet;

use lopdf::{dictionary, Document, Object, ObjectId};
use pdf_form_core::{decode_id, read_form, reset_bytes, Error, Field, FieldKind, Form, ResetMode};

const FIXTURE: &[u8] = include_bytes!("assets/filled_form.pdf");

fn form_of(bytes: &[u8]) -> Form {
    read_form(&Document::load_mem(bytes).expect("parse")).expect("read form")
}

fn field<'a>(form: &'a Form, name: &str) -> &'a Field {
    form.fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("no field named {name}, have {:?}", names(form)))
}

fn names(form: &Form) -> Vec<&str> {
    form.fields.iter().map(|f| f.name.as_str()).collect()
}

fn id_of(form: &Form, name: &str) -> ObjectId {
    decode_id(&field(form, name).id).expect("field id round-trips")
}

fn reset(bytes: &[u8], field_names: &[&str]) -> Vec<u8> {
    reset_with(bytes, field_names, ResetMode::Default).bytes
}

fn clear(bytes: &[u8], field_names: &[&str]) -> Vec<u8> {
    reset_with(bytes, field_names, ResetMode::Clear).bytes
}

fn reset_with(bytes: &[u8], field_names: &[&str], mode: ResetMode) -> pdf_form_core::ResetOutput {
    let form = form_of(bytes);
    let ids: Vec<ObjectId> = field_names.iter().map(|name| id_of(&form, name)).collect();
    reset_bytes(bytes.to_vec(), &ids, mode).expect("reset")
}

/// Object ids whose contents differ between two revisions of a file.
fn changed_objects(before: &[u8], after: &[u8]) -> BTreeSet<ObjectId> {
    let old = Document::load_mem(before).expect("parse before");
    let new = Document::load_mem(after).expect("parse after");
    let mut changed = BTreeSet::new();
    for (id, object) in &old.objects {
        match new.objects.get(id) {
            Some(updated) if updated == object => {}
            _ => {
                changed.insert(*id);
            }
        }
    }
    changed
}

#[test]
fn reads_the_field_tree_of_a_third_party_form() {
    let form = form_of(FIXTURE);
    assert_eq!(form.page_count, 2);
    assert_eq!(
        names(&form),
        vec!["choice", "agree", "fullname", "nickname", "colour", "confirm"]
    );

    let radio = field(&form, "choice");
    assert_eq!(radio.kind, FieldKind::Radio);
    assert_eq!(radio.value.as_deref(), Some("beta"));
    assert!(radio.is_set);
    // One entry per button: the group is a single selectable unit.
    assert_eq!(radio.widgets.len(), 3);
    assert_eq!(
        radio
            .widgets
            .iter()
            .map(|w| w.on_state.as_deref().unwrap())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta", "gamma"]
    );

    assert_eq!(field(&form, "colour").kind, FieldKind::Combo);
    assert_eq!(field(&form, "colour").value.as_deref(), Some("green"));

    // reportlab writes /DV equal to the value the document ships with, so this
    // field is filled but already at its default: a reset restores that text,
    // and only a clear empties it.
    let text = field(&form, "fullname");
    assert_eq!(text.value.as_deref(), Some("Ada Lovelace"));
    assert_eq!(text.default_value.as_deref(), Some("Ada Lovelace"));
    assert!(text.has_value);
    assert!(!text.is_set);
}

#[test]
fn a_checkbox_written_as_one_merged_dictionary_is_found() {
    // reportlab writes this field and its widget as a single object, which is
    // the other shape a terminal field comes in.
    let form = form_of(FIXTURE);
    let checkbox = field(&form, "agree");
    assert_eq!(checkbox.kind, FieldKind::Checkbox);
    assert_eq!(checkbox.value.as_deref(), Some("Yes"));
    assert_eq!(checkbox.widgets.len(), 1);
    assert_eq!(checkbox.widgets[0].id, checkbox.id);
    assert!(checkbox.is_set);
}

#[test]
fn widgets_report_the_page_they_sit_on() {
    let form = form_of(FIXTURE);
    for widget in &field(&form, "choice").widgets {
        assert_eq!(widget.page_index, Some(0));
        assert!(widget.rect.is_some());
    }
    for widget in &field(&form, "confirm").widgets {
        assert_eq!(widget.page_index, Some(1));
    }
}

#[test]
fn resetting_a_radio_group_clears_the_value_and_every_button() {
    let after = reset(FIXTURE, &["choice"]);
    let form = form_of(&after);
    let radio = field(&form, "choice");

    assert_eq!(radio.value, None, "the group should hold no value");
    assert!(!radio.is_set, "nothing left for a second reset to do");
    for widget in &radio.widgets {
        assert_eq!(
            widget.appearance_state.as_deref(),
            Some("Off"),
            "every button in the group must be drawn unselected"
        );
    }
}

#[test]
fn resetting_one_field_leaves_every_other_object_untouched() {
    let after = reset(FIXTURE, &["choice"]);

    let before = form_of(FIXTURE);
    // The field itself, plus only the one button that was actually on: the
    // two already showing /Off are left exactly as they were.
    let expected: BTreeSet<ObjectId> = std::iter::once(id_of(&before, "choice"))
        .chain(
            field(&before, "choice")
                .widgets
                .iter()
                .filter(|w| w.appearance_state.as_deref() != Some("Off"))
                .map(|w| decode_id(&w.id).unwrap()),
        )
        .collect();
    assert_eq!(expected.len(), 2);

    assert_eq!(
        changed_objects(FIXTURE, &after),
        expected,
        "only the radio group's own field and the button that was on may change"
    );

    // Spot-check the neighbours through the public model too.
    let form = form_of(&after);
    assert_eq!(field(&form, "agree").value.as_deref(), Some("Yes"));
    assert_eq!(
        field(&form, "fullname").value.as_deref(),
        Some("Ada Lovelace")
    );
    assert_eq!(field(&form, "confirm").value.as_deref(), Some("no"));
}

#[test]
fn the_previous_revision_stays_in_the_file_byte_for_byte() {
    let after = reset(FIXTURE, &["choice"]);
    assert!(after.len() > FIXTURE.len());
    assert_eq!(
        &after[..FIXTURE.len()],
        FIXTURE,
        "an incremental update appends; it never rewrites what was there"
    );
    // And the appended part is small: a handful of objects, not a rewrite.
    assert!(
        after.len() - FIXTURE.len() < 2_000,
        "appended {} bytes",
        after.len() - FIXTURE.len()
    );
}

#[test]
fn clearing_a_text_field_empties_its_appearance_stream() {
    let after = clear(FIXTURE, &["fullname"]);
    let form = form_of(&after);
    assert_eq!(field(&form, "fullname").value, None);
    assert!(!field(&form, "fullname").has_value);

    // The old text must not still be painted by a stale appearance stream.
    let doc = Document::load_mem(&after).expect("parse");
    let widget_id = decode_id(&field(&form, "fullname").widgets[0].id).unwrap();
    let widget = doc.get_dictionary(widget_id).expect("widget");
    let stream_id = widget
        .get(b"AP")
        .and_then(|ap| ap.as_dict())
        .and_then(|ap| ap.get(b"N"))
        .and_then(|n| n.as_reference())
        .expect("indirect appearance stream");
    let stream = doc
        .get_object(stream_id)
        .and_then(|o| o.as_stream())
        .expect("stream");
    assert!(
        stream.decompressed_content().unwrap_or_default().is_empty(),
        "appearance stream should be empty after the reset"
    );
    // The stream object itself survives, dictionary and all.
    assert!(stream.dict.has(b"BBox"));
}

#[test]
fn resetting_a_text_field_restores_the_value_the_document_calls_default() {
    // Not a blank: /DV is what a form's own reset button puts back.
    let out = reset_with(FIXTURE, &["fullname"], ResetMode::Default);
    assert_eq!(out.report.objects_changed, 0, "already at its default");
    assert_eq!(out.bytes, FIXTURE, "a no-op must not touch the file");

    let typed_over = clear(FIXTURE, &["fullname"]);
    let restored = reset(&typed_over, &["fullname"]);
    let form = form_of(&restored);
    assert_eq!(
        field(&form, "fullname").value.as_deref(),
        Some("Ada Lovelace")
    );
    assert!(form.need_appearances, "the viewer has to redraw the text");
}

#[test]
fn several_fields_can_be_reset_in_one_write() {
    let after = reset(FIXTURE, &["choice", "agree", "confirm"]);
    let form = form_of(&after);
    assert_eq!(field(&form, "choice").value, None);
    assert_eq!(field(&form, "agree").value, None);
    assert_eq!(field(&form, "confirm").value, None);
    assert_eq!(
        field(&form, "fullname").value.as_deref(),
        Some("Ada Lovelace")
    );
}

#[test]
fn resetting_again_is_safe_and_keeps_the_file_readable() {
    let once = reset(FIXTURE, &["choice"]);
    let twice = reset(&once, &["choice"]);
    let thrice = clear(&twice, &["colour"]);

    let form = form_of(&thrice);
    assert_eq!(field(&form, "choice").value, None);
    assert_eq!(
        field(&form, "fullname").value.as_deref(),
        Some("Ada Lovelace")
    );
    assert_eq!(field(&form, "colour").value, None);
    assert_eq!(field(&form, "agree").value.as_deref(), Some("Yes"));
    assert_eq!(&thrice[..once.len()], once.as_slice());
}

#[test]
fn a_push_button_is_refused_rather_than_mangled() {
    let bytes = synthetic_form();
    let form = form_of(&bytes);
    let button = field(&form, "print");
    assert_eq!(button.kind, FieldKind::PushButton);
    assert!(!button.resettable);
    assert!(!button.is_set);

    let error = reset_bytes(
        bytes.clone(),
        &[decode_id(&button.id).unwrap()],
        ResetMode::Default,
    )
    .unwrap_err();
    assert!(matches!(error, Error::NotResettable { .. }), "{error}");
}

#[test]
fn an_unknown_field_id_is_an_error_and_writes_nothing() {
    let error = reset_bytes(FIXTURE.to_vec(), &[(9999, 0)], ResetMode::Default).unwrap_err();
    assert!(matches!(error, Error::UnknownField(_)), "{error}");
}

#[test]
fn a_default_value_is_restored_instead_of_cleared() {
    let bytes = synthetic_form();
    let form = form_of(&bytes);

    // Radio group whose /DV names the second button.
    let after = reset_bytes(bytes.clone(), &[id_of(&form, "pick")], ResetMode::Default)
        .expect("reset")
        .bytes;
    let after_form = form_of(&after);
    let radio = field(&after_form, "pick");
    assert_eq!(radio.value.as_deref(), Some("two"));
    assert_eq!(
        radio
            .widgets
            .iter()
            .map(|w| w.appearance_state.as_deref().unwrap())
            .collect::<Vec<_>>(),
        vec!["Off", "two"],
        "the default button is the one drawn on"
    );

    // Text field with a default: the viewer is asked to redraw it.
    let out = reset_bytes(
        bytes.clone(),
        &[id_of(&form, "greeting")],
        ResetMode::Default,
    )
    .expect("reset");
    assert!(out.report.need_appearances_set);
    let text = form_of(&out.bytes);
    assert_eq!(field(&text, "greeting").value.as_deref(), Some("hello"));
    assert!(form_of(&out.bytes).need_appearances);
}

#[test]
fn a_field_that_is_already_clear_leaves_the_file_alone() {
    let after = reset(FIXTURE, &["choice"]);
    assert!(!field(&form_of(&after), "choice").is_set);

    let again = reset_with(&after, &["choice"], ResetMode::Default);
    assert_eq!(again.report.objects_changed, 0);
    assert_eq!(again.report.bytes_appended, 0);
    assert_eq!(again.bytes, after, "a second reset must not grow the file");
}

#[test]
fn clear_ignores_a_default_value() {
    let bytes = synthetic_form();
    let form = form_of(&bytes);

    let after = reset_bytes(bytes, &[id_of(&form, "pick")], ResetMode::Clear)
        .expect("clear")
        .bytes;
    let radio = form_of(&after);
    let radio = field(&radio, "pick");
    assert_eq!(radio.value, None, "/DV must not be put back by a clear");
    assert!(radio
        .widgets
        .iter()
        .all(|w| w.appearance_state.as_deref() == Some("Off")));
}

/// A small form built by hand, for the shapes reportlab will not produce:
/// a push button, a radio group with a /DV, and a text field with a /DV.
fn synthetic_form() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();

    let appearance = |doc: &mut Document| {
        doc.add_object(lopdf::Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 14.into(), 14.into()] },
            b"".to_vec(),
        ))
    };
    let on_one = appearance(&mut doc);
    let on_two = appearance(&mut doc);
    let off = appearance(&mut doc);
    let text_ap = appearance(&mut doc);

    let pick_id = doc.new_object_id();
    let kid_one = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "Parent" => pick_id,
        "Rect" => vec![72.into(), 700.into(), 86.into(), 714.into()],
        "AS" => "one",
        "AP" => dictionary! { "N" => dictionary! { "one" => on_one, "Off" => off } },
    });
    let kid_two = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "Parent" => pick_id,
        "Rect" => vec![72.into(), 680.into(), 86.into(), 694.into()],
        "AS" => "Off",
        "AP" => dictionary! { "N" => dictionary! { "two" => on_two, "Off" => off } },
    });
    doc.set_object(
        pick_id,
        dictionary! {
            "FT" => "Btn",
            "Ff" => Object::Integer(1 << 15),
            "T" => Object::string_literal("pick"),
            "V" => "one",
            "DV" => "two",
            "Kids" => vec![kid_one.into(), kid_two.into()],
        },
    );

    let greeting = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
        "T" => Object::string_literal("greeting"),
        "V" => Object::string_literal("goodbye"),
        "DV" => Object::string_literal("hello"),
        "Rect" => vec![72.into(), 640.into(), 272.into(), 660.into()],
        "AP" => dictionary! { "N" => text_ap },
    });
    let print = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn",
        "Ff" => Object::Integer(1 << 16),
        "T" => Object::string_literal("print"),
        "Rect" => vec![72.into(), 600.into(), 172.into(), 620.into()],
    });

    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Annots" => vec![kid_one.into(), kid_two.into(), greeting.into(), print.into()],
    });
    doc.set_object(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        },
    );
    let acroform = doc.add_object(dictionary! {
        "Fields" => vec![pick_id.into(), greeting.into(), print.into()],
    });
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
        "AcroForm" => acroform,
    });
    doc.trailer.set("Root", catalog);

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).expect("save synthetic form");
    bytes
}
