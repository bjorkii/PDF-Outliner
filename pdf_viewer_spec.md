# Rust PDF 뷰어 프로젝트 사양서

> 목적: **Sumatra급 속도의 PDF 뷰어 + 북마크(outline)의 편집·저장.** PDF 자체에 내장된
> 북마크(목차)를 열 때 자동 로드하고, 추가/이름수정/삭제/드래그 계층이동한 뒤 **PDF
> 문서 자체에** 다시 저장하는 것이 핵심 기능. CSV/Excel import·export는 보조 기능(대량
> 편집/백업용). ⚠️ 이 핵심 요구사항이 예전 세션 정리 과정에서 문서에서 빠진 채 전달된 적이
> 있었음(2026-07-13, 사용자 정정) — 새 세션에서 이 문서를 요약/재작성할 때 이 줄을 누락하지
> 말 것.
> 플랫폼: Apple Silicon Mac, Intel Mac, Windows
> 스택: Rust
> 이 문서는 세션 간 인계용 요약본입니다. 새 세션에서는 이 문서를 먼저 공유하세요.

---

## 0. 새 세션 빠른 시작

**빌드/실행**:
```bash
cd "/Users/researchkofa/Documents/VibeCoding/PDF/PDF Bookmark Editor-Rust"
cargo build --workspace   # rustc/cargo는 ~/.zshrc에 이미 PATH 잡혀있음
cargo test --workspace    # 전부 통과해야 정상 (bookmark 13, import_export 3+2, pdf_outline_writer 6, ui 3 등)
./target/debug/pdf_viewer [선택: 열 pdf 경로]
```
pdfium dylib은 Homebrew `ocrmypdf` 종속성 경로를 하드코딩 폴백으로 씀(`crates/ui/src/app.rs`의 `create_engine()`) — `PDFIUM_DYLIB_PATH` 환경변수로 override 가능. 경로: `/opt/homebrew/Cellar/ocrmypdf/17.8.0/libexec/lib/python3.14/site-packages/pypdfium2_raw/libpdfium.dylib`

**git**: 2026-07-13에 프로젝트 루트에 `git init` 완료(그 전엔 git repo 아니었음), 첫 커밋 존재. `target/`은 `.gitignore` 처리됨(6GB+). 앞으로 변경할 때마다 `git add . && git commit`으로 스냅샷 남길 것 — 이제 `git log`/`git diff`로 이력 추적 가능.

**핵심 기능(구현 완료, 사용자 실기 검증 대부분 완료)**: PDF 뷰어(렌더링/줌/팬/텍스트선택복사) + 북마크 사이드바(추가/이름수정/삭제/드래그재정렬/폴딩/Undo·Redo/단축키) + PDF 자체 outline에 저장(lopdf) + CSV/Excel import·export. 자세한 아키텍처는 §4 참고. 기능별 검증 상태는 §6.

**아직 실기 미확인**: F2 단축키, 사이드바 정렬 미세조정 정도만 남음(둘 다 사소함) — §6 참고. wgpu 크래시 수정은 사용자 재실행으로 해결 확인됨.

**이 세션 환경의 제약**: 화면 캡처/실제 GUI 조작 불가능한 샌드박스. 검증은 (a) headless 예제/테스트로 pdfium·lopdf 로직을 실제 라이브러리로 확인, (b) egui/wgpu 소스를 직접 읽어 API 동작 확인, (c) 사용자가 실행해보고 리포트 → 원인 규명 → 수정 → 재확인 요청, 의 조합. **GUI 상호작용 버그는 사용자의 실기 리포트 없이는 발견 자체가 어려움** — 다음 세션도 이 사이클을 유지할 것. 자세한 재발방지용 기술 교훈은 §7.

**디버깅 도구**(재사용 가능):
- `cargo run --example dump_outline -p pdf_engine -- <pdfium_dylib> <pdf>` — PDF 내장 북마크 트리 출력
- `cargo run --example dump_chars -p pdf_engine -- <pdfium_dylib> <pdf> <page_0based>` — 페이지 내 문자별 좌표/회전각 출력
- `cargo run --example render_crop -p pdf_engine -- <pdfium_dylib> <pdf> <page_0based> <out.png> [char_index]` — 렌더링 결과를 PNG로 저장해 Read 툴로 육안 확인(화면 캡처 안 되는 세션에서 유일한 시각 확인 수단)
- `cargo run --example smoke_test -p pdf_engine -- <pdfium_dylib> <pdf> [최대_페이지수]` — 임의 PDF 렌더링/텍스트선택 구조적 정상성 확인
- 테스트용 실제 PDF 샘플: `pdf-samples/` 안에 여러 개. **일부는 사용자가 수동 GUI 테스트에 실사용 중이라 자동화 테스트가 함부로 건드리면 안 됨**(§7 "테스트 설계 원칙" 참고) — 자동화 테스트는 항상 pristine 백업을 임시 디렉터리에 복사해서 쓸 것.

---

## 1. 전체 아키텍처 개요

| 영역 | 선택 | 이유 |
|---|---|---|
| PDF 렌더링 엔진 | **pdfium-render** (crates.io, 0.9.x) | Chromium PDFium 바인딩. 라이선스 깔끔(Apache 계열). 단, outline **쓰기** API는 없음 — §4 참고 |
| GUI 프레임워크 | **egui + eframe (wgpu 백엔드)** | Immediate-mode, 바이너리 수 MB, 콜드 스타트 매우 빠름. Tauri(webview)는 초기화 오버헤드로 제외 |
| 북마크 편집 | 메모리상 `Vec<BookmarkNode>` 트리 + 자체 드래그 재정렬 로직(`bookmark` 크레이트) | egui-arbor/egui_ltreeview 같은 서드파티 트리 위젯 대신 직접 구현(§4) |
| PDF 자체 북마크 쓰기 | **lopdf** (`IncrementalDocument`) | pdfium은 읽기만 가능 — §4 참고 |
| 북마크 저장(보조) | CSV/Excel export·import | SQLite는 아직 미착수, 우선순위 낮음(§6) |
| CSV | `csv` 크레이트 (+ 수동 BOM 처리) | §2 참고 |
| Excel | 읽기: `calamine` / 쓰기: `rust_xlsxwriter` | 인코딩 이슈 없음 |
| 클립보드 | `arboard` | macOS/Windows 유니코드(한글) 클립보드 안정적 |
| PDF 텍스트 API | pdfium-render `PdfPageText` (`FPDFText_*`) | §3 참고 |

PDFium은 런타임 바인딩 방식 → `bblanchon/pdfium-binaries`에서 플랫폼별 prebuilt 라이브러리를 받아 앱 번들에 동봉 예정(아직 미착수, §6).

---

## 2. 한글 인코딩 (CSV 고질 문제)

**원인**: Windows Excel은 BOM 없는 CSV를 시스템 로케일(CP949)로 추측 → UTF-8로 저장해도 한글 깨짐.

**대응**:
- **Export**: 파일 맨 앞에 UTF-8 BOM(`EF BB BF`) 수동 기록 후 `csv::Writer` 사용.
- **Import**: `encoding_rs`로 BOM 유무 확인, 없으면 자동 감지, 실패 시 EUC-KR로 폴백(오래된 한글 CSV 대비).
- **정책**: xlsx를 기본 권장 export 포맷으로 안내(인코딩 이슈 원천 차단), CSV는 호환용 보조.

---

## 3. 뷰어 인터랙션

### 줌/팬
- 트랙패드 핀치: `egui-winit`이 macOS `WindowEvent::PinchGesture`를 `egui::Event::Zoom`으로 이미 변환해줌 — `ctx.input(|i| i.zoom_delta())`만 읽으면 됨(raw winit 후킹 불필요, §7 참고).
- 트랙패드 두 손가락 스와이프 = 패닝(Ctrl 안 눌렸을 때 `smooth_scroll_delta`를 `pan_offset`에 반영). 세 손가락 드래그는 macOS 손쉬운 사용 설정이 OS 레벨에서 일반 드래그로 합성해줘서 별도 처리 불필요.
- Windows 마우스 휠: `smooth_scroll_delta` + Ctrl 조합 관례.
- 명시적 +/- 버튼 툴바 병행 배치(비전문 사용자 대비).
- 확대 시 drag 탐색: 드래그 델타로 뷰포트 오프셋 이동, 페이지 경계 clamp.

### 페이지 이동
- 방향키(북마크 미선택 시), 페이지 번호 입력 필드(파싱→범위 clamp→점프).

### 텍스트 선택/복사 — 핵심 설계 원칙
> **OCR 프로세싱 기능은 범위 밖.** 기존에 텍스트 레이어가 있는 PDF에서 "선택은 되는데 복사가 안 되거나 깨지는" 버그 대응이 목적. 원인은 PDF 자체 결함(ToUnicode CMap 누락, 앱이 못 고침)과 뷰어 구현 결함(좌표 기반 선택 로직, 우리가 고칠 부분) 둘로 나뉨.

**채택 설계: 좌표 교차 방식이 아닌 "문자 인덱스 range" 기반 선택**
1. 마우스 다운 시 `FPDFText_GetCharIndexAtPos`로 좌표→문자 인덱스 변환(PDFium 히트테스트에 위임)
2. 드래그 중엔 시작~끝 **인덱스**만 갱신(좌표 재계산 아님)
3. 복사 시 `FPDFText_GetText(start_index, count)`로 range 텍스트 추출 — ToUnicode 매핑을 pdfium이 처리
4. 줄바꿈은 문자별 y좌표 변화 감지로 개행 삽입

**스큐(기울어진 스캔) 대응**: `loose_bounds()`(축정렬 박스)를 회전 없이 그대로 사용(`crates/pdf_engine/src/skew.rs`). 원래는 `angle_radians()`로 quad를 회전시킬 계획이었으나, 실제 PDF 2건에서 이 값이 시각적 회전과 다르다는 걸 발견해 철회함 — 자세한 근거는 §7. 스캔 스큐 문서에서는 하이라이트가 살짝 헐렁할 수 있지만, 디자인 문서에서 하이라이트가 엉뚱한 방향으로 뒤집히는 것보다 안전.
- **미해결**: "진짜로 시각적으로 기울어진" 스캔 텍스트는 아직 샘플에서 못 만남 — 만나면 `angle_radians()`와 실제 시각 기울기 관계 재조사 필요.

**세로쓰기 대응**: CID 폰트 + vertical writing mode로 인코딩된 PDF는 문자 인덱스 순서가 논리적 읽기 순서를 그대로 따름(별도 분기 불필요) — 실제 세로 칼럼 디자인 문서(`BZR001088_01.pdf`)로 확인함. 하이라이트도 axis-aligned quad라 자연스럽게 맞음. 텍스트 레이어 없는(이미지만 있는) 페이지는 선택 불가 — 정상 동작.

---

## 4. 북마크

### 데이터 모델 (`crates/bookmark`)
```rust
struct BookmarkNode {   // 사이드바/편집용 트리 표현
    id: Uuid,
    title: String,
    page: u32,          // 1-based
    children: Vec<BookmarkNode>,
}
struct BookmarkRow {    // CSV/Excel/PDF outline 평탄화용
    filename: String,
    depth: u32,
    title: String,
    page: u32,
}
```
트리 조작 함수(전부 `bookmark` 크레이트 lib.rs에서 export, 단위테스트 있음): `build_tree`/`flatten_tree`(depth 스택 기반 상호 역연산), `insert_node`(parent_id 자식으로 삽입, 없으면 폴백 시 최상위), `remove_node`, `move_node`(Before/After/Inside 드래그 재구성, 사이클 방지), `parent_of`, `sibling_or_parent_after_removal`(삭제 후 다음 선택 대상 계산).

### PDF 자체 북마크(outline) 읽기/쓰기 — 렌더링(pdfium)과 편집(lopdf) 분리

**배경**: pdfium은 outline **읽기 API만** 있고 쓰기 API가 PDFium C 라이브러리 자체에 없음. pdf_oxide를 대안으로 검토했으나(`pdf_oxide_research.md`) 실제 소스 확인 결과 기존 문서 편집용 outline API가 없어 기각. **lopdf** 채택 — `add_bookmark()`/`build_outline()`, `IncrementalDocument`(안전한 증분 저장), 한글 등 비ASCII 제목 UTF-16BE 인코딩까지 이미 지원.

**아키텍처**:
```
읽기: 문서 열기 시 → pdfium이 이미 열어둔 PdfDocument에서
      pdf_engine::outline::read_bookmarks(&document) -> Vec<BookmarkNode>

편집: 사이드바 조작 → 메모리상의 Vec<BookmarkNode>만 갱신

저장: pdf_outline_writer::write_bookmarks_incremental(source_pdf, out_path, &bookmarks)
      └─ lopdf::IncrementalDocument::load(source_pdf)
           └─ catalog을 new_document로 복제 → bookmarks를 add_bookmark()/build_outline()로 변환
           └─ catalog의 /Outlines를 새 outline id로 교체(빈 트리면 제거) → incremental.save(out_path)
      └─ (호출측) out_path를 pdfium으로 재오픈해 페이지 수 검증 → 통과 시에만 원자적(rename) 교체
```

**안전장치**(`crates/ui/src/app.rs`의 `save_bookmarks_to_pdf`): 항상 임시 파일에 먼저 씀 → pdfium 재검증 → 통과 시에만 `std::fs::rename`으로 원자적 교체 → 실패 시 원본 보존.

**페이지 번호 → PDF 객체 매핑**: `BookmarkNode.page`(1-based 번호)를 `lopdf::Document::get_pages()`(번호→ObjectId 맵)로 변환. 존재하지 않는 페이지 번호는 첫 페이지로 폴백(항목 누락보다 안전).

**검증**(`crates/pdf_outline_writer/tests/write_and_verify.rs`, 6개 테스트, `cargo test -p pdf_outline_writer`):
원본 파일 바이트 단위 무변경 확인 / 빈 트리 저장 시 `/Outlines` 제거 / 존재하지 않는 페이지 폴백 / **lopdf가 쓴 걸 pdfium이 실제로 재읽기 가능한지**(가장 중요) / 실사용 워크플로우 재현(4단계 깊이 실제 한글 목차 문서로 읽기→편집→저장→재확인, 손 안 댄 부분도 보존 확인) / 반복 저장 시 pdfium이 계속 정상 파싱.

**미해결**: 증분 저장 반복 시 파일이 계속 자람(옛 outline 객체가 죽은 채로 남음, 손상은 아니지만 최적화 여지 — 필요시 `save_modern()` 검토) / 암호화 PDF는 빈 비밀번호만 지원(실제 비밀번호 걸린 PDF 미검증).

### CSV/Excel Import·Export 스키마 (확정)

| 컬럼 | 설명 |
|---|---|
| 파일명 | 원본 PDF 파일명 |
| 계층 | 0=root, 1, 2 ... (depth) |
| 북마크명 | 표시 제목 |
| 페이지번호 | 이동 대상 페이지 (1-based) |

**계층 재구성**: 행 순서대로 depth 스택 유지(증가=직전 행의 자식, 같거나 감소=그 depth까지 pop 후 편입) — **행 순서가 트리 구조를 결정**(depth-first 필수). 사용자가 Excel에서 행을 수동 재배열 후 import하면 계층이 깨질 수 있음(UX 안내 필요, 미착수).

CSV/Excel 컬럼 순서 동일, 헤더는 한글, CSV는 UTF-8 BOM 적용(§2).

---

## 5. 배포 (비전문가 대상, 전부 미착수)

### macOS
- `.app` 빌드: `cargo-bundle`/`cargo-packager`, `.pkg`화: `pkgbuild`/`productbuild`
- **Apple Developer ID 서명 + notarization 필수** — 없으면 Gatekeeper 차단. 연 $99

### Windows
- `.msi`: `cargo-wix`/`cargo-packager`
- **코드서명 인증서(가능하면 EV) 강력 권장** — 없으면 SmartScreen 경고로 이탈 다수

### 공통
- PDFium 동적 라이브러리를 설치 패키지 안에 동봉(별도 다운로드 요구 금지)
- CI: GitHub Actions matrix (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`)

## 5-1. 개발 환경 요구사항

**최소 Rust 버전: 1.82 이상, 1.85+ 권장.** `pdfium-render` 0.9.2가 내부적으로 edition2024 문법/API를 쓰기 때문(라이브러리 자체 요구사항, 우리 코드 문제 아님). 이 macOS 머신은 rustc 1.97로 문제없음. CI 이미지 구성 시 rustc 버전을 명시적으로 최신 고정할 것.

---

## 6. 기능별 현재 상태

### 완료 (사용자 실기 검증 완료)
- Cargo workspace 5개 크레이트 분리, 전 워크스페이스 빌드+테스트 통과
- **PDF 뷰어**: 렌더링, 줌(버튼/Ctrl+스크롤/핀치제스처), 팬(드래그/두손가락스크롤), 텍스트 선택+복사(Cmd+C/우클릭메뉴), 호버 시 I-beam 커서, 페이지 이동, 드래그앤드롭으로 파일 열기, 시작 시 마지막 파일 자동 재오픈
- **북마크 사이드바**: 자동 로드(PDF 내장 목차), 추가(+/Cmd+B, 선택 항목의 자식으로), 삭제(−/Delete/우클릭메뉴, 삭제 후 형제→부모 순으로 선택 유지), 이름수정(재클릭/F2/우클릭메뉴), 드래그 재정렬(Before/After 삽입선 + Inside 테두리 표시, 드래그 중인 항목 반투명 표시), 트리 접기/펼치기(`>`/`v` 아이콘 + 좌우 화살표키 — 리프 노드 선택 시 부모 레벨 대상으로 동작), 화살표키로 형제/자식 탐색, Undo/Redo(최대 20단계, Cmd+Z/Cmd+Shift+Z)
- **CSV/Excel import·export**: 툴바 메뉴 연결, 왕복 보존 검증(자동화 테스트 + 실기)
- **한글**: CSV BOM/EUC-KR 폴백, 폰트 tofu 문제 해결, lopdf UTF-16BE 북마크 제목
- 창 제목("PDF Outliner - 파일명")
- **PDF 자체 저장**("저장" 버튼, 미저장 변경 시 저장/저장하지않음/취소 확인창): 저장 로직은 headless 테스트로 강하게 검증됨(§4). UI에서 앱이 꺼지던 크래시(wgpu 텍스처 파괴 타이밍 문제, 원인은 §7)도 수정 후 **사용자 재실행으로 해결 확인됨**(2026-07-13).

### 완료 (코드 구현 + headless/빌드 검증까지, 실기 미확인)
- F2 단축키, 사이드바 정렬(자식 있는 항목 들여쓰기)

### 남은 작업 (우선순위 낮음, 전부 미착수)
- [ ] SQLite 스키마/마이그레이션 — 북마크 저장은 이제 PDF 자체 outline이 1차 수단이라 우선순위 낮음(CSV/Excel처럼 보조·백업 용도로만 필요할 수도)
- [ ] CI/서명/패키징 파이프라인 (§5)
- [ ] 배포용 pdfium dylib을 앱 번들에 정식 동봉(현재는 개발 편의상 Homebrew 종속 경로 하드코딩 — §0 참고)
- [ ] 진짜로 시각적으로 기울어진 스캔 텍스트 샘플로 하이라이트 정확도 검증(§3)
- [ ] Excel 행 수동 재배열 후 import 시 계층 깨짐 방지 UX (§4)

---

## 7. 값진 기술적 교훈 (다음 세션이 같은 삽질을 반복하지 않도록)

### pdfium-render 0.9.2 API가 스펙 작성 당시 가정과 달랐던 것들
- `chars_range()` 메서드 없음 → `chars()` 전체 컬렉션에서 인덱스로 `get()` 순회하는 방식으로 대체
- `PdfPageTextChar::matrix()` 없음 → `angle_radians()`(`FPDFText_GetCharAngle` 래핑)가 회전각을 직접 반환
- `get_char_near_point()`는 `Result`가 아니라 `Option<PdfPageTextChar>` 반환
- **`angle_radians()`가 실제 화면상 시각적 회전과 다를 수 있음**: 실제 PDF 2건(포스터/디자인 타이틀류)에서 큰 회전값(90°, 6.2rad)이 나왔는데, `render_crop`으로 그 문자를 직접 렌더링해 PNG로 크롭해 육안 확인하니 글리프는 완전히 똑바로 그려져 있었음 — 폰트가 내부적으로 상쇄하는 "쓰기방향 배치 행렬" 회전일 뿐, 실제 렌더링 결과와 무관할 수 있음. 하이라이트 quad 회전 로직을 걷어내고 axis-aligned로 바꾼 근거(§3).
- **pdfium은 outline(북마크) 쓰기 API가 아예 없음**(PDFium C 라이브러리 자체 한계, pdfium-render 탓 아님) — 대안으로 pdf_oxide를 조사했으나 실제 소스 확인 결과 기존 문서 편집용(`DocumentEditor`)에는 outline 코드가 전혀 없고, outline 빌더(`OutlineBuilder`)는 전혀 새 PDF를 만들 때만 쓰는 것이었음(docstring에 "for PDF generation"이라 명시) — 착각하기 쉬운 함정이니 주의. lopdf로 대체(§4).
- `Pdfium` 바인딩은 **프로세스당 딱 한 번만** 초기화 가능 — 자동화 테스트가 각자 `PdfEngine::new_with_library_path`를 새로 부르면 `cargo test` 기본 병렬 실행(테스트마다 별도 스레드) 시 여러 스레드가 동시에 초기화하려다 진짜로 크래시남(SIGTRAP, 힙 손상 감지). `OnceLock`으로 엔진 하나만 공유해서 여러 테스트가 재사용하게 할 것 — 실제 앱은 시작 시 메인 스레드에서 딱 한 번만 만드므로 이 문제 자체가 없음.

### egui/eframe/wgpu 0.29.x 관련
- `eframe`의 `wgpu` feature가 기본 비활성화(default는 `glow`) — `features = ["wgpu"]` 명시 필요
- `egui::Painter::rect_stroke`는 이 버전에서 인자 3개(rect, rounding, stroke) — `StrokeKind` 인자 없음
- `ctx.data_mut()`의 temp storage(`get_temp`/`insert_temp`)는 `Clone` bound 요구
- `TextureHandle::size_vec2()`는 텍스처의 **실제 픽셀 크기**를 반환(포인트로 안 나눔) — Retina(2x) 디스플레이에서 그대로 화면 크기로 쓰면 렌더링이 저해상도로 나옴. `ctx.pixels_per_point()`로 렌더링 타겟은 물리 픽셀 기준, 화면 배치는 포인트 기준으로 분리할 것.
- `response.interact_pointer_pos()`는 버튼이 눌려있을 때만 값이 있음 — 순수 호버(버튼 안 누른 상태) 감지엔 `response.hover_pos()` 필요.
- **`egui-winit`이 Cmd+C/Cut/Paste를 raw 키 이벤트로 안 주고 `Event::Copy`/`Event::Cut`/`Event::Paste`로 바꿔치기하고 원래 키 이벤트는 아예 안 만듦** — `i.modifiers.command && i.key_pressed(Key::C)` 조건은 Cmd+C를 눌러도 절대 참이 될 수 없음. `ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)))`처럼 세맨틱 이벤트를 직접 확인해야 함. (Cmd+Z나 Cmd+B는 이렇게 가로채지지 않음 — egui-winit 소스로 직접 확인함, raw 키 체크로 충분.)
- `egui::Label`은 기본적으로 `selectable(true)`라서, 커스텀 `Sense::drag()`로 드래그 재정렬을 구현하면 egui의 자체 텍스트 선택 UI가 그 드래그 제스처를 가로채 버림(마치 선택 영역을 조절하는 것처럼 보임). `.selectable(false)`를 반드시 명시할 것.
- **`egui::Response::hovered()`는 다른 위젯이 드래그 중이면 항상 `false`를 반환함**(문서에 명시) — 드래그 중에 "지금 이 위젯 위에 마우스가 있나"를 확인하려면(드롭 타겟 감지 등) `contains_pointer()`를 써야 함.
- 핀치 줌은 raw winit 이벤트 후킹이 필요할 거라 예상했지만, `egui-winit` 0.29.1이 macOS `WindowEvent::PinchGesture`를 이미 내부적으로 `egui::Event::Zoom`으로 변환해줌 — `ctx.input(|i| i.zoom_delta())`만 읽으면 끝.
- **`egui::Window`(기본 `Order::Middle`)는 `Panel`(`Order::Background`)보다 항상 위에 그려짐 — 이건 `update()` 안에서의 호출 순서가 아니라 `Order` 값으로 정렬되기 때문.** 하지만 화면 z-order와 별개로, **위젯이 참조하는 리소스(특히 `TextureHandle`)를 드롭/교체하는 상태 변경은 호출 순서가 실제로 중요함**: 어떤 위젯이 이번 프레임에 이미 그 텍스처로 draw call을 큐에 넣은 **뒤에** 다른 코드가 그 텍스처를 드롭하면, 프레임이 GPU에 제출될 때 "파괴된 텍스처를 참조함" wgpu validation panic이 남. 텍스처를 드롭할 가능성이 있는 로직(문서 전환 등)은 그 텍스처를 그리는 위젯보다 먼저 실행되도록 `update()` 안에서 순서를 배치할 것.
- `AppleSDGothicNeo.ttc`(TrueType Collection)는 egui 폰트 로더가 파싱 못 함 — 반드시 standalone `.ttf`/`.otf`(예: `AppleGothic.ttf`)만 후보로 쓸 것.
- 로드된 폰트(egui 기본 + 등록한 fallback)에 없는 유니코드 아이콘 글리프(예: "✕" U+2715)는 빈 사각형(tofu)으로 렌더링되는데, 작은 버튼에 쓰면 **체크박스처럼 보여서 오작동으로 오인되기 쉬움**(실제로 이걸로 "삭제 버튼이 체크박스처럼 눌려서 바로 삭제된다"는 리포트를 받았음) — 아이콘 버튼엔 커버리지가 검증 안 된 심볼 대신 plain ASCII(`+`, `-`, `>`, `v`) 사용 권장.

### 테스트/디버깅 설계 원칙
- headless 예제·테스트로 pdfium·lopdf 로직을 실제 라이브러리로 검증하는 게 화면 캡처 불가 세션에서 유일하게 강한 검증 수단. `render_crop`으로 PNG를 저장해 Read 툴로 육안 확인하는 것도 유효한 시각 검증 방법. GUI 상호작용 버그(호버, 드래그, 키보드 단축키 등) 자체는 headless로 발견 불가능 — 사용자의 실기 리포트에 의존할 수밖에 없음.
- **사용자가 수동 GUI 테스트에 실사용 중인 파일을 자동화 테스트가 같이 건드리면 안 됨.** 실사례: 사용자가 삭제+저장 기능을 실기 테스트하면서 `pdf-samples/embeddedoutline.pdf`(원래 6개 장)의 일부를 실제로 지우고 저장했는데, 그 파일에 하드코딩된 기대값(6개 장)을 갖고 있던 자동화 테스트가 갑자기 실패해서 "회귀 버그"로 오인할 뻔함 — 사실은 저장 기능이 의도대로 정확히 동작한 증거였음(파일 수정시각이 사용자 테스트 시각과 정확히 일치해서 확인함). 자동화 테스트는 항상 pristine 백업을 임시 디렉터리에 복사해서 그 복사본만 쓰도록 격리할 것.
- **재현에 성공했다고 바로 원인이라고 확신하지 말 것.** "pdfium이 문서 2개 동시에 못 견딤"이라는 가설을 세우고 재현 테스트를 작성해 실제로 SIGSEGV/SIGTRAP을 재현하는 데 성공했지만, 나중에 알고보니 그 테스트 자체가 테스트마다 `PdfEngine`을 따로 만들어서 `cargo test`의 기본 병렬 실행 때 여러 스레드가 동시에 pdfium을 초기화하려다 난 크래시였음(테스트 설계 결함) — 진짜 원인(wgpu 텍스처 파괴 타이밍)은 결국 사용자가 터미널에서 직접 실행해 얻은 정확한 panic 메시지로 찾음. 재현 테스트의 전제(엔진 공유 방식 등) 자체도 의심할 것.

---

## 확인 필요 미결 사항

- **개발 환경**: `~/.zshrc`에 `. "$HOME/.cargo/env"` 추가 완료(2026-07-12) — 새 터미널 세션이면 `rustc`/`cargo` 바로 사용 가능.
- 그 외 이전 논의된 요구사항은 모두 해소됨(북마크 페이지번호 컬럼, OCR 범위 밖 확정, 이미지 전용 페이지 안내 불필요).
