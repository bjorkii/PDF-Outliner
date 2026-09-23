//! 진단용: glyphless 폰트 프로그램을 파일로 쓴다(fontTools 등 외부 도구로 검사).
//!
//! 사용: cargo run -p pdf_ocr --example glyphless_font -- <출력.ttf>

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).expect("출력 경로");
    std::fs::write(path, pdf_ocr::glyphless::font_program())
}
