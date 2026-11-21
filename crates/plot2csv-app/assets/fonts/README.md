# Bundled fonts

The app embeds these two subset fonts instead of egui's `default_fonts`:

| File | Source | Licence |
|---|---|---|
| `Inter-Regular.subset.otf` | Inter 4.001 Regular, from the [rsms/inter](https://github.com/rsms/inter) release (`Inter-Regular.otf`) | SIL OFL 1.1, see `../../licenses/inter-ofl.txt` |
| `Hack-Regular.subset.ttf` | Hack 3.003 Regular, as shipped in `epaint_default_fonts` 0.37.0 (`fonts/Hack-Regular.ttf`) | MIT + Bitstream Vera, see `../../licenses/hack.txt` |

Both use the same subset: Latin (including Extended-A/B and Extended Additional), Greek,
Cyrillic, combining marks, punctuation, super/subscripts, currency, letterlike symbols,
arrows, maths operators, ⌘ ⌥ and other technical symbols, ①–⑦ and geometric shapes.
On the web, eframe draws any other character with the browser's fonts.

Regenerate with [fonttools](https://github.com/fonttools/fonttools):

```sh
U="U+0000-024F,U+0259,U+02B0-02FF,U+0300-036F,U+0370-03FF,U+0400-04FF,U+1E00-1EFF,U+2000-206F,U+2070-209F,U+20A0-20CF,U+2100-214F,U+2190-21FF,U+2200-22FF,U+2300-23FF,U+2460-24FF,U+25A0-25FF,U+FB01-FB02"
OPTS="--layout-features=kern,liga,calt,tnum,ccmp,mark,mkmk --no-hinting --desubroutinize"
pyftsubset Inter-Regular.otf --unicodes="$U" $OPTS --output-file=Inter-Regular.subset.otf
pyftsubset Hack-Regular.ttf  --unicodes="$U" $OPTS --output-file=Hack-Regular.subset.ttf
```

The icons in `../icons/` are unmodified solid SVGs from Font Awesome Free 7.3.1
(CC BY 4.0, see `../../licenses/fontawesome-free.txt`).
