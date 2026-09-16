#!/usr/bin/env python3
"""Turn captured Kindle frames into publishable artifacts.

Input is what `tests/e2e/kindle_demo.py` leaves behind:

    target/demo/frames.json    ordered storyboard: caption, dwell, source PNG
    target/demo/frames/*.png   raw Kindle framebuffer captures

Output, all under dist/demo/:

    kmux-demo.mp4           native 1264x1680 (Kindle screen), H.264 + faststart
    kmux-demo.gif           632x840 for web pages and chat
    kmux-demo-wide.mp4      1920x1080 with the device beside a caption panel
    kmux-demo-poster.png    full-size opening content frame

Composited frames are written to target/demo/composed/ so the burn-in can be
inspected without decoding video.
"""
import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
FRAMES_DIR = ROOT / "target/demo/frames"
STORYBOARD = ROOT / "target/demo/frames.json"
COMPOSED_DIR = ROOT / "target/demo/composed"
OUT_DIR = ROOT / "dist/demo"
FONT = ROOT / "kpm/waf/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"

SCREEN = (1264, 1680)
WIDE = (1920, 1080)
CAPTION_STRIP = 101          # replaces the Kindle title bar; keeps native screen size
INK = (0, 0, 0)
PAPER = (255, 255, 255)
WIDE_PAPER = (242, 242, 242)
GIF_WIDTH = 632


def font(size):
    return ImageFont.truetype(str(FONT), size)


def text_width(draw, text, face):
    return draw.textlength(text, font=face)


def fit_font(draw, text, max_width, start, minimum=24):
    """Largest monospace size at which `text` fits, so captions never clip."""
    size = start
    while size > minimum:
        face = font(size)
        if text_width(draw, text, face) <= max_width:
            return face
        size -= 2
    return font(minimum)


def wrap(draw, text, face, max_width):
    words = text.split()
    lines, current = [], ""
    for word in words:
        candidate = f"{current} {word}".strip()
        if current and text_width(draw, candidate, face) > max_width:
            lines.append(current)
            current = word
        else:
            current = candidate
    if current:
        lines.append(current)
    return lines


def caption_strip(image, caption, index, total):
    """Draw the caption bar over the Kindle title bar area."""
    draw = ImageDraw.Draw(image)
    draw.rectangle([0, 0, SCREEN[0], CAPTION_STRIP - 1], fill=PAPER)
    badge = font(34)
    label = f"{index + 1:02d}/{total}"
    badge_w = int(text_width(draw, label, badge)) + 34
    draw.rectangle([12, 18, 12 + badge_w, 82], fill=INK)
    draw.text((12 + badge_w / 2, 50), label, font=badge, fill=PAPER, anchor="mm")
    face = fit_font(draw, caption, SCREEN[0] - badge_w - 60, 42)
    draw.text((24 + badge_w, 50), caption, font=face, fill=INK, anchor="lm")
    draw.rectangle([0, CAPTION_STRIP - 5, SCREEN[0], CAPTION_STRIP - 1], fill=(190, 190, 190))
    progress = int(SCREEN[0] * (index + 1) / total)
    draw.rectangle([0, CAPTION_STRIP - 5, progress, CAPTION_STRIP - 1], fill=INK)


def render_screen(frame, index, total):
    image = device_screen(frame)
    caption_strip(image, frame.get("caption", ""), index, total)
    return image


def device_screen(frame):
    """A captured 1264x1680 device frame, or a card, without any caption burn-in."""
    if frame.get("kind") == "card":
        return render_card(frame)
    source = Image.open(FRAMES_DIR / frame["file"]).convert("RGB")
    if source.size == SCREEN:
        return source
    canvas = Image.new("RGB", SCREEN, PAPER)
    canvas.paste(source.crop((0, 0, *SCREEN)), (0, 0))
    return canvas


def render_card(frame, index=None, total=None):
    """Opening and closing cards: the only frames that are drawn, not captured."""
    image = Image.new("RGB", SCREEN, PAPER)
    draw = ImageDraw.Draw(image)
    draw.rectangle([24, 24, SCREEN[0] - 25, SCREEN[1] - 25], outline=INK, width=4)
    draw.text((SCREEN[0] / 2, 470), "kmux", font=font(190), fill=INK, anchor="mm")
    draw.line([(300, 610), (SCREEN[0] - 300, 610)], fill=INK, width=4)
    subtitle = frame.get("subtitle", "")
    if subtitle:
        face = fit_font(draw, subtitle, SCREEN[0] - 200, 54)
        draw.text((SCREEN[0] / 2, 700), subtitle, font=face, fill=INK, anchor="mm")
    y = 810
    for line in frame.get("lines", []):
        face = fit_font(draw, line, SCREEN[0] - 220, 44)
        draw.text((SCREEN[0] / 2, y), line, font=face, fill=INK, anchor="mm")
        y += 70
    footer = frame.get("footer", "")
    if footer:
        face = fit_font(draw, footer, SCREEN[0] - 200, 36)
        draw.text((SCREEN[0] / 2, SCREEN[1] - 150), footer, font=face, fill=INK, anchor="mm")
    if frame.get("caption") and index is not None:
        caption_strip(image, frame["caption"], index, total)
    return image


def render_wide(frame, index, total):
    """Landscape cut: the device screen beside a caption panel."""
    image = Image.new("RGB", WIDE, WIDE_PAPER)
    draw = ImageDraw.Draw(image)
    scale = 974 / SCREEN[1]
    device = device_screen(frame).resize((int(SCREEN[0] * scale), int(SCREEN[1] * scale)),
                                         Image.LANCZOS)
    image.paste(device, (70, 53))
    draw.rectangle([70, 53, 70 + device.width - 1, 53 + device.height - 1], outline=INK, width=2)

    x = 70 + device.width + 70
    width = WIDE[0] - x - 70
    draw.text((x, 150), "kmux", font=font(64), fill=INK)
    draw.line([(x, 240), (x + width, 240)], fill=INK, width=3)
    headline = frame.get("headline") or frame.get("caption") or frame.get("subtitle", "")
    face = fit_font(draw, headline, width, 84, minimum=40)
    y = 300
    for line in wrap(draw, headline, face, width):
        draw.text((x, y), line, font=face, fill=INK)
        y += face.size + 18
    detail = frame.get("detail", "")
    if detail:
        face = fit_font(draw, detail, width, 44, minimum=28)
        y += 20
        for line in wrap(draw, detail, face, width):
            draw.text((x, y), line, font=face, fill=(70, 70, 70))
            y += face.size + 12
    draw.rectangle([x, WIDE[1] - 170, x + width, WIDE[1] - 162], fill=(190, 190, 190))
    progress = int(width * (index + 1) / total)
    draw.rectangle([x, WIDE[1] - 170, x + progress, WIDE[1] - 162], fill=INK)
    draw.text((x, WIDE[1] - 140), f"{index + 1:02d}/{total}", font=font(38), fill=INK)
    return image


def load_storyboard(select=None):
    if not STORYBOARD.exists():
        sys.exit(f"error: {STORYBOARD} not found; run `just demo` (or the capture step) first")
    storyboard = json.loads(STORYBOARD.read_text())
    frames = storyboard["frames"]
    if select:
        wanted = [item.strip() for item in select.split(",")]
        frames = [frame for frame in frames
                  if any(frame["label"].startswith(item) for item in wanted)]
        if not frames:
            sys.exit(f"error: no frames match {select}")
    return frames


def encode(frames, composed):
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    concat = OUT_DIR / "frames.txt"
    with concat.open("w") as handle:
        handle.write("ffconcat version 1.0\n")
        for frame, path in zip(frames, composed):
            handle.write(f"file '{path}'\nduration {frame['dwell_ms'] / 1000:.3f}\n")
        handle.write(f"file '{composed[-1]}'\n")

    mp4 = OUT_DIR / "kmux-demo.mp4"
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
                    "-i", str(concat), "-fps_mode", "vfr", "-pix_fmt", "yuv420p",
                    "-c:v", "libx264", "-preset", "medium", "-crf", "20",
                    "-movflags", "+faststart", str(mp4)], check=True)
    gif = OUT_DIR / "kmux-demo.gif"
    palette = OUT_DIR / "palette.png"
    scale = f"scale={GIF_WIDTH}:-1:flags=lanczos"
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
                    "-i", str(concat), "-vf", f"fps=20,{scale},palettegen=stats_mode=diff",
                    str(palette)], check=True)
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
                    "-i", str(concat), "-i", str(palette), "-lavfi",
                    f"fps=20,{scale}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=3",
                    "-loop", "0", str(gif)], check=True)
    palette.unlink()
    concat.unlink()
    return mp4, gif


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--select", help="comma-separated label prefixes to include")
    parser.add_argument("--no-wide", action="store_true", help="skip the 16:9 cut")
    args = parser.parse_args()

    if not shutil.which("ffmpeg"):
        sys.exit("error: ffmpeg not found; install it (sudo apt-get install -y ffmpeg)")

    frames = load_storyboard(args.select)
    total = len(frames)
    COMPOSED_DIR.mkdir(parents=True, exist_ok=True)
    composed, wide = [], []
    for index, frame in enumerate(frames):
        portrait = render_card(frame, index, total) if frame.get("kind") == "card" \
            else render_screen(frame, index, total)
        path = COMPOSED_DIR / f"{frame['label']}.png"
        portrait.save(path)
        composed.append(path)
        wide_path = COMPOSED_DIR / f"wide-{frame['label']}.png"
        render_wide(frame, index, total).save(wide_path)
        wide.append(wide_path)
        print(f"composed {path.name}  {frame['dwell_ms'] / 1000:.1f}s  "
              f"{frame.get('caption') or frame.get('subtitle', '')}")

    mp4, gif = encode(frames, composed)
    poster = OUT_DIR / "kmux-demo-poster.png"
    shutil.copyfile(composed[1] if len(composed) > 1 else composed[0], poster)
    print(f"\n{mp4}\n{gif}\n{poster}")
    if not args.no_wide:
        wide_mp4 = OUT_DIR / "kmux-demo-wide.mp4"
        concat = OUT_DIR / "wide.txt"
        with concat.open("w") as handle:
            handle.write("ffconcat version 1.0\n")
            for frame, path in zip(frames, wide):
                handle.write(f"file '{path}'\nduration {frame['dwell_ms'] / 1000:.3f}\n")
            handle.write(f"file '{wide[-1]}'\n")
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
                        "-i", str(concat), "-fps_mode", "vfr", "-pix_fmt", "yuv420p",
                        "-c:v", "libx264", "-preset", "medium", "-crf", "20",
                        "-movflags", "+faststart", str(wide_mp4)], check=True)
        concat.unlink()
        print(wide_mp4)


if __name__ == "__main__":
    main()
