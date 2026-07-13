use anyhow::{Context, Result};
use bookmark::BookmarkRow;
use encoding_rs::{Encoding, EUC_KR, UTF_8};
use std::fs::File;
use std::io::Write;
use std::path::Path;

const HEADERS: [&str; 4] = ["파일명", "계층", "북마크명", "페이지번호"];

/// CSV export. Windows Excel이 BOM 없는 UTF-8 CSV를 CP949로 오판독해
/// 한글이 깨지는 문제를 막기 위해 파일 맨 앞에 UTF-8 BOM(EF BB BF)을 명시적으로 기록한다.
pub fn export_csv(rows: &[BookmarkRow], path: &Path) -> Result<()> {
    let mut file = File::create(path).with_context(|| format!("파일 생성 실패: {:?}", path))?;
    file.write_all(b"\xEF\xBB\xBF")?; // UTF-8 BOM

    let mut wtr = csv::WriterBuilder::new().from_writer(file);
    wtr.write_record(HEADERS)?;
    for row in rows {
        wtr.write_record(&[
            row.filename.as_str(),
            &row.depth.to_string(),
            row.title.as_str(),
            &row.page.to_string(),
        ])?;
    }
    wtr.flush()?;
    Ok(())
}

/// CSV import. 인코딩 우선순위:
/// 1) 명시적으로 override_encoding이 주어지면 그것을 사용(사용자가 UI에서 직접 선택한 경우)
/// 2) UTF-8 BOM이 있으면 UTF-8로 디코딩
/// 3) BOM이 없으면 자동 감지(chardetng) 시도, 디코딩 에러가 나면 EUC-KR(CP949 상위호환)로 폴백
///
/// 오래된 한글 CSV(BOM 없는 EUC-KR/CP949)를 열었을 때 깨짐을 방지하기 위한 안전망이다.
pub fn import_csv(
    path: &Path,
    override_encoding: Option<&'static Encoding>,
) -> Result<Vec<BookmarkRow>> {
    let raw = std::fs::read(path).with_context(|| format!("파일 읽기 실패: {:?}", path))?;
    let text = decode_csv_bytes(&raw, override_encoding);
    parse_csv_str(&text)
}

fn decode_csv_bytes(raw: &[u8], override_encoding: Option<&'static Encoding>) -> String {
    if let Some(enc) = override_encoding {
        let (text, _, _) = enc.decode(raw);
        return text.into_owned();
    }

    if let Some(stripped) = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let (text, _, _) = UTF_8.decode(stripped);
        return text.into_owned();
    }

    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(raw, true);
    let guessed = detector.guess(None, true);
    let (text, _, had_errors) = guessed.decode(raw);
    if had_errors {
        let (fallback_text, _, _) = EUC_KR.decode(raw);
        fallback_text.into_owned()
    } else {
        text.into_owned()
    }
}

fn parse_csv_str(text: &str) -> Result<Vec<BookmarkRow>> {
    let mut rdr = csv::ReaderBuilder::new().from_reader(text.as_bytes());
    let headers = rdr.headers()?.clone();

    let idx = |name: &str| -> Result<usize> {
        headers
            .iter()
            .position(|h| h.trim() == name)
            .with_context(|| format!("필수 컬럼 '{}'을 찾을 수 없음", name))
    };
    let i_filename = idx("파일명")?;
    let i_depth = idx("계층")?;
    let i_title = idx("북마크명")?;
    let i_page = idx("페이지번호")?;

    let mut rows = Vec::new();
    for result in rdr.records() {
        let record = result?;
        rows.push(BookmarkRow {
            filename: record.get(i_filename).unwrap_or_default().to_string(),
            depth: record
                .get(i_depth)
                .unwrap_or("0")
                .trim()
                .parse()
                .context("계층 컬럼이 숫자가 아님")?,
            title: record.get(i_title).unwrap_or_default().to_string(),
            page: record
                .get(i_page)
                .unwrap_or("1")
                .trim()
                .parse()
                .context("페이지번호 컬럼이 숫자가 아님")?,
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn export_then_import_roundtrip_preserves_korean() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bookmarks.csv");

        let rows = vec![
            BookmarkRow {
                filename: "계약서_최종본.pdf".to_string(),
                depth: 0,
                title: "제1장 총칙".to_string(),
                page: 1,
            },
            BookmarkRow {
                filename: "계약서_최종본.pdf".to_string(),
                depth: 1,
                title: "제1조 목적".to_string(),
                page: 2,
            },
        ];

        export_csv(&rows, &path).unwrap();

        // BOM이 실제로 기록됐는지 확인
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(&raw[0..3], &[0xEF, 0xBB, 0xBF]);

        let imported = import_csv(&path, None).unwrap();
        assert_eq!(rows, imported);
    }

    #[test]
    fn import_legacy_euc_kr_csv_without_bom() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("legacy.csv");

        let csv_text = "파일명,계층,북마크명,페이지번호\n오래된파일.pdf,0,서론,1\n";
        let (encoded, _, _) = EUC_KR.encode(csv_text);
        std::fs::write(&path, &encoded).unwrap();

        let imported = import_csv(&path, None).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].title, "서론");
        assert_eq!(imported[0].filename, "오래된파일.pdf");
    }
}
