#!/usr/bin/env python3
"""`assets/fonts/Phosphor-Custom-Light.ttf`를 다시 만든다.

Phosphor Light에 우리 아이콘(연속 스크롤 모드)을 더한 사본을 뽑는다. 원본
`assets/fonts/src/Phosphor-Light.ttf`는 건드리지 않으므로, 아이콘을 고치거나 Phosphor를
새 판으로 바꿔도 이 스크립트만 다시 돌리면 된다.

필요한 것: Inkscape(획을 외곽선으로 바꾼다)와 `fonttools`. 둘 다 **앱 빌드에는 필요 없다** —
결과물 `.ttf`가 저장소에 들어 있으면 그만이다.

    python3 scripts/build_icon_font.py
"""

import io
import re
import subprocess
import sys
from pathlib import Path

from fontTools.misc.transform import Transform
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib import SVGPath
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
SRC_FONT = ROOT / "assets/fonts/src/Phosphor-Light.ttf"
OUT_FONT = ROOT / "assets/fonts/Phosphor-Custom-Light.ttf"
STYLE_CSS = ROOT / "assets/fonts/src/style.css"
CODEPOINTS = ROOT / "assets/fonts/codepoints.txt"
INKSCAPE = "/usr/local/bin/inkscape"

# 더할 글리프: (코드포인트, 글리프 이름, 원본 SVG)
EXTRA = [(0xF000, "uniF000", ROOT / "assets/fonts/src/scroll-mode.svg")]

# SVG 256 viewBox → 폰트 단위(upem 1024).
#
# 배율은 **정확히 4**이고 y는 뒤집혀 960에서 내려온다. `file`(U+E230)의 실제 외곽선 상자
# (168,40)~(856,856)를 그 SVG의 잉크 상자(x 42~214, y 26~230, 획 12 기준)와 맞춰 역산한 값이다.
# 폰트에 **저장된** 글리프 상자는 부풀려져 있어 그 값으로 재면 안 된다 — `recalcBounds`로 다시
# 재야 맞다(아래 lsb 주석과 같은 사정).
XFORM = Transform(4, 0, 0, -4, 0, 960)


def outline(svg: Path) -> Path:
    """획을 외곽선으로 바꾼 SVG. 글리프는 채워진 도형이라야 한다."""
    out = svg.with_suffix(".outline.svg")
    subprocess.run(
        [INKSCAPE, "--actions", f"select-all;object-stroke-to-path;"
         f"export-filename:{out};export-plain-svg;export-do", str(svg)],
        check=True, capture_output=True,
    )
    # 배경용 <rect width=256 height=256 fill="none">도 fontTools가 도형으로 읽어 그린다 — 뺀다.
    text = re.sub(r"<rect\b[^>]*/>", "", io.open(out, encoding="utf-8").read())
    io.open(out, "w", encoding="utf-8").write(text)
    return out


def main() -> int:
    if not SRC_FONT.exists():
        print(f"원본 폰트가 없습니다: {SRC_FONT}", file=sys.stderr)
        return 1
    font = TTFont(SRC_FONT)

    for code, name, svg in EXTRA:
        pen = TTGlyphPen(None)
        SVGPath(str(outline(svg)), transform=XFORM).draw(Cu2QuPen(pen, max_err=1.0))
        font["glyf"][name] = pen.glyph()
        font["hmtx"][name] = (font["head"].unitsPerEm, 0)  # lsb는 아래에서 한꺼번에 맞춘다
        if name not in font.getGlyphOrder():
            font.setGlyphOrder(list(font.getGlyphOrder()) + [name])
        for table in font["cmap"].tables:
            table.cmap[code] = name
        print(f"  더함: U+{code:04X} ← {svg.name}")

    # **모든 글리프의 lsb를 외곽선의 xMin에 맞춘다.**
    #
    # fontTools는 저장할 때 글리프 상자를 다시 잰다. 원본 Phosphor는 그 상자가 부풀려져 있어서
    # (`file`은 xMin이 0으로 적혀 있지만 실제 외곽선은 168에서 시작한다) 다시 재면 xMin이 바뀌는데,
    # hmtx의 lsb는 0 그대로 남는다. 둘이 어긋나면 래스터라이저가 그 차이만큼 글리프를 **옮겨서**
    # 그린다 — 아이콘이 칸 왼쪽에 붙어 보인다. 맞춰 두면 원본과 똑같이 그려진다(검증: 아이콘 12개의
    # 좌우 여백이 원본과 픽셀까지 같다).
    glyf = font["glyf"]
    for name in font.getGlyphOrder():
        glyph = glyf[name]
        glyph.recalcBounds(glyf)
        if glyph.numberOfContours:
            advance, _ = font["hmtx"][name]
            font["hmtx"][name] = (advance, glyph.xMin)

    # 고친 사본임이 어디서 보더라도 드러나게 이름을 바꾼다. MIT 고지(13·14)는 그대로 둔다.
    for rec in font["name"].names:
        value = rec.toUnicode()
        if rec.nameID == 6:
            rec.string = value.replace("Phosphor", "Phosphor-Custom")
        elif rec.nameID in (1, 3, 4, 16):
            rec.string = value.replace("Phosphor", "Phosphor Custom")
    font["name"].setName(
        "Phosphor Light에 PDF Outliner용 글리프를 더한 사본. 원본은 MIT(assets/fonts/NOTICE.md). "
        "scripts/build_icon_font.py로 다시 만들 수 있다.",
        10, 3, 1, 0x409,
    )

    font.save(OUT_FONT)
    print(f"  만듦: {OUT_FONT.relative_to(ROOT)}")

    # 이름표를 함께 적는다. 폰트의 글리프 이름은 `uniE230` 꼴이라 **어느 아이콘인지 알 수 없다** —
    # `crates/ui/src/icons.rs`의 시험이 이 표를 보고 상수마다 이름이 맞는지 확인한다. 그러지 않으면
    # 코드포인트를 잘못 적어도 "글리프가 있다"는 것만 확인되고 엉뚱한 그림이 나온다(실제로 겪었다:
    # minus를 U+E352로 적었는데 그것은 number-circle-eight였다).
    css = io.open(STYLE_CSS, encoding="utf-8").read()
    pairs = re.findall(r"\.ph-light\.ph-([a-z0-9-]+):before\s*\{\s*content:\s*\"\\?([0-9a-f]{4})\"", css)
    lines = [f"{name}\t{code}" for name, code in sorted(pairs)]
    lines += [f"{name}\t{code:04x}" for code, _, name in
              ((c, g, s.stem) for c, g, s in EXTRA)]
    io.open(CODEPOINTS, "w", encoding="utf-8").write("\n".join(lines) + "\n")
    print(f"  만듦: {CODEPOINTS.relative_to(ROOT)} ({len(lines)}줄)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
