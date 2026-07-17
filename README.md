# PDF Outliner

PDF 북마크(목차) 편집, 저장, 내보내기/불러오기 기능을 갖춘 빠른 PDF 뷰어입니다.

- **플랫폼**: Apple Silicon Mac, Intel Mac, Windows
- **스택**: Rust · [egui](https://github.com/emilk/egui)/eframe (wgpu) · [pdfium-render](https://github.com/ajrcarey/pdfium-render) · [lopdf](https://github.com/J-F-Liu/lopdf)

## 기능

- **PDF 뷰어**: 렌더링, 확대/축소(버튼·트랙패드 핀치·Ctrl+스크롤), 팬, 텍스트 선택/복사,
  문서 전체 검색(Cmd/Ctrl+F), 문서 내 링크 클릭(페이지 이동/외부 URL 열기), 페이지 이동
  히스토리(Cmd+[/])
- **북마크 편집**: 추가/이름수정/삭제/드래그 재정렬/폴딩/Undo·Redo, 새 북마크 페이지 순서
  자동 삽입, 뷰어에 표시 중인 페이지의 북마크를 사이드바에서 자동 강조
- **PDF 자체에 저장**: pdfium은 읽기 전용이라 lopdf로 북마크(outline)만 다시 씀 — 원본
  구조는 그대로 유지
- **CSV/Excel 가져오기·내보내기**: 대량 편집/백업용 보조 기능(한글 인코딩 문제 없음)
- **사이드바 ↔ 뷰어 키보드 포커스 전환**(Tab), 화살표 키로 북마크 탐색 시 즉시 해당
  페이지 표시
- **한글 지원**: CSV BOM/EUC-KR 폴백, 파일명 NFD/NFC 정규화, macOS 한글 IME 조합 버그 수정
- **파일 연결**: macOS Finder 더블클릭/"다음으로 열기", Windows 기본 프로그램 지정
- 최근 파일 목록, 열린 파일이 같은 폴더에서 이름 바뀌면 자동 추적, 저장 위치를 잃으면
  "다른 이름으로 저장"으로 유도, 크래시 복구 자동저장

## 다운로드

[Releases](https://github.com/bjorkii/PDF-Outliner/releases)에서 플랫폼에 맞는 zip을
받아 압축을 풀고 실행하세요.

- **macOS**: 서명 없는 무료 배포라 Gatekeeper가 "확인되지 않은 개발자" 경고를 띄웁니다 —
  우클릭 → "열기"(또는 시스템 설정에서 "그래도 열기")로 실행하세요.
- **Windows**: 서명이 없어 SmartScreen이 경고를 띄웁니다 — "추가 정보" → "실행"으로
  진행하세요.

## 소스에서 빌드

```bash
git clone https://github.com/bjorkii/PDF-Outliner.git
cd PDF-Outliner
cargo build --workspace --release
```

pdfium 동적 라이브러리가 런타임에 필요합니다(릴리스 zip에는 이미 동봉됨). 직접 빌드해
실행할 때는 `PDFIUM_DYLIB_PATH` 환경변수로 경로를 지정하세요 —
[bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)에서 플랫폼별
바이너리를 받을 수 있습니다.

## What's New

전체 변경 이력은 각 [릴리스](https://github.com/bjorkii/PDF-Outliner/releases) 페이지의
"Full Changelog" 링크에서 커밋 단위로 확인할 수 있습니다. 아래는 버전별 주요 하이라이트만
정리한 것입니다.

### v0.1.8 (2026-07-17)
- 사이드바 선택 하이라이트/포커스 테두리 색상을 `#69178A`(흰 글자)로 통일
- 화살표 키로 북마크 순회 중 선택 항목이 화면 밖으로 벗어나면 부드럽게 중앙으로 스크롤

### v0.1.7 (2026-07-17)
- macOS: Finder 더블클릭/"다음으로 열기"로 PDF 열기 지원(앱이 꺼져 있을 때도 동작)
- Windows: 기본 프로그램 지정 시 앱 이름이 "ui"로 표시되던 버그 수정
- 사이드바 ↔ 뷰어 키보드 포커스 전환(Tab) 도입, 선택된 북마크와 현재 페이지 자동 동기화
- 사이드바에 여러 줄 항목이 많을 때 시각적 구분이 어렵던 문제 개선

### v0.1.6 (2026-07-16)
- 툴바에 최근 파일 드롭다운, Cmd/Ctrl+S 저장 단축키, 단축키 도움말 추가
- 열린 파일이 같은 폴더에서 이름이 바뀌면 자동으로 따라가고, 저장 위치를 잃으면 "다른
  이름으로 저장"으로 유도
- 한글 파일명이 자소 분리되어 보이던 문제 수정

### v0.1.5 (2026-07-16)
- Dock/Cmd+Tab 전환 화면에 기본 아이콘이 뜨던 문제 수정, 아이콘 여백 보정

### v0.1.4 (2026-07-16)
- 뷰어에 표시 중인 페이지에 해당하는 북마크를 사이드바에서 자동 강조
- 새 북마크를 페이지 순서에 맞게 자동 삽입, 사이드바 스크롤·인라인 편집 UX 개선
- 앱 재실행 시 마지막 페이지와 사이드바 위치 복원

### v0.1.3 (2026-07-14)
- 고배율 줌 시 발생하던 크래시 수정(Apple Silicon GPU 텍스처 한도 초과)
- Windows 배포본 실행 시 빈 콘솔 창이 함께 뜨던 문제 수정

### v0.1.2 (2026-07-14)
- 한글 IME 조합 버그 수정(세션 시작 후 첫 글자가 자소 분리되던 문제)

### v0.1.1 (2026-07-14)
- 앱 아이콘 추가(macOS .icns / Windows .ico)
- 한글 IME 조합 버그 수정(텍스트 필드 전환 시 자소가 새어 들어가던 문제)

### v0.1.0 (2026-07-13)
- 최초 릴리스: PDF 뷰어(렌더링/줌/팬/텍스트선택) + 북마크 사이드바(추가/수정/삭제/드래그
  재정렬) + PDF 자체 outline 저장
- 사이드바 선택 항목 Enter로 페이지 이동, 문서 내 링크 클릭, 페이지 이동 히스토리
