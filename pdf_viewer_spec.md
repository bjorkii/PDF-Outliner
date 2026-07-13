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

## 0. 새 세션 빠른 시작 (2026-07-13 기준 현재 상태 — 아래부터 읽으면 됨, 6번 섹션은 시간순 상세 로그라 급하면 안 읽어도 됨)

**빌드/실행**:
```bash
cd "/Users/researchkofa/Documents/VibeCoding/PDF/PDF Bookmark Editor-Rust"
cargo build --workspace   # rustc/cargo는 ~/.zshrc에 이미 PATH 잡혀있음, 새 터미널이면 바로 됨
cargo test --workspace    # 전부 통과해야 정상 (bookmark 13, import_export 3+2, pdf_outline_writer 6, ui 3 등)
./target/debug/pdf_viewer [선택: 열 pdf 경로]   # 실행. pdfium dylib는 Homebrew ocrmypdf 종속성 경로를 하드코딩 폴백으로 씀(app.rs의 create_engine 참고) — PDFIUM_DYLIB_PATH 환경변수로 override 가능
```
**이 프로젝트는 git repo가 아님**(세션 시작 시 확인됨) — 롤백/diff 수단이 스펙 문서와 코드 자체뿐이니 큰 변경 전엔 신중히.

**핵심 기능(이미 구현+대부분 실기 검증됨)**: PDF 뷰어(렌더링/줌/팬/텍스트선택복사) + 북마크 사이드바(추가/이름수정/삭제/드래그재정렬/폴딩/Undo·Redo/단축키) + PDF 자체 outline에 저장(lopdf) + CSV/Excel import export. 자세한 아키텍처는 §4 "PDF 자체 북마크(outline) 읽기/쓰기" 참고.

**아직 실기 미확인인 것 (다음 세션에서 우선 확인)**:
- **가장 최근에 고친 wgpu 크래시 수정**(§6 맨 아래 "저장/저장하지 않음 크래시" 항목) — "저장"/"저장하지 않음" 눌러도 이제 안 꺼지는지 재확인 필요. 원인/수정 내용은 확실하지만(사용자가 준 정확한 panic 메시지로 근거 확보) 수정 후 재실행 테스트는 아직 못 함.
- 창 제목 표시, 사이드바 정렬(자식 있는 항목 들여쓰기), F2 단축키 — 코드 구현+빌드확인까지만 함.
- 실제 스캔 텍스트(글리프 자체가 시각적으로 기울어진 경우)의 하이라이트 정확도 — 아직 그런 샘플을 못 만나봄.

**이 세션 환경의 제약**: 화면 캡처/실제 GUI 조작이 불가능한 샌드박스라, 대부분의 검증은 (a) headless 예제/테스트로 pdfium/lopdf 로직만 실제 라이브러리로 확인, (b) egui/wgpu 소스 코드를 직접 읽어 API 동작 확인, (c) 사용자가 실행해보고 리포트한 내용을 바탕으로 원인 규명 후 수정, 의 조합으로 이뤄짐. **즉 GUI 상호작용 버그는 사용자의 실기 리포트 없이는 발견 자체가 어려움** — 다음 세션도 이 사이클(사용자 리포트 → 원인 규명 → 수정 → 재확인 요청)을 유지할 것.

**알아두면 좋은 디버깅 도구들**(전부 이미 만들어져 있음, 재사용 가능):
- `cargo run --example dump_outline -p pdf_engine -- <pdfium_dylib> <pdf>` — PDF 내장 북마크 트리 출력
- `cargo run --example dump_chars -p pdf_engine -- <pdfium_dylib> <pdf> <page_0based>` — 페이지 내 문자별 좌표/회전각 출력
- `cargo run --example render_crop -p pdf_engine -- <pdfium_dylib> <pdf> <page_0based> <out.png> [char_index]` — 렌더링 결과를 PNG로 저장해 Read 툴로 육안 확인(화면 캡처 안 되는 세션에서 유일한 시각 확인 수단)
- `cargo run --example smoke_test -p pdf_engine -- <pdfium_dylib> <pdf> [최대_페이지수]` — 임의 PDF 렌더링/텍스트선택 구조적 정상성 확인
- pdfium dylib 경로(이 머신 기준): `/opt/homebrew/Cellar/ocrmypdf/17.8.0/libexec/lib/python3.14/site-packages/pypdfium2_raw/libpdfium.dylib`
- 테스트용 실제 PDF 샘플: `pdf-samples/` 안에 여러 개(일부는 사용자가 수동 테스트에 실사용 중이라 자동화 테스트에서 함부로 건드리면 안 됨 — `embeddedoutline.pdf` vs 손 안 댄 백업 `embeddedoutline 복사본.pdf` 사례 참고, §6 "사이드바 UX 대규모 개선" 항목)

---

## 1. 전체 아키텍처 개요

| 영역 | 선택 | 이유 |
|---|---|---|
| PDF 렌더링 엔진 | **pdfium-render** (crates.io, 0.9.x, 활발히 유지보수) | Chromium PDFium 바인딩. 라이선스 깔끔(Apache 계열), 속도/용량 면에서 MuPDF보다 상업 배포에 적합 |
| GUI 프레임워크 | **egui + eframe (wgpu 백엔드)** | Immediate-mode, 바이너리 수 MB, 콜드 스타트 매우 빠름. Tauri(webview)는 초기화 오버헤드로 "Sumatra급" 체감에 불리해 제외 |
| 북마크 트리 UI | **egui-arbor** 또는 **egui_ltreeview** | Before/After/Inside 드롭 위치 기반 재구성 지원 → 하위 트리 통째 이동 자동 처리 |
| 북마크 저장 | SQLite(`rusqlite`) 또는 문서별 사이드카 JSON | 여러 문서 오가며 검색/필터 계획 시 SQLite 권장 |
| CSV | `csv` 크레이트 (+ 수동 BOM 처리) | 아래 3번 참고 |
| Excel | 읽기: `calamine` / 쓰기: `rust_xlsxwriter` | 인코딩 이슈 없음, 한글 기본 안전 |
| 클립보드 | `arboard` | macOS/Windows 유니코드(한글) 클립보드 안정적 |
| PDF 텍스트 API | pdfium-render `PdfPageText` (`FPDFText_*`) | 아래 4번 참고 |

PDFium은 런타임 바인딩 방식 → `bblanchon/pdfium-binaries`에서 플랫폼별(`aarch64`/`x64` mac, Windows x64) prebuilt 라이브러리를 받아 앱 번들에 동봉.

---

## 2. 한글 인코딩 (CSV 고질 문제)

**원인**: Windows Excel은 BOM 없는 CSV를 시스템 로케일(CP949)로 추측 → UTF-8로 저장해도 한글 깨짐. macOS 앱은 대체로 영향 적음.

**대응**:
- **Export**: CSV 저장 시 파일 맨 앞에 UTF-8 BOM(`EF BB BF`)을 수동으로 기록 후 `csv::Writer` 사용.
  ```rust
  let mut file = File::create(path)?;
  file.write_all(b"\xEF\xBB\xBF")?;
  let mut wtr = csv::Writer::from_writer(file);
  ```
- **Import**: `encoding_rs`로 BOM 유무 확인, 없으면 인코딩 자동 감지 또는 사용자에게 인코딩 선택 UI 제공(오래된 EUC-KR/CP949 파일 대비).
- **정책**: xlsx를 기본 권장 export 포맷으로 안내(인코딩 이슈 원천 차단), CSV는 호환용 보조 옵션으로 위치시킴.

---

## 3. 뷰어 인터랙션

### 줌
- macOS 트랙패드 핀치: ~~winit `WindowEvent::PinchGesture` 후킹 필요~~ **정정(2026-07-13)**: `egui-winit` 0.29.1이 이미 이걸 `egui::Event::Zoom`으로 내부 변환해줌 — 별도 후킹 불필요, `ctx.input(|i| i.zoom_delta())`만 읽으면 됨. 구현 완료(`crates/ui/src/viewer_panel.rs`), 사용자 실기 확인 완료("pinch to zoom 정상").
- Windows 마우스 휠: `smooth_scroll_delta` + Ctrl 조합 관례
- 명시적 +/- 버튼 툴바 병행 배치(비전문 사용자 대비)
- 트랙패드 두 손가락 스와이프 = 패닝(Ctrl 안 눌렸을 때 `smooth_scroll_delta`를 `pan_offset`에 반영). 세 손가락 드래그는 macOS 손쉬운 사용 설정이 OS 레벨에서 일반 드래그로 합성해줘서 별도 처리 불필요.

### 확대 시 drag 탐색
- `Sense::drag()` 위젯 + 드래그 델타로 뷰포트 오프셋 이동, 페이지 경계 clamp, 줌 배율 비례 스케일링

### 페이지 이동
- 방향키: `ctx.input(|i| i.key_pressed(Key::ArrowRight/ArrowLeft))`
- 페이지 번호 입력 필드: 파싱→범위 clamp→점프

### 텍스트 선택/복사 — 핵심 설계 원칙
> **OCR 프로세싱 기능은 범위 밖.** 기존에 텍스트 레이어가 있는 PDF에서 "선택은 되는데 복사가 안 되거나 깨지는" 버그 대응이 목적.

원인 2가지:
1. **PDF 자체 결함**(ToUnicode CMap 누락) — 앱이 고칠 수 없는 영역. 특히 HWP→PDF 변환기 등에서 흔함.
2. **뷰어 구현 결함**(좌표 기반 선택 로직) — 여기가 우리가 고칠 부분.

**채택 설계: 좌표 교차 방식이 아닌 "문자 인덱스 range" 기반 선택**
1. 마우스 다운 시 `FPDFText_GetCharIndexAtPos(x, y, tolerance)`로 좌표→문자 인덱스 변환 (PDFium 내부 히트테스트에 위임, 직접 기하 계산 재구현 안 함)
2. 드래그 중엔 시작~끝 **인덱스**만 갱신(좌표 재계산 아님)
3. 복사 시 `FPDFText_GetText(start_index, count)`로 range 텍스트 추출 — ToUnicode 매핑을 pdfium이 내부 처리
4. 줄바꿈은 문자별 y좌표 변화 감지로 개행 삽입

**스큐(기울어진 스캔+OCR 페이지) 대응 — ⚠️ 2026-07-12 실제 PDF 테스트로 아래 설계가 틀렸음을 확인, 수정함**
- `FPDFText_GetCharBox`는 축정렬 박스만 반환 → 기울어진 문자에는 부정확 (이 진단 자체는 맞음)
- ~~`FPDFText_GetMatrix`/`angle_radians()`로 문자별 회전각을 얻어 하이라이트 quad를 그만큼 회전~~ → **실측 결과 이 가정이 틀림.** 사용자가 제공한 실제 PDF(`pdf-samples/`)로 테스트하던 중, `BZR001088_01.pdf` 1페이지가 세로 칼럼 디자인 타이틀(문화독립/영화주권)임을 지적받아 조사하다가 발견: `angle_radians()`가 idx 0에서 1.571rad(≈90°), `BZB000877_01.pdf` idx 26에서 6.215rad를 반환했지만, `render_crop` 예제로 해당 문자를 직접 렌더링해 크롭한 이미지를 육안 확인하니 글리프는 **완전히 똑바로(upright)** 그려져 있었음(스크린샷 2건 모두 확인). 즉 이 각도는 폰트가 내부적으로 상쇄하는 "텍스트 배치 행렬" 회전일 뿐 실제 화면상 시각적 회전과 다를 수 있음 — 원래 설계대로 이 각도로 quad를 회전시켰다면 똑바로 선 글자 위에 90도 뒤틀린 하이라이트가 그려지는 버그가 됐을 것.
- **수정된 설계**: `loose_bounds()`가 이미 최종 렌더링 좌표계 기준 축정렬 박스이므로, 회전을 적용하지 않고 그대로 사용(`crates/pdf_engine/src/skew.rs`의 `char_quads_with_rotation`). 스캔 스큐 문서에서는 하이라이트가 살짝 헐렁하게(loose) 나올 수 있지만, 디자인 문서에서 하이라이트가 엉뚱한 방향으로 뒤집히는 것보다 안전한 절충.
- `rotation_radians` 필드는 진단용으로만 `CharQuad`에 남겨둠(더 이상 quad 계산에 사용 안 함).
- **미해결**: "진짜로 시각적으로 기울어진 스캔 텍스트"(글자 자체가 기울어져 렌더링된 경우)는 아직 샘플에서 못 봄 — 그런 케이스를 만나면 `angle_radians()`와 실제 시각적 기울기가 언제 일치/불일치하는지 재조사 필요. 지금은 "회전 안 함"이 기본값.
- 클릭 히트테스트는 이 이슈와 무관(GetCharIndexAtPos가 내부 처리, 영향 없음) — 실제 3개 샘플 전부 히트테스트 왕복 OK 확인됨.

**세로쓰기(고문서) 대응**
- PDF가 CID 폰트 + vertical writing mode로 올바르게 인코딩되어 있다면 **인덱스 순서 자체가 논리적 읽기 순서**를 따름 → 별도 코드 분기 불필요
- **실제 검증** (`BZR001088_01.pdf` p1, `crates/pdf_engine/examples/dump_chars.rs`로 확인): "문화독립"(오른쪽 칼럼)과 "영화주권"(왼쪽 칼럼)이 각각 연속된 문자 인덱스 블록으로 나오고, 블록 내부는 위→아래 순서가 인덱스 순서와 일치 — 위 가정이 이 문서에서는 성립함. 단, 이 문서가 진짜 CID 세로쓰기 인코딩인지 디자인 툴이 개별 배치한 것인지는 확인 안 됨(문자 수가 15개뿐인 짧은 타이틀이라 판단 근거 부족).
- 하이라이트는 이제 축정렬 박스라 세로 칼럼 문자에도 자연스럽게 맞음(회전을 걷어냈기 때문에 오히려 세로쓰기 대응은 더 단순해짐).
- 텍스트 레이어 없는(이미지만 있는) 페이지는 선택 불가 — 이는 정상 동작이며 별도 안내 불필요

---

## 4. 북마크

### 데이터 모델
```rust
struct Bookmark {
    id: Uuid,
    parent_id: Option<Uuid>,  // 계층 구조 (사이드바 드래그 재구성용)
    title: String,
    page: u32,                 // 1-based
    order: i32,
    // 추가 가능: zoom, scroll_y, tags 등
}
```

### 사이드바 드래그
- `egui-arbor`/`egui_ltreeview`가 Before/After/Inside 드롭을 지원 → 노드를 다른 노드 "안"으로 드롭 시 하위 트리 전체 자동 이동

### PDF 자체 북마크(outline) 읽기/쓰기 — 렌더링(pdfium)과 편집(lopdf) 분리 (2026-07-13 확정)

**배경**: pdfium은 outline(북마크) **읽기 API만** 있고 쓰기 API가 아예 없다(`FPDFBookmark_GetFirstChild`/`GetTitle`/`GetDest`류만 존재, `FPDFBookmark_Add`류는 PDFium C 라이브러리 자체에 없음 — pdfium-render의 한계가 아니라 PDFium 자체의 한계). `pdf_oxide_research.md`에서 대안으로 pdf_oxide를 검토했으나, 실제 소스 코드를 직접 받아 확인한 결과 pdf_oxide의 문서 편집 모듈(`DocumentEditor`)에는 outline 관련 코드가 전혀 없어 기각(자세한 근거는 그 문서의 정정 박스 참고). 최종적으로 **lopdf**를 채택 — `add_bookmark()`/`build_outline()`이 실제로 존재하고, 한글 등 비ASCII 제목을 UTF-16BE로 인코딩하는 코드까지 이미 포함돼 있으며, `IncrementalDocument`로 안전한 증분 저장을 지원한다.

**아키텍처**:
```
읽기: 문서 열기 시 → pdfium이 이미 열어둔 PdfDocument에서
      pdf_engine::outline::read_bookmarks(&document) -> Vec<BookmarkNode>
      (FPDFBookmark_GetFirstChild/GetTitle/GetDest/GetNextSibling로 순회, 추가 파일 I/O 없음)

편집: 사이드바에서 추가/이름수정/삭제/드래그이동 → 메모리상의 Vec<BookmarkNode>만 갱신
      (bookmark::move_node 등 기존 트리 로직 그대로 재사용)

저장: pdf_outline_writer::write_bookmarks_incremental(source_pdf, out_path, &bookmarks)
      └─ lopdf::IncrementalDocument::load(source_pdf)
           └─ catalog 객체를 new_document로 복제(opt_clone_object_to_new_document)
           └─ bookmarks 트리를 lopdf::Bookmark로 변환해 add_bookmark()/build_outline() 호출
           └─ catalog의 /Outlines를 새 outline id로 교체(또는 빈 트리면 제거)
           └─ incremental.save(out_path)  ← 증분 저장, 원본 바이트는 절대 안 건드림
      └─ (호출측 책임) out_path를 pdfium으로 재오픈해 페이지 수 등 검증
      └─ 검증 통과 시에만 원자적(rename)으로 원본과 교체
```

**안전장치**(`crates/ui/src/app.rs`의 `save_bookmarks_to_pdf`에 구현):
1. 항상 임시 파일(`<원본>.bookmarks_tmp.pdf`)에 먼저 씀 — 원본 직접 덮어쓰기 금지
2. 저장 직후 pdfium으로 그 임시 파일을 재오픈해 페이지 수가 원본과 일치하는지 검증
3. 검증 통과 시에만 `std::fs::rename`으로 원자적 교체, 실패 시 임시 파일 삭제 후 원본 보존 + 에러 메시지

**페이지 번호 → PDF 객체 매핑**: `bookmark::BookmarkNode.page`는 1-based 페이지 "번호"인데, lopdf의 `Bookmark`는 페이지 objectId(`lopdf::ObjectId`)를 요구한다. `lopdf::Document::get_pages()`(1-based 번호→ObjectId 맵)로 변환. 존재하지 않는 페이지 번호가 들어오면(있을 수 없는 상황이지만 방어적으로) 첫 페이지로 폴백 — 항목을 통째로 누락시키는 것보다 안전.

**검증**(`crates/pdf_outline_writer/tests/write_and_verify.rs`, 6개 테스트 전부 통과):
- 실제 샘플 PDF(`pdf-samples/BZR001088_01.pdf`, `KKZ000160_01.pdf`)에 한글 계층 북마크를 써서 lopdf로 재확인
- **원본 파일이 바이트 단위로 전혀 안 바뀌었는지 확인**(증분 저장 안전장치의 핵심)
- 빈 북마크 트리 저장 시 `/Outlines` 제거 확인
- 존재하지 않는 페이지 번호 → 첫 페이지 폴백 확인
- **가장 중요한 테스트**: lopdf로 쓴 결과를 pdfium이 실제로 다시 읽을 수 있는지(`pdfium_can_reread_what_lopdf_wrote`) — "저장 후 pdfium으로 재검증"이라는 안전장치 자체가 이게 성립해야 의미가 있음
- **실사용 워크플로우 재현**(`read_edit_save_reread_real_document_with_existing_outline`): 진짜 4단계 깊이의 한글 목차를 가진 실제 문서(`pdf-samples/embeddedoutline.pdf`, 32페이지 한국현대사 자료 논문, 사용자가 테스트용으로 제공)를 pdfium으로 읽고 → 항목 하나 수정 + 하나 추가 → lopdf로 저장 → pdfium으로 재오픈해 편집 결과와 손대지 않은 깊은 하위 항목(4단계 깊이) 둘 다 정확히 보존됐는지 확인
- 여러 번 연속 저장(증분 저장 누적)해도 pdfium이 계속 정상 파싱하는지 확인
- 실행: `cargo test -p pdf_outline_writer`

**미해결**:
- 증분 저장을 반복하면 파일이 계속 자라기만 한다(옛 outline 객체가 죽은 채로 파일에 남음, PDF 표준 동작이라 손상은 아니지만 용량 최적화 여지 있음) — 필요하면 주기적으로 lopdf의 `save_modern()`(전체 재직렬화, object stream 압축) 같은 "완전 재압축" 옵션 제공 검토
- 암호화된 PDF는 lopdf가 빈 비밀번호 암호화만 지원 — 실제 비밀번호 걸린 PDF는 저장 실패할 것(아직 실제 암호화 PDF로 테스트 안 함)
- UI 레벨(다른 문서 열 때 "저장하시겠습니까?" 확인창, 저장 버튼)은 실제 GUI 클릭으로 검증 안 됨 — 아래 6번 항목 참고

### CSV/Excel Import·Export 스키마 (확정)

| 컬럼 | 설명 |
|---|---|
| 파일명 | 원본 PDF 파일명 |
| 계층 | 0=root, 1, 2 ... (depth) |
| 북마크명 | 표시 제목 |
| 페이지번호 | 이동 대상 페이지 (1-based) |

```rust
struct BookmarkRow {
    filename: String,
    depth: u32,
    title: String,
    page: u32,
}
```

**계층 재구성 알고리즘** (parent_id 없이 depth 순차값으로 트리 복원):
```
행 순서대로 읽으며 depth 스택 유지
depth가 이전보다 크면 → 직전 행의 자식으로 편입
depth가 같거나 작으면 → 스택을 depth까지 pop 후 그 부모에 편입
```
→ **행 순서가 트리 구조를 결정**(depth-first 순서 필수). Export는 자동 보장되지만, 사용자가 Excel에서 행을 수동 재배열 후 import하면 계층이 깨질 수 있음 — UX 안내 필요.

CSV/Excel 컬럼 순서 동일, 헤더는 한글, CSV는 UTF-8 BOM 적용(2번 항목 참고).

---

## 5. 배포 (비전문가 대상)

### macOS
- `.app` 빌드: `cargo-bundle` 또는 `cargo-packager`
- `.pkg`화: `pkgbuild`/`productbuild` 또는 `cargo-packager` pkg 타겟
- **Apple Developer ID 서명 + notarization(`xcrun notarytool`) 필수** — 없으면 Gatekeeper가 "확인되지 않은 개발자" 경고로 실행 차단. Developer Program 연 $99 필요

### Windows
- `.msi` 빌드: `cargo-wix`(WiX Toolset) 또는 `cargo-packager`
- **코드서명 인증서(가능하면 EV) 강력 권장** — 없으면 SmartScreen 경고("Windows에서 PC를 보호했습니다")로 비전문 사용자 다수 이탈. EV는 즉시 신뢰, OV는 다운로드 수 누적 필요

### 공통
- PDFium 동적 라이브러리를 설치 패키지 안에 동봉(별도 다운로드 요구 금지)
- CI: GitHub Actions matrix 빌드 (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`)

---

## 5-1. 개발 환경 요구사항 (스캐폴딩 중 확인됨)

**최소 Rust 버전: 1.82 이상 필요, 1.85+ 권장.**

리눅스 샌드박스(cargo 1.75, 2024년 초 apt 버전)에서 실제 컴파일을 시도해 확인한 사실:
- `pdfium-render` 0.9.2 자체 소스코드가 `unsafe extern "C"` 블록 문법(Rust edition 2024에서 도입)과 최신 `CString` API를 내부적으로 사용함 → **rustc 1.82 미만에서는 아예 컴파일 불가**(우리 코드 문제가 아니라 라이브러리 자체 요구사항).
- `libloading` 0.9.0(더 최신 버전)은 rustc 1.88 이상 요구 — pdfium-render가 의존성으로 끌어올 수 있으므로 오래된 리눅스 배포판의 apt 기본 rustc로는 대응 불가.
- xlsx 관련 의존성(`zip`→`hashbrown` 등)도 최신 버전은 edition2024(Rust 1.85 안정화) 요구.

**결론**: rustup으로 최신 stable(1.85 이상, 가능하면 최신) 설치 후 개발/빌드 진행 권장. macOS/Windows 실제 타겟 머신에는 보통 최신 rustup 툴체인을 쓸 테니 문제되지 않을 가능성이 높지만, CI 이미지(GitHub Actions 등) 구성 시 rustc 버전을 명시적으로 최신으로 고정해둘 것.

## 6. 다음 단계 (진행 상황)

- [x] Cargo workspace 스캐폴딩: `pdf_engine` / `bookmark` / `import_export` / `ui` 크레이트 분리 완료
- [x] 북마크 계층 재구성/드래그 이동 로직 — 실제 컴파일+단위테스트 통과 (4개 테스트)
- [x] CSV 한글 인코딩(BOM export, 레거시 EUC-KR import 감지) — 실제 컴파일+단위테스트 통과 (2개 테스트)
- [x] xlsx import/export — macOS(rustc 1.97) 실제 컴파일+단위테스트 통과. `calamine::DataType` 트레이트 미임포트로 `as_f64()` 컴파일 에러 있었음 → import 추가로 수정
- [x] pdf_engine(텍스트 선택/스큐 대응) — macOS(rustc 1.97) 실제 컴파일 통과. pdfium-render 0.9.2 실제 API가 스펙 작성 당시 가정과 달랐음:
  - `chars_range()` 메서드 없음 → `chars()` 전체 컬렉션에서 인덱스로 하나씩 `get()` 조회하는 방식으로 대체 (`selection.rs`의 `chars_in_range`)
  - `PdfPageTextChar::matrix()` 메서드 없음 → `angle_radians()`(FPDFText_GetCharAngle 래핑)가 회전각을 직접 반환하므로 `atan2` 역산 불필요, `skew.rs`에서 직접 사용
  - `get_char_near_point()`는 `Result`가 아니라 `Option<PdfPageTextChar>` 반환 → `.ok()` 제거
- [x] ui(egui/eframe 뷰어+북마크 사이드바 드래그) — macOS 실기에서 `cargo run`으로 창 실행 확인. 수정 필요했던 부분:
  - `eframe`에 `wgpu` feature가 기본 비활성화(default는 `glow`) → `Cargo.toml`에 `features = ["wgpu"]` 명시해야 `eframe::Renderer::Wgpu` 사용 가능
  - `egui::Painter::rect_stroke`가 0.29.1에서는 인자 3개(rect, rounding, stroke) — `StrokeKind` 인자 없음
  - `ctx.data_mut()`의 `get_temp`/`insert_temp`가 `Clone` bound 요구 → `DragState`에 `#[derive(Clone)]` 추가
- [x] **PDF 렌더링/텍스트 선택 실기 검증 완료** (2026-07-12) — 이 macOS 머신에 Homebrew `ocrmypdf` 패키지의 종속성으로 이미 설치돼 있던 실제 `libpdfium.dylib`(pypdfium2_raw 소속, arm64)를 사용해 검증. 새로 다운로드하지 않음.
  - `PdfEngine`을 `Box::leak`으로 `Pdfium`을 `'static` 참조로 승격하도록 변경(`crates/pdf_engine/src/lib.rs`) — `PdfDocument<'static>`를 자기참조 구조체 없이 `PdfViewerApp`에 필드로 직접 저장 가능하게 함. egui App은 프로세스 전체 생명주기 동안 사는 단일 인스턴스라 리크해도 실질적 누수 없음.
  - 툴바 "파일 열기" → `PdfEngine::open_document` 연결 완료, `viewer_panel.rs`에서 `PdfRenderConfig::set_target_width`(줌 반영) 기반 실제 페이지 렌더링 → egui 텍스처 업로드 → 화면 표시까지 구현
  - 마우스 드래그 기반 텍스트 선택(문자 인덱스 히트테스트 → range → quad 하이라이트)과 Cmd+C 클립보드 복사(`arboard`) 구현
  - CLI 인자로 PDF 경로를 받아 시작 시 자동으로 여는 기능 추가(`crates/ui/src/main.rs`) — Finder 연결 프로그램 연동에도 필요한 기능
  - **검증 방법**: 이 세션 환경(샌드박스)은 화면 캡처 권한이 없어(`screencapture` 실패) 실행 중인 네이티브 창을 스크린샷으로 눈으로 확인하지 못함. 대신 `crates/pdf_engine/examples/verify_render.rs`를 작성해 GUI 없이 동일한 `pdf_engine::selection::*` 함수(UI가 호출하는 것과 동일한 코드 경로)를 실제 pdfium 바인딩 + 실제 PDF(cupsfilter로 생성한 한글/영문 혼합 텍스트 PDF)로 직접 호출해 검증:
    - 900px 폭 렌더링 → 900x1165 비트맵, 흰색 아닌 픽셀 15,272개(텍스트가 실제로 그려짐 확인)
    - `text_page.all()`로 한글+영문 텍스트 정확히 추출됨(ToUnicode/인코딩 문제 없음)
    - `char_index_at_point`: 문자 중심 좌표 클릭 시 정확히 해당 인덱스 반환(좌표→인덱스 히트테스트 왕복 검증)
    - `extract_text`: range(0..=19) → `"PDF Bookmark Editor "` 정확 추출
    - `selection_quads`: 20개 문자 → quad 20개, 좌표 비퇴화 확인
    - `pixels_to_points`/`points_to_pixels` 왕복 오차 1pt 미만(뷰어의 화면 좌표↔PDF 포인트 변환 로직과 동일 API)
    - 실행: `cargo run --example verify_render -p pdf_engine -- <pdfium_dylib_경로> <pdf_경로>`
  - **실제 사용자 PDF 3종으로 추가 검증** (2026-07-12, `pdf-samples/`): `crates/pdf_engine/examples/smoke_test.rs`(범용 스모크 테스트, assert 없이 구조적 정상성만 확인) 작성 후 실행. 히트테스트/range 추출/quad 개수는 세 파일 모두 통과했으나, 이 테스트 자체는 quad의 **회전 방향까지는** 검증하지 못함(개수·비퇴화만 확인) — 실제 회전 정확성 문제는 사용자가 `BZR001088_01.pdf` 1~2페이지가 세로쓰기 디자인 타이틀임을 지적해 별도로 조사하다가 발견함(아래 참고, 스큐 대응 섹션에 상세 기록).
    - `BZB000877_01.pdf`(4페이지, 스캔 포스터류), `BZR001088_01.pdf`(24페이지, 3페이지 샘플링, 한글+영문+특수문자 혼용), `KKZ000160_01.pdf`(1페이지, 스캔 공문) — 렌더링/히트테스트/range 추출/quad 개수 전부 통과.
    - **회전 quad 버그 발견 및 수정** (사용자가 세로쓰기 페이지를 지적하며 발견): `BZB000877_01.pdf` idx 26, `BZR001088_01.pdf` idx 0 두 실제 문자 모두 `angle_radians()`가 큰 값(6.215rad, 1.571rad≈90°)을 반환했지만 `render_crop` 예제로 렌더링해 육안 확인하니 글리프는 완전히 똑바로 그려져 있었음 — 원래 설계(각도만큼 quad 회전)는 이 경우 90도 뒤틀린 하이라이트를 그렸을 실제 버그였음. `crates/pdf_engine/src/skew.rs`를 axis-aligned 방식으로 수정함(3. 텍스트 선택/복사 섹션의 "스큐 대응" 항목에 근거와 트레이드오프 상세 기록).
    - 실행: `cargo run --example smoke_test -p pdf_engine -- <pdfium_dylib_경로> <pdf_경로> [최대_페이지수]`, 개별 문자 진단은 `cargo run --example dump_chars -p pdf_engine -- <pdfium_dylib> <pdf> <page_0based>`, 시각 확인은 `cargo run --example render_crop -p pdf_engine -- <pdfium_dylib> <pdf> <page_0based> <out.png> [char_index]`
  - **아직 실기 미검증**: "진짜로 시각적으로 기울어진" 스캔 텍스트(글리프 자체가 화면에 기울어져 렌더링되는 경우)는 샘플에서 못 만남 — 지금까지 만난 회전값이 큰 케이스는 전부 "각도값은 크지만 시각적으로는 똑바름"이었음. 네이티브 창에서의 실제 마우스 드래그 조작감(트랙패드 핀치, pan 관성 등)도 디스플레이 접근 가능한 환경에서 눈으로 확인 필요.
- [x] **한글 폰트 렌더링(빈 사각형/tofu 버그) 수정** (2026-07-12) — egui 기본 폰트(Hack/Ubuntu-Light)는 한글 글리프가 없어 툴바/사이드바의 "파일 열기", "북마크" 등 한글 텍스트가 빈 사각형으로 표시됨. `crates/ui/src/fonts.rs` 추가: OS에 이미 설치된 한글 폰트(macOS `AppleGothic.ttf`, Windows `malgun.ttf`, 리눅스 Noto CJK 경로)를 찾아 fallback으로 등록. `AppleSDGothicNeo.ttc`는 TrueType Collection이라 egui 폰트 로더가 파싱 못 함 — 반드시 standalone `.ttf`/`.otf`만 후보로 사용해야 함. `main.rs`에서 `eframe::run_native` 진입 시 `fonts::install_korean_font(&cc.egui_ctx)` 호출.
  - **검증**: 화면 캡처 불가 환경이라 `crates/ui/examples/verify_fonts.rs`로 헤드리스 검증 — `egui::Context`에 동일 로직으로 폰트 등록 후 `Fonts::has_glyphs()`로 한글 문자열 글리프 보유 확인 + 실제 `layout_no_wrap()` 결과 폭이 0이 아님을 확인. `cargo run --example verify_fonts -p ui`로 재현 가능.
  - **미해결**: 배포 시에는 OS 폰트 경로에 의존하지 말고 폰트를 앱에 직접 동봉하는 것을 권장(Windows 구버전/최소 설치본에 malgun.ttf가 없을 수 있음). 현재는 개발 편의상 시스템 폰트 재사용.
- [x] **실제 GUI 사용 중 발견된 버그 3건 수정** (2026-07-13, 사용자가 실행해보고 직접 리포트) — 이 세션은 화면 접근이 안 돼 코드 검토 + 재현 가능한 원인 파악 후 수정만 했고, 실제 클릭/드래그로 재검증은 못 함(사용자 재확인 필요):
  - **호버 시 커서가 안 바뀜**: 원인 확정 — `char_index_at_screen_pos` 히트테스트를 `response.interact_pointer_pos()`(버튼이 눌려있을 때만 값 있음)로만 호출하고 있어서, 그냥 마우스를 올리기만 하는 호버 상태에서는 애초에 히트테스트 자체가 실행되지 않았음. `response.hover_pos()`(버튼 상태 무관하게 항상 갱신)로 별도 호버 히트테스트를 추가하고 `ctx.set_cursor_icon(CursorIcon::Text)` 호출(`crates/ui/src/viewer_panel.rs`).
  - **Cmd+C 복사 안 됨**: 원인 확정 — `handle_page_navigation_keys`가 함수 맨 위에서 `if ctx.wants_keyboard_input() { return; }`로 조기 리턴하는데, Cmd+C 체크가 이 리턴문 아래(같은 함수 안)에 있어서 페이지 번호 입력창 등 아무 텍스트 위젯이라도 포커스를 쥐고 있으면 복사가 조용히 씹혔음. 화살표 페이지 이동만 그 가드 안쪽으로 남기고 Cmd+C 체크는 가드 밖으로 분리(`crates/ui/src/app.rs`).
  - **우클릭 복사**: 애초에 구현이 안 돼 있었음(신규 기능). `response.context_menu(...)`로 "복사" 메뉴 추가, 선택 영역 없으면 버튼 비활성화.
  - **드래그 앤 드롭으로 파일 열기 안 됨**: 애초에 구현이 안 돼 있었음(신규 기능) — `ctx.input(|i| i.raw.dropped_files)`를 어디서도 읽지 않고 있었음. `app.rs`에 `handle_dropped_files` 추가해 `update()`에서 매 프레임 확인, 경로가 있으면 `open_file()` 호출.
  - **부수 발견**: `TextureHandle::size_vec2()`가 텍스처의 실제 픽셀 크기를 반환하는데(포인트로 안 나눔) 이를 그대로 화면 표시 크기(포인트)로 써서, Retina(2x) 디스플레이에서 PDF 렌더링이 필요 이상으로 저해상도로 나와 흐릿하게 보일 수 있는 버그도 같이 발견해 수정(`ctx.pixels_per_point()`로 렌더링 타겟은 물리 픽셀 기준, 화면 배치는 포인트 기준으로 분리).
  - **사용자 실기 재검증 결과** (2026-07-13, 화면 녹화 권한 부여 후): 호버 커서 OK, 우클릭 복사 OK. **Cmd+C는 여전히 안 됨** — 위에서 고친 `wants_keyboard_input()` 가드 분리만으로는 부족했음.
  - **Cmd+C 진짜 원인**: `egui-winit` 0.29.1이 Cmd+C를 감지하면 raw `Key::C` 키 이벤트 자체를 만들지 않고 `egui::Event::Copy`로 바꿔치기한 뒤 그대로 `return`해버림(`egui-winit-0.29.1/src/lib.rs`의 `is_copy_command` 분기: `is_copy_command(...) { events.push(Event::Copy); return; }`). 즉 `i.key_pressed(Key::C) && i.modifiers.command` 조건은 Cmd+C 눌러도 **절대 참이 될 수 없는 검사**였음 — modifiers.command 체크는 처음부터 무의미했음. `ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)))`로 직접 `Event::Copy`를 봐야 함(`crates/ui/src/app.rs`).
  - 재빌드 후 사용자가 재실행해 Cmd+C 정상 동작 확인함(2026-07-13). **호버 커서/우클릭 복사/Cmd+C 3가지 모두 실기 확인 완료.** 드래그앤드롭 파일 열기는 아직 실기 미확인.
- [x] **북마크 CSV/Excel 가져오기·내보내기를 툴바에 연결** (2026-07-13) — `bookmark`/`import_export` 크레이트는 이미 만들어지고 단위테스트도 통과했지만 UI에 버튼이 전혀 없어 앱에서 못 쓰던 상태였음(스펙 맨 위 "목적"에 있는 핵심 기능인데 누락). 툴바에 "북마크 내보내기"/"북마크 가져오기" 메뉴 버튼 추가, 각각 CSV/Excel 옵션(`crates/ui/src/toolbar.rs`). `PdfViewerApp`에 `export_bookmarks_csv`/`export_bookmarks_xlsx`/`import_bookmarks_csv`/`import_bookmarks_xlsx` 추가 — `bookmark::flatten_tree`/`build_tree`와 `import_export::export_*`/`import_*`를 그대로 연결(`crates/ui/src/app.rs`).
  - 내보내기 시 파일명 컬럼은 현재 열려있는 PDF 파일명 사용(없으면 빈 문자열). 가져오기는 파일 전체를 하나의 트리로 만들어 `app.bookmarks`를 통째로 교체(기존 북마크 확인 없이 덮어씀 — MVP 단순화, 필요하면 나중에 확인 다이얼로그 추가).
  - **검증**: 개별 함수(`flatten_tree`/`build_tree`, `export_csv`/`import_csv`, `export_xlsx`/`import_xlsx`)는 이미 단위테스트가 있었지만, 이걸 다 이어붙인 전체 경로(트리→flat row→파일→flat row→트리)가 실제로 원래 트리 구조를 보존하는지는 테스트된 적이 없었음 — `crates/import_export/tests/tree_roundtrip.rs` 추가해 CSV/Excel 둘 다 계층 구조(부모-자식, title, page) 보존 확인. `cargo test -p import_export` 통과.
  - **실기 미검증**: 툴바 메뉴 버튼 자체(파일 다이얼로그 열기/닫기, 클릭 반응)는 실제 GUI로 안 눌러봄 — 실행해서 확인 요망.
- [x] **PDF 자체 북마크(outline) 편집·저장 기능 구현** (2026-07-13) — 자세한 설계/검증은 §4 "PDF 자체 북마크(outline) 읽기/쓰기" 참고. 요약:
  - 새 크레이트 `crates/pdf_outline_writer` 추가(lopdf 기반 쓰기), `pdf_engine::outline` 모듈 추가(pdfium 기반 읽기)
  - `crates/ui/src/app.rs`: 문서 열 때 내장 북마크 자동 로드, `bookmarks_dirty` 플래그로 변경 추적, `save_bookmarks_to_pdf()`(임시파일→pdfium 재검증→원자적 교체), 다른 문서를 열려 할 때 저장 안 된 변경사항 있으면 "저장/저장하지 않음/취소" 확인창(`show_unsaved_changes_dialog`)
  - `crates/ui/src/sidebar.rs` 전면 재작성 — 실사용 중 발견된 버그 수정:
    - **이름 수정**: 원래 `// TODO` 주석만 있고 미구현이었음 → 더블클릭으로 인라인 편집 모드 진입, Enter/포커스 아웃 시 커밋, Escape로 취소
    - **삭제**: 코드 자체는 있었지만 우클릭 메뉴 안에 숨어있어 발견하기 어려웠음("삭제 안 됨"으로 보고됨) → 항상 보이는 "✕" 버튼 추가(우클릭 메뉴도 유지)
    - **클릭해서 페이지 이동**: `jump_page` 값을 계산은 해놓고 `let _ = jump_page;`로 버리고 있었음(미완성 배선) → 실제로 `app.go_to_page()` 호출하도록 연결
    - **재귀 버그**: `render_nodes`가 재귀 호출마다 `jump_page`/`delete_id`를 그 프레임의 지역 변수로 새로 선언해서, 하위(자식) 노드에서 벌어진 일이 상위 호출로 전파되지 않고 조용히 버려지고 있었음(최상위 레벨 클릭만 반영됨) → `RenderOutcome` 누적 구조체를 재귀 전체에 `&mut`로 그대로 전달하도록 수정
  - `crates/ui/src/toolbar.rs`: "저장" 버튼 추가(북마크 변경 있을 때만 활성화), "파일 열기"/드래그앤드롭/CLI 인자 전부 `request_open_file`(dirty 체크 경유)로 통일
  - **검증**: `pdf_outline_writer` 크레이트 레벨은 실제 PDF(사용자가 제공한 `embeddedoutline.pdf` 포함)로 강하게 검증됨(§4 참고, 6개 테스트). **UI 레벨(저장 버튼 클릭, 확인창 3버튼, 사이드바 이름수정/삭제/드래그 실제 조작)은 실제 GUI로 확인 안 됨** — 다음 실기 테스트 시 확인 필요.
- [x] **사이드바 UX 대규모 개선 + 뷰어 트랙패드 제스처** (2026-07-13, 사용자가 실기 테스트하며 리포트한 항목 일괄 처리) — 실기 확인된 것: 삭제/저장/export·import는 정상 동작 확인됨. 이번에 추가로 처리한 항목들:
  - **선택 모델 도입**: `app.selected_bookmark: Option<Uuid>` 신설. 클릭하면 선택되면서 뷰어가 그 페이지로 이동, 이미 선택된 항목을 다시 클릭하면 인라인 편집 모드 진입(요청 그대로: "선택 → 클릭 → 이동", "선택된 걸 한 번 더 클릭 → 편집"). 뷰어 패널 클릭 시 선택 해제(그래야 화살표 키가 다시 페이지 이동으로 돌아옴).
  - **"+"/"−"/Undo 버튼**: 기존 "+ 추가"를 "+"로, 옆에 "−"(선택 항목 삭제), 옆에 "Undo" 버튼 추가. "+"는 이제 **선택된 항목의 자식**으로 추가(없으면 최상위) — `bookmark::insert_node` 새로 추가(크레이트 lib.rs에 export, 단위테스트 3개). 추가 즉시 편집 모드로 들어가 바로 이름을 타이핑할 수 있음.
  - **체크박스처럼 보이던 삭제 버튼 버그**: 원인 확정 — 이전 세션에서 삭제 버튼에 "✕"(U+2715) 문자를 썼는데, 로드된 폰트(egui 기본 + AppleGothic 폴백) 어디에도 이 글리프가 없어 빈 사각형(tofu)으로 렌더링돼 체크박스처럼 보였음. 이번에 아예 per-row 삭제 버튼 자체를 없애고(상단 "−" 버튼으로 통합), 남은 아이콘(fold 화살표 `>`/`v`, +/−/Undo)은 전부 plain ASCII만 사용해 이 클래스의 버그 재발을 원천 차단.
  - **실행취소(Cmd+Z)**: `app.bookmark_undo_stack: VecDeque<Vec<BookmarkNode>>` 최대 20개 스냅샷. 추가/삭제/이름수정/드래그이동 직전에 `push_bookmark_undo_snapshot()` 호출. Cmd+C 때와 달리 egui-winit이 Cmd+Z를 별도 세맨틱 이벤트로 가로채지 않는 것을 소스로 직접 확인 후 raw 키 체크로 구현(`crates/ui/src/app.rs`).
  - **트리 접기/펼치기**: 자식 있는 노드에 `>`(접힘)/`v`(펼침) 토글 버튼. 접힘 상태는 `DragState.collapsed: HashSet<Uuid>`에 순수 UI 상태로만 보관(BookmarkNode/CSV/PDF outline 스키마에는 안 들어감).
  - **화살표 키 트리 탐색**: 북마크가 선택된 동안 위/아래=형제·자식 포함 화면에 보이는 순서로 이전/다음 이동, 왼쪽=접기, 오른쪽=펼치기. 페이지 이동용 좌/우 화살표와 충돌해서, 북마크가 선택된 동안은 페이지 이동 화살표를 비활성화(뷰어 클릭으로 선택 해제하면 복귀).
  - **드래그 삽입 위치 표시**: 기존엔 대상 행 전체를 테두리로 감싸기만 해서 Before/After/Inside 구분이 안 됐음 → Before/After는 대상 행 위/아래에 가로선을 그리고, Inside만 테두리 박스로 표시(Acrobat류 관례).
  - **사이드바 너비/줄바꿈**: `SidePanel::min_width(90.0)` 추가(예전엔 사실상 컨텐츠 폭 이하로 안 줄어들었음), 라벨을 `selectable_label`(줄바꿈 불가) 대신 `egui::Label::new(text).wrap().sense(Sense::click_and_drag())`로 교체 — 긴 제목이 패널 폭에 맞춰 자동 줄바꿈되고 패널을 계속 좁힐 수 있음.
  - **핀치 줌**: 스펙 문서에 예전부터 "raw winit 후킹 필요"로 남아있던 미해결 항목이었는데, 실제로 `egui-winit` 0.29.1 소스를 확인해보니 macOS `WindowEvent::PinchGesture`를 이미 내부적으로 `egui::Event::Zoom`으로 변환해주고 있었음 — 별도 후킹 불필요, `ctx.input(\|i\| i.zoom_delta())`만 읽으면 됨. 훨씬 간단하게 해결됨(`crates/ui/src/viewer_panel.rs`).
  - **트랙패드 두 손가락 패닝**: `ctx.input(\|i\| i.smooth_scroll_delta)`를 Ctrl 안 눌렸을 때 `pan_offset`에 직접 반영하도록 추가(이전엔 Ctrl+스크롤=줌에만 쓰이고 평범한 스크롤은 아무 데도 안 쓰이고 있었음 — "두 손가락 작동 안 함" 리포트의 원인). 세 손가락 드래그는 macOS 손쉬운 사용 설정("세 손가락으로 끌기")이 켜져 있으면 OS가 일반 마우스 드래그로 합성해주므로 기존 pan 코드가 별도 처리 없이 그대로 먹음 — 우리 쪽에서 추가로 할 일 없음.
  - **회귀 버그 발견 및 수정**: `pdf_outline_writer`의 실제 문서 테스트가 갑자기 실패해서 조사해보니, 사용자가 방금 위 "삭제/저장" 기능을 실기 테스트하면서 `pdf-samples/embeddedoutline.pdf`(원래 6개 장)에서 일부 장을 실제로 지우고 저장한 상태였음(파일 수정시각이 사용자 테스트 시각과 정확히 일치) — **코드 버그가 아니라 저장 기능이 의도대로 정확히 동작했다는 증거**였음. 테스트가 사용자의 실사용 파일에 하드코딩된 기대값을 갖고 있던 게 테스트 설계 결함이었음 → 사용자가 이미 갖고 있던 손 안 댄 백업(`embeddedoutline 복사본.pdf`, 2022년 파일)을 임시 디렉터리로 복사해 그 복사본만 쓰도록 테스트를 격리해 수정.
  - **실기 미검증**: 이번에 만든 모든 사이드바 상호작용(fold, 화살표 키, 드래그 삽입선, undo, 패닝/핀치줌)은 화면 접근 안 되는 세션에서 코드 레벨로만 구현·빌드확인했음 — 실행해서 확인 필요.
- [x] **사이드바 2차 개선 라운드** (2026-07-13, 사용자 실기 리포트 반영) — 실기 확인됨: 핀치줌, 화살표 기본 동작. 이번에 처리한 것:
  - **드래그가 "텍스트 선택 박스처럼 보이는" 버그의 진짜 원인**: `egui::Label`은 기본적으로 `selectable_labels` 스타일이 켜져 있어 드래그를 자체 텍스트 선택 UI가 가로챈다(egui 소스의 `Label::ui`가 `selectable`이면 `LabelSelectionState::label_text_selection`을 호출) — 우리가 원하는 `Sense::drag()` 재정렬과 충돌해서 마치 선택 영역을 조절하는 것처럼 보였음. `.selectable(false)`로 해결(`crates/ui/src/sidebar.rs`). 드래그 중인 항목 자체에 반투명 하이라이트도 추가해 "지금 이게 들려서 옮겨지고 있다"는 피드백 보강. 드래그 재구성(`move_node`)이 undo 스택에 안 쌓이던 것도 이번에 같이 고침(예전엔 드래그 이동만 undo 불가능했음).
  - **문서 아이콘(📄) 제거**: 요청대로 라벨 텍스트에서 뺌.
  - **Delete 단축키**: 원인 확정 — 애초에 구현 자체가 없었음(신규 기능). `app.rs`에 Delete/Backspace 키 체크 추가, `wants_keyboard_input()` 가드 안쪽이라 이름 편집 중 백스페이스가 항목을 지워버리는 사고는 안 남.
  - **Cmd+B(북마크 추가)**: 신규. DragState(편집 포커스 진입)가 sidebar.rs 로컬 상태라 app.rs 전역 단축키에서 직접 못 건드림 → `app.request_add_bookmark` 플래그를 세워두고 sidebar.rs가 매 프레임 소비하는 방식으로 연결. "+"버튼과 동일한 로직(`add_new_bookmark` 헬퍼로 공유).
  - **좌우 화살표 fold/unfold가 리프 노드에서 안 먹던 문제**: 원인 확정 — 선택된 노드 자신이 자식을 가질 때만 접기/펼치기가 동작했음. "선택된 항목이 속한 레벨"을 조작하도록 수정 — 리프 노드가 선택돼 있으면 그 부모(`parent_of` 신규 헬퍼)를 대상으로 접기/펼치기.
  - **Redo**: `bookmark_redo_stack` 추가, Undo 버튼 옆에 Redo 버튼, Cmd+Shift+Z. 기존 Cmd+Z 체크가 shift 여부를 안 가려서 Cmd+Shift+Z를 눌러도 매번 undo만 실행되던 버그도 같이 고침(shift 있으면 redo, 없으면 undo로 분기).
  - **시작 시 마지막 파일 자동 열기**: `eframe`의 `persistence` feature 활성화(→ `ron` 의존성 추가), `App::save()`에서 `current_file` 경로를 문자열로 저장, `PdfViewerApp::new()`에서 복원 후 `main.rs`가 CLI 인자보다 낮은 우선순위로 자동 오픈.
  - **"저장/취소" 확인창**: 기존 3버튼(저장/저장하지 않음/취소) 구현이 이미 정확히 요청대로 동작함을 코드 재검토로 확인(취소 시 `pending_open_path`만 지우고 `current_file`/`document`는 안 건드려서 원래 화면 그대로 유지) — 변경 없음.
  - **현재쪽/전체쪽 필드 폭**: `desired_width(50.0)` 고정값이었던 걸 `total_pages` 자릿수 기반으로 계산하도록 수정(`crates/ui/src/toolbar.rs`).
  - **회귀 재확인**: `pdf_outline_writer` 6개 테스트 + 전체 워크스페이스 테스트 그대로 통과.
  - **실기 미검증**: 이번 라운드 전부(드래그 시각 피드백, Delete/Cmd+B/Cmd+Shift+Z 단축키, 좌우 화살표 리프 노드 동작, 시작 시 자동 열기, 페이지 필드 폭)는 화면 접근 안 되는 세션이라 코드 레벨 구현+빌드확인까지만 함.
- [x] **사이드바 3차 개선 — 드래그 표시 안 됨/포커스 상실 근본 원인 규명** (2026-07-13) —
  - **드래그 위치/삽입선이 전혀 안 보이던 진짜 원인**: `egui::Response::hovered()`는 문서에 "다른 위젯이 드래그 중일 때는 항상 false"라고 명시돼 있음 — 우리 코드가 다른 행(대상 행)의 `hovered()`로 드롭 위치를 계산했는데, 드래그 중엔 드래그 원본이 아닌 다른 행의 `hovered()`가 절대 true가 안 돼서 `hover_target`이 드래그 내내 `None`으로 고정돼 있었음. `contains_pointer()`로 교체(egui 문서가 정확히 이 용도로 안내하는 API). 이 버그 때문에 `move_node`가 호출되는 조건(`dragging`과 `hover_target` 둘 다 `Some`)이 성립한 적이 없어 **드래그 재정렬 자체가 실제로는 한 번도 적용된 적이 없었음** — 화면에 안 보이는 것뿐 아니라 실제로 트리도 안 바뀌고 있었음.
  - **"새 문서 열 때 확인창 안 뜸"의 유력한 원인**: 위 버그와 연결돼 있을 가능성이 높음. 드래그로 재정렬을 시도했지만 실제로는 아무 변경도 적용 안 됐다면 `bookmarks_dirty`가 계속 `false`로 남아 확인창이 뜨지 않는 게 오히려 정상 동작임. 이름수정/추가/삭제처럼 드래그가 아닌 경로로 dirty가 되는지는 코드 재검토로는 문제를 못 찾음 — 드래그 버그 수정 후 재확인 필요(안 뜨면 알려주면 좋음, 그때는 정말 별개 버그로 다시 조사).
  - **폴딩 후/삭제 후 포커스 상실**: 둘 다 원인 확정 — (1) 화살표 키로 리프 노드의 부모를 접을 때 선택은 그대로 리프에 남아있는데 그 리프가 화면에서 사라져서, 다음 화살표 키 입력 시 "보이는 목록에서 선택된 id를 못 찾음" → 아무 반응 없음. 접은 노드(부모)로 선택을 옮기도록 수정. (2) 삭제 시 무조건 `selected_bookmark = None`으로 날려버렸음 → 삭제 "전에" 다음 형제/이전 형제/부모 순으로 차기 선택 대상을 계산해두는 `bookmark::sibling_or_parent_after_removal` 신규 헬퍼 추가(단위테스트 4개) 후 삭제 뒤 그 값으로 선택 유지.
  - `parent_of`도 sidebar.rs 로컬 복제본을 없애고 `bookmark` 크레이트의 공용 함수로 통합(단위테스트 추가) — app.rs(삭제)와 sidebar.rs(화살표 키) 양쪽에서 같은 구현을 재사용.
  - **실기 검증 완료(2026-07-13, 사용자 확인)**: "항목 드래그 관련 기능은 모두 정상" — `hovered()` → `contains_pointer()` 수정이 실제로 드래그 하이라이트/삽입선/재정렬 전부를 고쳤음을 확인. `bookmarks_dirty`가 드래그로도 정상적으로 true가 되는 것도 간접 확인됨(재정렬이 실제로 적용된다는 뜻이므로).
- [x] **"저장"/"저장하지 않음" 선택 시 앱이 꺼지는 크래시 — 원인 확정 및 수정** (2026-07-13). 조사 경위 전체를 기록해둠(1차 가설은 틀렸고, 사용자가 터미널에서 직접 실행해 패닉 메시지를 받아준 덕에 진짜 원인을 찾음):
  - **1차 가설(틀림)**: pdfium이 같은 바인딩에서 문서 두 개가 동시에 열려있는 걸 못 견뎌서 크래시하는 거라고 추정하고 `crates/ui/tests/document_switch_crash_repro.rs`로 재현 시도 → SIGSEGV/SIGTRAP 재현됐으나, 원인은 그 테스트 자체가 테스트마다 `PdfEngine::new_with_library_path`를 새로 호출해서 `cargo test` 병렬 실행 시 여러 스레드가 동시에 pdfium 바인딩을 초기화하려다 난 크래시였음(테스트 설계 결함, 실제 앱과 무관). `OnceLock` 공유 엔진으로 고쳐서 재확인하니 문서 A 열기→저장→문서 B 열기 전체 시퀀스가 크래시 없이 정상 동작 — pdfium document 라이프사이클은 무죄로 판명.
  - **진짜 원인(사용자가 터미널에서 직접 실행해 알아냄)**: wgpu validation panic —
    ```
    thread 'main' panicked at wgpu-22.1.0/src/backend/wgpu_core.rs:2314:30:
    Error in Queue::submit: Validation Error
    Caused by:
      Texture with 'egui_texid_Managed(1)' label has been destroyed
    ```
    `update()`의 호출 순서가 `toolbar → sidebar → viewer_panel → 확인창`이었는데, `viewer_panel::show`가 `self.page_texture`를 참조하는 draw call을 이번 프레임 큐에 이미 넣어놓은 **뒤에** 확인창의 "저장"/"저장하지 않음" 버튼이 `open_file_now`를 호출해 그 텍스처를 드롭(`self.page_texture = None`)함 — 프레임이 끝나고 wgpu에 제출될 때 이미 파괴된 텍스처를 참조하는 draw call이 남아있어 Validation Error로 패닉. 툴바 "파일 열기"/드래그앤드롭은 전부 `viewer_panel::show`보다 **먼저** 실행되기 때문에 이 문제가 없었음(그 프레임의 viewer_panel이 이미 None이 된 텍스처를 보고 그냥 안 그림) — 확인창만 유일하게 순서가 뒤였음.
  - **수정**: `update()`에서 `show_unsaved_changes_dialog(ctx, self)` 호출을 `viewer_panel::show(ctx, self)`보다 앞으로 옮김(`crates/ui/src/app.rs`). egui의 `Window`는 `Order::Middle`, `Panel`은 `Order::Background`라 레이어가 `Order` 값으로 정렬되므로(콜 순서 아님) 코드 순서를 바꿔도 확인창이 여전히 화면 맨 위에 그려짐 — egui 소스로 확인.
  - **부수적으로 남겨둔 방어적 수정**: `open_file_now`/`save_bookmarks_to_pdf`에서 재오픈 전 `self.document = None`으로 먼저 명시적으로 드롭하는 코드는 (1차 가설 조사 중 추가한 것) 근거는 사라졌지만 해롭지 않아 그대로 둠.
  - **미검증**: 실제 클릭으로 크래시가 정말 사라졌는지는 화면 접근이 안 되는 세션이라 다시 실행해서 확인 필요. wgpu 패닉 메시지 자체가 정확한 위치를 짚어줘서 근거는 확실하지만, 재현 테스트는 못 함(이 버그는 GUI 프레임 타이밍 문제라 headless pdfium 테스트로는 애초에 재현 불가능한 종류였음).
- [x] **앱 이름 'PDF Outliner' 확정, 창 헤더에 파일명 표시, 사이드바 정렬/F2 단축키** (2026-07-13)
  - 창 제목: `ctx.send_viewport_cmd(ViewportCommand::Title(...))`로 매 프레임 필요시 갱신, "PDF Outliner" 또는 "PDF Outliner - 파일명" 형식. `main.rs`의 초기 타이틀/앱 이름도 통일.
  - **자식 있는 항목이 왼쪽으로 살짝 밀려 보이던 정렬 문제**: 원인 확정 — 접기/펼치기 화살표 버튼(`Button::new(icon).small().frame(false)`)의 실제 렌더링 폭이 리프 노드용 `add_space(18.0)`와 정확히 안 맞았음. `add_sized`로 두 경우 모두 정확히 같은 고정 폭(18.0)을 쓰도록 통일.
  - F2: 선택된 항목을 즉시 이름 편집 모드로(재클릭/우클릭 메뉴와 동일한 진입점, 키보드 전용 경로 추가).
  - **실기 미검증**: 이번 항목들도 화면 접근 불가 세션이라 빌드 확인까지만 함.
- [ ] SQLite 스키마 확정 및 마이그레이션(북마크 저장은 이제 PDF 자체 outline이 1차 수단이라 우선순위 낮아짐 — CSV/Excel처럼 보조/백업 용도로만 필요할 수도)
- [ ] CI/서명/패키징 파이프라인 구성
- [ ] 배포용 pdfium 동적 라이브러리를 앱 번들에 정식 동봉(현재는 개발 편의상 Homebrew에 이미 설치된 라이브러리를 코드에 하드코딩된 폴백 경로로 사용 중 — `crates/ui/src/app.rs`의 `create_engine()` 참고. 배포 전 반드시 앱 리소스 경로로 교체 필요)

## 확인 필요 미결 사항

- **개발 환경**: `~/.zshrc`에 `. "$HOME/.cargo/env"` 추가 완료(2026-07-12) — 새 터미널 세션이면 `rustc`/`cargo` 바로 사용 가능.
- 그 외 이전 논의된 요구사항은 모두 해소됨(북마크 페이지번호 컬럼, OCR 범위 밖 확정, 이미지 전용 페이지 안내 불필요).
