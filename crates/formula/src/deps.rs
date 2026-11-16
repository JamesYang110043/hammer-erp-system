//! 從 AST 取出公式的相依欄位，以及 `field_definitions.formula_deps` 的 JSON 格式。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::ast::{Expr, ExprKind};

/// 公式引用的一個欄位。
///
/// JSON 格式（`formula_deps` 存的是這些物件的陣列，見 architecture.md §8.5）：
///
/// | 引用 | JSON |
/// |---|---|
/// | `{qty}` | `{"kind":"field","field":"qty"}` |
/// | `{customer.discount_rate}` | `{"kind":"relation","relation":"customer","field":"discount_rate"}` |
/// | `SUM(lines.amount)` | `{"kind":"detail","detail":"lines","field":"amount"}` |
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Dependency {
    /// 本筆資料的欄位。
    Field { field: String },
    /// 關聯欄位 `relation` 所指資料的 `field` 欄位。同時依賴本筆的關聯欄位 `relation`。
    Relation { relation: String, field: String },
    /// 明細子表 `detail` 的 `field` 欄位（子表彙總）。
    Detail { detail: String, field: String },
}

/// 取出公式引用的所有欄位，排序並去除重複（順序：本筆欄位、關聯引用、子表彙總，各自依代碼排序）。
///
/// 同一個公式的相依清單是固定的，重新排版公式文字不會改變結果。
pub fn dependencies(expr: &Expr) -> Vec<Dependency> {
    let mut out = BTreeSet::new();
    collect(expr, &mut out);
    out.into_iter().collect()
}

fn collect(expr: &Expr, out: &mut BTreeSet<Dependency>) {
    match &expr.kind {
        ExprKind::Number(_) | ExprKind::String(_) | ExprKind::Bool(_) => {}
        ExprKind::Field(field) => {
            out.insert(Dependency::Field {
                field: field.clone(),
            });
        }
        ExprKind::RelationField { relation, field } => {
            out.insert(Dependency::Relation {
                relation: relation.clone(),
                field: field.clone(),
            });
        }
        ExprKind::Aggregate { detail, field, .. } => {
            out.insert(Dependency::Detail {
                detail: detail.clone(),
                field: field.clone(),
            });
        }
        ExprKind::Unary { operand, .. } => collect(operand, out),
        ExprKind::Binary { left, right, .. } => {
            collect(left, out);
            collect(right, out);
        }
        ExprKind::Call { args, .. } => {
            for a in args {
                collect(a, out);
            }
        }
    }
}

/// 相依清單轉成 `formula_deps` 的 JSON（陣列）。
pub fn deps_to_json(deps: &[Dependency]) -> Json {
    Json::Array(
        deps.iter()
            .map(|d| match d {
                Dependency::Field { field } => serde_json::json!({
                    "kind": "field",
                    "field": field,
                }),
                Dependency::Relation { relation, field } => serde_json::json!({
                    "kind": "relation",
                    "relation": relation,
                    "field": field,
                }),
                Dependency::Detail { detail, field } => serde_json::json!({
                    "kind": "detail",
                    "detail": detail,
                    "field": field,
                }),
            })
            .collect(),
    )
}

/// 從 `formula_deps` 的 JSON 讀回相依清單。格式不符時回傳錯誤。
pub fn deps_from_json(json: &Json) -> Result<Vec<Dependency>, serde_json::Error> {
    Vec::<Dependency>::deserialize(json)
}
