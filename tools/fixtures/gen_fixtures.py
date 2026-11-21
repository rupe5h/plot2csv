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
print("fixtures written to", OUT)
