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
cargo test --workspace    # 전부 통과해야 정상 (bookmark 13, import_export 3+2, pdf_outline_writer 6, ui 4+3 등)
./target/debug/PDF-Outliner [선택: 열 pdf 경로]
```
pdfium dylib 탐색 순서(`crates/ui/src/app.rs`의 `create_engine()`, 2026-07-13 배포 대응으로 순서 변경): (1) 실행 파일 기준 배포 번들 경로(macOS `../Frameworks/libpdfium.dylib`, Windows 같은 디렉토리 `pdfium.dll`) → (2) `PDFIUM_DYLIB_PATH` 환경변수 → (3) 이 머신 전용 Homebrew `ocrmypdf` 종속성 경로 하드코딩 폴백: `/opt/homebrew/Cellar/ocrmypdf/17.8.0/libexec/lib/python3.14/site-packages/pypdfium2_raw/libpdfium.dylib`. 배포 패키징은 §5 참고.

**git**: 2026-07-13에 프로젝트 루트에 `git init` 완료(그 전엔 git repo 아니었음), 첫 커밋 존재. `target/`은 `.gitignore` 처리됨(6GB+). 앞으로 변경할 때마다 `git add . && git commit`으로 스냅샷 남길 것 — 이제 `git log`/`git diff`로 이력 추적 가능.

**핵심 기능(구현 완료, 사용자 실기 검증 대부분 완료)**: PDF 뷰어(렌더링/줌/팬/텍스트선택복사) + 북마크 사이드바(추가/이름수정/삭제/드래그재정렬/폴딩/Undo·Redo/단축키) + PDF 자체 outline에 저장(lopdf) + CSV/Excel import·export. 자세한 아키텍처는 §4 참고. 기능별 검증 상태는 §6.

**아직 실기 미확인**: F2 단축키, 크래시 복구 자동저장(1분 주기 JSON 스냅샷 + 시작 시 복구 프롬프트, 2026-07-13 신규 — 실제 강제종료로 재현 필요), 저장 안 된 변경사항이 있을 때 앱 종료 시도 시 저장/저장안함/취소 확인창(2026-07-13 신규 — 구현 직후 바로 검색 기능 요청으로 넘어가 실기 확인 기회가 없었음) — §6 참고. wgpu 크래시 수정, Cmd+[/Cmd+] 페이지 이동 히스토리, 문서 전체 텍스트 검색(Cmd/Ctrl+F, 검색 실행/크래시 미재발/포커스 이동/현재 페이지 전체 하이라이트 전부 포함)은 이미 사용자 실기로 확인됨. **egui/eframe은 0.29.1 그대로다** — 0.35로 올렸다가 한글 IME가 오히려 더 심하게 깨지고 사이드바 글자 렌더링까지 흐릿해지는 명백한 회귀가 나서 즉시 0.29.1로 되돌림(§7 "egui/eframe 0.29 → 0.35 업그레이드 시도와 롤백" 참고). 그 후 앱 코드 레벨 워크어라운드도 두 버전(v1/v2) 시도했으나 둘 다 실기로 효과가 없거나 불분명해 전부 되돌림 — 당시엔 업스트림 winit 버그([#3095](https://github.com/rust-windowing/winit/issues/3095))로 결론냈으나, **2026-07-14 재조사에서 winit 포크 패치([patch.crates-io] + bjorkii/winit)로 필드 전환 자소 유출(증상 B: discardMarkedText 패치 + 앱 레벨 토글)과 세션 첫 글자 자소 분리(증상 A: CGEvent 웜업)를 둘 다 해결, 사용자 실기 확인 완료**(§ "확인 필요 미결 사항" 참고, §7 참고).

**2026-07-13 사용자 실기로 확인 완료**: 사이드바 선택 항목 Enter로 페이지 이동, 사이드바 자식 있는/없는 항목 정렬(근본 원인은 egui `add_sized`가 요청한 크기가 아니라 실제 렌더된 크기만큼만 부모 커서를 전진시키는 것이었음 — §7), PDF 문서 내 링크 클릭(내부 페이지 이동/외부 URI 열기) 전부 "모두 잘 반영됐다"고 확인받음.

**이 세션 환경의 제약**: 화면 캡처/실제 GUI 조작 불가능한 샌드박스. 검증은 (a) headless 예제/테스트로 pdfium·lopdf 로직을 실제 라이브러리로 확인, (b) egui/wgpu 소스를 직접 읽어 API 동작 확인, (c) 사용자가 실행해보고 리포트 → 원인 규명 → 수정 → 재확인 요청, 의 조합. **GUI 상호작용 버그는 사용자의 실기 리포트 없이는 발견 자체가 어려움** — 다음 세션도 이 사이클을 유지할 것. 자세한 재발방지용 기술 교훈은 §7.

**디버깅 도구**(재사용 가능):
- `cargo run --example dump_outline -p pdf_engine -- <pdfium_dylib> <pdf>` — PDF 내장 북마크 트리 출력
- `cargo run --example dump_chars -p pdf_engine -- <pdfium_dylib> <pdf> <page_0based>` — 페이지 내 문자별 좌표/회전각 출력
- `cargo run --example render_crop -p pdf_engine -- <pdfium_dylib> <pdf> <page_0based> <out.png> [char_index]` — 렌더링 결과를 PNG로 저장해 Read 툴로 육안 확인(화면 캡처 안 되는 세션에서 유일한 시각 확인 수단)
- `cargo run --example smoke_test -p pdf_engine -- <pdfium_dylib> <pdf> [최대_페이지수]` — 임의 PDF 렌더링/텍스트선택 구조적 정상성 확인
- 테스트용 실제 PDF 샘플: `pdf-samples/` 안에 여러 개. **일부는 사용자가 수동 GUI 테스트에 실사용 중이라 자동화 테스트가 함부로 건드리면 안 됨**(§7 "테스트 설계 원칙" 참고) — 자동화 테스트는 항상 pristine 백업을 임시 디렉터리에 복사해서 쓸 것.
  - `pdf-samples/SQ-main.pdf`(2026-07-13 추가, git 미추적 — 의도적으로 커밋 안 함, §7 "테스트 설계 원칙" 참고): 358페이지, 링크 3641개, 실제 한글 텍스트를 담은 큰 실사용 문서 — 링크/검색 기능을 대량·현실적 데이터로 검증할 때 이 파일을 씀(§3의 링크·검색 절 전부 이 파일로 검증함).

---

## 1. 전체 아키텍처 개요

| 영역 | 선택 | 이유 |
|---|---|---|
| PDF 렌더링 엔진 | **pdfium-render** (crates.io, 0.9.x) | Chromium PDFium 바인딩. 라이선스 깔끔(Apache 계열). 단, outline **쓰기** API는 없음 — §4 참고 |
| GUI 프레임워크 | **egui + eframe 0.29.1 (wgpu 백엔드)** | Immediate-mode, 바이너리 수 MB, 콜드 스타트 매우 빠름. Tauri(webview)는 초기화 오버헤드로 제외. 2026-07-13에 IME 버그 수정 시도로 0.35까지 올렸다가 렌더링/IME 둘 다 악화돼 롤백함(§7 참고) — 함부로 재시도하지 말 것 |
| 북마크 편집 | 메모리상 `Vec<BookmarkNode>` 트리 + 자체 드래그 재정렬 로직(`bookmark` 크레이트) | egui-arbor/egui_ltreeview 같은 서드파티 트리 위젯 대신 직접 구현(§4) |
| PDF 자체 북마크 쓰기 | **lopdf** (`IncrementalDocument`) | pdfium은 읽기만 가능 — §4 참고 |
| 북마크 저장(보조) | CSV/Excel export·import | SQLite는 아직 미착수, 우선순위 낮음(§6) |
| CSV | `csv` 크레이트 (+ 수동 BOM 처리) | §2 참고 |
| Excel | 읽기: `calamine` / 쓰기: `rust_xlsxwriter` | 인코딩 이슈 없음 |
| 클립보드 | `arboard` | macOS/Windows 유니코드(한글) 클립보드 안정적 |
| PDF 텍스트 API | pdfium-render `PdfPageText` (`FPDFText_*`) | §3 참고 |
| PDF 링크(주석) API | pdfium-render `PdfPageLinks`/`PdfAction`/`PdfDestination` | §3, §7 참고 |
| 외부 URI 링크 열기 | `open` 크레이트 | OS 기본 브라우저/메일 클라이언트로 위임, 크로스플랫폼 |
| 크래시 복구 자동저장 | JSON 스냅샷 파일 하나(`serde_json`) + eframe `save()`/`on_exit()` 훅 | §4, §7 참고 — SQLite는 이 용도로도 불필요하다고 판단(§6 하단 근거) |
| 문서 전체 텍스트 검색 | pdfium-render `PdfPageText::search()`(페이지별) + 자체 조립(문서 전체), **메인 스레드에서만** 프레임당 페이지 단위로 청크 실행 | §3, §7 참고 — 백그라운드 스레드로 시도했다가 세그폴트로 기각(§7 "PDFium 스레드 안전성" 항목) |

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
- 사이드바에서 북마크 선택 후 Enter — 선택된 항목의 페이지를 뷰어에 표시(`crates/ui/src/sidebar.rs`). 클릭으로 처음 선택할 땐 바로 페이지가 넘어가지만 화살표 키로 선택만 옮긴 뒤엔 안 따라오므로, Enter로 다시 확인 가능.

### 페이지 이동 히스토리 (Cmd+[ / Cmd+], 2026-07-13 신규)
- 웹브라우저 뒤로/앞으로가기와 동일한 관례. `PdfViewerApp`에 `page_back_history`/`page_forward_history`(`Vec<u32>`) 스택을 두고, `go_to_page()`(경로 무관 — 방향키/페이지번호입력/북마크클릭/문서내링크클릭 전부 이 함수를 거침)가 실제로 페이지를 바꿀 때마다 이전 페이지를 back 스택에 쌓고 forward 스택을 비운다(`crates/ui/src/app.rs`).
- `navigate_back`/`navigate_forward`(Cmd+[/Cmd+])는 스택을 서로 주고받으며 페이지만 옮기는 `set_current_page()`(private)를 호출 — `go_to_page()`를 그대로 부르면 히스토리 순회 자체가 다시 히스토리에 쌓이는 순환이 생기므로 반드시 분리해야 함.
- 새 문서를 열면 두 스택 다 초기화(`open_file_now`) — 이전 문서의 페이지 번호가 새 문서에서 아무 의미 없기 때문.
- 방향키로 페이지를 여러 장 순차로 넘긴 경우도 매번 히스토리에 쌓임(실제 웹브라우저가 페이지네이션 "다음" 클릭도 매 번 히스토리에 쌓는 것과 동일한 관례로 의도한 설계) — Cmd+[를 여러 번 누르면 그 순서를 그대로 되짚어간다.

### 문서 내 링크 클릭 (2026-07-13 신규)
- `crates/pdf_engine/src/links.rs`의 `link_target_at_point(page, x, y)`가 PDF 포인트 좌표 위의 링크를 찾아 `LinkTarget::Page(1-based)` 또는 `LinkTarget::Uri(String)`로 반환. `viewer_panel.rs`가 클릭 시(`response.clicked()`) 히트테스트해 내부 링크는 `go_to_page`로, 외부 URI는 `app.open_external_link()`(`open` 크레이트, OS 기본 브라우저)로 연다. 호버 시 손가락 커서로 링크임을 표시(텍스트 I-beam 커서보다 우선).
- **링크 탐색은 action(`/A`)과 destination(`/Dest`) 둘 다 확인해야 함**: pdfium이 이 둘을 별도 API로 노출(`PdfLink::action()` vs `PdfLink::destination()`) — 단순 GoTo 링크는 action 없이 destination만 있는 경우가 실제로 있어 action이 없으면 destination으로 폴백.
- **URI 액션에 스킴이 없는 실사용 사례 확인함**: 실제 샘플(`pdf-samples/SQ-main.pdf`, 3641개 링크 보유)에서 메일 링크가 `mailto:` 없이 `kofa@koreafilm.or.kr`로, 웹 링크가 `http://` 없이 `www.koreafilm.or.kr`로 저장돼 있었음(`FPDFAction_GetURIPath`가 PDF에 저장된 그대로 반환하는 것이지 pdfium 문제 아님) — 그대로 OS에 넘기면 파일 경로로 오인해 실패하므로 `normalize_uri()`가 스킴 없고 `@` 포함 시 `mailto:`, 그 외엔 `http://`를 붙여 보정.
- headless로 `dump_links` 예제(임시로 작성해 확인 후 삭제)를 돌려 `SQ-main.pdf` 전체에서 실제 pdfium 링크를 순회하며 위 두 케이스(내부 GoTo, 스킴 없는 mailto/도메인)를 실제로 재현·검증함 — 화면 캡처 불가 세션에서 pdfium API 가정을 검증하는 방식은 §7 원칙과 동일.

### 문서 전체 텍스트 검색 (Cmd/Ctrl+F, 2026-07-13 신규 — **전체 기능 사용자 실기 확인 완료**)

**UI**: 툴바 제일 오른쪽에 검색창 + 돋보기(🔍) 버튼 + ◀/▶ + "현재/전체" 카운트(`crates/ui/src/toolbar.rs`, `Layout::right_to_left` 하위 레이아웃으로 오른쪽 끝에 고정). Cmd+F(맥)/Ctrl+F(윈도우, `modifiers.command`)로 검색창에 포커스. 결과 없으면 "일치하는 결과가 없습니다" 알림창(Enter 또는 확인 버튼으로 닫기 → 닫히면 검색창에 재포커스, `request_focus_search` 플래그로 app.rs→toolbar.rs 전달, sidebar.rs의 `focus_editing`과 같은 패턴).

**검색창 포커스와 Enter 의미 분리**: 검색창이 포커스를 갖고 있는 동안의 Enter는 항상 `execute_search()`(새로 검색)만 의미한다. 처음엔 "결과 있으면 다음으로, 없으면 새로 실행"으로 짰는데, 결과를 순회하던 중 다른 검색어를 입력하고 Enter를 눌러도 재검색되지 않고 이전 검색어의 다음 결과로 넘어가버리는 버그가 있어(포커스 여부와 무관하게 `has_results`만 보고 분기했기 때문) 위처럼 단순화함. 검색이 결과와 함께 끝나면 `request_focus_next_result` 플래그로 포커스를 "다음 결과"(▶) 버튼으로 옮겨준다 — egui는 `Sense::click` 위젯이 포커스를 가진 채 Enter/Space를 누르면 클릭으로 처리하므로(`context.rs`의 `fake_primary_click`), 그 뒤 Enter는 자연스럽게 "다음 결과"가 되고 검색창은 다시 클릭해야만 포커스를 되찾는다.

**검색 로직 — `crates/pdf_engine/src/search.rs`**: pdfium은 페이지 단위로만 텍스트 검색을 지원한다(`PdfPageText::search()`). 문서 전체 검색은 그 위에 직접 조립 — 하이라이트용 bounding box는 문자 인덱스에서 재계산하지 않고 pdfium이 이미 계산해 둔 `PdfPageTextSegment::bounds()`(줄바꿈/폰트 경계로 병합된 사각형)를 그대로 씀(`crate::selection`의 문자별 quad 방식과는 다른, 더 단순한 경로).

**⚠️ 처음엔 백그라운드 스레드로 시도했다가 세그폴트 — 최종적으로 단일 스레드 청크 방식으로 대체.** PDFium이 스레드 안전하지 않다는 걸 몰라서(pdfium-render `thread_safe` feature의 README 설명을 믿었으나 실제 0.9.2 구현과 다름) 검색을 백그라운드 스레드로 돌렸다가 검색 버튼을 누르는 즉시 세그폴트가 났음 — 재현·원인 규명 전체 기록은 §7 "PDFium 스레드 안전성" 참고. **최종 설계**: 스레드를 아예 안 쓰고 `pdf_engine::search::IncrementalSearch::step(document, batch_size)`를 매 프레임(`app.rs`의 `poll_search_job`) 호출해 한 번에 페이지 몇 장(`PAGES_PER_FRAME = 8`)만 진행시킨다 — 358페이지 문서 기준 약 45프레임에 걸쳐 끝남. 진행 중엔 `ctx.request_repaint()` 필요(egui 즉시모드 특성상 안 그러면 다음 배치가 사용자 조작 전까지 안 이어짐).

**결과 탐색**: 검색이 끝나면 현재 페이지와 같거나 그 이후에 있는 첫 결과부터 보여주고(훑어보던 위치에서 "다음"을 찾는 통상적인 찾기 동작), 없으면 문서의 첫 결과로 순환. ◀/▶(및 포커스가 옮겨간 뒤의 Enter)는 `search_current_index`를 순환 이동시키고 `go_to_page()`를 호출하므로 페이지 이동 히스토리(Cmd+[/Cmd+])에도 자연스럽게 쌓인다(의도한 동작 — 링크/북마크 점프와 같은 취급).

**하이라이트**: `viewer_panel.rs`의 `draw_search_highlight`는 현재 페이지에 있는 검색 결과를 **전부** 그린다 — 지금 순회 중인 항목은 주황색, 같은 페이지의 나머지 항목은 노란색으로 옅게 표시해 구별(브라우저 찾기 기능의 통상적인 관례). 처음엔 현재 항목만 그렸는데 사용자 요청으로 전체 표시로 바꿈.

**검증**: headless로 `SQ-main.pdf`에 "영화" 검색해 2116개 일치 확인(페이지 순서, rect 정상값) + `IncrementalSearch` 청크 결과가 `search_document`(한 번에 전체 스캔) 결과와 정확히 일치함을 별도 검증(둘 다 임시 예제 작성 후 삭제). **사용자 실기로 전체 확인 완료**(검색 실행/크래시 미재발/검색창 포커스·Enter 동작/하이라이트 색 구분 전부, 2026-07-13).

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

### 크래시 복구 자동저장 (2026-07-13 신규, `crates/ui/src/autosave.rs`)

**배경**: "저장" 버튼을 누르기 전까지 북마크 편집은 메모리(`app.bookmarks`)에만 있어, 비정상 종료(크래시/강제종료/시스템 재부팅)가 나면 그대로 유실된다. SQLite 도입도 검토했으나, 데이터 모델이 "현재 열린 문서 하나에 대한 트리 스냅샷" 하나뿐이라 관계형 쿼리가 필요 없다고 판단해 기각(사용자와 논의 후 결정) — 대신 훨씬 가벼운 JSON 자동저장으로 충분.

**설계 원칙**:
- **파일 하나, 항상 덮어씀**: `eframe::storage_dir("PDF Outliner")`(eframe 자체 persistence와 동일 디렉터리, 별도 의존성 없이 재사용) 아래 고정 경로 `autosave.json` 하나만 쓴다 — 문서/세션마다 새 파일을 만들지 않으므로 시간이 지나도 쌓이지 않는다.
- **`clean_exit` 플래그로 크래시 감지**: 저장 안 된 편집이 있을 때만 주기적으로 `clean_exit: false`로 갱신하고, 정상 종료 시엔(저장 여부·경로 무관) 항상 `true`로 남긴다. 다음 실행 때 `false`가 남아있으면 그 사이 정상 종료 경로(`save()`/`on_exit()`)가 실행되지 못했다는 뜻 — 즉 비정상 종료.
  - **핵심 함정**: 플래그를 "저장 성공"에만 반응해 끄면 안 됨. 사용자가 "저장 안 함"을 선택하고 의도적으로 끈 경우까지 다음 실행에 크래시로 오인해 복구 프롬프트가 뜨면 안 되므로, `on_exit()`은 실제 dirty 상태와 무관하게 항상 `dirty: false`를 넘겨 기록한다.
- **주기**: eframe `App::auto_save_interval()`을 오버라이드해 60초(사용자 요청) — 기본값 30초.
- **흐름**: `PdfViewerApp::new()`가 이번 세션이 파일을 건드리기 전에 `check_for_crash_recovery()`로 먼저 읽어 `pending_recovery`에 보관 → `app.rs`의 `show_crash_recovery_dialog`가 "복구"/"무시" 확인창을 띄움 → 복구 선택 시 해당 문서를 열고 자동저장에 남아있던 북마크 트리로 덮어쓴 뒤 dirty로 표시(아직 PDF엔 저장 안 됐으므로 "저장"을 눌러야 확정) / 무시 선택 시 즉시 `clean_exit: true`로 재기록해 다음 실행에 다시 묻지 않게 함.

**테스트** (`crates/ui/src/autosave.rs`의 `#[cfg(test)]`, `cargo test -p ui`): 경로를 인자로 받는 `record_at`/`check_for_crash_recovery_at`로 분리해 `tempfile` 임시 디렉터리에서 검증 — 실제 `eframe::storage_dir()` 경로(이 개발 머신의 진짜 Application Support 폴더)를 자동화 테스트가 건드리면 실사용 중인 진짜 크래시 복구 데이터를 덮어쓸 위험이 있어(§7 "테스트 설계 원칙"과 같은 이유) 반드시 격리했다.

**미확인**: 실제 강제종료(kill -9 등)로 크래시를 재현해 다음 실행에 복구 프롬프트가 정확히 뜨는지는 이 세션(GUI 조작 불가) 환경에서 검증 불가 — 사용자 실기 필요.

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

## 5. 배포 (비전문가 대상)

**2026-07-13: 서명/공증 없는 배포 파이프라인 구축 완료(사용자가 유료 Developer ID/코드서명 없이 진행하기로 결정 — 우클릭→열기로 우회 가능함을 확인 후).**

### macOS
- `.app` 번들: 직접 작성한 `scripts/package-macos.sh`가 `cargo build --release --target <triple>`로 빌드 후 수동으로 `Contents/{MacOS,Frameworks}` 구조 + `Info.plist` 생성(cargo-bundle/cargo-packager 미사용 — 의존성 추가 없이 셸 스크립트로 충분).
- **ad-hoc 코드서명 적용**(`codesign --sign -`, 무료, 계정 불필요) — Apple Silicon은 서명이 전혀 없는 바이너리는 아예 실행이 안 되기 때문에 기술적으로 필수. 유료 Developer ID 서명·notarization은 미적용 → 다른 기기에서 실행 시 Gatekeeper가 "확인되지 않은 개발자" 경고를 띄우며, 사용자는 우클릭→열기(또는 시스템 설정에서 "그래도 열기")로 우회해야 함. App Store/불특정 다수 배포에는 부적합, 소수 배포용.
- `.pkg`화(`pkgbuild`/`productbuild`)는 미착수 — zip 배포로 충분하다고 판단.

### Windows
- exe + `pdfium.dll`을 `scripts/package-windows.ps1`이 zip으로 묶음. `.msi`(`cargo-wix`/`cargo-packager`)는 미착수(우선순위 낮음, zip으로 충분).
- **코드서명 없음** → SmartScreen 경고 뜸(사용자가 감수하기로 함).

### 공통
- PDFium 동적 라이브러리를 패키지 안에 동봉 완료: CI가 `bblanchon/pdfium-binaries`(태그는 `.github/workflows/release.yml`의 `PDFIUM_TAG_ENCODED`로 고정)에서 플랫폼별 바이너리를 받아 각 패키징 스크립트에 전달.
- `crates/ui/src/app.rs`의 `create_engine()`이 실행 파일 기준 상대경로(macOS: `../Frameworks/libpdfium.dylib`, Windows: 같은 디렉토리의 `pdfium.dll`)를 최우선으로 탐색하도록 수정 — 기존 이 머신 전용 Homebrew 하드코딩 경로는 개발 편의용 최후 폴백으로만 남김.
- CI: `.github/workflows/release.yml` — GitHub Actions matrix(`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`), `v*.*.*` 태그 push 또는 수동 실행(`workflow_dispatch`)으로 트리거, 3개 zip을 GitHub Release에 자동 첨부.
- 로컬 검증: 이 머신(arm64 mac)에서 `scripts/package-macos.sh aarch64-apple-darwin <pdfium dylib>`로 만든 `.app`을 실제로 실행해 358페이지 샘플 PDF 렌더링 + 북마크 사이드바까지 정상 동작 확인함(2026-07-13). x86_64 mac/Windows 빌드는 이 머신에서 직접 실행 검증 불가 — CI 실행 후 사용자가 실기 확인 필요.
- **앱 아이콘 완료(2026-07-14)**: `assets/icon/icon.svg`(핑크 그라데이션 배경 + 흰 문서 + 책갈피 리본, Inkscape로 1024×1024 PNG 래스터화 후 `sips`+`iconutil`로 `.icns`, Pillow로 `.ico` 생성) → macOS는 `scripts/package-macos.sh`가 `.icns`를 번들에 넣고 `Info.plist`의 `CFBundleIconFile`로 연결, Windows는 `crates/ui/build.rs`(`winres` 크레이트, `[target.'cfg(windows)'.build-dependencies]`)가 컴파일 시 exe에 `.ico`를 리소스로 심음. macOS는 Finder에서 실기 확인됨 — Windows는 실물 PC 확보 전이라 exe 아이콘 표시 여부 미확인.

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
- 사이드바 선택 항목에서 Enter로 페이지 뷰어 표시(§3)
- 사이드바 정렬(자식 있는 항목이 없는 항목보다 더 들여써지던 문제) — 근본 원인은 egui `add_sized`가 요청 크기가 아니라 실제 렌더 크기만큼만 부모 커서를 전진시키는 것이었음(§7 "egui add_sized" 항목)
- PDF 문서 내 링크 클릭 시 내부 페이지 이동/외부 URI 열기(§3) — 실제 링크 3641개 보유한 샘플(`SQ-main.pdf`)로 headless 검증 + **사용자 실기로 확인 완료**
- Cmd+[/Cmd+] 페이지 이동 히스토리 뒤로/앞으로가기(§3) — **사용자 실기로 확인 완료**
- 문서 전체 텍스트 검색(§3 "문서 전체 텍스트 검색" 참고, `crates/pdf_engine/src/search.rs`) — 2026-07-13 신규. 처음엔 백그라운드 스레드 버전이 세그폴트를 냈다가(§7) 메인 스레드 청크 방식으로 재구현, 이후 검색창 포커스/Enter 충돌과 하이라이트 색 구분도 리포트받아 수정 — **전부 사용자 실기로 확인 완료**

### 완료 (코드 구현 + headless/빌드 검증까지, 실기 미확인)
- F2 단축키
- 크래시 복구 자동저장(§4 "크래시 복구 자동저장" 참고) — 2026-07-13 신규, 단위 테스트로 로직은 검증했지만 실제 강제종료 재현 + 복구 프롬프트 UI는 실기 필요
- 저장 안 된 북마크 변경사항이 있을 때 앱 종료(창 닫기/Cmd+Q) 시도 시 저장/저장안함/취소 확인창(`crates/ui/src/app.rs`의 `handle_close_request`/`show_quit_confirmation_dialog`) — 2026-07-13 신규. `ctx.input(|i| i.viewport().close_requested())`를 보고 `ViewportCommand::CancelClose`로 일단 종료를 취소한 뒤, 사용자가 확인창에서 저장/저장안함을 고르면 `ViewportCommand::Close`를 다시 보내 실제로 닫는 방식(eframe `on_exit` 문서에 명시된 관례) — 새 문서를 열 때의 `show_unsaved_changes_dialog`와 동일한 문구/구성. 구현 직후 바로 검색 기능 요청으로 넘어가 사용자가 실기로 짚어볼 기회가 없었을 뿐 — 다음 세션에서 먼저 확인 권장

### 남은 작업 (우선순위 낮음, 전부 미착수)
- [ ] SQLite 스키마/마이그레이션 — 북마크 저장은 이제 PDF 자체 outline이 1차 수단이라 우선순위 낮음(CSV/Excel처럼 보조·백업 용도로만 필요할 수도). 크래시 복구 용도로도 검토했으나 데이터 모델이 "열린 문서 하나짜리 트리 스냅샷"뿐이라 관계형 쿼리가 필요 없어 기각 — JSON 자동저장으로 대체(§4)
- [ ] 유료 Apple Developer ID 서명 + notarization, Windows 코드서명 인증서 (§5 — 사용자가 현재는 의도적으로 미적용 결정)
- [ ] macOS `.pkg`화, Windows `.msi`화 (§5 — 현재는 zip 배포로 충분하다고 판단)
- [ ] 진짜로 시각적으로 기울어진 스캔 텍스트 샘플로 하이라이트 정확도 검증(§3)
- [ ] Excel 행 수동 재배열 후 import 시 계층 깨짐 방지 UX (§4)
- [x] GitHub 저장소 연결 + 릴리스 — 완료(2026-07-13~14): [github.com/bjorkii/PDF-Outliner](https://github.com/bjorkii/PDF-Outliner) (private), v0.1.0~v0.1.2 릴리스됨. 배포본 실기 확인: Apple Silicon Mac ✅, Windows ✅(아래 콘솔 창 이슈 제외), Intel Mac 확인 대기 중
- [x] **고배율 줌 크래시 — 해결, 사용자 실기 확인 완료(2026-07-14)**. 원인: 렌더 폭 = 패널폭×줌×픽셀배율(`viewer_panel.rs`)인데 Retina(×2)에서 고배율이면 세로형 페이지 텍스처 높이가 GPU 최대 텍스처 한도를 넘어 wgpu validation 패닉 → release 프로필 `panic="abort"`라 SIGABRT(크래시 리포트의 `abort() called`가 이것). Windows에서 안 났던 건 비Retina(×1)라 절반 크기였기 때문. **수정 3중**: (1) `main.rs`의 `wgpu_options.device_descriptor` — egui-wgpu 기본값이 `max_texture_dimension_2d: 8192`로 하드코딩돼 있어(이 때문에 1차 수정 후 줌이 312%에서 멈췄음) GPU 실제 한도(Apple Silicon Metal 16384)로 디바이스를 열도록 오버라이드. (2) `viewer_panel.rs` — 페이지 종횡비(`app.page_aspect`)로 텍스처 한도에 걸리는 줌을 역산해 `viewport.zoom` 자체를 상한에서 멈춤(% 표시도 함께 멈춤 — 한도 초과분을 흐릿하게 스케일업하는 방안은 사용자가 기각). (3) `app.rs`의 `clamped_render_width`(단위 테스트 5개) — 만에 하나 줌 상한 계산을 우회해도 렌더 해상도가 한도를 못 넘게 하는 안전망. 결과: Retina A4 기준 ~625%에서 선명한 채로 상한, 크래시 없음. Windows 비Retina는 800% 전 구간 영향 없음. **실전 검증 사례**: `pdf-samples/DTFA00006.pdf`의 2페이지는 216×3663pt(종횡비 ~1:17)의 극단적으로 좁고 긴 스캔 페이지라, 수정 전(v0.1.2)에는 100% 줌에서도 페이지 이동 순간 렌더 높이가 한도를 넘어 macOS/Windows 양쪽에서 크래시했음(사용자 리포트) — 수정 후 재현 안 됨(사용자 확인, 2026-07-14). 고배율 줌뿐 아니라 이런 비정상 종횡비 문서도 같은 방어선이 막아준다. **줌 % 상한은 고정값이 아님에 주의**: %는 "패널 폭 대비 배율"이라 창이 좁거나 사이드바가 넓으면 같은 800%라도 텍스처가 작아 상한에 안 걸린다(실측: 기본 창에서 DTFA 세로형 페이지는 647%에서 멈추지만 패널 ~734pt 이하면 800% 전체가 합법 — 사용자가 릴리스 앱에서 800% 도달을 보고 의문 제기 → 디버그 로그로 창 배치 차이임을 확인, 2026-07-14). 불변식은 "텍스처 ≤ GPU 한도" 하나다. 유력 가설: 줌 배율만큼 페이지를 크게 래스터화해 텍스처로 올리는 구조라서(`MAX_ZOOM = 8.0`, `crates/ui/src/app.rs`의 `ViewportState`) 고배율에서 텍스처 크기가 wgpu 디바이스의 최대 텍스처 한도(보통 8192 또는 16384px)를 넘어 validation error로 패닉하는 것일 수 있음. 다음 세션에서 터미널로 실행해 크래시 메시지부터 채집할 것 — 수정 방향은 (a) 렌더 해상도를 한도 이하로 클램프하고 확대는 GPU 스케일링에 맡기기, (b) 타일 렌더링.
- [x] Windows 배포본 실행 시 빈 콘솔 창이 같이 뜸(v0.1.2에서 사용자 확인) — `crates/ui/src/main.rs` 맨 위 `#![cfg_attr(windows, windows_subsystem = "windows")]`로 수정, v0.1.3에 포함, **Windows 실기로 콘솔 창 사라짐 확인 완료(2026-07-14)**. 대가: Windows에서 println/env_logger 콘솔 출력이 안 보임(최종 사용자 무관, 디버깅 시 유의).
- [ ] 다른 실리콘 맥에 릴리스 앱 복사 시 아이콘이 기본 이미지로 보이는 현상 — 번들 자체는 정상(.icns 동봉, 원 기기에서 Finder/Dock 표시 확인됨)이라 대상 기기의 Finder/LaunchServices 캐시로 추정. 사용자가 추후 확인 예정(앱 실행 후 Dock 아이콘 확인 → 폴더 이동/killall Finder 순).

### 사이드바/북마크 UX 요청 (2026-07-14 접수, 순서대로 처리 예정)
- [x] **사이드바 하단 여백 + Cmd+B 시 새 항목이 안 보이는 문제 — 완료, 사용자 실기 확인(2026-07-14)**. `sidebar.rs`의 `ScrollArea` 안, 트리 렌더링 뒤에 `ui.add_space(48.0)`로 하단 여백 추가. `focus_editing`이 세워지는 시점(편집 TextEdit을 그리는 곳)에 `edit_response.scroll_to_me(Some(egui::Align::Center))` 추가해 새 항목/편집 시작 위치로 자동 스크롤.
- [x] **새 북마크 삽입 위치를 페이지 번호 순으로 + 레벨 결정 로직 — 완료, 사용자 실기 확인(2026-07-14)**. 두 요구사항이 사실상 하나의 알고리즘으로 풀림: `crates/bookmark/src/tree.rs`에 `insert_node_by_page` 신설(기존 `insert_node`는 그대로 유지, 호출부만 교체). `parent_id=Some`(선택된 노드 있음)이면 그 노드의 `children` 안에서 페이지 순서 위치에 삽입(기존 "자식으로" 관례 유지). `parent_id=None`(선택 없음)이면 트리 전체를 depth-first로 훑어 "페이지가 이하인 마지막 노드"(anchor)를 찾고, **anchor의 자식이 아니라 anchor와 같은 레벨(형제)**로 anchor 바로 뒤에 삽입 — 이 한 번의 탐색으로 "선택 없으면 직전 북마크와 같은 레벨" 요구사항과 "A(34쪽)/B(37쪽) 사이에 35쪽 삽입" 예시가 동시에 충족됨. anchor가 없으면(모든 기존 북마크보다 페이지가 앞섬) 최상위 맨 앞. 단위 테스트 8개 추가(`crates/bookmark/src/tree.rs` 테스트 모듈, 사용자 예시 그대로 하나 포함). `add_bookmark_under_selection`(`crates/ui/src/app.rs`)이 `insert_node`→`insert_node_by_page`로 교체됨. **동일 페이지 내 순서**: 페이지 내 위치(y좌표 등) 정보가 데이터에 아예 없어(pdfium outline도 마찬가지) `insert_ordered`가 "같은 페이지면 기존 항목들 뒤에 추가"로 처리 — 사용자가 이 정도로 충분하다고 확정(2026-07-14), 더 정교한 지면 내 위치 반영은 범위 밖으로 확정.
- [ ] **새 북마크 placeholder 텍스트 자동 선택**: 지금은 `drag_state.editing = Some((new_id, "새 북마크".to_string()))`(sidebar.rs:198)로 편집 모드는 진입하고 포커스도 옮기지만(`request_focus`, sidebar.rs:258) 텍스트가 선택 상태는 아니라서, 사용자가 바로 타이핑해도 "새 북마크"에 이어붙거나 수동으로 전체 선택해야 함. `egui::TextEdit`에 `CCursorRange`로 전체 선택 상태를 얹어 포커스 이동과 같은 프레임에 적용해야 함(egui-winit/egui TextEdit state API 확인 필요).
- [ ] **한글 IME 신규 버그 2건(편집 모드)**: (a) 한글 입력 후 스페이스바 1번 눌렀는데 공백이 2칸 들어감. (b) 한글 조합 중 다음 글자로 기호/숫자(`(`, `)` 등)를 치면 첫 입력이 화면에 안 뜨고 두 번째 입력부터 보임 — 간헐적(재현조건 미확정, 추가 조사 필요). 둘 다 §7 "한글 IME"에서 고친 winit 포크(`bjorkii/winit`)의 조합 처리 경로와 관련 가능성 있음 — 그 두 수정(discardMarkedText, CGEvent 웜업) 이후 새로 나타난 것인지, 원래 있었는데 이제 눈에 띈 것인지부터 확인.
- [ ] **뷰어에 현재 보이는 페이지의 북마크를 사이드바에서 볼드체로 표시**: `sidebar.rs`의 트리 렌더링(약 284행 `egui::Label::new(node.title.clone())`)에서 `node.page == app.current_page`일 때 `.strong()` 또는 폰트 굵기 조정. 페이지 이동은 되는데 해당 북마크가 없는 페이지(북마크 사이 페이지)일 때 어떻게 표시할지(가장 가까운 이전 북마크를 볼드? 아무것도 안 볼드?) 결정 필요.

---

## 7. 값진 기술적 교훈 (다음 세션이 같은 삽질을 반복하지 않도록)

### pdfium-render 0.9.2 API가 스펙 작성 당시 가정과 달랐던 것들
- `chars_range()` 메서드 없음 → `chars()` 전체 컬렉션에서 인덱스로 `get()` 순회하는 방식으로 대체
- `PdfPageTextChar::matrix()` 없음 → `angle_radians()`(`FPDFText_GetCharAngle` 래핑)가 회전각을 직접 반환
- `get_char_near_point()`는 `Result`가 아니라 `Option<PdfPageTextChar>` 반환
- **`angle_radians()`가 실제 화면상 시각적 회전과 다를 수 있음**: 실제 PDF 2건(포스터/디자인 타이틀류)에서 큰 회전값(90°, 6.2rad)이 나왔는데, `render_crop`으로 그 문자를 직접 렌더링해 PNG로 크롭해 육안 확인하니 글리프는 완전히 똑바로 그려져 있었음 — 폰트가 내부적으로 상쇄하는 "쓰기방향 배치 행렬" 회전일 뿐, 실제 렌더링 결과와 무관할 수 있음. 하이라이트 quad 회전 로직을 걷어내고 axis-aligned로 바꾼 근거(§3).
- **pdfium은 outline(북마크) 쓰기 API가 아예 없음**(PDFium C 라이브러리 자체 한계, pdfium-render 탓 아님) — 대안으로 pdf_oxide를 조사했으나 실제 소스 확인 결과 기존 문서 편집용(`DocumentEditor`)에는 outline 코드가 전혀 없고, outline 빌더(`OutlineBuilder`)는 전혀 새 PDF를 만들 때만 쓰는 것이었음(docstring에 "for PDF generation"이라 명시) — 착각하기 쉬운 함정이니 주의. lopdf로 대체(§4).
- `Pdfium` 바인딩은 **프로세스당 딱 한 번만** 초기화 가능 — 자동화 테스트가 각자 `PdfEngine::new_with_library_path`를 새로 부르면 `cargo test` 기본 병렬 실행(테스트마다 별도 스레드) 시 여러 스레드가 동시에 초기화하려다 진짜로 크래시남(SIGTRAP, 힙 손상 감지). `OnceLock`으로 엔진 하나만 공유해서 여러 테스트가 재사용하게 할 것.
  - ⚠️ **2026-07-13 정정**: 이 항목을 처음 쓸 때 "실제 앱은 시작 시 메인 스레드에서 딱 한 번만 만드므로 이 문제 자체가 없음"이라고 결론 냈었는데, 이건 "초기화만 한 번이면 그 이후엔 여러 스레드에서 pdfium을 불러도 된다"는 뜻이 **아니었다** — 실제로는 초기화 이후에도 **PDFium 호출 자체를 여러 스레드에서 동시에 하면 안 됨**(아래 "PDFium 스레드 안전성" 항목 참고). 문서 전체 검색을 백그라운드 스레드로 돌렸다가 이 착각 때문에 실제 세그폴트가 남.
- **링크(`PdfLink`)는 action(`/A`)과 destination(`/Dest`)이 별개 API**(`PdfLink::action()` vs `PdfLink::destination()`) — 단순 GoTo 링크가 action 없이 destination만 갖는 실제 사례가 있어 action이 `None`이면 destination으로 폴백해야 함(`crates/pdf_engine/src/links.rs`).
- **`FPDFAction_GetURIPath`는 PDF에 저장된 문자열을 스킴 보정 없이 그대로 반환** — 실제 샘플(`SQ-main.pdf`)에서 메일 링크가 `mailto:` 없이, 웹 링크가 `http://` 없이 저장돼 있었음. OS 오프너(`open` 크레이트)에 스킴 없는 문자열을 그대로 넘기면 파일 경로로 오인해 실패하므로 호출 전에 정규화 필요(`links.rs`의 `normalize_uri()`).
- **⚠️ PDFium 스레드 안전성 — `thread_safe` feature를 믿고 백그라운드 스레드에서 pdfium을 불렀다가 실제 세그폴트를 냄(2026-07-13, 검색 버튼 클릭 즉시 `zsh: segmentation fault`로 재현).** 경위: 문서 전체 텍스트 검색이 무거워서(§3 참고) `std::thread::spawn`으로 백그라운드에서 돌리고, 메인 스레드가 쓰는 `PdfDocument`와는 별개로 그 스레드가 같은 파일을 독립적으로 다시 열게 했음 — "서로 다른 문서 객체를 서로 다른 스레드가 읽기 전용으로만 쓰는 건 안전하다"고 판단한 근거는 pdfium-render의 `thread_safe` feature(README: "achieves thread safety by locking access to Pdfium behind a mutex")였음. **실제 0.9.2 소스를 열어보면 이 주장이 사실이 아니다** — `src/pdfium.rs`의 `PdfiumLibraryBindingsAccessor` trait는 `thread_safe`일 때 `BINDINGS.get().unwrap()` 대신 `BINDINGS.wait()`를 쓰고 `Send + Sync`를 요구/`unsafe impl`하는 게 전부고, 실제 FFI 호출(`bindings/dynamic_bindings.rs`)을 감싸는 `Mutex`는 이 크레이트 어디에도 없음(`grep -rn Mutex src/`로 직접 확인 — `index_cache.rs` 한 곳뿐이고 FFI 디스패치와 무관). 즉 README의 설명과 실제 0.9.2 구현이 다르다(더 최신/다른 버전엔 있을 수도 있음 — 버전 고정 안 하고 문서만 믿지 말 것). **교훈**: PDFium은 정말로 스레드 안전하지 않고(Pdfium 저자들도 "멀티스레딩 대신 병렬 프로세스를 쓰라"고 권장 — README "Multi-threading" 절), pdfium-render의 `thread_safe`는 (적어도 이 버전에서는) 타입 레벨 마커일 뿐 실제 동시 호출을 막아주지 않는다. **결론**: pdfium을 다루는 모든 코드는 프로세스 전체에서 항상 같은 스레드(이 앱에선 UI 메인 스레드)에서만 호출할 것 — 무거운 작업을 매끄럽게 하려면 스레드를 늘리지 말고 `pdf_engine::search::IncrementalSearch`처럼 프레임당 청크로 나눠 메인 스레드에서 여러 프레임에 걸쳐 진행시킬 것.

### egui/eframe/wgpu 0.29.x 관련 (**현재 버전 — 2026-07-13에 0.35로 올렸다가 회귀 발견 후 0.29.1로 롤백함**, 아래 "0.29 → 0.35 업그레이드 시도와 롤백" 절 참고)
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
- 로드된 폰트(egui 기본 + 등록한 fallback)에 없는 유니코드 아이콘 글리프(예: "✕" U+2715)는 빈 사각형(tofu)으로 렌더링되는데, 작은 버튼에 쓰면 **체크박스처럼 보여서 오작동으로 오인되기 쉬움**(실제로 이걸로 "삭제 버튼이 체크박스처럼 눌려서 바로 삭제된다"는 리포트를 받았음) — 아이콘 버튼엔 커버리지가 검증 안 된 심볼 대신 plain ASCII(`+`, `-`, `>`, `v`) 사용 권장. 단, **egui는 기본 폰트에 `emoji-icon-font.ttf`/`NotoEmoji-Regular.ttf`(약 1216개 이모지)를 이미 번들함** — `egui-0.29.1/src/lib.rs`의 doc comment에 지원 목록이 나열돼 있고(🔍 포함), 이 안에 있는 문자는 별도 폰트 설치 없이 안전하게 쓸 수 있다(검색 버튼에 🔍 사용, 2026-07-13). 새 아이콘을 쓰기 전엔 이 목록에 있는지부터 확인할 것 — 없으면 plain ASCII로.
- **`ui.add_sized(size, widget)`는 실제로 그 `size`만큼 부모 레이아웃 커서를 전진시킨다는 보장이 없음** — `add_sized`는 내부적으로 `centered_and_justified` 레이아웃의 자식 `Ui`를 만들어 위젯을 그 안에 그리지만, 최종적으로 부모 커서를 전진시키는 값은 요청한 `size`가 아니라 **그 자식 `Ui`의 `min_rect()`(위젯이 실제로 그려진 뒤 차지한 크기)**다(egui 0.29.1 `ui.rs`의 `allocate_new_ui_dyn`: `let rect = child_ui.min_rect(); self.placer.advance_after_rects(rect, rect, ...)`). 사이드바 접기/펼치기 아이콘 버튼을 `add_sized(vec2(18.0, ..), Button::new(icon).small().frame(false))`로 고정폭을 시도했는데도 "자식 있는 행이 없는 행보다 더 들여써지는" 정렬 어긋남이 계속 있었던 게 바로 이 때문 — 작은 아이콘 하나짜리 `Button`의 실제 렌더 폭이 18.0과 미묘하게 달랐던 것. **해결**: `ui.allocate_exact_size(size, sense)`로 폭을 직접 못박고, 그 반환된 `rect` 안에 `ui.painter().text(...)`로 글리프만 수동으로 그리는 방식으로 바꿔야 두 branch(자식 있음/없음)가 항상 정확히 같은 폭을 차지함(`crates/ui/src/sidebar.rs`의 toggle 렌더링, 2026-07-13).
- **`eframe::App::on_exit`의 시그니처는 `glow` feature 활성화 여부에 따라 다름** — `Cargo.toml`엔 `features = ["wgpu", "persistence"]`만 명시했는데도(glow 안 켰음) 실제로는 워크스페이스 의존성 트리 어딘가에서 `glow` feature가 함께 활성화돼(cargo의 feature unification 특성상 한 워크스페이스 안에서 어느 크레이트든 켜면 전체에 적용됨) `fn on_exit(&mut self, _gl: Option<&glow::Context>)` 시그니처가 요구됨(글로우 없는 변형인 `fn on_exit(&mut self)`가 아니라). 컴파일 에러(E0050, "expected 2 parameters, found 1")로 바로 드러나므로 심각한 문제는 아니지만, 문서만 보고 시그니처를 가정하지 말고 실제 컴파일러 에러 메시지로 확정할 것.
- `eframe::storage_dir(app_id)`는 `pub` 함수로 크레이트 루트에 노출돼 있어(`eframe::storage_dir`), `run_native`에 넘긴 것과 같은 app_id 문자열을 넘기면 eframe이 자체 `.ron` persistence에 쓰는 것과 **정확히 같은 디렉터리**를 얻을 수 있다 — 별도로 `directories`/`dirs` 크레이트를 추가할 필요 없이 재사용 가능(`crates/ui/src/autosave.rs`).
- **egui는 기본적으로 입력 이벤트가 있을 때만 다시 그리는 즉시모드**라, 여러 프레임에 걸쳐 나눠 진행하는 job(예: 문서 전체 검색을 청크로 나눈 것 — §3, §7 "PDFium 스레드 안전성" 참고)이 사용자 입력과 무관하게 다음 배치를 진행해도 그 순간 화면이 자동으로 갱신되지 않는다 — 다음 마우스/키보드 조작이 있을 때까지 진행이 "멈춘 것처럼" 안 보일 수 있음. job이 진행 중인 동안은 그걸 폴링하는 코드에서 매 프레임 `ctx.request_repaint()`를 걸어둬야 다음 프레임에 계속 이어진다(`crates/ui/src/app.rs`의 `poll_search_job`).

### egui/eframe 0.29 → 0.35 업그레이드 시도와 롤백 (2026-07-13)

**⚠️ 결론부터: 시도했다가 회귀만 만들고 0.29.1로 되돌림. 같은 이유로 재시도하지 말 것.**

**동기**: 한글 IME 조합이 필드 전환 직후 첫 글자에서만 깨지는(그 이후엔 정상인) 좁은 범위의 버그(§ "확인 필요 미결 사항" 참고)를 조사하다가, egui 0.34.0 체인지로그에 `Memory`에 `owns_ime_events`가 추가됐고 "Fix backspacing leaving last character in IME prediction not removed on macOS native" 항목이 있는 걸 발견 — 우리가 겪는 증상(필드 전환 시 이전 필드의 마지막 자소가 새 필드로 새어 들어옴)과 정확히 같은 계열의 문제로 보여 사용자 승인 하에 0.29.1 → 0.35.0(2026-07-13 기준 최신)으로 업그레이드함.

**결과 — 명백한 회귀(사용자 실기 리포트)**:
- IME가 고쳐지기는커녕 **더 심하게 깨짐**: 업그레이드 전엔 "세션 중 첫 입력, 그리고 필드 전환 직후"에만 자모가 분리됐는데, 업그레이드 후엔 **모든 글자가 항상** 자모로 분리되고, 북마크/검색창을 오가도 계속 지속됨(즉 "가끔 나는 좁은 버그"에서 "상시 발생하는 심각한 버그"로 악화).
- **사이드바 텍스트 렌더링 화질 저하**: 마치 ClearType을 끈 것처럼, 또는 글자 크기를 잘못 조정했을 때처럼 획 굵기가 일정치 않고 흐릿하게 보임(IME와 무관한 별개의 회귀 — 아마 wgpu/egui-wgpu가 22.1.0→29.0.4로 같이 올라가면서 폰트 텍스처 필터링/래스터화 방식이 바뀐 것으로 추정되나 원인까지 깊게 파진 않음).
- 즉 **6개 마이너 버전을 한 번에 건너뛴 업그레이드가 API 차원에서는 컴파일이 다 통과했음에도 런타임 동작/렌더링 품질에 실제로 큰 회귀를 만들 수 있다**는 걸 실기로 확인함 — `cargo build`/`cargo test` 통과와 짧은 백그라운드 구동 확인만으로는 절대 못 잡아내는 종류의 문제였음(이 세션이 화면을 볼 수 없다는 근본적 한계 때문에 예견은 했지만 회피는 못 했음).

**조치**: 즉시 0.29.1로 롤백(`crates/ui/Cargo.toml`의 `egui`/`eframe` 버전 되돌림 + 그 사이 API 차이 때문에 바꿨던 코드 전부 원복 — `App::update`↔`App::ui`, `SidePanel`/`TopBottomPanel`↔통합 `Panel`, `wants_keyboard_input`↔`egui_wants_keyboard_input`, `close_menu`↔`close`, `rect_stroke` 3인자↔4인자, `FontData` Arc 래핑 등). 이 업그레이드 시도와 별개로 **같은 시점에 진행 중이던 검색 기능 수정(검색창 포커스 관리, Enter 항상 재검색, 결과 다중 하이라이트)은 손대지 않고 그대로 보존**했다 — egui 버전과 무관한 우리 쪽 로직이었기 때문. 롤백 후 `cargo build`/`cargo test` 전부 통과 재확인.

**교훈**: 체인지로그에 정확히 맞아떨어지는 근거(owns_ime_events)가 있어도, 여러 마이너 버전을 한 번에 건너뛰는 GUI 프레임워크 업그레이드는 **이 세션처럼 화면을 볼 수 없는 환경에서 시도하기엔 리스크가 실제로 구현됨** — 다음에 이 IME 버그를 다시 파고들 땐 (a) 사용자가 직접 로컬에서 버전을 올려보고 결과를 알려주는 방식으로 하거나, (b) egui-winit의 IME 관련 소스만 훨씬 좁게 읽어서 우리 코드 레벨에서 뭔가 조정할 여지가 있는지부터 찾는 게 나을 것 — 프레임워크 전체를 통째로 올리는 건 최후의 수단으로 미룰 것.

### 테스트/디버깅 설계 원칙
- headless 예제·테스트로 pdfium·lopdf 로직을 실제 라이브러리로 검증하는 게 화면 캡처 불가 세션에서 유일하게 강한 검증 수단. `render_crop`으로 PNG를 저장해 Read 툴로 육안 확인하는 것도 유효한 시각 검증 방법. GUI 상호작용 버그(호버, 드래그, 키보드 단축키 등) 자체는 headless로 발견 불가능 — 사용자의 실기 리포트에 의존할 수밖에 없음.
- **사용자가 수동 GUI 테스트에 실사용 중인 파일을 자동화 테스트가 같이 건드리면 안 됨.** 실사례: 사용자가 삭제+저장 기능을 실기 테스트하면서 `pdf-samples/embeddedoutline.pdf`(원래 6개 장)의 일부를 실제로 지우고 저장했는데, 그 파일에 하드코딩된 기대값(6개 장)을 갖고 있던 자동화 테스트가 갑자기 실패해서 "회귀 버그"로 오인할 뻔함 — 사실은 저장 기능이 의도대로 정확히 동작한 증거였음(파일 수정시각이 사용자 테스트 시각과 정확히 일치해서 확인함). 자동화 테스트는 항상 pristine 백업을 임시 디렉터리에 복사해서 그 복사본만 쓰도록 격리할 것.
- **재현에 성공했다고 바로 원인이라고 확신하지 말 것.** "pdfium이 문서 2개 동시에 못 견딤"이라는 가설을 세우고 재현 테스트를 작성해 실제로 SIGSEGV/SIGTRAP을 재현하는 데 성공했지만, 나중에 알고보니 그 테스트 자체가 테스트마다 `PdfEngine`을 따로 만들어서 `cargo test`의 기본 병렬 실행 때 여러 스레드가 동시에 pdfium을 초기화하려다 난 크래시였음(테스트 설계 결함) — 진짜 원인(wgpu 텍스처 파괴 타이밍)은 결국 사용자가 터미널에서 직접 실행해 얻은 정확한 panic 메시지로 찾음. 재현 테스트의 전제(엔진 공유 방식 등) 자체도 의심할 것.

---

## 확인 필요 미결 사항

- **개발 환경**: `~/.zshrc`에 `. "$HOME/.cargo/env"` 추가 완료(2026-07-12) — 새 터미널 세션이면 `rustc`/`cargo` 바로 사용 가능.
- **한글 IME 자소 유출(필드 전환) + 세션 첫 글자 자소 분리 — 둘 다 winit 포크 패치로 해결, 사용자 실기 확인 완료(2026-07-14)**: 아래 2026-07-13 항목들에서 "앱 코드로는 불가"로 결론냈던 문제 중 **증상 B(필드 전환 시 이전 필드의 마지막 자소가 새 필드로 유출)를 winit 포크 + 앱 레벨 트리거 조합으로 해결**했다. 재조사에서 발견한 결정적 사실: winit 0.30.13의 `set_ime_allowed(false)`는 winit **내부** 조합 버퍼(`marked_text`)만 비우고 **OS 입력기(NSTextInputContext)의 조합 상태는 `discardMarkedText()`로 정리하지 않는다** — IME가 스스로 부르는 `unmarkText`는 OS 쪽까지 정리하는 것과 대조적. v2 워크어라운드가 실패했던 진짜 이유가 바로 이것(토글은 일어났지만 OS 입력기가 자소를 계속 쥐고 있다가 flush). winit 최신 master의 재작업 코드에도 이 문제가 그대로고 메인테이너들이 "it was also broken but let's not break things worse" 주석만 달아둔 상태라 업스트림 수정은 기약 없음([winit#3095](https://github.com/rust-windowing/winit/issues/3095)도 여전히 오픈, 연결 PR 없음).
  - **수정 1 — winit 포크**: [bjorkii/winit](https://github.com/bjorkii/winit)의 `korean-ime-discard-marked-text-0.30.13` 브랜치(v0.30.13 + 1커밋). `set_ime_allowed(false)`에 `inputContext().discardMarkedText()` 호출 추가(10줄). 워크스페이스 `Cargo.toml`의 `[patch.crates-io]`로 적용 — upstream이 고쳐지면 패치 블록만 지우면 됨.
  - **수정 2 — 앱 트리거**(`crates/ui/src/app.rs`의 `guard_ime_across_focus_change`): 텍스트 위젯 A→B 직행 포커스 전환(egui-winit이 IME allowed를 계속 true로 유지해 winit이 정리할 기회가 없는 경우)이 감지되면 같은 프레임 안에서 `ViewportCommand::IMEAllowed(false)`→`(true)`를 연속 전송. 두 명령이 연달아 처리되므로 v2와 달리 "IME 꺼진 프레임"이 없어 경합을 새로 만들지 않고, `IMEAllowed` viewport command는 egui-winit의 debounce(`allow_ime` 캐시)를 우회해 `window.set_ime_allowed`를 직접 부른다(egui-winit 0.29.1 `lib.rs`의 `process_viewport_command` 확인). B가 텍스트 위젯이 아니면 egui-winit이 어차피 allowed를 false로 내리며(포크 패치 덕에 discard 포함) 정리되므로 앱 트리거는 `ime.is_some()`일 때만 발동.
  - **수정 3 — 증상 A(세션 첫 글자 자소 분리)도 같은 날 해결, 사용자 실기 확인 완료(2026-07-14)**: winit 포크에 디버그 로그를 심어 실제 이벤트 순서를 확보한 결과, 첫 키는 IME가 조합(`setMarkedText`)을 아예 시작하지 않고 `insertText`로 날 자소를 삽입하며 **두 번째 키부터** 조합이 시작됨을 확인 — macOS IMKit이 입력기 서버와 이 프로세스 사이의 세션을 **첫 실제 키 이벤트 때에야** 맺는데 그 핸드셰이크가 해당 키 자신에겐 늦기 때문. **해결**: 포크의 `set_ime_allowed(true)` 경로에서 1회성 "웜업" — CGEvent 기반 합성 키 이벤트(`CGEvent::new_keyboard_event` + `NSEvent eventWithCGEvent:`)를 자기 input context의 `handleEvent()`에 직접 먹여 핸드셰이크를 미리 시키고, 그동안 `ime_suppress_events` 플래그로 NSTextInputClient 콜백의 Ime 이벤트 큐잉을 전부 억제한 뒤 `discardMarkedText()`로 찌꺼기 폐기(앱에는 완전히 불가시).
    - **실패했던 실험들(기록)**: (1) `NSTextInputContext.activate()` 명시 호출 — 핸드셰이크를 유발하지 못함, 효과 없음. (2) 순수 합성 `NSEvent`(keyEventWithType…) — IME가 상대하지 않고 날 문자("a")로 통과시킴; **IMKit은 윈도우 서버 기반(CGEvent-backed) 이벤트와 순수 합성 이벤트를 구별한다**. (3) `interpretKeyEvents` 대신 `inputContext.handleEvent()`로 키 라우팅 변경 — 첫 키 증상에 효과 없음, upstream과의 차이 최소화를 위해 원복. (4) objc2는 디버그 빌드에서 셀렉터 타입 인코딩을 검증하므로 `eventWithCGEvent:`에 `*mut c_void`를 넘기면 패닉(`expected '^{__CGEvent=}', found '^v'`) — `RefEncode`로 정확한 인코딩을 선언한 불투명 타입 필요.
    - **한계**: 웜업은 첫 텍스트 필드 포커스 시점의 활성 입력기와 핸드셰이크함 — 사용자가 영문 상태로 포커스한 뒤 세션 중간에 한글로 전환하는 경우의 첫 글자는 이론상 여전히 깨질 수 있음(미검증, 실사용 빈도 낮다고 판단).
- **(과거 기록) 한글 IME 조합 끊김 — 2026-07-13 당시 "업스트림 버그, 자체 수정 불가"로 결론냈던 조사 전문** (증상 B는 위 2026-07-14 항목으로 해결됨, 증상 A는 여전히 유효): 정확한 재현 조건(사용자가 정밀하게 좁혀줌, 0.29.1 기준): 앱을 새로 띄운 뒤 **이번 세션에서 처음으로** 아무 텍스트 필드(북마크든 검색창이든)에 포커스를 주고 입력하면 그 첫 글자가 자모로 완전히 분리됨(예: 검색창에 "송" → "ㅅㅗㅇ"). 그 상태에서 **두 번째로** 다른 필드로 옮겨가 입력하면, 첫 번째 필드에서 입력했던 마지막 자소가 새 필드에 먼저 "flush"되어 나타난 뒤 새로 입력한 글자가 정상 조합됨. 즉 IME composition/commit 이벤트가 지금 포커스를 가진 위젯이 아니라 **이전에 포커스를 가졌던 위젯에 속한 채로 지연 전달**되는 것으로 보이는 패턴. 그 이후의 입력은 정상 — 실사용에 거슬리지만 좁은 범위의 문제.
  - **시도했다가 되돌린 것**: egui 0.34.0 체인지로그의 `Memory::owns_ime_events` 추가 + IME 관련 macOS 수정 항목이 정확히 이 증상과 맞아떨어져 보여 0.29.1 → 0.35.0으로 업그레이드했었음(§7 "egui/eframe 0.29 → 0.35 업그레이드 시도와 롤백" 참고). **결과는 악화**: 모든 글자가 항상 자모 분리되고 필드를 옮겨도 지속되는 데다, 사이드바 텍스트 렌더링까지 흐릿해지는 별개의 회귀까지 발생(사용자 실기 리포트, 2026-07-13). 즉시 0.29.1로 롤백해 원래의(좁은 범위) 증상으로 되돌아옴 — 지금 이 항목에 적힌 재현 조건이 바로 그 "되돌아온 뒤" 기준이다.
  - **근본 원인 확정(2026-07-13, winit 0.30.13 `platform_impl/macos/view.rs` 소스로 직접 확인)**: macOS에서 winit은 창 전체를 `NSView` 하나(`WinitView`)로 다루고, IME 조합 버퍼(`marked_text`)도 그 뷰 하나에 전역으로 저장한다 — egui가 그리는 여러 "텍스트 필드"는 macOS/IME 입장에서는 전부 같은 뷰 하나일 뿐, 필드별로 별도의 조합 세션이 있는 게 아니다. 그 조합 버퍼는 `set_ime_allowed(false)`가 호출될 때만 비워지는데(`view.rs`의 `set_ime_allowed` 함수: `if self.ivars().ime_allowed.get() == ime_allowed { return; }` 다음 값이 `true`로 바뀔 땐 그냥 return, `false`로 바뀔 때만 `marked_text`를 지움), egui-winit은 이 값을 "IME가 필요한 위젯이 하나라도 포커스를 가졌는지"(`ctx.output().ime.is_some()`)만 보고 정한다. 텍스트 위젯 A에서 텍스트 위젯 B로 포커스가 바로 넘어가면 그 값이 계속 `true`로 유지돼 `set_ime_allowed`가 다시 호출되지 않고, 그래서 A에서 조합 중이던 잔여 자모가 지워지지 않은 채 B로 새어 들어간다 — 사용자가 재현한 증상과 정확히 일치, 로그의 TSM/IMK 메시지도 같은 시점의 OS 레벨 흔적으로 보임.
  - **실험적 워크어라운드 v1 — 실패, 원인 규명함(2026-07-13)**: 처음엔 `crates/ui/src/app.rs`의 `guard_ime_focus_transition`이 위젯 A→B 포커스 전환을 감지해 `Memory`에서 B의 포커스 자체를 취소(`surrender_focus`)하고 다음 프레임에 도로 부여(`restore_pending_focus`)하는 방식으로 짰음 — 사용자가 실기로 테스트했으나 **증상이 그대로**였음("첫 글자 자소분리, 두 번째 글자는 정상, 다른 필드로 옮기면 그 두 번째 글자가 먼저 입력됨" — v1 적용 전과 사실상 동일한 패턴). **원인**: 이 함수를 위젯을 전부 그린 *뒤에* 호출하는데, B는 이미 그 프레임 렌더링 도중(자기 자신의 클릭 처리 시점) 포커스를 얻고 `ctx.output_mut(|o| o.ime = Some(..))`까지 스스로 이미 선언해버린 뒤였다 — 그 뒤에 `Memory`의 포커스 기록만 취소해봐야 이미 그 프레임 출력에 박제된 `ime = Some(B)`는 안 바뀐다. 즉 `ime.is_some()`이 실제로는 단 한 프레임도 false가 된 적이 없어 `set_ime_allowed`가 아예 다시 호출되지 않았던 것 — 겉보기엔 그럴듯한 개입이었지만 타이밍이 늦어 완전히 no-op이었음.
  - **실험적 워크어라운드 v2 — 시도했으나 되돌림(2026-07-13)**: 포커스 자체는 건드리지 않고, 대신 위젯 A→B 전환이 감지된 프레임에 `ctx.output_mut(|o| o.ime = None)`으로 그 프레임의 IME 출력값 자체를 직접 덮어쓰는 방식(v1과 달리 타이밍 문제는 없음 — `eframe`이 `App::update`를 `ctx.run()` 클로저 **안에서** 부르므로 `output_mut` 값이 그 프레임의 실제 출력에 반영됨, `eframe::native::epi_integration` 확인). 사용자가 실기 테스트했으나 여전히 필드 전환 시 이전 필드의 문자가 새 필드로 새는 증상이 남아있었고, 오히려 v2 자체가 "포커스 전환 직후 정확히 그 프레임에 IME를 강제로 끔"으로써 사용자가 새 필드에 타이핑을 시작하는 바로 그 타이밍에 IME 비활성 구간을 만들어 **아래 winit #3095와 같은 종류의 경합을 새로 만들었을 가능성**이 있음(사용자가 web 검색으로 찾아준 실제 winit 이슈로 뒷받침됨). 순효과가 불분명하고 부작용 가능성까지 있어 코드 완전히 되돌림 — 필드 A/B 전환, 포커스 관련 필드/메서드 전부 제거, 0.29.1 순정 동작으로 복귀.
  - **결론(2026-07-13): 앱 코드로는 고칠 수 있는 범위 밖 — 확정된 업스트림 winit 버그.** 사용자가 찾아준 [rust-windowing/winit#3095](https://github.com/rust-windowing/winit/issues/3095)가 정확히 이 증상(Neovide 등 winit 기반 앱에서 한글 입력 시 첫 글자만 자소 분리, 이후 정상)을 다루는 실제 오픈 이슈이며 **winit 자체에서도 아직 해결 안 됨**(winit 메인테이너들도 못 고친 상태) — 즉 우리 앱이나 egui-winit 통합 문제가 아니라 winit의 macOS 백엔드 자체 한계로 확인됨. 앱 레벨에서 시도 가능한 두 가지 실질적 지렛대(포커스 자체 조작, IME 출력값 강제 조작)를 둘 다 시도했지만 winit이 실제 키 이벤트와 IME 활성화 사이의 경합을 내부적으로 어떻게 처리하는지에는 개입할 수 있는 공개 API가 없어 근본 해결이 불가능함. 프레임워크를 패치하거나(범위 밖) winit/egui 자체의 향후 수정을 기다리는 것 외에 다른 방법이 없다고 결론 — 더 이상 이 방향으로 자체 시도하지 않기로 함.
- 그 외 이전 논의된 요구사항은 모두 해소됨(북마크 페이지번호 컬럼, OCR 범위 밖 확정, 이미지 전용 페이지 안내 불필요).
