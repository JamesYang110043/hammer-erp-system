//! 公式欄位的相依圖：檢查循環引用，依拓撲排序決定計算順序（architecture.md §7.6）。

use std::collections::HashMap;
use std::fmt;

use crate::deps::Dependency;

/// 一個資料類別（含其明細子表）中的欄位。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldRef {
    /// 明細子表代碼；主表的欄位為 `None`。
    pub detail: Option<String>,
    /// 欄位代碼。
    pub field: String,
}

impl FieldRef {
    /// 主表的欄位。
    pub fn master(field: impl Into<String>) -> Self {
        Self {
            detail: None,
            field: field.into(),
        }
    }

    /// 明細子表 `detail` 的欄位。
    pub fn detail(detail: impl Into<String>, field: impl Into<String>) -> Self {
        Self {
            detail: Some(detail.into()),
            field: field.into(),
        }
    }
}

/// 主表欄位顯示為 `field`，明細欄位顯示為 `detail.field`。
impl fmt::Display for FieldRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(d) => write!(f, "{d}.{}", self.field),
            None => f.write_str(&self.field),
        }
    }
}

/// 一個公式欄位與它的相依（[`dependencies`](crate::dependencies) 或 `formula_deps` 讀回的清單）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaField {
    pub field: FieldRef,
    pub deps: Vec<Dependency>,
}

/// 公式之間有循環引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleError {
    /// 循環的路徑，第一個與最後一個相同，例如 `a → b → c → a` 為 `[a, b, c, a]`；
    /// 引用自己時為 `[a, a]`。
    pub path: Vec<FieldRef>,
}

impl CycleError {
    /// 顯示給使用者的訊息（繁體中文）。
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for CycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("公式有循環引用：")?;
        for (i, r) in self.path.iter().enumerate() {
            if i > 0 {
                f.write_str(" → ")?;
            }
            write!(f, "{r}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CycleError {}

/// 檢查循環引用並回傳公式欄位的計算順序。
///
/// `formulas` 是一個資料類別（含其明細子表）**所有**公式欄位。相依對應到的欄位：
///
/// | 相依 | 公式在主表 | 公式在明細子表 `d` |
/// |---|---|---|
/// | `{x}` | 主表的 `x` | `d.x` |
/// | `{r.x}` | 主表的 `r`（`x` 在其他資料，存檔時已算好，不參與排序） | `d.r` |
/// | `SUM(c.x)` | `c.x` | `c.x` |
///
/// 只有指向公式欄位的相依會形成邊，其他欄位是輸入值。
///
/// 回傳的順序中，被引用的公式一定排在引用它的公式之前；明細子表的公式排在主表的公式之前
/// （明細的公式不能引用主表欄位，由 [`check_formula`](crate::check_formula) 檢查，所以這個順序總是成立）。順序是固定的：明細子表依代碼排序，
/// 同一張表內依 `formulas` 的順序，再依相依調整。
///
/// `formulas` 中同一個欄位出現多次時，只使用第一個。
pub fn calculation_order(formulas: &[FormulaField]) -> Result<Vec<FieldRef>, CycleError> {
    // 去除重複，保留第一個
    let mut index: HashMap<&FieldRef, usize> = HashMap::new();
    let mut nodes: Vec<&FormulaField> = Vec::new();
    for f in formulas {
        if !index.contains_key(&f.field) {
            index.insert(&f.field, nodes.len());
            nodes.push(f);
        }
    }

    let edges: Vec<Vec<usize>> = nodes
        .iter()
        .map(|n| {
            n.deps
                .iter()
                .filter_map(|d| index.get(&target(&n.field, d)).copied())
                .collect()
        })
        .collect();

    // 起點順序：明細子表（依代碼）在前，主表在後；同一張表內依輸入順序（sort_by_key 是穩定排序）
    let mut roots: Vec<usize> = (0..nodes.len()).collect();
    roots.sort_by_key(|&i| match &nodes[i].field.detail {
        Some(d) => (0, d.as_str()),
        None => (1, ""),
    });

    let mut sorter = TopoSort {
        edges: &edges,
        state: vec![State::Unvisited; nodes.len()],
        stack: Vec::new(),
        order: Vec::with_capacity(nodes.len()),
    };
    for i in roots {
        sorter.visit(i).map_err(|cycle| CycleError {
            path: cycle.into_iter().map(|i| nodes[i].field.clone()).collect(),
        })?;
    }
    Ok(sorter
        .order
        .into_iter()
        .map(|i| nodes[i].field.clone())
        .collect())
}

/// 相依指向的欄位（見 [`calculation_order`] 的表格）。
fn target(owner: &FieldRef, dep: &Dependency) -> FieldRef {
    match dep {
        Dependency::Field { field } => FieldRef {
            detail: owner.detail.clone(),
            field: field.clone(),
        },
        Dependency::Relation { relation, .. } => FieldRef {
            detail: owner.detail.clone(),
            field: relation.clone(),
        },
        Dependency::Detail { detail, field } => FieldRef::detail(detail.clone(), field.clone()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Unvisited,
    Visiting,
    Done,
}

/// 深度優先的拓撲排序。欄位數量有限（一個資料類別的公式欄位），遞迴深度不會過深。
struct TopoSort<'a> {
    edges: &'a [Vec<usize>],
    state: Vec<State>,
    /// 目前的走訪路徑，用來回報循環。
    stack: Vec<usize>,
    order: Vec<usize>,
}

impl TopoSort<'_> {
    /// 失敗時回傳循環路徑（節點編號，首尾相同）。
    fn visit(&mut self, i: usize) -> Result<(), Vec<usize>> {
        match self.state[i] {
            State::Done => return Ok(()),
            State::Visiting => {
                let start = self.stack.iter().position(|&n| n == i).unwrap_or(0);
                let mut path = self.stack[start..].to_vec();
                path.push(i);
                return Err(path);
            }
            State::Unvisited => {}
        }
        self.state[i] = State::Visiting;
        self.stack.push(i);
        for &next in &self.edges[i] {
            self.visit(next)?;
        }
        self.stack.pop();
        self.state[i] = State::Done;
        self.order.push(i);
        Ok(())
    }
}
