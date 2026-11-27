//! Excel 匯入客戶品名的檔案解析（docs/architecture.md §8.7「Excel 匯入客戶品名」，M12）。
//!
//! 只讀第一個工作表；第一列是標題，依標題名稱對應欄位（順序不限，其他欄位忽略）：
//!
//! | 標題 | 必要 | 對應 |
//! |---|---|---|
//! | 客戶編號 | 是 | `customer_code` |
//! | 料號 | 是 | `product_code` |
//! | 客戶品名 | 是 | `name` |
//! | 備註 | 否 | `notes` |
//!
//! 儲存格轉成文字（[`cell_text`]）；四個欄位都空白的列略過；「備註」沒有這一欄或空白時不帶（保留既有的
//! 備註）。列層級的檢查（客戶、料號是否存在等）由伺服器的批次寫入 API 處理。

use std::path::Path;

use calamine::{Data, Range, Reader, open_workbook_auto};
use serde::{Deserialize, Serialize};
use specta::Type;

/// 一次最多匯入幾列（與伺服器的上限相同）。
pub const MAX_ROWS: usize = hammer_shared::sales::customer_product_name_batch::MAX_ROWS;

const CUSTOMER_CODE: &str = "客戶編號";
const PRODUCT_CODE: &str = "料號";
const NAME: &str = "客戶品名";
const NOTES: &str = "備註";
/// 必要的標題，依錯誤訊息列出的順序。
const REQUIRED: [&str; 3] = [CUSTOMER_CODE, PRODUCT_CODE, NAME];

/// 檔案中的一列。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ImportedCustomerProductName {
    /// Excel 的列號（標題為第 1 列）。
    pub row_no: u32,
    pub customer_code: String,
    pub product_code: String,
    pub name: String,
    /// 沒有「備註」欄或空白時為 `null`（不更新既有的備註）。
    pub notes: Option<String>,
}

/// 解析結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CustomerProductNameImport {
    /// 選擇的檔名（不含路徑）。
    pub file_name: String,
    pub rows: Vec<ImportedCustomerProductName>,
}

/// 讀取 `path` 的第一個工作表並解析。錯誤為顯示給使用者的訊息。
pub fn read_file(path: &Path) -> Result<CustomerProductNameImport, String> {
    let mut workbook = open_workbook_auto(path).map_err(|e| format!("無法讀取檔案：{e}"))?;
    let range = match workbook.worksheet_range_at(0) {
        None => return Err("檔案中沒有資料".to_owned()),
        Some(Err(e)) => return Err(format!("無法讀取檔案：{e}")),
        Some(Ok(range)) => range,
    };
    let rows = parse_range(&range)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(CustomerProductNameImport { file_name, rows })
}

/// 解析工作表的內容（第一列為標題）。
pub fn parse_range(range: &Range<Data>) -> Result<Vec<ImportedCustomerProductName>, String> {
    let Some((start_row, _)) = range.start() else {
        return Err("檔案中沒有資料".to_owned());
    };
    let mut rows = range.rows();
    let Some(header) = rows.next() else {
        return Err("檔案中沒有資料".to_owned());
    };
    let titles: Vec<String> = header
        .iter()
        .map(|c| cell_text(c).trim().to_owned())
        .collect();
    let column = |title: &str| -> Result<Option<usize>, String> {
        let mut found = titles.iter().enumerate().filter(|(_, t)| *t == title);
        let first = found.next().map(|(i, _)| i);
        if found.next().is_some() {
            return Err(format!("標題「{title}」重複"));
        }
        Ok(first)
    };
    let missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|t| !titles.iter().any(|x| x == t))
        .collect();
    if !missing.is_empty() {
        return Err(format!("缺少標題：{}", missing.join("、")));
    }
    let customer = column(CUSTOMER_CODE)?.unwrap_or_default();
    let product = column(PRODUCT_CODE)?.unwrap_or_default();
    let name = column(NAME)?.unwrap_or_default();
    let notes = column(NOTES)?;

    let text = |row: &[Data], i: usize| row.get(i).map(cell_text).unwrap_or_default();
    let mut out = Vec::new();
    for (offset, row) in rows.enumerate() {
        let customer_code = text(row, customer);
        let product_code = text(row, product);
        let name_text = text(row, name);
        let notes_text = notes.map(|i| text(row, i)).unwrap_or_default();
        if [&customer_code, &product_code, &name_text, &notes_text]
            .iter()
            .all(|s| s.trim().is_empty())
        {
            continue;
        }
        out.push(ImportedCustomerProductName {
            // 工作表可能不是從 A1 開始：列號 = 起始列（從 0 開始）+ 1 + 標題之後的第幾列 + 1
            row_no: start_row + 2 + offset as u32,
            customer_code: customer_code.trim().to_owned(),
            product_code: product_code.trim().to_owned(),
            name: name_text.trim().to_owned(),
            notes: Some(notes_text.trim().to_owned()).filter(|s| !s.is_empty()),
        });
    }
    if out.is_empty() {
        return Err("檔案中沒有資料".to_owned());
    }
    if out.len() > MAX_ROWS {
        return Err(format!(
            "一次最多匯入 {MAX_ROWS} 列，這個檔案有 {} 列",
            out.len()
        ));
    }
    Ok(out)
}

/// 儲存格轉成文字：整數值的數字不帶小數（`1001`）、其他數字以最短的十進位表示、日期為 `YYYY-MM-DD`
/// （有時間時加上 `HH:mm:ss`）、布林為 `TRUE` / `FALSE`、空白與錯誤為空字串。
pub fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty | Data::Error(_) => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => float_text(*f),
        Data::Bool(true) => "TRUE".to_owned(),
        Data::Bool(false) => "FALSE".to_owned(),
        Data::DateTime(dt) if dt.is_datetime() => {
            let (y, m, d, hh, mm, ss, _) = dt.to_ymd_hms_milli();
            if (hh, mm, ss) == (0, 0, 0) {
                format!("{y:04}-{m:02}-{d:02}")
            } else {
                format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
            }
        }
        Data::DateTime(dt) => float_text(dt.as_f64()),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
    }
}

/// `1001.0` → `1001`；`12.5` → `12.5`。
fn float_text(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::{Cell, ExcelDateTime, ExcelDateTimeType};

    fn s(v: &str) -> Data {
        Data::String(v.to_owned())
    }

    /// 由列（從 `(row0, 0)` 開始）建立工作表範圍。
    fn range_at(row0: u32, rows: Vec<Vec<Data>>) -> Range<Data> {
        let cells = rows
            .into_iter()
            .enumerate()
            .flat_map(|(r, row)| {
                row.into_iter()
                    .enumerate()
                    .map(move |(c, v)| Cell::new((row0 + r as u32, c as u32), v))
            })
            .collect();
        Range::from_sparse(cells)
    }

    fn range(rows: Vec<Vec<Data>>) -> Range<Data> {
        range_at(0, rows)
    }

    fn row(no: u32, c: &str, p: &str, n: &str, notes: Option<&str>) -> ImportedCustomerProductName {
        ImportedCustomerProductName {
            row_no: no,
            customer_code: c.into(),
            product_code: p.into(),
            name: n.into(),
            notes: notes.map(str::to_owned),
        }
    }

    #[test]
    fn maps_columns_by_title_in_any_order() {
        let r = range(vec![
            vec![
                s("料號"),
                s("其他"),
                s(" 客戶品名 "),
                s("客戶編號"),
                s("備註"),
            ],
            vec![
                s("P001"),
                s("x"),
                s(" X 型扳手 "),
                s("C001"),
                s("圖面 W-10"),
            ],
            vec![
                Data::Empty,
                Data::Empty,
                Data::Empty,
                Data::Empty,
                Data::Empty,
            ],
            vec![
                Data::Float(1001.0),
                s("y"),
                s("零件"),
                Data::Int(2001),
                Data::Empty,
            ],
        ]);
        assert_eq!(
            parse_range(&r).unwrap(),
            [
                row(2, "C001", "P001", "X 型扳手", Some("圖面 W-10")),
                // 空白列略過，列號仍是 Excel 的列號
                row(4, "2001", "1001", "零件", None),
            ]
        );
    }

    #[test]
    fn notes_column_is_optional() {
        let r = range(vec![
            vec![s("客戶編號"), s("料號"), s("客戶品名")],
            vec![s("C001"), s("P001"), s("X")],
        ]);
        assert_eq!(
            parse_range(&r).unwrap(),
            [row(2, "C001", "P001", "X", None)]
        );
    }

    #[test]
    fn rows_with_missing_values_are_kept_for_the_server_to_report() {
        let r = range(vec![
            vec![s("客戶編號"), s("料號"), s("客戶品名")],
            vec![s("C001"), Data::Empty, s("X")],
        ]);
        assert_eq!(parse_range(&r).unwrap(), [row(2, "C001", "", "X", None)]);
    }

    #[test]
    fn sheet_not_starting_at_a1_uses_excel_row_numbers() {
        let r = range_at(
            2,
            vec![
                vec![s("客戶編號"), s("料號"), s("客戶品名")],
                vec![s("C001"), s("P001"), s("X")],
            ],
        );
        assert_eq!(parse_range(&r).unwrap()[0].row_no, 4);
    }

    #[test]
    fn header_errors() {
        let missing = range(vec![vec![s("客戶品名"), s("備註")], vec![s("X"), s("")]]);
        assert_eq!(
            parse_range(&missing).unwrap_err(),
            "缺少標題：客戶編號、料號"
        );
        let dup = range(vec![
            vec![s("客戶編號"), s("料號"), s("客戶品名"), s("料號")],
            vec![s("C"), s("P"), s("X"), s("P")],
        ]);
        assert_eq!(parse_range(&dup).unwrap_err(), "標題「料號」重複");
        let empty: Range<Data> = Range::empty();
        assert_eq!(parse_range(&empty).unwrap_err(), "檔案中沒有資料");
        let header_only = range(vec![vec![s("客戶編號"), s("料號"), s("客戶品名")]]);
        assert_eq!(parse_range(&header_only).unwrap_err(), "檔案中沒有資料");
    }

    #[test]
    fn too_many_rows() {
        let mut rows = vec![vec![s("客戶編號"), s("料號"), s("客戶品名")]];
        rows.extend((0..=MAX_ROWS).map(|i| vec![s("C"), s(&format!("P{i}")), s("X")]));
        assert_eq!(
            parse_range(&range(rows)).unwrap_err(),
            format!("一次最多匯入 {MAX_ROWS} 列，這個檔案有 {} 列", MAX_ROWS + 1)
        );
    }

    #[test]
    fn cell_text_formats() {
        assert_eq!(cell_text(&Data::Float(1001.0)), "1001");
        assert_eq!(cell_text(&Data::Float(12.5)), "12.5");
        assert_eq!(cell_text(&Data::Int(-3)), "-3");
        assert_eq!(cell_text(&Data::Bool(true)), "TRUE");
        assert_eq!(cell_text(&Data::Empty), "");
        // 45943 = 2025-10-13
        let date = ExcelDateTime::new(45943.0, ExcelDateTimeType::DateTime, false);
        assert_eq!(cell_text(&Data::DateTime(date)), "2025-10-13");
        let datetime = ExcelDateTime::new(45943.5, ExcelDateTimeType::DateTime, false);
        assert_eq!(cell_text(&Data::DateTime(datetime)), "2025-10-13 12:00:00");
    }
}
