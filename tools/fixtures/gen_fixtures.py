"""Generate synthetic chart fixtures with ground truth (requires Pillow).

    python3 -I tools/fixtures/gen_fixtures.py tests/fixtures

Each fixture is NAME.png plus NAME.json:
  {"panels": [{"x": {"min","max","log"}, "y": {...}, "series": [{"name","kind","color","points": [[x,y],...]}]}]}
Points are in data units; line series are sampled densely.
"""
import json, math, os, random, sys
from PIL import Image, ImageDraw, ImageFont, ImageFilter

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)
FONT = "/System/Library/Fonts/Supplemental/Arial.ttf"
def font(sz):
    try:
        return ImageFont.truetype(FONT, sz)
    except Exception:
        return ImageFont.load_default()
F, FT, FS = font(16), font(20), font(11)
random.seed(7)

class Axes:
    def __init__(self, d, box, xr, yr, xlog=False, ylog=False):
        self.d, (self.L, self.T, self.R, self.B) = d, box
        self.xr, self.yr, self.xlog, self.ylog = xr, yr, xlog, ylog
    def tx(self, v):
        lo, hi = self.xr
        if self.xlog: v, lo, hi = math.log10(v), math.log10(lo), math.log10(hi)
        return self.L + (self.R - self.L) * (v - lo) / (hi - lo)
    def ty(self, v):
        lo, hi = self.yr
        if self.ylog: v, lo, hi = math.log10(v), math.log10(lo), math.log10(hi)
        return self.B - (self.B - self.T) * (v - lo) / (hi - lo)
    def frame(self, frame=True, grid=None, xticks=(), yticks=(), xlab=str, ylab=str, ysup=False):
        d = self.d
        if grid:
            for v in xticks: d.line([self.tx(v), self.T, self.tx(v), self.B], fill=grid)
            for v in yticks: d.line([self.L, self.ty(v), self.R, self.ty(v)], fill=grid)
        if frame: d.rectangle([self.L, self.T, self.R, self.B], outline="black", width=2)
        else:
            d.line([self.L, self.B, self.R, self.B], fill="black", width=2)
            d.line([self.L, self.T, self.L, self.B], fill="black", width=2)
        for v in xticks:
            x = self.tx(v); d.line([x, self.B, x, self.B + 7], fill="black", width=2)
            s = xlab(v); w = d.textlength(s, font=F); d.text((x - w / 2, self.B + 10), s, fill="black", font=F)
        for v in yticks:
            y = self.ty(v); d.line([self.L - 7, y, self.L, y], fill="black", width=2)
            if ysup:
                e = str(int(round(math.log10(v)))); w = d.textlength("10", font=F) + d.textlength(e, font=FS)
                d.text((self.L - 12 - w, y - 9), "10", fill="black", font=F)
                d.text((self.L - 12 - d.textlength(e, font=FS), y - 15), e, fill="black", font=FS)
            else:
                s = ylab(v); w = d.textlength(s, font=F); d.text((self.L - 12 - w, y - 9), s, fill="black", font=F)
    def line(self, pts, col, width=3, dash=None):
        P = [(self.tx(x), self.ty(y)) for x, y in pts]
        if not dash:
            self.d.line(P, fill=col, width=width, joint="curve"); return
        on, off = dash; acc = 0.0; seg = [P[0]]
        for a, b in zip(P, P[1:]):
            L = math.dist(a, b); acc += L
            if (acc % (on + off)) < on: seg.append(b)
            else:
                if len(seg) > 1: self.d.line(seg, fill=col, width=width)
                seg = [b]
        if len(seg) > 1: self.d.line(seg, fill=col, width=width)
    def marker(self, x, y, col, shape="o", r=5):
        cx, cy = self.tx(x), self.ty(y)
        if shape == "o": self.d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=col)
        elif shape == "s": self.d.rectangle([cx - r, cy - r, cx + r, cy + r], fill=col)
        else: self.d.polygon([(cx, cy - r - 1), (cx - r, cy + r), (cx + r, cy + r)], fill=col)

def save(name, im, panels):
    im.save(os.path.join(OUT, name + ".png"))
    json.dump({"panels": panels}, open(os.path.join(OUT, name + ".json"), "w"), indent=1)

def sample(f, lo, hi, n=400):
    return [[lo + (hi - lo) * i / n, f(lo + (hi - lo) * i / n)] for i in range(n + 1)]

def fmt1(v): return f"{v:.1f}"

# 1. sine_basic: two coloured lines, linear axes, grid
im = Image.new("RGB", (800, 560), "white"); d = ImageDraw.Draw(im)
ax = Axes(d, (90, 50, 760, 480), (0, 6), (-1, 1))
ax.frame(grid=(220, 220, 220), xticks=range(7), yticks=[-1, -0.5, 0, 0.5, 1], ylab=fmt1)
s1, s2 = sample(math.sin, 0, 6), sample(math.cos, 0, 6)
ax.line(s1, (255, 165, 0)); ax.line(s2, (30, 60, 220), width=2)
d.text((400, 520), "Time (s)", fill="black", font=FT); d.text((320, 15), "Sine and cosine", fill="black", font=FT)
panel = {"x": {"min": 0, "max": 6, "log": False}, "y": {"min": -1, "max": 1, "log": False},
         "series": [{"name": "Orange", "kind": "line", "points": s1}, {"name": "Blue", "kind": "line", "points": s2}]}
save("sine_basic", im, [panel])
sine_img = im.copy()

# 2. log_markers: log y with superscript labels, two marker series
im = Image.new("RGB", (800, 560), "white"); d = ImageDraw.Draw(im)
ax = Axes(d, (110, 40, 760, 470), (0, 10), (1, 1000), ylog=True)
ax.frame(frame=False, xticks=range(0, 11, 2), yticks=[1, 10, 100, 1000], ysup=True)
pa = [[x, 2.0 * 1.8 ** x] for x in [0.5, 1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5]]
pb = [[x, 500.0 * 0.6 ** x] for x in [1, 2, 3, 4, 5, 6, 7, 8, 9]]
for x, y in pa: ax.marker(x, y, (200, 30, 30), "o")
for x, y in pb: ax.marker(x, y, (30, 90, 200), "s")
d.text((380, 515), "Dose", fill="black", font=FT)
save("log_markers", im, [{"x": {"min": 0, "max": 10, "log": False}, "y": {"min": 1, "max": 1000, "log": True},
     "series": [{"name": "Red", "kind": "markers", "points": pa}, {"name": "Blue", "kind": "markers", "points": pb}]}])

# 3. bw_dashed: same-colour solid / dashed / dotted lines with a legend
im = Image.new("RGB", (800, 560), "white"); d = ImageDraw.Draw(im)
ax = Axes(d, (90, 40, 760, 470), (0, 100), (0, 50))
ax.frame(xticks=range(0, 101, 20), yticks=range(0, 51, 10))
fa = lambda x: 5 + 0.4 * x
fb = lambda x: 45 - 0.35 * x
fc = lambda x: 25 + 15 * math.sin(x / 15)
sa, sb, sc = sample(fa, 0, 100), sample(fb, 0, 100), sample(fc, 0, 100)
ax.line(sa, (0, 0, 0), width=2); ax.line(sb, (0, 0, 0), width=2, dash=(14, 8)); ax.line(sc, (0, 0, 0), width=2, dash=(3, 5))
d.text((400, 515), "Distance (km)", fill="black", font=FT)
save("bw_dashed", im, [{"x": {"min": 0, "max": 100, "log": False}, "y": {"min": 0, "max": 50, "log": False},
     "series": [{"name": "solid", "kind": "line", "points": sa}, {"name": "dashed", "kind": "line", "points": sb},
                {"name": "dotted", "kind": "line", "points": sc}]}])

# 4. bars
im = Image.new("RGB", (800, 560), "white"); d = ImageDraw.Draw(im)
ax = Axes(d, (90, 40, 760, 470), (0, 5), (0, 40))
cats, vals = ["Jan", "Feb", "Mar", "Apr", "May"], [12, 27, 35, 18, 30]
for i, (c, v) in enumerate(zip(cats, vals)):
    x0, x1 = ax.tx(i + 0.2), ax.tx(i + 0.8)
    d.rectangle([x0, ax.ty(v), x1, ax.ty(0)], fill=(70, 130, 180))
    w = d.textlength(c, font=F); d.text(((x0 + x1) / 2 - w / 2, ax.B + 10), c, fill="black", font=F)
ax.frame(frame=False, yticks=range(0, 41, 10))
save("bars", im, [{"kind": "bar", "y": {"min": 0, "max": 40, "log": False},
     "series": [{"name": "bars", "kind": "bars", "points": [[i + 0.5, v] for i, v in enumerate(vals)], "labels": cats}]}])

# 5. multipanel: two framed plots side by side
im = Image.new("RGB", (1100, 480), "white"); d = ImageDraw.Draw(im)
a1 = Axes(d, (80, 40, 500, 400), (0, 10), (0, 100)); a1.frame(xticks=range(0, 11, 2), yticks=range(0, 101, 25))
a2 = Axes(d, (630, 40, 1050, 400), (-5, 5), (-2, 2)); a2.frame(xticks=range(-5, 6, 5), yticks=[-2, -1, 0, 1, 2])
q1 = sample(lambda x: x * x, 0, 10); q2 = sample(lambda x: 2 * math.tanh(x / 2), -5, 5)
a1.line(q1, (0, 150, 70)); a2.line(q2, (180, 0, 160))
d.text((250, 440), "(a)", fill="black", font=FT); d.text((800, 440), "(b)", fill="black", font=FT)
save("multipanel", im, [
    {"x": {"min": 0, "max": 10, "log": False}, "y": {"min": 0, "max": 100, "log": False}, "series": [{"name": "Green", "kind": "line", "points": q1}]},
    {"x": {"min": -5, "max": 5, "log": False}, "y": {"min": -2, "max": 2, "log": False}, "series": [{"name": "Magenta", "kind": "line", "points": q2}]}])

# 6. photo: sine chart photographed: perspective, rotation, lighting gradient, noise, blur, JPEG
W, H = sine_img.size
big = Image.new("RGB", (W + 200, H + 200), (235, 232, 225)); big.paste(sine_img, (100, 100))
def find_coeffs(src, dst):
    import itertools
    # solve 8x8 for PIL's perspective transform (dst -> src mapping)
    A, b = [], []
    for (x, y), (u, v) in zip(dst, src):
        A.append([x, y, 1, 0, 0, 0, -u * x, -u * y]); b.append(u)
        A.append([0, 0, 0, x, y, 1, -v * x, -v * y]); b.append(v)
    n = 8
    M = [row[:] + [bb] for row, bb in zip(A, b)]
    for c in range(n):
        p = max(range(c, n), key=lambda r: abs(M[r][c])); M[c], M[p] = M[p], M[c]
        for r in range(n):
            if r != c:
                f = M[r][c] / M[c][c]
                M[r] = [a - f * cc for a, cc in zip(M[r], M[c])]
    return [M[i][n] / M[i][i] for i in range(n)]
BW, BH = big.size
src = [(0, 0), (BW, 0), (BW, BH), (0, BH)]
dst = [(60, 40), (BW - 30, 10), (BW - 10, BH - 20), (20, BH - 60)]
photo = big.transform((BW, BH), Image.PERSPECTIVE, find_coeffs(src, dst), Image.BICUBIC, fillcolor=(90, 80, 70))
photo = photo.rotate(-3, resample=Image.BICUBIC, fillcolor=(90, 80, 70))
px = photo.load()
for y in range(BH):
    for x in range(BW):
        g = 0.78 + 0.22 * (x / BW) * (1 - 0.3 * y / BH)
        n = random.gauss(0, 5)
        r, gg, b = px[x, y]
        px[x, y] = (max(0, min(255, int(r * g + n))), max(0, min(255, int(gg * g + n))), max(0, min(255, int(b * g * 0.97 + n))))
photo = photo.filter(ImageFilter.GaussianBlur(0.8))
photo.save(os.path.join(OUT, "photo_sine.jpg"), quality=85)
json.dump({"panels": [panel]}, open(os.path.join(OUT, "photo_sine.json"), "w"), indent=1)

# 7. spectrum_grid: spreadsheet-style response spectra. Log X with minor grid
# lines (semi-transparent, so they darken where they cross), a title and a
# legend in white boxes inside the frame, two-line legend entries, thick
# dashed model curves and spiky solid curves that cross. Drawn at 2x and
# downsampled, which anti-aliases and leaves resampling fringes.
S = 2
im = Image.new("RGB", (850 * S, 520 * S), "white")
ax = Axes(ImageDraw.Draw(im), (73 * S, 13 * S, 812 * S, 479 * S), (0.01, 10), (0, 0.6), xlog=True)
vgrid = Image.new("RGBA", im.size, (0, 0, 0, 0)); hgrid = Image.new("RGBA", im.size, (0, 0, 0, 0))
dv, dh = ImageDraw.Draw(vgrid), ImageDraw.Draw(hgrid)
for e in (0.01, 0.1, 1):
    for k in range(1, 10):
        x = ax.tx(e * k); dv.line([x, ax.T, x, ax.B], fill=(0, 0, 0, 64), width=S)
for k in range(31):
    y = ax.ty(k * 0.02); dh.line([ax.L, y, ax.R, y], fill=(0, 0, 0, 64), width=S)
im = Image.alpha_composite(Image.alpha_composite(im.convert("RGBA"), vgrid), hgrid).convert("RGB")
d = ImageDraw.Draw(im); ax.d = d
FB, FL = font(16 * S), font(13 * S)
d.rectangle([ax.L, ax.T, ax.R, ax.B], outline="black", width=S)
for v, s in ((0.01, "0.01"), (0.1, "0.1"), (1, "1"), (10, "10")):
    w = d.textlength(s, font=FB); d.text((ax.tx(v) - w / 2, ax.B + 10 * S), s, fill="black", font=FB)
for k in range(7):
    s = "0" if k == 0 else f"0.{k}"; w = d.textlength(s, font=FB); d.text((ax.L - 8 * S - w, ax.ty(k / 10) - 9 * S), s, fill="black", font=FB)
d.text((390 * S, 494 * S), "Period (sec)", fill="black", font=FB)
def spectrum(a, b, u0, s1, s2, tail, extra=lambda u: 0.0):
    def f(x):
        u = math.log10(x)
        y = a + (b - a) * math.exp(-((u - u0) / s1) ** 2) if u <= u0 else tail + (b - tail) * math.exp(-((u - u0) / s2) ** 2)
        return y + extra(u)
    return [[10 ** (-2 + 3 * i / 600), f(10 ** (-2 + 3 * i / 600))] for i in range(601)]
spike = lambda u, c, w, h: h * math.exp(-((u - c) / w) ** 2)
curves = [  # legend order = drawing order; (lines, colour, dash, points)
    (["Model A (2008)", "Vs30=760m/s"], (112, 48, 160), (22, 12), spectrum(0.12, 0.265, -0.8, 0.45, 0.55, 0.02)),
    (["Model A (2008)", "Vs30=284m/s"], (255, 0, 0), (22, 12), spectrum(0.155, 0.345, -0.65, 0.45, 0.6, 0.05)),
    (["Model B (2013)", "Vs30=284m/s"], (15, 36, 62), (22, 12), spectrum(0.19, 0.53, -0.6, 0.55, 0.7, 0.08)),
    (["Site profile 1"], (150, 54, 52), None, spectrum(0.107, 0.245, -0.78, 0.45, 0.5, 0.005, lambda u: 0.004 * math.sin(60 * u))),
    (["Site profile 2"], (134, 166, 74), None, spectrum(0.117, 0.235, -0.65, 0.45, 0.5, 0.035, lambda u: spike(u, -0.4, 0.05, 0.09) + 0.006 * math.sin(50 * u))),
    (["Site profile 3"], (60, 150, 175), None, spectrum(0.137, 0.27, -0.6, 0.4, 0.42, 0.065, lambda u: spike(u, -0.84, 0.035, 0.12) + spike(u, -0.37, 0.04, 0.15))),
    (["Artificial motions -", "profile 3"], (220, 130, 54), None, spectrum(0.16, 0.30, -0.6, 0.4, 0.45, 0.095, lambda u: spike(u, -0.83, 0.04, 0.11) + spike(u, -0.36, 0.05, 0.16) + 0.01 * math.sin(40 * u))),
]
for _, col, dash, pts in curves:
    ax.line(pts, col, width=4 * S, dash=dash and (dash[0] * S, dash[1] * S))
d.rectangle([150 * S, 15 * S, 600 * S, 50 * S], fill="white")
d.text((160 * S, 20 * S), "Response Spectrum - Site Class C", fill="black", font=font(20 * S))
LX, LY = 612, 56
d.rectangle([LX * S, LY * S, 808 * S, (LY + 6 + sum(18 * len(c[0]) + 6 for c in curves)) * S], fill="white")
y = LY + 6
for lines, col, dash, _ in curves:
    d.line([(LX + 6) * S, (y + 8) * S, (LX + 40) * S, (y + 8) * S], fill=col, width=4 * S) if not dash else [
        d.line([(LX + 6 + k) * S, (y + 8) * S, (LX + 6 + k + 9) * S, (y + 8) * S], fill=col, width=4 * S) for k in (0, 13, 26)]
    for line in lines:
        d.text(((LX + 48) * S, y * S), line, fill="black", font=FL); y += 18
    y += 6
im = im.resize((850, 520), Image.LANCZOS)
im.save(os.path.join(OUT, "spectrum_grid.png"))
json.dump({"panels": [{"x": {"min": 0.01, "max": 10, "log": True}, "y": {"min": 0, "max": 0.6, "log": False},
     "series": [{"name": f"s{i}", "legend": " ".join(l.rstrip(" -") for l in lines), "kind": "line", "points": pts}
                for i, (lines, _, _, pts) in enumerate(curves)]}]},
    open(os.path.join(OUT, "spectrum_grid.json"), "w"), indent=1)
# 8. hazard_gray: a grayscale log-log hazard plot (matplotlib style) enlarged
# from a small figure, so every stroke has soft edges a few pixels wide.
# Light grey major and minor grid, a bundle of light grey curves of the same
# shade (not in the legend), and black mean / dashed / dash-dot curves that
# run through the bundle and meet at the top left. A framed legend, and the
# dark strips a screenshot leaves on the left and bottom edges.
def hazard(m, beta, lam=3.0):
    def f(x):
        q = 0.5 * math.erfc(math.log(x / m) / beta / math.sqrt(2))
        return 1 - math.exp(-lam * q)
    return f
def pattern_line(d, P, col, width, pattern):
    """Polyline P drawn with a dash pattern (on, off, on, off, ...) in px."""
    dense = [P[0]]
    for a, b in zip(P, P[1:]):
        n = max(1, int(math.dist(a, b) / 0.5))
        dense += [(a[0] + (b[0] - a[0]) * k / n, a[1] + (b[1] - a[1]) * k / n) for k in range(1, n + 1)]
    period, s, seg = sum(pattern), 0.0, []
    def on(s):
        t = s % period
        for i, p in enumerate(pattern):
            if t < p: return i % 2 == 0
            t -= p
        return False
    for a, b in zip(dense, dense[1:]):
        if on(s): seg.append(a)
        elif len(seg) > 1: d.line(seg + [a], fill=col, width=width); seg = []
        else: seg = []
        s += math.dist(a, b)
    if len(seg) > 1: d.line(seg, fill=col, width=width)
S = 2
FBB = lambda sz: ImageFont.truetype("/System/Library/Fonts/Supplemental/Arial Bold.ttf", sz)
im = Image.new("RGB", (640 * S, 520 * S), "white"); d = ImageDraw.Draw(im)
ax = Axes(d, (106 * S, 6 * S, 628 * S, 462 * S), (0.0005, 2), (1e-4, 1.5), xlog=True, ylog=True)
for e in range(-4, 1):
    for k in range(1, 10):
        v = k * 10.0 ** e
        if 0.0005 <= v <= 2: x = ax.tx(v); d.line([x, ax.T, x, ax.B], fill=(205, 205, 205), width=S)
        if 1e-4 <= v <= 1.5: y = ax.ty(v); d.line([ax.L, y, ax.R, y], fill=(205, 205, 205), width=S)
xs = [0.0005 * (2 / 0.0005) ** (i / 600) for i in range(601)]
m0, b0 = 0.00128, 1.705
random.seed(11)
for _ in range(60):
    f = hazard(m0 * math.exp(random.gauss(0, 0.4)), b0 * math.exp(random.gauss(0, 0.05)))
    P = [(ax.tx(x), ax.ty(f(x))) for x in xs if f(x) > 0.6e-4]
    d.line(P, fill=(185, 185, 185), width=S)
truth = []
for name, m, width, pattern in (("mean", m0, 3, None), ("Quantile 0.95", 1.35 * m0, 2, (7.4, 3.2)),
                                ("Quantile 0.05", 0.45 * m0, 2, (12.8, 3.2, 2, 3.2))):
    f = hazard(m, b0); pts = [[x, f(x)] for x in xs if f(x) >= 1e-4]
    P = [(ax.tx(x), ax.ty(y)) for x, y in pts]
    if pattern: pattern_line(d, P, (0, 0, 0), width * S, [p * S for p in pattern])
    else: d.line(P, fill=(0, 0, 0), width=width * S, joint="curve")
    truth.append({"name": name, "legend": name, "kind": "line", "points": pts})
d.rectangle([ax.L, ax.T, ax.R, ax.B], outline=(110, 110, 110), width=S)
FB = FBB(17 * S)
for v, s in ((0.0005, "0.0005"), (0.01, "0.01"), (0.1, "0.1"), (0.5, "0.5"), (1, "1"), (2, "2")):
    x = ax.tx(v); d.line([x, ax.B, x, ax.B + 5 * S], fill=(110, 110, 110), width=S)
    w = d.textlength(s, font=FB); d.text((x - w / 2, ax.B + 7 * S), s, fill="black", font=FB)
for v, s in ((1, "1"), (0.1, "0.1"), (0.01, "0.01"), (0.001, "0.001"), (1e-4, "1e-04")):
    y = ax.ty(v); d.line([ax.L - 5 * S, y, ax.L, y], fill=(110, 110, 110), width=S)
    w = d.textlength(s, font=FB); d.text((ax.L - 8 * S - w, y - 10 * S), s, fill="black", font=FB)
d.text((300 * S, 488 * S), "PGA [g]", fill="black", font=FBB(20 * S))
title = Image.new("RGB", (330 * S, 26 * S), "white")
ImageDraw.Draw(title).text((0, 0), "Annual probability of exceedance", fill="black", font=FBB(19 * S))
im.paste(title.rotate(90, expand=True), (8 * S, 70 * S))
LX, LY, FL = 112, 378, FBB(13 * S)
d.rectangle([LX * S, LY * S, 262 * S, 456 * S], fill="white", outline=(205, 205, 205), width=S)
for k, (t, width, pattern) in enumerate((("mean", 3, None), ("Quantile 0.95", 2, (7.4, 3.2)), ("Quantile 0.05", 2, (12.8, 3.2, 2, 3.2)))):
    y = (LY + 14 + 24 * k) * S; P = [((LX + 8) * S, y), ((LX + 44) * S, y)]
    if pattern: pattern_line(d, P, (0, 0, 0), width * S, [p * S for p in pattern])
    else: d.line(P, fill=(0, 0, 0), width=width * S)
    d.text(((LX + 52) * S, y - 9 * S), t, fill="black", font=FL)
im = im.resize((640, 520), Image.LANCZOS).resize((1280, 1040), Image.BICUBIC)
d = ImageDraw.Draw(im)
d.rectangle([0, 0, 6, 1039], fill=(14, 14, 14)); d.line([0, 1039, 1279, 1039], fill=(14, 14, 14))
im.save(os.path.join(OUT, "hazard_gray.png"))
json.dump({"panels": [{"x": {"min": 0.0005, "max": 2, "log": True}, "y": {"min": 1e-4, "max": 1.5, "log": True}, "series": truth}]},
          open(os.path.join(OUT, "hazard_gray.json"), "w"), indent=1)
print("fixtures written to", OUT)
