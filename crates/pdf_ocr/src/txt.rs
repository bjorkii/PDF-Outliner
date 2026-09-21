//! txt 출력(설계 문서 5.4).
//!
//! 각 페이지 앞에 `=== [p. 12] ===` 줄을 넣는다. 페이지 레이블이 있고 물리 번호와 다르면
//! `=== [p. 12 | xii] ===`로 함께 적는다. 텍스트가 없는 페이지도 표기 줄은 쓴다. 페이지 사이는 빈
//! 줄 하나로 나누고, 옵션을 켜면 두 번째 페이지부터 표기 줄 앞에 폼피드(`\f`, pdftotext 호환)를
//! 넣는다. 줄바꿈 기본값은 LF다(Windows 10 1809 이후 메모장도 LF를 정상 표시).
//!
//! 좌표 정보가 없으므로 이 형식은 가져오기 대상이 될 수 없다.

use crate::layout::Line;
use std::io::{self, Write};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxtOptions {
    pub crlf: bool,
    pub form_feed: bool,
    pub page_labels: bool,
}

impl Default for TxtOptions {
    fn default() -> Self {
        Self { crlf: false, form_feed: false, page_labels: true }
    }
}

pub struct TxtWriter<W: Write> {
    out: W,
    options: TxtOptions,
    pages_written: usize,
}

impl<W: Write> TxtWriter<W> {
    pub fn new(out: W, options: TxtOptions) -> Self {
        Self { out, options, pages_written: 0 }
    }

    /// `number`는 1부터 시작하는 물리 페이지 번호.
    pub fn page(&mut self, number: usize, label: Option<&str>, lines: &[Line]) -> io::Result<()> {
        let eol = if self.options.crlf { "\r\n" } else { "\n" };
        if self.pages_written > 0 {
            self.out.write_all(eol.as_bytes())?;
            if self.options.form_feed {
                self.out.write_all(b"\x0c")?;
            }
        }
        let label: Option<String> = label
            .filter(|_| self.options.page_labels)
            .map(|l| l.trim().nfc().collect::<String>())
            .filter(|l| !l.is_empty() && *l != number.to_string());
        match label {
            Some(label) => write!(self.out, "=== [p. {number} | {label}] ==={eol}")?,
            None => write!(self.out, "=== [p. {number}] ==={eol}")?,
        }
        for line in lines {
            write!(self.out, "{}{eol}", line.text())?;
        }
        self.pages_written += 1;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{build_lines, tests::line_chars, LayoutOptions};

    fn lines(text: &str) -> Vec<Line> {
        build_lines(&line_chars(text, 0, true), &LayoutOptions::default()).0
    }

    #[test]
    fn page_headers_labels_and_separators() {
        let mut w = TxtWriter::new(Vec::new(), TxtOptions::default());
        w.page(1, Some("1"), &lines("ab cd")).unwrap();
        w.page(2, Some("xii"), &[]).unwrap();
        w.page(3, None, &lines("e")).unwrap();
        let text = String::from_utf8(w.finish().unwrap()).unwrap();
        assert_eq!(text, "=== [p. 1] ===\nab cd\n\n=== [p. 2 | xii] ===\n\n=== [p. 3] ===\ne\n");
    }

    #[test]
    fn crlf_form_feed_and_labels_off() {
        let options = TxtOptions { crlf: true, form_feed: true, page_labels: false };
        let mut w = TxtWriter::new(Vec::new(), options);
        w.page(1, Some("i"), &lines("a")).unwrap();
        w.page(2, Some("ii"), &lines("b")).unwrap();
        let text = String::from_utf8(w.finish().unwrap()).unwrap();
        assert_eq!(text, "=== [p. 1] ===\r\na\r\n\r\n\x0c=== [p. 2] ===\r\nb\r\n");
    }
}
