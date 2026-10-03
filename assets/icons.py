"""Cut the rendered mark out of its background and write one PNG per icon size.

Run by hand, not by the build. `build.rs` consumes the `app-*.png` files this writes
— it assembles them into `app.ico` and hands the small ones to the binary as raw
pixels — so the only reason to run this again is to change the art or add a size.

    python assets/icons.py "path/to/render.png"

The input is a square render of the mark on a **flat, near-black background**, with
the plate's own rounded square somewhere inside it. What this does with it:

1.  **Crops** to the plate. The background is the darkest thing in the image, so the
    bounding box of everything above a floor is the plate and nothing else. The art
    this was written for renders 1042x1060 inside a 1254px square; the 1.5% that is
    not square is squashed rather than padded, which no eye can see and which keeps
    the plate touching all four edges the way an icon's plate has to.
2.  **Masks** the corners, at each size, from the plate's own geometry rather than by
    scaling one alpha channel down: a square with circular corners at [`CORNER`],
    measured off the render. This is what stops the icon being a black square on a
    light taskbar, and rendering it per size is what keeps the 16px corner as clean
    as the 256px one.
3.  **Divides the background out** before scaling. The colours are multiplied by the
    mask at full resolution and divided by the scaled mask afterwards, so the black
    outside the plate contributes nothing to the pixels along its edge. Skipping this
    is the usual reason a downscaled icon has a dark fringe.
4.  **Sharpens the small sizes** a little. A downscale by 20:1 is soft by nature, and
    at 16px the three knobs are the whole icon; [`SHARPEN`] is the most that can be
    asked of an unsharp mask here before it rings around them.
"""

import os
import sys

import numpy as np
from PIL import Image, ImageFilter

#: Every size written, and the same list `build.rs` reads. 16/32/48/256 are what
#: Windows shows in its own views; 20/24/28/40/64 are the display-scaling steps the
#: notification area asks for between them; 128 is the large-icon view.
SIZES = [16, 20, 24, 28, 32, 40, 48, 64, 128, 256]

#: The plate's corner radius, as a fraction of the side. Fit to the render: a circular
#: corner at this radius clips none of the art and lets none of the background through.
CORNER = 0.207

#: Anything at or under this is sharpened after the downscale. Above it the art has
#: enough pixels to carry its own detail.
SHARPEN = 64

#: Unsharp mask radius and strength, in the units `ImageFilter.UnsharpMask` takes.
#: Gentle on purpose — twice this puts a visible dark ring around every knob.
UNSHARP = (0.5, 60)

#: Subsamples per pixel per axis when the corner mask is rendered.
SAMPLES = 8

#: Pixels this dark are the background rather than the plate.
FLOOR = 5


def plate(size, samples=SAMPLES):
    """The plate at `size`, as antialiased coverage in 0..1: a square, corners cut."""
    n = size * samples
    r = CORNER * n
    axis = np.arange(n) + 0.5
    edge = np.minimum(axis, n - axis)  # distance to the nearer edge, per axis
    dx, dy = np.meshgrid(edge, edge)
    # Inside the corner's own box, the cut is a quarter circle; everywhere else the
    # plate is solid out to the edge.
    u, v = np.clip((r - dx) / r, 0, 1), np.clip((r - dy) / r, 0, 1)
    covered = np.where((dx < r) & (dy < r), u * u + v * v <= 1.0, True)
    return covered.astype(np.float64).reshape(size, samples, size, samples).mean((1, 3))


def crop(path):
    """The plate, cut out of the render, and the mask that says where it is."""
    art = np.asarray(Image.open(path).convert("RGB")).astype(np.float64)
    rows, columns = np.nonzero(art.max(axis=2) > FLOOR)
    top, bottom = int(rows.min()), int(rows.max())
    left, right = int(columns.min()), int(columns.max())
    body = art[top : bottom + 1, left : right + 1]
    height, width = body.shape[:2]
    print(f"{path}: {art.shape[1]}x{art.shape[0]} -> plate {width}x{height} at {left},{top}")

    # The mask at the art's own resolution. Rendered at the square side and resampled
    # to the crop, because the crop is the 1.5% off square that step 1 squashes.
    side = max(width, height)
    coverage = Image.fromarray((plate(side, samples=2) * 255).astype(np.uint8), "L")
    coverage = coverage.resize((width, height), Image.LANCZOS)
    mask = np.asarray(coverage).astype(np.float64) / 255.0
    premultiplied = Image.fromarray(
        np.clip(body * mask[..., None], 0, 255).astype(np.uint8), "RGB"
    )
    return premultiplied, coverage


def icon(premultiplied, coverage, size):
    """One size: scaled, un-premultiplied, sharpened if it is small, and masked."""
    colour = np.asarray(premultiplied.resize((size, size), Image.LANCZOS)).astype(np.float64)
    weight = np.asarray(coverage.resize((size, size), Image.LANCZOS)).astype(np.float64) / 255.0
    # Divided by the coverage that was multiplied in, which is what keeps the plate's
    # edge the colour of the plate. Floored rather than guarded: where there is no
    # coverage there is no colour to recover, and the alpha below is zero there.
    straight = np.clip(colour / np.maximum(weight, 1e-3)[..., None], 0, 255)
    image = Image.fromarray(np.round(straight).astype(np.uint8), "RGB")
    if size <= SHARPEN:
        radius, percent = UNSHARP
        image = image.filter(ImageFilter.UnsharpMask(radius, percent, threshold=0))

    alpha = Image.fromarray(np.round(np.clip(plate(size), 0, 1) * 255).astype(np.uint8), "L")
    return Image.merge("RGBA", (*image.split(), alpha))


def main():
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <render.png>")
    here = os.path.dirname(os.path.abspath(__file__))
    premultiplied, coverage = crop(sys.argv[1])
    for size in SIZES:
        out = os.path.join(here, f"app-{size}.png")
        image = icon(premultiplied, coverage, size)
        # Straight RGBA, 8 bits a channel, and no palette: an ICO's PNG entries are
        # required to be 32bpp, and `build.rs` puts these files in one verbatim.
        image.save(out, optimize=True)
        with open(out, "rb") as f:
            header = f.read(26)
        colour_type = header[25]
        assert colour_type == 6, f"{out}: PNG colour type {colour_type}, not RGBA"
        print(f"  app-{size}.png  {os.path.getsize(out):>6} bytes")


if __name__ == "__main__":
    main()
