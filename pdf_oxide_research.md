# pdf_oxide 조사 - 북마크(Outline) 문서 내장 저장을 위한 대안 라이브러리 검토

> ⚠️ **2026-07-13 정정: 이 문서의 결론(pdf_oxide 추천)은 틀렸음. 아래 원문은 보존하되
> 신뢰하지 말 것 — 실제 채택 판단은 이 정정 박스 기준으로 할 것.**
>
> 실제로 `cargo add`로 lopdf 0.44.0 / pdf_oxide 0.3.73 소스를 받아 직접 grep/read한 결과:
>
> - **pdf_oxide는 기존 PDF의 북마크를 편집하는 기능이 없다.** `src/editor/document_editor.rs`
>   (기존 문서 편집 담당 모듈)에 outline/bookmark 관련 코드가 0건. `pdf_oxide::outline`
>   모듈은 `get_outline()` 읽기 전용 함수 1개뿐. `src/writer/outline_builder.rs`는 자체
>   docstring이 "Document outline builder **for PDF generation**"이라 명시 — 이건 완전히
>   새 PDF를 `DocumentBuilder`로 처음부터 만들 때 쓰는 것이지 기존 PDF 편집 용도가 아님.
>   아래 §3의 "핵심 근거" 코드도 실은 폼 필드 예시이고 outline 편집 API가 아니었음(원문에도
>   "실제 채택 시엔 outline/bookmark 편집 API로 대체" 주석이 있는데, 그 대체할 API 자체가
>   존재하지 않는다는 게 이번에 확인된 사실).
> - **lopdf는 반대로 완성도가 높다.** `src/bookmarks.rs`에 `add_bookmark()`/`build_outline()`이
>   있고 Parent/First/Last/Next/Prev/Count를 포함한 완전한 아웃라인 트리를 만듦. 한글 등
>   비ASCII 제목을 UTF-16BE(BOM)로 인코딩하는 코드가 이미 들어있음. `get_toc()`는 실제 PDF로
>   라운드트립하는 단위테스트(`parse_toc`)로 검증됨. `IncrementalDocument`가 암호화된 PDF의
>   증분저장을 막는 안전장치(`check_incremental_save_supported`)까지 갖춤 — §5의 "손상 위험"
>   우려에 대한 방어가 라이브러리 차원에 이미 있다는 뜻.
> - 아래 §4 비교표의 "lopdf는 outline 지원 미흡, `Outlines/Outline => not supported yet` 주석"
>   근거는 재현 안 됨(GitHub 검색으로 못 찾음) — 결론: **lopdf를 채택할 것.**
>
> 결론: pdfium(렌더링) + lopdf(북마크 편집·저장)로 진행.

---

## 1. 결론 (원문 — 위 정정 박스로 대체됨, 신뢰하지 말 것)

**`pdf_oxide` (yfedoseev/pdf_oxide)를 유력 후보로 추천.** pdfium(렌더링)은 그대로 두고,
북마크 쓰기 경로만 pdf_oxide로 분리하는 하이브리드 구성을 제안.

단, 2025년 11월경 시작된 비교적 젊은 프로젝트이므로 실제 채택 전 라운드트립(열기→북마크
추가→저장→재확인) 테스트를 실제 사용할 법한 PDF 샘플들로 검증 필요.

---

## 2. pdf_oxide 핵심 정보

| 항목 | 내용 |
|---|---|
| 저장소 | github.com/yfedoseev/pdf_oxide |
| 라이선스 | MIT / Apache-2.0 (dual, 택1) — copyleft 없음, 상업 프로젝트에 자유롭게 사용 가능 |
| 언어 | Rust 코어 + 19개 언어 바인딩(Python, Go, JS/TS, C#, Java, Kotlin, C++, Swift 등) |
| 성능 (자체 벤치마크) | 텍스트 추출 기준 0.8ms 평균, PyMuPDF 대비 5배, pypdf 대비 15배 |
| 검증 방식 | veraPDF + Mozilla pdf.js + DARPA SafeDocs 테스트 코퍼스(PDF 3,830개)로 100% pass rate 주장 |
| 플랫폼 | Linux/macOS/Windows(x64+ARM64) 네이티브 빌드가 이미 다른 바인딩(C# 등)에 제공됨 → 우리 타겟(Apple Silicon/Intel Mac, Windows)과 부합 |
| 개발 활성도 | 2026년 4월 기준 활발한 릴리스 진행 중, 실사용 버그 리포트 기반 수정 이력 다수 |

### 관련 기능
- **북마크/outline**: 읽기 + 쓰기 모두 지원 ("Bookmarks/Outline" 기능 명시)
- **폼 필드/주석**: 읽기/쓰기 지원
- **문서 편집**: `pdf_oxide::editor::DocumentEditor` API로 기존 문서를 열어 수정 후 저장
- **PDF 생성**: 별도의 `DocumentBuilder`로 신규 PDF 생성도 가능(우리 용도와는 무관)

---

## 3. 핵심 근거: Incremental Save 지원

```rust
use pdf_oxide::editor::{DocumentEditor, EditableDocument, SaveOptions};

let mut editor = DocumentEditor::open("문서.pdf")?;
// 폼 필드 예시 (실제 채택 시엔 outline/bookmark 편집 API로 대체)
editor.set_form_field_value("employee_name", FormFieldValue::Text("Jane Doe".into()))?;
editor.save_with_options("결과.pdf", SaveOptions::incremental())?;
```

`SaveOptions::incremental()`이 핵심입니다. PDF 스펙이 표준으로 지원하는 "증분 업데이트"
방식으로, **기존 파일의 바이트를 전혀 건드리지 않고 바뀐 객체(북마크 트리 등)만 파일 맨
뒤에 추가**합니다. Adobe Acrobat이 주석/폼을 저장할 때 쓰는 방식과 동일한 메커니즘입니다.

이 방식이 손상 위험을 구조적으로 낮추는 이유:
- lopdf처럼 문서 전체를 파싱 → 내부 객체 모델로 재구성 → 통째로 재직렬화하는 과정이 없음
- 따라서 라이브러리가 완전히 이해하지 못하는 객체 타입(압축 방식, 특수 필터, 손상된 부분 등)이
  문서 안에 있어도, 그 부분은 원본 그대로 남아있고 우리가 건드리지 않은 나머지는 100% 보존됨
- 실패 시나리오가 "전체 파일 손상"이 아니라 "새로 추가한 객체 부분만 문제"로 국한됨

### 한글 관련 실사용 근거
changelog에서 확인된 실제 수정 이력:
> "북마크 제목이 UTF-16BE/LE로 인코딩된 경우 디코딩이 잘못되던 버그 수정 — PDF outline의
> `/Title` 문자열은 UTF-16BE/LE(BOM 포함) 또는 PDFDocEncoding일 수 있는데, 디코딩 경로를
> 정리해 비라틴 문자(한글 등) 북마크 라벨이 올바르게 읽히도록 수정"

한글 북마크 제목 처리를 실제로 신경 쓰고 있다는 신호이며, 우리 프로젝트의 한글 인코딩
민감도와 직접 관련된 부분이라 눈여겨볼 지점입니다.

---

## 4. 대안 비교표

| 옵션 | 라이선스 | 북마크 쓰기 | 손상 리스크 | 비고 |
|---|---|---|---|---|
| **pdf_oxide** (추천) | MIT/Apache-2.0 | O (`DocumentEditor`) | 낮음 (incremental save) | 젊은 프로젝트, 실전 검증 필요 |
| **lopdf** | MIT | 부분 지원 — `Document::add_bookmark()` API 존재 | 저장 시 문서 전체 재직렬화 → 특이 객체(오래된 압축, 암호화, 손상 xref) 만나면 그 부분 깨질 수 있음 | 완전히 기능이 없는 건 아니지만, 공식 merge 예제 코드 자체에 `Outlines/Outline => not supported yet` 주석이 있을 정도로 outline 관련 지원이 미흡한 영역 존재 |
| **MuPDF (`mupdf-rs`)** | **AGPL** (또는 Artifex 상업 라이선스) | O, 가장 정통적인 편집 엔진 | 가장 낮음 — 수십 년 실전 검증 | 애초에 렌더링에 pdfium을 택한 이유가 이 라이선스 문제였음 — 재검토 시 다시 부딪히는 지점 |
| **qpdf** (C++, Rust 바인딩 필요) | Apache-2.0 | 구조적 변환엔 강하지만 outline 편집은 저수준 객체 조작 필요 | 중간 | lopdf와 비슷한 수준의 수고 소요 |

---

## 5. 제안 아키텍처

```
문서 열기
  └─ pdfium (렌더링용으로 이미 열림)
       └─ PdfBookmarks 컬렉션으로 기존 outline 읽기 → 사이드바 로딩
            (pdfium-render가 FPDFBookmark_* 바인딩을 이미 고수준 API로 제공,
             추가 비용 없이 사이드바 초기 로딩 가능)

사용자가 북마크 추가/수정/삭제/드래그 이동
  └─ 메모리상의 bookmark::BookmarkNode 트리만 갱신 (이전 세션에서 구현한
     계층 재구성/드래그 로직 그대로 재사용)

저장 시점 (사용자가 명시적으로 저장하거나 문서 닫을 때)
  └─ pdf_oxide::editor::DocumentEditor::open(원본 경로)
       └─ 메모리상의 북마크 트리 → outline 갱신
            └─ save_with_options(임시_경로, SaveOptions::incremental())
                 └─ pdfium으로 임시 파일 재오픈 검증 (페이지 수/파싱 성공 확인)
                      ├─ 검증 성공 → 원자적 rename으로 원본 교체
                      └─ 검증 실패 → 원본 보존, 사용자에게 알림, SQLite 사이드카는
                                      그대로 유지되므로 북마크 데이터 자체는 안전
```

### 안전장치 (라이브러리 선택과 무관하게 필수)
1. 항상 임시 파일에 먼저 저장 — 원본 직접 덮어쓰기 금지
2. 저장 직후 pdfium으로 그 임시 파일을 재오픈해 파싱 성공 여부 검증
3. 검증 통과 시에만 원자적(atomic rename)으로 원본 교체
4. SQLite 사이드카 DB를 계속 유지 — PDF 파일 쓰기가 실패해도 북마크 데이터 자체는
   보존되어 재시도/재내보내기 가능

---

## 6. 남은 검증 항목 (실제 도입 전 확인 필요)

- [ ] `pdf_oxide`의 outline/bookmark 전용 편집 API 정확한 시그니처 확인 (위 예시는 폼 필드
      기준이며, outline 편집 API는 최신 docs.rs에서 별도 확인 필요)
- [ ] 실제 사용할 법한 복잡한 PDF(양식, 서명, 암호화 등 포함)로 라운드트립 테스트
- [ ] incremental save를 여러 번 반복했을 때 파일 크기 증가 폭 확인(매 저장마다 객체가
      누적되므로, 필요 시 주기적 "완전 재압축(linearize)" 옵션 유무 확인)
- [ ] 크레이트 버전 안정성(API breaking change 빈도) 모니터링 — 아직 0.x 버전대

---

## 7. 참고 레퍼런스

### pdf_oxide 공식 자료
- GitHub 저장소: https://github.com/yfedoseev/pdf_oxide
- README (설치법, 코드 예시 포함): https://github.com/yfedoseev/pdf_oxide/blob/main/README.md
- Releases / changelog (한글 북마크 UTF-16BE/LE 디코딩 수정 등 실제 수정 이력): https://github.com/yfedoseev/pdf_oxide/releases
- 공식 문서 사이트: https://pdf.oxide.fyi/
- docs.rs (Rust API 문서): https://docs.rs/pdf_oxide
- crates.io (Rust 패키지): https://crates.io/crates/pdf_oxide
- PyPI (Python 바인딩, 동일 코어 공유): https://pypi.org/project/pdf-oxide/
- C# 바인딩 README (플랫폼별 네이티브 빌드 배포 현황 확인용): https://github.com/yfedoseev/pdf_oxide/blob/main/csharp/README.md
- pdf_oxide_cli (CLI 패키지 정보, 활성도 참고): https://libraries.io/cargo/pdf_oxide_cli

### 비교 대상 라이브러리
- lopdf GitHub 저장소 (merge 예제의 `Outlines/Outline => not supported yet` 주석 확인): https://github.com/J-F-Liu/lopdf
- lopdf Document 구조체 API 문서 (Bookmark/add_bookmark 관련): https://docs.rs/lopdf/latest/lopdf/struct.Document.html
- lopdf crates.io 페이지: https://crates.io/crates/lopdf
- krilla (PDF 신규 생성 전용, 기존 문서 편집 용도 아님 — 비교를 위해 확인): https://github.com/LaurenzV/krilla
- pdf-rs/pdf (또 다른 read/write 후보, 비교를 위해 확인): https://github.com/pdf-rs/pdf
- printpdf (신규 생성 전용, 비교를 위해 확인): https://lib.rs/crates/printpdf

### 커뮤니티/색인 자료 (활성도·채택 현황 교차 확인용)
- awesome-rust 색인 등재 확인: https://awesome.ecosyste.ms/projects/github.com/yfedoseev/pdf_oxide
- Project Awesome 색인: https://project-awesome.org/r/yfedoseev-pdf_oxide
