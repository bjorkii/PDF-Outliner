//! 문서 내 링크(주석) 히트테스트. 뷰어에서 클릭한 화면 좌표가 PDF 링크 위에 있으면
//! 그 링크가 가리키는 대상(문서 내 다른 페이지, 또는 외부 URI)을 반환한다.

use pdfium_render::prelude::*;

/// 링크를 클릭했을 때 뷰어가 취해야 할 동작.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    /// 문서 내 다른 페이지로 이동한다 (1-based).
    Page(u32),
    /// 외부 URI(웹 링크 등)를 시스템 기본 브라우저로 연다.
    Uri(String),
}

/// 페이지 위의 한 점(PDF 포인트 좌표)에 링크가 있으면 그 대상을 반환한다.
///
/// 링크는 두 가지 방식으로 대상을 지정할 수 있다: `/A`(action) 딕셔너리(URI 열기,
/// 원격/임베디드 문서 GoTo 등, `PdfLink::action()`) 또는 액션 없이 `/Dest`만 직접
/// 가리키는 단순 GoTo(`PdfLink::destination()`). 실제 PDF마다 둘 중 하나만 있을 수
/// 있어 action을 먼저 확인하고 없으면 destination으로 폴백한다.
pub fn link_target_at_point(page: &PdfPage, x: PdfPoints, y: PdfPoints) -> Option<LinkTarget> {
    let link = page.links().link_at_point(x, y)?;

    if let Some(action) = link.action() {
        match action {
            PdfAction::LocalDestination(dest_action) => {
                return dest_action
                    .destination()
                    .ok()
                    .and_then(|dest| dest.page_index().ok())
                    .map(|idx| LinkTarget::Page(idx as u32 + 1));
            }
            PdfAction::Uri(uri_action) => {
                return uri_action.uri().ok().map(|uri| LinkTarget::Uri(normalize_uri(&uri)));
            }
            // 원격/임베디드 문서 GoTo, Launch 등은 아직 지원 범위 밖 — 조용히 무시.
            _ => return None,
        }
    }

    link.destination()
        .and_then(|dest| dest.page_index().ok())
        .map(|idx| LinkTarget::Page(idx as u32 + 1))
}

/// 일부 PDF는 URI 액션에 스킴 없이 값을 저장한다(실사용 샘플에서 메일 주소가
/// `mailto:` 없이, 도메인이 `http://` 없이 저장된 경우를 실제로 확인함) — 스킴이
/// 없으면 시스템 브라우저/OS가 그대로 열 수 없으므로(파일 경로로 오인) 여기서 보정한다.
fn normalize_uri(uri: &str) -> String {
    if uri.contains("://") || uri.starts_with("mailto:") {
        uri.to_string()
    } else if uri.contains('@') && !uri.contains('/') {
        format!("mailto:{uri}")
    } else {
        format!("http://{uri}")
    }
}
