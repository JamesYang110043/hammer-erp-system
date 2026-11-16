//! 公式引擎：解析器、Decimal 計算、相依性分析（`docs/architecture.md` §7.6）。
//!
//! - 語法解析：[`parse`] 把公式文字轉成 [`Expr`]；`Expr` 的 `Display` 輸出正規化的公式文字。
//! - 計算：[`evaluate`] 依呼叫端提供的 [`Environment`] 算出 [`Value`]；[`to_storage`] 依
//!   `formula_result_type` 轉成 §7.3 的 JSON 存值；[`compute`] 兩者合一。
//!
//! 相依分析與型別檢查尚未實作。本 crate 只依賴 `hammer-shared`（§4），不讀取資料庫：
//! 伺服器（`crates/core`）與桌面端各自實作 [`Environment`]。
//!
//! # 語法
//!
//! | 語法 | 範例 |
//! |---|---|
//! | 欄位引用 | `{qty}` |
//! | 關聯引用（最多一層） | `{customer.discount_rate}` |
//! | 子表彙總 | `SUM(lines.amount)`、`AVG`、`COUNT`、`MIN`、`MAX`；參數只能是一個「明細子表代碼.欄位代碼」 |
//! | 數字 | `12`、`0.05`（直接轉成 `Decimal`，不經過浮點數；沒有指數表示法） |
//! | 字串 | `"open"`，跳脫字元只有 `\"`、`\\` |
//! | 布林 | `TRUE`、`FALSE` |
//! | 函式 | `IF`(3)、`ROUND`(2)、`ABS`(1)、`TODAY`(0)、`DAYS`(2)、`CONCAT`(≥1)、`COALESCE`(≥1) |
//!
//! 函式名稱與關鍵字（`TRUE`、`FALSE`、`AND`、`OR`、`NOT`）不分大小寫；代碼依
//! `hammer_shared::schema::key` 的命名規則（小寫英文字母開頭）。
//!
//! # 運算子優先順序
//!
//! 由低到高（越下面越先結合）：
//!
//! | 層級 | 運算子 | 結合性 |
//! |---|---|---|
//! | 1 | `OR` | 左結合 |
//! | 2 | `AND` | 左結合 |
//! | 3 | `NOT`（前置） | 右結合（`NOT NOT a`） |
//! | 4 | `=` `<>` `<` `<=` `>` `>=` | 左結合 |
//! | 5 | `&`（字串連接） | 左結合 |
//! | 6 | `+` `-` | 左結合 |
//! | 7 | `*` `/` | 左結合 |
//! | 8 | `-`（一元負號） | 右結合（`--a`） |
//!
//! 因此 `NOT a = b` 是 `NOT (a = b)`、`a & b = c` 是 `(a & b) = c`、`-2 * 3` 是 `(-2) * 3`。
//!
//! # 限制
//!
//! - 公式最多 [`MAX_FORMULA_LEN`] 個字元
//! - 巢狀最多 [`MAX_DEPTH`] 層（括號、函式參數、`NOT`、一元負號各算一層），避免遞迴過深
//!
//! 錯誤以 [`ParseError`] 回傳，包含錯誤種類與字元位移，訊息為繁體中文。
//!
//! # 計算
//!
//! ## 值
//!
//! [`Value`]：數字（`Decimal`）、文字、布林、日期、日期時間、空值。欄位值依欄位型態從 JSON
//! 存值轉換（[`json_to_value`]）：`integer`、`decimal`、`money` 都是數字；`text`、`long_text`、
//! `select` 是文字；`multi_select`、`relation`、`file` 不能用在公式。`formula` 欄位以它的
//! `formula_result_type` 轉換。
//!
//! 全程使用 `rust_decimal`，不經過浮點數（D9）：`0.1 + 0.2 = 0.3` 為真。加減乘除都檢查溢位；
//! 除法結果超過 28 位小數時由 `Decimal` 截到可表示的精度。
//!
//! ## 運算子
//!
//! | 運算子 | 參數 | 結果 | 空值 |
//! |---|---|---|---|
//! | `+ - * /`、一元 `-` | 數字 | 數字 | 任一邊為空值 → 空值 |
//! | `&` | 任何值，轉成文字（[`Value::to_text`]） | 文字 | 視為空字串 |
//! | `= <>` | 兩邊同型態：數字、文字、布林、日期、日期時間 | 布林 | 任一邊為空值 → 空值 |
//! | `< <= > >=` | 兩邊同型態：數字、文字、日期、日期時間（布林不能比大小） | 布林 | 任一邊為空值 → 空值 |
//! | `AND`、`OR`、`NOT` | 布林 | 布林 | 三值邏輯：`FALSE AND 空值` 為 `FALSE`、`TRUE OR 空值` 為 `TRUE`，其餘為空值 |
//!
//! - 數字比較依數值：`1.0 = 1.00` 為真。文字比較區分大小寫，依 Unicode 碼位排序。
//! - 日期時間比較的是時間點，不受時區位移影響。
//! - 型態不符優先於空值：`"a" * {空值的欄位}` 仍是型態錯誤。
//!
//! ## 函式
//!
//! | 函式 | 說明 |
//! |---|---|
//! | `IF(條件, 是, 否)` | 條件為 `TRUE` 時取「是」，`FALSE` 或空值時取「否」。**只計算選到的分支**，所以 `IF({qty} = 0, 0, {total} / {qty})` 不會除以零 |
//! | `ROUND(x, n)` | 四捨五入到 `n` 位小數（MidpointAwayFromZero，`ROUND(2.5, 0) = 3`、`ROUND(-2.5, 0) = -3`）；`n` 必須是 0 到 [`MAX_ROUND_DIGITS`] 的整數。任一參數為空值 → 空值 |
//! | `ABS(x)` | 絕對值；空值 → 空值 |
//! | `TODAY()` | 由 [`Environment::today`] 提供；伺服器使用 Asia/Taipei（D13） |
//! | `DAYS(a, b)` | `a − b` 的天數。參數為日期，日期時間以 Asia/Taipei 的日期計算；任一參數為空值 → 空值 |
//! | `CONCAT(a, …)` | 同 `&`：每個參數轉成文字後連接，空值視為空字串 |
//! | `COALESCE(a, …)` | 依序計算，回傳第一個非空值；全部為空值 → 空值 |
//! | `SUM(子表.欄位)` | 明細非空值的總和；沒有明細或全部為空值 → `0` |
//! | `AVG(子表.欄位)` | 明細非空值的平均；沒有可計算的值 → 空值 |
//! | `MIN`、`MAX(子表.欄位)` | 明細非空值的最小、最大值；沒有可計算的值 → 空值 |
//! | `COUNT(子表.欄位)` | 明細的列數（含空值的列）；沒有明細 → `0`。欄位可以是任何支援的型態 |
//!
//! `SUM`、`AVG`、`MIN`、`MAX` 的欄位必須是 `integer`、`decimal`、`money`（或結果為這些型態的公式）。
//!
//! ## 錯誤
//!
//! 除以零、型態不符、溢位、欄位不存在、存值格式錯誤時，[`evaluate`] 回傳 [`EvalError`]
//! （訊息為繁體中文），公式的結果視為空值；計算不會 panic。[`compute`] 回傳的 [`Computed`]
//! 同時帶有存值（失敗時為 `null`）與錯誤原因。

mod ast;
mod env;
mod error;
mod eval;
mod lexer;
mod parser;
mod storage;
mod value;

pub use ast::{AggregateFunc, Arity, BinaryOp, Expr, ExprKind, Function, Span, UnaryOp};
pub use env::{DetailColumn, Environment, FieldData, json_to_value};
pub use error::{EvalError, ParseError, ParseErrorKind};
pub use eval::{MAX_ROUND_DIGITS, evaluate};
pub use parser::{MAX_DEPTH, MAX_FORMULA_LEN, parse};
pub use storage::{Computed, compute, to_storage};
pub use value::{Value, ValueKind};

#[cfg(test)]
mod eval_tests;
#[cfg(test)]
mod tests;
