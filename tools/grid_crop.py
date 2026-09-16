#!/usr/bin/env python3
"""Measure Kindle framebuffer screenshots: crop a region and label a pixel grid.

Wrap coordinates in a 1264x1680 device capture are chosen by reading them off
these crops, since the demo capture taps have to be exact. Labels are drawn
every 50 px in x and 20 px in y; the crop is written next to the input unless
--out is given.

Usage:
    python3 tools/grid_crop.py capture.png 0,900,700,1120 [--out target/grid.png]
"""
import argparse
from pathlib import Path

from PIL import Image, ImageDraw

RED = (200, 0, 0)
BLUE = (0, 90, 220)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("image")
    parser.add_argument("region", help="x0,y0,x1,y1 in framebuffer pixels")
    parser.add_argument("--out")
    parser.add_argument("--step", type=int, default=50)
    args = parser.parse_args()

    x0, y0, x1, y1 = (int(value) for value in args.region.split(","))
    image = Image.open(args.image).convert("RGB").crop((x0, y0, x1, y1))
    draw = ImageDraw.Draw(image)
    for x in range(x0, x1, args.step):
        draw.line([(x - x0, 0), (x - x0, image.height)], fill=RED)
        draw.text((x - x0 + 2, 1), str(x), fill=RED)
    for y in range(y0, y1, 20):
        draw.line([(0, y - y0), (image.width, y - y0)], fill=BLUE)
        draw.text((2, y - y0 + 1), str(y), fill=BLUE)
    out = Path(args.out) if args.out else Path(args.image).with_suffix(".grid.png")
    image.save(out)
    print(f"{out} {image.width}x{image.height}")


if __name__ == "__main__":
    main()
