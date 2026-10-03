# 아이콘 폰트

`Phosphor-Custom-Light.ttf`는 **Phosphor Icons의 Light 굵기를 고쳐 만든 사본**이다.
원본은 `src/Phosphor-Light.ttf`로 함께 두었고, 손대지 않는다.

고친 내용은 둘뿐이다.

1. **`U+F000`에 글리프 하나를 더했다** — 연속 스크롤 모드 아이콘. 원본은 `src/scroll-mode.svg`.
   Phosphor에 마땅한 것이 없어 `file`(U+E230)의 규격을 그대로 따라 직접 그렸다.
2. **모든 글리프의 `lsb`를 외곽선의 `xMin`에 맞췄다.** fontTools로 저장하면 글리프 상자를 다시
   재는데, 원본은 그 상자가 부풀려져 있어 다시 재면 `lsb`와 어긋난다. 어긋난 채로 두면 그만큼
   왼쪽으로 밀려 그려진다. 맞춰 두면 원본과 똑같이 그려진다(`scripts/build_icon_font.py` 주석).

다시 만들려면:

```
python3 scripts/build_icon_font.py
```

Inkscape(획을 외곽선으로 바꾸는 데 쓴다)와 `fonttools`가 필요하다. 앱 빌드에는 필요 없다 —
결과물인 `.ttf`만 저장소에 들어 있으면 된다.

## 원본 라이선스

Phosphor Icons — MIT License
Copyright (c) 2023 Phosphor Icons (Tobias Fried & Helena Zhang)
<https://phosphoricons.com> · <https://github.com/phosphor-icons/homepage/blob/master/LICENSE>

> Permission is hereby granted, free of charge, to any person obtaining a copy of this software
> and associated documentation files (the "Software"), to deal in the Software without restriction,
> including without limitation the rights to use, copy, modify, merge, publish, distribute,
> sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or
> substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
> BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
> NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
> DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
