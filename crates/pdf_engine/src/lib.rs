//! PDFium 기반 렌더링/텍스트 엔진 래퍼.
//!
//! PDFium은 런타임 바인딩 방식이다(pdfium-render 문서 참고): 컴파일 타임에 정적 링크하지 않고,
//! 앱 실행 시 `bblanchon/pdfium-binaries`에서 받은 플랫폼별 동적 라이브러리
//! (macOS: libpdfium.dylib, Windows: pdfium.dll)를 앱 번들 안에서 찾아 로드한다.
//! 따라서 이 크레이트 자체의 컴파일에는 실제 pdfium 라이브러리가 필요 없다.

pub mod links;
pub mod outline;
pub mod search;
pub mod selection;
pub mod skew;

use anyhow::{Context, Result};
use pdfium_render::prelude::*;
use std::path::Path;

/// `&'static Pdfium` 하나만 담고 있어 Clone/Copy가 그냥 포인터 복사라 저렴하다.
/// (테스트에서 여러 곳에 나눠 쓰기 편하도록 derive — pdfium 바인딩은 프로세스당 한 번만
/// 초기화 가능하므로, 실제 앱에서도 여러 인스턴스를 새로 만들지 말고 이렇게 복사해 공유할 것.)
#[derive(Clone, Copy)]
pub struct PdfEngine {
    /// egui/eframe의 `App`은 단일 인스턴스로 프로세스 전체 생명주기 동안 유지되지만,
    /// `PdfDocument<'a>`가 `Pdfium`을 빌려오는 구조라 App 구조체 필드로 두 값을 함께
    /// 저장하려면 자기참조(self-referential) 구조체가 된다. `Pdfium` 인스턴스 자체가
    /// 어차피 앱 종료 시까지 살아있어야 하므로, `Box::leak`으로 `'static` 참조를 만들어
    /// `PdfDocument<'static>`를 앱 상태에 그냥 필드로 저장할 수 있게 한다.
    pdfium: &'static Pdfium,
}

impl PdfEngine {
    /// 앱 번들에 동봉된 pdfium 동적 라이브러리 경로를 지정해 초기화한다.
    /// (macOS: Contents/Frameworks, Windows: exe와 같은 디렉토리에 동봉 권장)
    pub fn new_with_library_path(library_path: &Path) -> Result<Self> {
        let bindings = Pdfium::bind_to_library(library_path)
            .or_else(|_| Pdfium::bind_to_system_library())
            .context("pdfium 라이브러리 로드 실패: 앱 번들 내 동봉 여부 확인 필요")?;
        let pdfium: &'static Pdfium = Box::leak(Box::new(Pdfium::new(bindings)));
        Ok(Self { pdfium })
    }

    pub fn open_document(&self, path: &Path) -> Result<PdfDocument<'static>> {
        self.pdfium
            .load_pdf_from_file(path, None)
            .with_context(|| format!("PDF 열기 실패: {:?}", path))
    }
}
