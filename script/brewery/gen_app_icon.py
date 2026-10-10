"""Generate Brewery's placeholder app icons over Zed's icon files.

Each existing icon is replaced by one of the same size. Channel variants get a
coloured corner badge. Run from the repository root:
    python script/brewery/gen_app_icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

RESOURCES = Path("crates/zed/resources")
BADGES = {"dev": (90, 160, 90), "nightly": (90, 110, 200), "preview": (200, 150, 60)}
ICO_SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]


def channel_of(path: Path) -> str | None:
    stem = path.stem.replace("@2x", "")
    suffix = stem.removeprefix("app-icon").removeprefix("-")
    return suffix or None


def draw_icon(size: int, channel: str | None) -> Image.Image:
    image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    margin = size // 16
    box = (margin, margin, size - margin, size - margin)
    draw.rounded_rectangle(box, radius=size // 5, fill=(122, 74, 34))
    inner = (margin * 2, margin * 2, size - margin * 2, size - margin * 2)
    draw.rounded_rectangle(inner, radius=size // 6, fill=(199, 125, 58))
    font = ImageFont.load_default(size=int(size * 0.62))
    draw.text((size / 2, size / 2), "B", font=font, fill=(255, 248, 235), anchor="mm")
    if channel:
        r = size // 5
        draw.ellipse((size - margin - 2 * r, size - margin - 2 * r, size - margin, size - margin),
                     fill=BADGES[channel], outline=(255, 255, 255), width=max(1, size // 64))
    return image


def main() -> None:
    pngs = sorted(RESOURCES.glob("app-icon*.png"))
    icos = sorted((RESOURCES / "windows").glob("app-icon*.ico"))
    if not pngs or not icos:
        raise SystemExit("run from the repository root; Zed icon files not found")
    for path in pngs:
        width, _ = Image.open(path).size
        draw_icon(width, channel_of(path)).save(path)
        print(f"{path} {width}x{width}")
    for path in icos:
        draw_icon(256, channel_of(path)).save(path, sizes=ICO_SIZES)
        print(f"{path} ico")


if __name__ == "__main__":
    main()
