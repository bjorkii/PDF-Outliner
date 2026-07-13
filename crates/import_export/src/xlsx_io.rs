use anyhow::{Context, Result};
use bookmark::BookmarkRow;
use calamine::{open_workbook, DataType, Reader, Xlsx};
use rust_xlsxwriter::Workbook;
use std::path::Path;

const HEADERS: [&str; 4] = ["파일명", "계층", "북마크명", "페이지번호"];

/// xlsx는 바이너리 XML 포맷이라 CSV 같은 BOM/로케일 인코딩 문제가 원천적으로 없다.
/// 비전문 사용자에게는 이쪽을 기본 권장 export 포맷으로 안내.
pub fn export_xlsx(rows: &[BookmarkRow], path: &Path) -> Result<()> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();

    for (col, header) in HEADERS.iter().enumerate() {
        sheet.write_string(0, col as u16, *header)?;
    }

    for (i, row) in rows.iter().enumerate() {
        let r = (i + 1) as u32;
        sheet.write_string(r, 0, &row.filename)?;
        sheet.write_number(r, 1, row.depth as f64)?;
        sheet.write_string(r, 2, &row.title)?;
        sheet.write_number(r, 3, row.page as f64)?;
    }

    workbook
        .save(path)
        .with_context(|| format!("xlsx 저장 실패: {:?}", path))?;
    Ok(())
}

pub fn import_xlsx(path: &Path) -> Result<Vec<BookmarkRow>> {
    let mut workbook: Xlsx<_> =
        open_workbook(path).with_context(|| format!("xlsx 열기 실패: {:?}", path))?;
    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .context("워크시트가 없음")?;
    let range = workbook
        .worksheet_range(&sheet_name)
        .context("워크시트 읽기 실패")?;

    let mut rows_iter = range.rows();
    let header_row = rows_iter.next().context("헤더 행이 없음")?;

    let find_col = |name: &str| -> Result<usize> {
        header_row
            .iter()
            .position(|c| c.to_string().trim() == name)
            .with_context(|| format!("필수 컬럼 '{}'을 찾을 수 없음", name))
    };
    let i_filename = find_col("파일명")?;
    let i_depth = find_col("계층")?;
    let i_title = find_col("북마크명")?;
    let i_page = find_col("페이지번호")?;

    let mut result = Vec::new();
    for row in rows_iter {
        result.push(BookmarkRow {
            filename: row
                .get(i_filename)
                .map(|c| c.to_string())
                .unwrap_or_default(),
            depth: row
                .get(i_depth)
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0) as u32,
            title: row.get(i_title).map(|c| c.to_string()).unwrap_or_default(),
            page: row.get(i_page).and_then(|c| c.as_f64()).unwrap_or(1.0) as u32,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn export_then_import_roundtrip_preserves_korean() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bookmarks.xlsx");

        let rows = vec![
            BookmarkRow {
                filename: "보고서.pdf".to_string(),
                depth: 0,
                title: "요약".to_string(),
                page: 1,
            },
            BookmarkRow {
                filename: "보고서.pdf".to_string(),
                depth: 1,
                title: "세부 분석".to_string(),
                page: 3,
            },
        ];

        export_xlsx(&rows, &path).unwrap();
        let imported = import_xlsx(&path).unwrap();
        assert_eq!(rows, imported);
    }
}
