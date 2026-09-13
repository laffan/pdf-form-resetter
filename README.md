# PDF Form Resetter

A PDF viewer will happily let you check a radio button and then give you no way
to uncheck it. This is a small desktop app for exactly that: open a PDF, click
the field you want, press RESET.

It changes the selected field and nothing else. Everything already in the file —
the other fields, the page content, metadata, bookmarks, attachments, structure
tags — is left byte for byte as it was.

![the app](docs/screenshot.png)

## Using it

- **Open PDF…**, or drop a file on the window, or pass a path on the command
  line (`pdf-form-resetter form.pdf`).
- Fields are outlined on the page. **Amber means the field holds a value** — the
  ones you are probably here for. Blue means empty.
- Click one to select it. Shift-click (or ⌘/Ctrl-click) to pick several.
  Escape deselects.
- **RESET** clears it. Push buttons and signature fields are drawn with a dashed
  outline and cannot be selected: a push button has no value to reset, and
  rewriting a signature is not something this tool will pretend to do.

A radio group is a single field with one button per option, so selecting any
button in the group selects the group, and one RESET clears the whole thing.
That is the operation a viewer does not give you.

### RESET and CLEAR

Where a field has a default value (`/DV`), two buttons appear, because there are
two different things "reset" could mean:

- **RESET TO DEFAULT** does what the form's own reset button does: puts the
  default value back (PDF 32000-1, 12.7.5.3).
- **CLEAR** leaves the field empty whatever the default says.

The distinction matters more than it sounds like it should. Plenty of PDF
producers write `/DV` equal to the value the document shipped with, so on those
files "reset" restores that text rather than emptying the field. Where a field
has no default, the two are the same thing and only one button is shown.

## What it does to the file

The write is an **incremental update** (PDF 32000-1, 7.5.6): the original bytes
are copied through untouched, and only the objects that actually changed are
appended, with a new cross-reference section pointing back at the old one.

Three things follow from that, all of them the point:

- Nothing else in the file is rewritten, re-compressed or re-ordered. Resetting
  a three-button radio group appends two objects — the field, and the one button
  that was on.
- The previous revision is still in the file. A reset is recoverable by anything
  that can read an earlier revision, without a `.bak` file cluttering the
  directory.
- The output is checked against the input before anything is written: if the
  original bytes were not preserved exactly, the write is refused. The new
  revision goes to a temporary file in the same directory and is renamed over
  the original, so an interrupted write cannot leave a half-updated PDF.

Resetting a field that is already in the state you asked for writes nothing at
all — the file is left alone rather than gaining an empty revision.

### The details that bite

- **Appearance streams.** A text field keeps a little drawing of its own
  contents. Clearing the value without touching that drawing would leave the old
  text on screen, so the drawing is emptied too — the stream object and its
  dictionary survive, only the content goes. Buttons need none of this: their
  appearances are per-state and the widget is simply pointed back at `/Off`.
- **Missing values vs. empty ones.** Readers treat a field with no `/V` as
  "show the default", so clearing a field that has a `/DV` writes an explicit
  empty value rather than deleting the entry. Without that, the default comes
  straight back.
- **Restoring a non-empty default** is the one case that needs the viewer's
  help: the app sets `/NeedAppearances` so the text is redrawn. That is a
  document-level flag, and it is only set when a reset actually puts text back.
- **Encrypted PDFs** are refused rather than silently mangled.

## Building and running

Needs [Rust](https://rustup.rs) and [Node](https://nodejs.org) 20+, plus the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) for your platform
(on Debian/Ubuntu: `libwebkit2gtk-4.1-dev libgtk-3-dev
libayatana-appindicator3-dev librsvg2-dev`).

```sh
npm install
npm run app          # run it
npm run app:build    # build an installer into src-tauri/target/release/bundle
```

## Layout

| | |
| --- | --- |
| `crates/pdf-form-core` | Reading the field tree and doing the reset. No UI, no platform dependency, all of the behaviour. |
| `src-tauri` | The Tauri 2 app: three commands and a window. |
| `src`, `index.html` | The viewer: pdf.js, the field overlays, the selection panel. |

The split is the point: the code that touches your documents is a plain library
with its own tests, and the app is a shell around it. The same core is reachable
from the command line, which is how the cross-check below drives it:

```sh
cargo run -p pdf-form-core --example reset -- list form.pdf
cargo run -p pdf-form-core --example reset -- reset form.pdf choice
cargo run -p pdf-form-core --example reset -- clear form.pdf fullname
```

## Tests

```sh
cargo test        # the field tree and the reset semantics
npm run check     # the same results, read back by pdf.js
```

The Rust tests run against a form produced by reportlab — a producer this
project did not write — plus a hand-built one for the shapes reportlab will not
emit (a push button, a radio group with a default). They assert the field is
cleared, and separately that no object outside that field changed.

`npm run check` asks a second implementation what it sees in the file before and
after. That is not redundant: it is what caught the `/DV` fallback above, which
every test written against the library doing the writing had agreed was fine.
