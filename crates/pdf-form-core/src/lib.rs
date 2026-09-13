//! Reading and selectively resetting PDF AcroForm fields.
//!
//! The crate is deliberately free of any UI or platform dependency so that the
//! part that touches the user's documents can be tested on its own.

mod form;
mod reset;

pub use form::{decode_id, encode_id, read_form, Field, FieldKind, Form, Widget};
pub use reset::{reset_bytes, reset_file, ResetOutput, ResetReport};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("this PDF is encrypted; open it in a viewer and save an unencrypted copy first")]
    Encrypted,
    #[error("the PDF has no document catalog")]
    NoCatalog,
    #[error("the PDF has no AcroForm dictionary")]
    NoForm,
    #[error("no field with id {0} in this PDF")]
    UnknownField(String),
    #[error("{name} cannot be reset: {reason}")]
    NotResettable { name: String, reason: String },
    #[error("refusing to write: the update would not have preserved the original bytes")]
    OriginalNotPreserved,
    #[error("PDF error: {0}")]
    Pdf(#[from] lopdf::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}
