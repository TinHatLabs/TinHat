#!/usr/bin/env python3
"""prestige-glyphs.py — the prestige token's glyphs (GAMES-V1-SPEC §3.1) for the Tin Hat icon pack.

One drawing, four looks, every one a single-colour fill (the pack's Breeze convention: class ColorScheme-Text,
fill currentColor) so both apps tint it with the prestige theme tokens:
  prestige-symbolic          outline hat                     (tier 1)
  prestige-filled-symbolic   filled hat                      (tiers 2, 3)
  prestige-ring-symbolic     filled hat + thin ring          (tier 4)
  prestige-star-symbolic     filled hat + ring + star notch  (tiers 5, 6)
The hat follows the app icon: a faceted foil cone leaning right with a fold near the peak, on a brim whose tips turn up.
Usage: glyph.py OUTDIR [--qml PRESTIGE_QML]
  -> OUTDIR/<size>/actions/<name>.svg for sizes 16 22 24 32 (same drawing, viewBox 0 0 24 24);
  --qml: rewrites the generated block of the desktop's Prestige.qml with the 24 px files' exact text (the desktop tints
  the SVG text before decode, like the pack's motion engine: Kirigami's recolour isn't reliable on every platform).
"""
import math
import os
import sys

# the hat silhouette, clockwise, in a 24 x 24 box
HAT = [
    (1.8, 14.0),   # left brim tip (blunt, turned up a little)
    (2.6, 13.6),
    (6.8, 14.9),   # crown meets brim, left
    (12.2, 2.6),   # peak
    (16.6, 4.2),   # the tip flops over to the right
    (14.6, 6.4),   # and tucks back under
    (17.6, 14.7),  # crown meets brim, right
    (21.4, 13.4),  # right brim tip (blunt)
    (22.2, 13.9),
    (20.9, 18.2),  # brim, bottom right
    (3.1, 18.2),   # brim, bottom left
]
# the crease that runs from the fold down to the brim (negative space, filled looks only)
CREASE = [(13.9, 8.0), (11.2, 13.6)]
CREASE_W = 1.1
OUTLINE_W = 2.3  # rank 1: ~1.35 px at 14 px (was 1.7: read too faint on OLED, lab7 review 11-19)
RING_R = 10.9
RING_W = 1.5
STAR_ANGLE = -45.0  # degrees, 0 = right, negative = up
STAR_R_OUT = 3.5
STAR_R_IN = 1.45
STAR_GAP_DEG = 26.0  # ring is cut this far either side of the star
HAT_SCALE_RINGED = 0.70  # the hat inside a ring
HAT_CENTRE = (12.0, 11.4)  # where the scaled hat is centred (optical centre of the silhouette)


def f(x):
    s = f"{x:.3f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def area(poly):
    return sum(poly[i][0] * poly[(i + 1) % len(poly)][1] - poly[(i + 1) % len(poly)][0] * poly[i][1] for i in range(len(poly))) / 2


def offset(poly, d, mitre_limit=2.2):
    """Offset a simple polygon by d (d > 0 = outwards). Mitred joins, bevelled past the limit."""
    n = len(poly)
    sign = 1 if area(poly) > 0 else -1  # SVG y-down: positive area = clockwise on screen
    out = []
    for i in range(n):
        p0, p1, p2 = poly[i - 1], poly[i], poly[(i + 1) % n]

        def normal(a, b):
            dx, dy = b[0] - a[0], b[1] - a[1]
            L = math.hypot(dx, dy)
            # outward normal for a clockwise-on-screen polygon
            return (-dy / L * -sign, dx / L * -sign)

        n1, n2 = normal(p0, p1), normal(p1, p2)
        bx, by = n1[0] + n2[0], n1[1] + n2[1]
        bl = math.hypot(bx, by)
        cos_half = max(bl / 2, 1e-6)
        m = 1 / cos_half
        if m <= mitre_limit or d < 0:  # inner offsets always mitre (a bevel there would cross itself)
            out.append((p1[0] + bx / bl * d * m, p1[1] + by / bl * d * m))
        else:  # bevel: two points along each edge's normal
            out.append((p1[0] + n1[0] * d, p1[1] + n1[1] * d))
            out.append((p1[0] + n2[0] * d, p1[1] + n2[1] * d))
    return out


def poly_d(poly):
    return "M" + " L".join(f"{f(x)} {f(y)}" for x, y in poly) + " Z"


def transform(poly, s, centre):
    # scale around the silhouette's bounding-box centre, then move that centre to `centre`
    xs, ys = [p[0] for p in HAT], [p[1] for p in HAT]
    cx, cy = (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2
    return [((x - cx) * s + centre[0], (y - cy) * s + centre[1]) for x, y in poly]


def crease_poly(a, b, w):
    dx, dy = b[0] - a[0], b[1] - a[1]
    L = math.hypot(dx, dy)
    nx, ny = -dy / L * w / 2, dx / L * w / 2
    return [(a[0] + nx, a[1] + ny), (b[0] + nx, b[1] + ny), (b[0] - nx, b[1] - ny), (a[0] - nx, a[1] - ny)]


def hat_filled(s=1.0, centre=None):
    hat = HAT if centre is None else transform(HAT, s, centre)
    cr = CREASE if centre is None else transform(CREASE, s, centre)
    # the crease is cut out (evenodd); it stops short of the brim edge so the brim stays one piece
    return poly_d(hat) + " " + poly_d(crease_poly(cr[0], cr[1], CREASE_W * s))


def hat_outline():
    outer = offset(HAT, OUTLINE_W / 2)
    inner = offset(HAT, -OUTLINE_W / 2)
    return poly_d(outer) + " " + poly_d(list(reversed(inner)))


def arc_band(r_out, r_in, a0, a1, cx=12.0, cy=12.0):
    """An annulus segment from angle a0 to a1 (degrees, a1 > a0, clockwise on screen)."""
    def pt(r, a):
        return (cx + r * math.cos(math.radians(a)), cy + r * math.sin(math.radians(a)))
    large = 1 if (a1 - a0) > 180 else 0
    p0, p1 = pt(r_out, a0), pt(r_out, a1)
    q1, q0 = pt(r_in, a1), pt(r_in, a0)
    return (f"M{f(p0[0])} {f(p0[1])} A{f(r_out)} {f(r_out)} 0 {large} 1 {f(p1[0])} {f(p1[1])} "
            f"L{f(q1[0])} {f(q1[1])} A{f(r_in)} {f(r_in)} 0 {large} 0 {f(q0[0])} {f(q0[1])} Z")


def ring_full():
    r_out, r_in = RING_R + RING_W / 2, RING_R - RING_W / 2
    # two half bands (one arc can't close a full circle)
    return arc_band(r_out, r_in, 0, 180) + " " + arc_band(r_out, r_in, 180, 360)


def ring_with_gap():
    r_out, r_in = RING_R + RING_W / 2, RING_R - RING_W / 2
    a0 = STAR_ANGLE + STAR_GAP_DEG
    a1 = STAR_ANGLE - STAR_GAP_DEG + 360
    return arc_band(r_out, r_in, a0, a1)


def star():
    cx = 12 + RING_R * math.cos(math.radians(STAR_ANGLE))
    cy = 12 + RING_R * math.sin(math.radians(STAR_ANGLE))
    pts = []
    for k in range(10):
        a = math.radians(-90 + k * 36)
        r = STAR_R_OUT if k % 2 == 0 else STAR_R_IN
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    return poly_d(pts)


def svg(size, d):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 24 24">'
            '<defs><style id="current-color-scheme" type="text/css">.ColorScheme-Text { color:#eff0f1; }</style></defs>'
            f'<path class="ColorScheme-Text" style="fill:currentColor" fill-rule="evenodd" d="{d}"/></svg>\n')


LOOKS = {
    "prestige-symbolic": lambda: hat_outline(),
    "prestige-filled-symbolic": lambda: hat_filled(),
    "prestige-ring-symbolic": lambda: hat_filled(HAT_SCALE_RINGED, HAT_CENTRE) + " " + ring_full(),
    "prestige-star-symbolic": lambda: hat_filled(HAT_SCALE_RINGED, HAT_CENTRE) + " " + ring_with_gap() + " " + star(),
}


BEGIN = "    // BEGIN generated by games-token glyph.py: the pack's 24 px prestige glyphs, byte for byte (do not edit)"
END = "    // END generated by games-token glyph.py"


def write_qml(path):
    import json
    text = open(path, encoding="utf-8").read()
    a, b = text.index(BEGIN), text.index(END)
    body = ["    readonly property var glyphs: ({"]
    names = list(LOOKS)
    for i, name in enumerate(names):
        comma = "," if i < len(names) - 1 else ""
        body.append(f"        {json.dumps(name)}: {json.dumps(svg(24, LOOKS[name]()))}{comma}")
    body.append("    })")
    text = text[: a + len(BEGIN)] + "\n" + "\n".join(body) + "\n" + text[b:]
    open(path, "w", encoding="utf-8").write(text)


def main():
    out = sys.argv[1]
    for size in (16, 22, 24, 32):
        d = os.path.join(out, str(size), "actions")
        os.makedirs(d, exist_ok=True)
        for name, fn in LOOKS.items():
            with open(os.path.join(d, name + ".svg"), "w") as fh:
                fh.write(svg(size, fn()))
    print("wrote", len(LOOKS) * 4, "files under", out)
    if "--qml" in sys.argv:
        write_qml(sys.argv[sys.argv.index("--qml") + 1])
        print("rewrote the glyph block of", sys.argv[sys.argv.index("--qml") + 1])


if __name__ == "__main__":
    main()
