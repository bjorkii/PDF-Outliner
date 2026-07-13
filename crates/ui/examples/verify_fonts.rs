//! 화면 캡처가 불가능한 환경에서, 한글 폰트가 실제로 로드되어 글리프를 보유하는지
//! 헤드리스로 검증한다. `crates/ui/src/fonts.rs`의 후보 경로 중 첫 번째로 존재하는
//! 파일을 그대로 사용해, 실제 앱이 시작 시 밟는 것과 동일한 로직을 검증한다.

use egui::{Context, FontData, FontDefinitions, FontFamily, FontId};

const CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
    "C:\\Windows\\Fonts\\malgun.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJKkr-Regular.otf",
];

fn main() {
    let path = CANDIDATES
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .expect("후보 한글 폰트 경로 중 존재하는 것이 없음");
    println!("[1] 폰트 파일 발견: {}", path);

    let bytes = std::fs::read(path).expect("폰트 파일 읽기 실패");
    println!("    파일 크기: {} bytes", bytes.len());

    let ctx = Context::default();
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("korean".to_owned(), FontData::from_owned(bytes));
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push("korean".to_owned());
    ctx.set_fonts(fonts);
    println!("[2] egui::Context에 폰트 등록 완료");

    let korean_text = "파일 열기 북마크 페이지 삭제 추가";
    let ascii_text = "Open File Bookmark Page";
    let font_id = FontId::proportional(14.0);

    // egui는 첫 프레임(Context::run)이 한 번 돌기 전까지는 폰트 텍스처가 준비되지 않아
    // ctx.fonts(...)를 호출하면 패닉한다 — 실제 앱도 eframe이 내부적으로 이 과정을 거친다.
    let mut has_korean = false;
    let mut has_ascii = false;
    let mut layout_size = egui::Vec2::ZERO;

    let _ = ctx.run(Default::default(), |ctx| {
        has_korean = ctx.fonts(|f| f.has_glyphs(&font_id, korean_text));
        has_ascii = ctx.fonts(|f| f.has_glyphs(&font_id, ascii_text));
        let galley = ctx.fonts(|f| {
            f.layout_no_wrap(korean_text.to_owned(), font_id.clone(), egui::Color32::WHITE)
        });
        layout_size = galley.size();
    });

    println!("[3] 한글 문자열 {:?} → glyph 보유: {}", korean_text, has_korean);
    println!("    영문 문자열 {:?} → glyph 보유: {}", ascii_text, has_ascii);

    assert!(
        has_korean,
        "한글 글리프를 찾지 못함 — 폰트 로드가 실패했거나 해당 폰트에 한글이 없음"
    );
    assert!(has_ascii, "영문 글리프를 찾지 못함(기본 폰트 문제)");

    // 실제 레이아웃까지 돌려서 텍스트에 유효한(0이 아닌) 폭이 있는지도 확인.
    // tofu(대체 glyph)로 빠지면 has_glyphs가 true를 반환하더라도 폭이 비정상일 수 있어,
    // 실제 텍스트 레이아웃 결과도 함께 확인한다.
    println!("[4] 레이아웃 결과 크기: {:.1} x {:.1}", layout_size.x, layout_size.y);
    assert!(
        layout_size.x > 50.0,
        "레이아웃된 한글 텍스트 폭이 비정상적으로 좁음 — 렌더링이 비어있을 가능성"
    );

    println!("\n모든 검증 통과 — 한글 폰트가 정상적으로 로드되고 글리프를 보유함.");
}
