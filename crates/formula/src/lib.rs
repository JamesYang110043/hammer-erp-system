//! 公式引擎：解析器、Decimal 計算、相依性分析（`docs/architecture.md` §7.6）。
//!
//! 目前只有語法解析：[`parse`] 把公式文字轉成 [`Expr`]；`Expr` 的 `Display` 輸出正規化的公式文字。
//! 計算、相依分析與型別檢查尚未實作。本 crate 只依賴 `hammer-shared`（§4）。
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

mod ast;
mod error;
mod lexer;
mod parser;

pub use ast::{AggregateFunc, Arity, BinaryOp, Expr, ExprKind, Function, Span, UnaryOp};
pub use error::{ParseError, ParseErrorKind};
pub use parser::{MAX_DEPTH, MAX_FORMULA_LEN, parse};

#[cfg(test)]
mod tests;
