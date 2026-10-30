"""Builds assets/brand/snapling.ico from the shapes of assets/brand/app-icon.svg."""
from PIL import Image, ImageDraw

S = 1024
K = S / 24


def quad(a, c, b, n=24):
    return [
        tuple((1 - t) ** 2 * p + 2 * (1 - t) * t * q + t * t * r for p, q, r in zip(a, c, b))
        for t in (i / n for i in range(n + 1))
    ]


def stroke(d, pts, width, color):
    r = width * K / 2
    for x, y in pts:
        d.ellipse([x * K - r, y * K - r, x * K + r, y * K + r], fill=color)
    d.line([(x * K, y * K) for x, y in pts], fill=color, width=int(width * K))


img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
d.rounded_rectangle([0, 0, S - 1, S - 1], radius=5.3 * K, fill="#295BE8")
white = "#FFFFFF"
stroke(d, [(4, 10), *quad((4, 7), (4, 4), (7, 4)), (10, 4)], 3, white)
stroke(d, [(4, 14), *quad((4, 17), (4, 20), (7, 20)), (10, 20)], 3, white)
stroke(d, [(20, 14), *quad((20, 17), (20, 20), (17, 20)), (14, 20)], 3, white)
spark = (
    quad((17, 2), (17.9, 6.1), (22, 7))
    + quad((22, 7), (17.9, 7.9), (17, 12))
    + quad((17, 12), (16.1, 7.9), (12, 7))
    + quad((12, 7), (16.1, 6.1), (17, 2))
)
d.polygon([(x * K, y * K) for x, y in spark], fill="#FFB04A")
sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
img.resize((256, 256), Image.LANCZOS).save(
    "assets/brand/snapling.ico", sizes=[(s, s) for s in sizes]
)
