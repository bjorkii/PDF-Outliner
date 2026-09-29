//! egui 기본 폰트(Hack/Ubuntu-Light)에는 한글 글리프가 없어, 툴바/사이드바의 한글 텍스트가
//! 빈 사각형(tofu)으로 표시된다. OS에 이미 설치된 한글 폰트를 찾아 fallback으로 추가한다.
//! (별도 폰트 파일을 앱에 동봉하는 것은 배포 단계에서 결정 — 지금은 OS 제공 폰트 재사용)

use egui::{FontData, FontDefinitions, FontFamily};

const CANDIDATES: &[&str] = &[
    // macOS: AppleSDGothicNeo.ttc는 TrueType Collection이라 egui의 폰트 로더가 파싱하지
    // 못한다(단일 sfnt만 지원) — 반드시 standalone .ttf/.otf만 후보로 둔다.
    "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
    // Windows 10/11 기본 한글 폰트
    "C:\\Windows\\Fonts\\malgun.ttf",
    // 일부 리눅스 배포판의 Noto CJK 설치 경로
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJKkr-Regular.otf",
];

/// 팝업창 안쪽 여백. egui 기본값(6)은 글이 창 테두리에 닿아 답답했다(2026-09-29 요청).
/// 모든 `egui::Window`가 `style.spacing.window_margin`을 쓰므로 여기 한 번만 주면 된다.
///
/// **위쪽만 기본값(6)으로 둔다.** 이 여백은 창 테두리와 내용 사이가 아니라 제목 글자 **위**에도
/// 그대로 붙어서, 20을 주면 제목 띠가 두껍게 부풀어 어색해진다(2026-09-29 리포트). 제목 아래
/// 본문의 위쪽 간격은 egui가 제목줄과 구분선으로 이미 만들어 준다.
const WINDOW_MARGIN: f32 = 20.0;
const WINDOW_MARGIN_TOP: f32 = 6.0;

/// 창 여백처럼 앱 전체에 한 번 정하는 모양새.
///
/// **`style_mut`이 아니라 `all_styles_mut`을 쓴다.** egui 0.29는 밝은 테마와 어두운 테마의
/// `Style`을 따로 들고 있어서(`context.rs:1790`), `style_mut`은 **지금 테마 하나에만** 닿는다.
/// 시작할 때 한 번 주고 마는 값은 두 테마 모두에 넣어야 한다 — 그래서 여백을 줬는데도 창이
/// 여전히 조여 보였다(2026-09-29 리포트).
pub fn install_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.window_margin = egui::Margin {
            left: WINDOW_MARGIN,
            right: WINDOW_MARGIN,
            top: WINDOW_MARGIN_TOP,
            bottom: WINDOW_MARGIN,
        };
        // 창 안의 줄 사이도 조금 벌린다 — 기본값(4)은 문장이 여러 줄 이어질 때 답답하다.
        style.spacing.item_spacing.y = 6.0;
    });
}

pub fn install_korean_font(ctx: &egui::Context) {
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = FontDefinitions::default();
            fonts
                .font_data
                .insert("korean".to_owned(), FontData::from_owned(bytes));

            // 기존 기본 폰트 뒤에 fallback으로 추가: 라틴 문자는 기본 폰트가 그대로 담당하고,
            // 기본 폰트에 없는 한글 글리프만 이 폰트가 보완한다.
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .push("korean".to_owned());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push("korean".to_owned());

            ctx.set_fonts(fonts);
            return;
        }
    }
}
