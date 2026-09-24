#!/usr/bin/env python3
"""Writes the pictures crates/covers/tests/decode.rs decodes, and what Pillow (libjpeg-turbo, libpng,
libwebp) decodes them to: the reference. Run again only to change them; the tests read the files.

    python3 crates/covers/testdata/make.py
"""
import math
import os
import random

from PIL import Image, ImageOps

here = os.path.dirname(os.path.abspath(__file__))
W, H = 40, 30


def path(name):
    return os.path.join(here, name)


def raw(im, name):
    with open(path(name), "wb") as f:
        f.write(im.convert("RGBA").tobytes())


# A small "photo": smooth gradients, a hard edge and some grain, so both flat and busy blocks are there.
random.seed(7)
photo = Image.new("RGB", (W, H))
for y in range(H):
    for x in range(W):
        r = int(255 * x / (W - 1))
        g = int(128 + 100 * math.sin(y / 4))
        b = 200 if x + y > 35 else 40
        n = random.randint(-12, 12)
        photo.putpixel((x, y), tuple(max(0, min(255, v + n)) for v in (r, g, b)))
photo.save(path("photo.png"))

photo.save(path("photo.jpg"), quality=90, subsampling=2)
raw(Image.open(path("photo.jpg")), "photo.jpg.rgba")
photo.save(path("photo-progressive.jpg"), quality=85, progressive=True, subsampling=2)
raw(Image.open(path("photo-progressive.jpg")), "photo-progressive.jpg.rgba")
photo.convert("L").save(path("grey.jpg"), quality=90)
raw(Image.open(path("grey.jpg")), "grey.jpg.rgba")
photo.save(path("photo.webp"), quality=90)
raw(Image.open(path("photo.webp")), "photo.webp.rgba")

# Transparency: alpha across, colour down. Lossless, but both encoders drop the colour of fully
# transparent pixels, so the reference is what they decode to.
alpha = Image.new("RGBA", (W, H))
for y in range(H):
    for x in range(W):
        alpha.putpixel((x, y), (255 - 6 * y, 40 + 5 * y, 90, int(255 * x / (W - 1))))
alpha.save(path("alpha.png"))
alpha.save(path("alpha.webp"), lossless=True)
raw(Image.open(path("alpha.png")), "alpha.png.rgba")
raw(Image.open(path("alpha.webp")), "alpha.webp.rgba")

# A palette with one transparent entry, as tRNS.
palette = photo.quantize(16)
palette.info["transparency"] = 0
palette.save(path("palette.png"), transparency=0)
raw(Image.open(path("palette.png")), "palette.png.rgba")


def area(im, side, k=2):
    """The middle square of `im` averaged exactly into side x side: each pixel made k x k, so that a
    whole number of them go to one (2.5 source pixels to one become 5), then those blocks averaged.
    Pillow's BOX filter weighs whole pixels, not areas."""
    w, h = im.size
    big = im.convert("RGB").resize((k * w, k * h), Image.NEAREST)
    s = min(w, h) * k
    x, y = (k * w - s) // 2, (k * h - s) // 2
    return big.crop((x, y, x + s, y + s)).reduce(s // side)


# The scaler: the middle 30x30 of the photo averaged into 12x12, and grown to 50x50 by Pillow's bilinear.
raw(area(photo, 12), "photo-12-area.rgba")
raw(photo.resize((50, 50), Image.BILINEAR, box=(5, 0, 35, 30)), "photo-50-bilinear.rgba")
# The JPEG decoded whole and averaged to 12x12, and decoded at half size by libjpeg-turbo's IDCT (Android's inSampleSize 2), then averaged to
# 12x12: what the two ways of shrinking a JPEG should look like.
raw(area(Image.open(path("photo.jpg")), 12), "photo.jpg-12-area.rgba")
half = Image.open(path("photo.jpg"))
half.draft("RGB", (20, 15))
raw(area(half, 12, 4), "photo.jpg-12-half.rgba")

# GIF: the first frame of an animation, and a first frame that covers only part of the screen. The rest of
# that one is transparent, as a browser shows it (Pillow paints it black), so the test checks it itself.
second = photo.transpose(Image.FLIP_LEFT_RIGHT)
photo.quantize(64).save(path("photo.gif"), save_all=True, append_images=[second.quantize(64)], duration=100, loop=0)
raw(Image.open(path("photo.gif")), "photo.gif.rgba")
with open(path("part.gif"), "wb") as f:
    # A 40x30 screen with no global palette, then one 20x10 frame at (8, 6) with its own two colours.
    f.write(b"GIF89a" + W.to_bytes(2, "little") + H.to_bytes(2, "little") + b"\x00\x00\x00")
    f.write(b"\x2c" + (8).to_bytes(2, "little") + (6).to_bytes(2, "little") + (20).to_bytes(2, "little") + (10).to_bytes(2, "little") + b"\x80")
    f.write(bytes([0, 0, 0, 250, 40, 60]))
    # LZW with a minimum code size of 2: clear, then 200 ones as literals, resetting before the table fills.
    out, acc, n = bytearray(), 0, 0
    def put(code, width):
        global acc, n
        acc |= code << n
        n += width
        while n >= 8:
            out.append(acc & 0xFF)
            acc >>= 8
            n -= 8
    for i in range(200):
        if i % 2 == 0:
            put(4, 3)  # clear: the table never grows past three bits
        put(1, 3)
    put(5, 3)  # end
    if n:
        out.append(acc & 0xFF)
    f.write(b"\x02")
    for i in range(0, len(out), 255):
        chunk = out[i:i + 255]
        f.write(bytes([len(chunk)]) + chunk)
    f.write(b"\x00\x3b")

# EXIF orientation: the same photo saved with each turn and mirror, as a camera writes it. The pixels are
# stored as they were. The reference is Pillow turning them (exif_transpose).
for o in (3, 6, 8, 2, 5, 7):
    exif = Image.Exif()
    exif[0x0112] = o
    photo.save(path(f"turned-{o}.jpg"), quality=90, subsampling=2, exif=exif.tobytes())
    raw(ImageOps.exif_transpose(Image.open(path(f"turned-{o}.jpg"))), f"turned-{o}.jpg.rgba")
exif = Image.Exif()
exif[0x0112] = 6
photo.save(path("turned-6.png"), exif=exif.tobytes())
photo.save(path("turned-6.webp"), quality=90, exif=exif.tobytes())
