"""Draws the app icon (a placeholder until there's an art direction) and
writes packaging/macos/AppIcon.icns. Needs Pillow and macOS's iconutil.

Usage: python3 packaging/macos/make_icon.py
"""

import pathlib
import shutil
import subprocess
import tempfile

from PIL import Image, ImageDraw, ImageFilter

HERE = pathlib.Path(__file__).resolve().parent
SIZE = 1024


def icon() -> Image.Image:
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    # The macOS icon grid: an 824 px rounded square, centered.
    body = (100, 100, 924, 924)
    shadow = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle((100, 112, 924, 936), 185, fill=(0, 0, 0, 140))
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(14)))
    draw.rounded_rectangle(body, 185, fill=(28, 26, 38, 255))
    draw.rounded_rectangle((116, 116, 908, 908), 172, outline=(70, 66, 92, 255), width=6)

    # A CRT screen showing color bars, bulged a little and lit from inside.
    screen = Image.new("RGBA", (640, 500), (0, 0, 0, 255))
    bars = [(192, 192, 192), (192, 192, 0), (0, 192, 192), (0, 192, 0), (192, 0, 192), (192, 0, 0), (0, 0, 192)]
    width = 640 / len(bars)
    sdraw = ImageDraw.Draw(screen)
    for i, color in enumerate(bars):
        sdraw.rectangle((round(i * width), 0, round((i + 1) * width), 370), fill=color + (255,))
    for i, color in enumerate([(0, 0, 192), (19, 19, 19), (192, 0, 192), (19, 19, 19), (0, 192, 192), (19, 19, 19), (192, 192, 192)]):
        sdraw.rectangle((round(i * width), 370, round((i + 1) * width), 420), fill=color + (255,))
    sdraw.rectangle((0, 420, 640, 500), fill=(16, 16, 24, 255))
    # A torn band, like the Line Tear module.
    band = screen.crop((0, 200, 640, 250))
    screen.paste(band, (46, 200))
    screen.paste(screen.crop((0, 270, 640, 290)), (-38, 270))
    # Scan lines.
    for y in range(0, 500, 6):
        sdraw.line((0, y, 640, y), fill=(0, 0, 0, 90), width=2)
    mask = Image.new("L", screen.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, 639, 499), 90, fill=255)
    glow = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    ImageDraw.Draw(glow).rounded_rectangle((172, 212, 852, 732), 100, fill=(120, 200, 255, 120))
    img.alpha_composite(glow.filter(ImageFilter.GaussianBlur(30)))
    img.paste(screen, (192, 232), mask)
    draw.rounded_rectangle((186, 226, 838, 738), 96, outline=(10, 10, 14, 255), width=12)

    # Two knobs under the screen.
    for cx, accent in ((360, (255, 47, 185)), (664, (47, 230, 255))):
        draw.ellipse((cx - 52, 770, cx + 52, 874), fill=(14, 14, 20, 255), outline=accent + (255,), width=8)
        draw.line((cx, 822, cx + 30, 790), fill=(235, 235, 245, 255), width=10)
    return img


def main():
    base = icon()
    base.save(HERE / "AppIcon.png")
    with tempfile.TemporaryDirectory() as tmp:
        iconset = pathlib.Path(tmp) / "AppIcon.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
                base.resize((size * scale, size * scale), Image.LANCZOS).save(iconset / name)
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(HERE / "AppIcon.icns")], check=True)
    print(HERE / "AppIcon.icns")


if __name__ == "__main__":
    if shutil.which("iconutil") is None:
        raise SystemExit("needs macOS's iconutil")
    main()
