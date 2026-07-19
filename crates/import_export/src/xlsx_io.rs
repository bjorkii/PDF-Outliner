use anyhow::{Context, Result};
use bookmark::BookmarkRow;
use calamine::{open_workbook, DataType, Reader, Xlsx};
use rust_xlsxwriter::Workbook;
use std::path::Path;

const HEADERS: [&str; 5] = ["순서", "파일명", "계층", "북마크명", "페이지번호"];

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
        sheet.write_number(r, 0, row.order as f64)?;
        sheet.write_string(r, 1, &row.filename)?;
        sheet.write_number(r, 2, row.depth as f64)?;
        sheet.write_string(r, 3, &row.title)?;
        sheet.write_number(r, 4, row.page as f64)?;
    }

    workbook
        .save(path)
        .with_context(|| format!("xlsx 저장 실패: {:?}", path))?;
    Ok(())
}

/// 모든 시트(탭)를 순회하며 행을 수집한다(2026-07-19 — 예전엔 첫 시트만 읽었음).
/// 탭마다 다른 PDF의 북마크를 관리하는 통합 xlsx를 지원하기 위함 — 어느 탭의 행이든
/// 일단 다 모으고, "현재 파일명 일치 필터 + '순서' 정렬"은 호출부 공통 정책
/// (`bookmark::prepare_imported_rows`)이 처리한다. 스키마 헤더가 없는 탭(메모용 등)은
/// 조용히 건너뛰되, 유효한 탭이 하나도 없으면 에러.
pub fn import_xlsx(path: &Path) -> Result<Vec<BookmarkRow>> {
    let mut workbook: Xlsx<_> =
        open_workbook(path).with_context(|| format!("xlsx 열기 실패: {:?}", path))?;
    let sheet_names = workbook.sheet_names().to_vec();
    if sheet_names.is_empty() {
        anyhow::bail!("워크시트가 없음");
    }

    let mut result = Vec::new();
    let mut valid_sheets = 0usize;
    for sheet_name in &sheet_names {
        let Ok(range) = workbook.worksheet_range(sheet_name) else {
            continue;
        };
        let mut rows_iter = range.rows();
        let Some(header_row) = rows_iter.next() else {
            continue; // 빈 시트
        };

        let find_col =
            |name: &str| header_row.iter().position(|c| c.to_string().trim() == name);
        // 필수 컬럼이 하나라도 없으면 이 탭은 북마크 시트가 아닌 것으로 보고 건너뛴다.
        let (Some(i_filename), Some(i_depth), Some(i_title), Some(i_page)) = (
            find_col("파일명"),
            find_col("계층"),
            find_col("북마크명"),
            find_col("페이지번호"),
        ) else {
            continue;
        };
        // '순서' 컬럼(2026-07-19 신설)은 구버전 export 파일 호환을 위해 선택적 — 없으면
        // 그 탭 안에서의 행 순서를 그대로 일련번호로 쓴다(예전 동작과 동일). csv_io와
        // 같은 정책.
        let i_order = find_col("순서");
        valid_sheets += 1;

        for (row_index, row) in rows_iter.enumerate() {
            result.push(BookmarkRow {
                order: i_order
                    .and_then(|i| row.get(i))
                    .and_then(|c| c.as_f64())
                    .map(|v| v as u32)
                    .unwrap_or(row_index as u32 + 1),
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
    }

    if valid_sheets == 0 {
        anyhow::bail!(
            "필수 컬럼(파일명/계층/북마크명/페이지번호)을 갖춘 시트를 찾을 수 없음 ({}개 시트 검사)",
            sheet_names.len()
        );
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
                order: 1,
                filename: "보고서.pdf".to_string(),
                depth: 0,
                title: "요약".to_string(),
                page: 1,
            },
            BookmarkRow {
                order: 2,
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

    #[test]
    fn import_collects_rows_from_all_sheets_and_skips_invalid_ones() {
        use rust_xlsxwriter::Workbook;
        let dir = tempdir().unwrap();
        let path = dir.path().join("multi.xlsx");

        // 시트1: a.pdf 북마크 / 시트2: 스키마와 무관한 메모 탭 / 시트3: b.pdf 북마크
        let mut wb = Workbook::new();
        let headers = ["순서", "파일명", "계층", "북마크명", "페이지번호"];
        let s1 = wb.add_worksheet();
        for (c, h) in headers.iter().enumerate() {
            s1.write_string(0, c as u16, *h).unwrap();
        }
        s1.write_number(1, 0, 1.0).unwrap();
        s1.write_string(1, 1, "a.pdf").unwrap();
        s1.write_number(1, 2, 0.0).unwrap();
        s1.write_string(1, 3, "A문서 1장").unwrap();
        s1.write_number(1, 4, 1.0).unwrap();

        let s2 = wb.add_worksheet();
        s2.write_string(0, 0, "그냥 메모").unwrap();

        let s3 = wb.add_worksheet();
        for (c, h) in headers.iter().enumerate() {
            s3.write_string(0, c as u16, *h).unwrap();
        }
        s3.write_number(1, 0, 1.0).unwrap();
        s3.write_string(1, 1, "b.pdf").unwrap();
        s3.write_number(1, 2, 0.0).unwrap();
        s3.write_string(1, 3, "B문서 1장").unwrap();
        s3.write_number(1, 4, 2.0).unwrap();
        wb.save(&path).unwrap();

        let imported = import_xlsx(&path).unwrap();
        assert_eq!(imported.len(), 2);
        let files: Vec<_> = imported.iter().map(|r| r.filename.as_str()).collect();
        assert!(files.contains(&"a.pdf") && files.contains(&"b.pdf"));

        // 통합 파일에서 특정 문서 것만 골라내는 하위 정책과의 연동 확인
        let (kept, skipped) = bookmark::prepare_imported_rows(imported, "b.pdf");
        assert_eq!((kept.len(), skipped), (1, 1));
        assert_eq!(kept[0].title, "B문서 1장");
    }
}
