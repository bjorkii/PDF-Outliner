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

/// 앱 전체에 한 번 정하는 모양새.
///
/// **창 여백(`window_margin`)은 건드리지 않는다.** egui는 그 값으로 제목 띠 높이까지 계산해서
/// (`window.rs:470`), 키우면 제목 띠가 부풀고 위쪽만 줄이면 제목 글자가 띠 위에 붙는다
/// (2026-09-29, 두 번 다 화면에서 확인). 본문 여백은 창마다 `app::window_body`로 준다.
pub fn install_style(ctx: &egui::Context) {
    // egui 0.29는 밝은 테마와 어두운 테마의 `Style`을 따로 들고 있어서(`context.rs:1790`)
    // `style_mut`은 지금 테마 하나에만 닿는다. 시작할 때 한 번 주는 값은 둘 다 채운다.
    ctx.all_styles_mut(|style| {
        // 창 안의 줄 사이를 조금 벌린다 — 기본값(4)은 문장이 여러 줄 이어질 때 답답하다.
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
