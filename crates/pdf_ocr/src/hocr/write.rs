//! hOCR 출력(설계 문서 5.3). 여러 페이지를 파일 하나에 담는다.
//!
//! - 좌표는 표시 페이지 프레임을 `dpi`로 환산한 픽셀이다. `ocr_page`에 `bbox 0 0 W H`와
//!   `scan_res`를 적어, 가져오기가 같은 프레임으로 되돌릴 수 있게 한다.
//! - `ppageno`는 hOCR 규격대로 **이 파일 안에서** 0부터 센다. 원본의 3~10쪽만 내보내도 2가 아니라
//!   0부터 시작한다. 규격이 말하는 "문서"가 곧 이 파일이고, 무엇보다 그래야 다른 도구에서 탈이
//!   없다 — Tesseract가 쪽마다 뽑은 hOCR은 전부 `ppageno 0`이라 현장에서 이 값은 이미 믿을 수 없고
//!   (`ui::ocr_import::load_hocr` 주석), 도구들은 `ocr_page`의 등장 순서를 쓴다. 게다가 2~9로 적으면
//!   그 값을 색인으로 쓰는 도구에서 8쪽짜리 파일의 범위를 벗어난다. 원본 쪽 번호는 잃지 않도록
//!   `ocr-source-pages` 메타로 따로 적는다(표준 밖 이름이라 다른 도구는 무시한다).
//! - 계층은 `ocr_page → ocr_carea → ocr_par → ocr_line → ocrx_word`. 영역·문단 정보가 없으므로
//!   글자가 있는 페이지마다 carea와 par를 하나씩 둔다. 글자가 없는 페이지도 빈 `ocr_page`로 넣어
//!   페이지 번호 대응을 유지한다.
//! - `baseline`은 Tesseract와 같게 줄 bbox의 왼쪽 아래를 원점으로 한 `기울기 오프셋`(픽셀, y는
//!   아래로)이다. 신뢰도(`x_wconf`)는 원본에 없으므로 넣지 않는다.
//!
//! 페이지 단위로 바로 써 내려가므로 수천 쪽 문서도 전체를 메모리에 모으지 않는다.

use crate::layout::{DRect, Line};
use std::io::{self, Write};

pub struct HocrWriter<W: Write> {
    out: W,
    dpi: f64,
    page_index: usize,
}

impl<W: Write> HocrWriter<W> {
    /// 문서 머리를 쓴다. `system`은 `ocr-system` 메타 값(예: "PDF-Outliner v0.2.2").
    /// `source_pages`는 원본에서 잘라 온 쪽 범위(1부터, 양끝 포함) — 전체를 내보낼 때는 `None`이다.
    pub fn new(
        mut out: W,
        dpi: f64,
        title: &str,
        system: &str,
        source_pages: Option<(usize, usize)>,
    ) -> io::Result<Self> {
        write!(
            out,
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\"\n",
                "    \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd\">\n",
                "<html xmlns=\"http://www.w3.org/1999/xhtml\">\n",
                " <head>\n",
                "  <title>{title}</title>\n",
                "  <meta http-equiv=\"Content-Type\" content=\"text/html;charset=utf-8\"/>\n",
                "  <meta name=\"ocr-system\" content=\"{system}\"/>\n",
                "  <meta name=\"ocr-capabilities\" content=\"ocr_page ocr_carea ocr_par ocr_line ocrx_word\"/>\n",
            ),
            title = escape(title),
            system = escape(system),
        )?;
        if let Some((first, last)) = source_pages {
            writeln!(out, "  <meta name=\"ocr-source-pages\" content=\"{first}-{last}\"/>")?;
        }
        write!(out, " </head>\n <body>\n")?;
        Ok(Self { out, dpi, page_index: 0 })
    }

    /// 페이지 하나. `size_pt`는 표시 프레임 크기(포인트).
    pub fn page(&mut self, size_pt: (f64, f64), lines: &[Line]) -> io::Result<()> {
        let n = self.page_index + 1;
        let scale = self.dpi / 72.0;
        let (w, h) = ((size_pt.0 * scale).round().max(1.0) as i64, (size_pt.1 * scale).round().max(1.0) as i64);
        let bbox = |r: &DRect| {
            let x0 = ((r.x0 * scale).floor() as i64).clamp(0, w);
            let y0 = ((r.y0 * scale).floor() as i64).clamp(0, h);
            let x1 = ((r.x1 * scale).ceil() as i64).clamp(x0, w);
            let y1 = ((r.y1 * scale).ceil() as i64).clamp(y0, h);
            (x0, y0, x1, y1)
        };
        let dpi = self.dpi.round() as i64;
        writeln!(
            self.out,
            "  <div class=\"ocr_page\" id=\"page_{n}\" title=\"bbox 0 0 {w} {h}; ppageno {}; scan_res {dpi} {dpi}\">",
            self.page_index
        )?;
        if !lines.is_empty() {
            let area = lines.iter().skip(1).fold(lines[0].rect, |acc, l| acc.union(&l.rect));
            let (ax0, ay0, ax1, ay1) = bbox(&area);
            writeln!(self.out, "   <div class=\"ocr_carea\" id=\"block_{n}_1\" title=\"bbox {ax0} {ay0} {ax1} {ay1}\">")?;
            writeln!(self.out, "    <p class=\"ocr_par\" id=\"par_{n}_1\" title=\"bbox {ax0} {ay0} {ax1} {ay1}\">")?;
            let mut word_id = 0;
            for (li, line) in lines.iter().enumerate() {
                let (x0, y0, x1, y1) = bbox(&line.rect);
                let mut title = format!("bbox {x0} {y0} {x1} {y1}");
                if line.text_angle != 0 {
                    title.push_str(&format!("; textangle {}", line.text_angle));
                }
                if let Some(baseline) = line.baseline {
                    let offset = ((baseline * scale).round() as i64).clamp(y0, y1) - y1;
                    title.push_str(&format!("; baseline 0 {offset}"));
                }
                title.push_str(&format!("; x_size {}", (line.char_height * scale).round() as i64));
                writeln!(self.out, "     <span class=\"ocr_line\" id=\"line_{n}_{}\" title=\"{title}\">", li + 1)?;
                for word in &line.words {
                    word_id += 1;
                    let (x0, y0, x1, y1) = bbox(&word.rect);
                    writeln!(
                        self.out,
                        "      <span class=\"ocrx_word\" id=\"word_{n}_{word_id}\" title=\"bbox {x0} {y0} {x1} {y1}\">{}</span>",
                        escape(&word.text)
                    )?;
                }
                writeln!(self.out, "     </span>")?;
            }
            writeln!(self.out, "    </p>")?;
            writeln!(self.out, "   </div>")?;
        }
        writeln!(self.out, "  </div>")?;
        self.page_index += 1;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        write!(self.out, " </body>\n</html>\n")?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// XML 이스케이프. XML 1.0에서 쓸 수 없는 제어 문자는 뺀다.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || matches!(c, '\u{FFFE}' | '\u{FFFF}') => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{build_lines, tests::line_chars, LayoutOptions};

    #[test]
    fn writes_pages_lines_words() {
        let mut chars = line_chars("a<b c", 0, true);
        chars.extend(line_chars("d", 1, true));
        let (lines, _) = build_lines(&chars, &LayoutOptions::default());
        let mut writer = HocrWriter::new(Vec::new(), 144.0, "t&t", "PDF-Outliner test", None).unwrap();
        writer.page((100.0, 50.0), &lines).unwrap();
        writer.page((100.0, 50.0), &[]).unwrap();
        let html = String::from_utf8(writer.finish().unwrap()).unwrap();
        assert!(html.contains("<title>t&amp;t</title>"));
        assert!(html.contains("title=\"bbox 0 0 200 100; ppageno 0; scan_res 144 144\""));
        assert!(html.contains("ppageno 1;"));
        assert!(!html.contains("ocr-source-pages"), "전체를 내보낼 때는 범위 메타를 적지 않는다");
        // 첫 줄: x 0~50pt, y 0~12pt → 0~100px, 0~24px, 기준선 10pt → 20px(아래 끝에서 -4)
        assert!(html.contains("title=\"bbox 0 0 100 24; baseline 0 -4; x_size 24\""));
        assert!(html.contains(">a&lt;b</span>"));
        assert!(html.contains("id=\"word_1_3\" title=\"bbox 0 40 20 64\">d</span>"));
        assert_eq!(html.matches("class=\"ocr_page\"").count(), 2);
        assert_eq!(html.matches("class=\"ocr_carea\"").count(), 1);
    }

    /// 일부만 내보내도 `ppageno`는 0부터 다시 매기고(다른 도구가 색인으로 써도 범위를 벗어나지
    /// 않게), 원본 쪽 번호는 `ocr-source-pages` 메타로 남긴다(모듈 문서).
    #[test]
    fn a_partial_export_restarts_ppageno_and_records_the_source_range() {
        let mut writer = HocrWriter::new(Vec::new(), 144.0, "doc", "test", Some((3, 10))).unwrap();
        writer.page((100.0, 50.0), &[]).unwrap();
        writer.page((100.0, 50.0), &[]).unwrap();
        let html = String::from_utf8(writer.finish().unwrap()).unwrap();

        assert!(html.contains("<meta name=\"ocr-source-pages\" content=\"3-10\"/>"), "{html}");
        assert!(html.contains("ppageno 0;"), "첫 쪽은 2가 아니라 0이어야 한다: {html}");
        assert!(html.contains("ppageno 1;"));
        assert!(!html.contains("ppageno 2;"), "원본 쪽 번호를 ppageno에 넣으면 안 된다");
    }
}
