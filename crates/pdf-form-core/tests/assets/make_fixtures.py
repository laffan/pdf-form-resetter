"""Regenerate the test PDFs.

    pip install reportlab && python3 make_fixtures.py

Kept in the repo so the fixtures are reproducible rather than opaque blobs.
The point of generating them with a third-party producer is that the tests
then run against an AcroForm this project did not lay out itself.
"""

from reportlab.lib.colors import black, white
from reportlab.pdfgen import canvas

OUT = "filled_form.pdf"


def build(path: str) -> None:
    c = canvas.Canvas(path, pagesize=(612, 792))
    form = c.acroForm

    c.drawString(72, 740, "Pick one (the stuck radio group):")
    for index, (label, value) in enumerate(
        [("Alpha", "alpha"), ("Beta", "beta"), ("Gamma", "gamma")]
    ):
        y = 710 - index * 24
        c.drawString(96, y, label)
        form.radio(
            name="choice",
            value=value,
            selected=(value == "beta"),
            x=150,
            y=y - 3,
            buttonStyle="circle",
            borderColor=black,
            fillColor=white,
            size=14,
            shape="circle",
        )

    c.drawString(72, 620, "Agree:")
    form.checkbox(
        name="agree",
        checked=True,
        x=150,
        y=617,
        borderColor=black,
        fillColor=white,
        size=14,
    )

    c.drawString(72, 580, "Name:")
    form.textfield(
        name="fullname",
        value="Ada Lovelace",
        x=150,
        y=574,
        width=200,
        height=20,
        borderColor=black,
        fillColor=white,
    )

    c.drawString(72, 545, "Nickname (has a default):")
    form.textfield(
        name="nickname",
        value="Countess",
        x=250,
        y=539,
        width=200,
        height=20,
        borderColor=black,
        fillColor=white,
    )

    c.drawString(72, 505, "Colour:")
    form.choice(
        name="colour",
        value="green",
        options=[("Red", "red"), ("Green", "green"), ("Blue", "blue")],
        x=150,
        y=499,
        width=120,
        height=20,
        borderColor=black,
        fillColor=white,
    )

    c.showPage()

    # A second page, so page indexing is actually exercised.
    c.drawString(72, 740, "Second page radio group:")
    for index, (label, value) in enumerate([("Yes", "yes"), ("No", "no")]):
        y = 710 - index * 24
        c.drawString(96, y, label)
        form.radio(
            name="confirm",
            value=value,
            selected=(value == "no"),
            x=150,
            y=y - 3,
            buttonStyle="circle",
            borderColor=black,
            fillColor=white,
            size=14,
            shape="circle",
        )
    c.showPage()
    c.save()


if __name__ == "__main__":
    build(OUT)
    print(f"wrote {OUT}")
