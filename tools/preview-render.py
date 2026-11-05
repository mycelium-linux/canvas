#!/usr/bin/env python3
"""Rasterise the JSON that tools/preview-region.lua writes.

A preview, not a compositor: rectangles with rounded corners, alpha compositing,
and the same palette the shell resolves ink names to. Enough to judge rhythm,
hierarchy and colour before launching anything.
"""
import json
import sys
from PIL import Image, ImageDraw, ImageFont

PALETTE = {
    "background": (12, 13, 16),
    "surface": (24, 26, 31),
    "text": (238, 240, 245),
    "muted": (150, 155, 168),
    "primary": (108, 168, 255),
    "success": (110, 200, 140),
    "warning": (232, 178, 90),
    "danger": (226, 110, 110),
    "info": (130, 190, 240),
}
FONT = "/System/Library/Fonts/Helvetica.ttc"
WALL = (32, 34, 40)


def main(source: str, out: str, backdrop: str = "wall") -> None:
    document = json.load(open(source))
    nodes = document["nodes"]
    width, height = document["width"], document["height"]
    image = Image.new("RGBA", (width, height), (*WALL, 255) if backdrop == "wall" else (0, 0, 0, 0))
    for node in nodes:
        layer = Image.new("RGBA", (width, height), (0, 0, 0, 0))
        draw = ImageDraw.Draw(layer)
        box = [node["x"], node["y"], node["x"] + node["w"], node["y"] + node["h"]]
        if node["kind"] == "rect":
            colour = node["rgb"] if node["rgb"] and node["rgb"][0] is not None else PALETTE.get(node["ink"], PALETTE["text"])
            alpha = int(255 * max(0.0, min(1.0, node["opacity"])))
            radius = min(node["radius"], node["w"] / 2, node["h"] / 2)
            draw.rounded_rectangle(box, radius=radius, fill=(*colour, alpha))
        elif node["kind"] == "text":
            colour = node["rgb"] if node["rgb"] and node["rgb"][0] is not None else PALETTE.get(node["ink"], PALETTE["text"])
            size = max(6, int(round(node["size"])))
            try:
                font = ImageFont.truetype(FONT, size)
            except OSError:
                font = ImageFont.load_default()
            draw.text((node["x"], node["y"]), node["text"], font=font, fill=(*colour, 255))
        image = Image.alpha_composite(image, layer)
    image.convert("RGB").save(out)
    print(f"{len(nodes)} nodes -> {out} ({width}x{height})")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], *(sys.argv[3:4]))
