//! 公式的語法樹（AST）與正規化文字輸出。

use std::fmt;

use hammer_shared::decimal::Decimal;

/// 公式文字中的範圍，以**字元**位移表示（不是 byte），`start` 含、`end` 不含。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// 涵蓋 `self` 與 `other` 的最小範圍。
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

/// 語法樹節點。
///
/// 相等比較**只比較結構**，不比較 [`Span`]：同一個公式重新排版後再解析，AST 仍然相等。
#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for Expr {}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    /// 數字常數。
    Number(Decimal),
    /// 字串常數（已處理跳脫字元）。
    String(String),
    /// `TRUE` / `FALSE`。
    Bool(bool),
    /// 欄位引用 `{field_key}`。
    Field(String),
    /// 關聯引用 `{relation_field.field_key}`。
    RelationField { relation: String, field: String },
    /// 子表彙總，例如 `SUM(lines.amount)`。
    Aggregate {
        func: AggregateFunc,
        detail: String,
        field: String,
    },
    /// 一元運算。
    Unary { op: UnaryOp, operand: Box<Expr> },
    /// 二元運算。
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// 一般函式呼叫（彙總函式見 [`ExprKind::Aggregate`]）。
    Call { func: Function, args: Vec<Expr> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// 一元負號 `-`。
    Neg,
    /// `NOT`。
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Concat => "&",
            Self::Eq => "=",
            Self::Ne => "<>",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::And => "AND",
            Self::Or => "OR",
        }
    }

    /// 優先順序，數字越大越先結合。見 crate 文件。
    pub(crate) fn precedence(self) -> u8 {
        match self {
            Self::Or => PREC_OR,
            Self::And => PREC_AND,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge => PREC_CMP,
            Self::Concat => PREC_CONCAT,
            Self::Add | Self::Sub => PREC_ADD,
            Self::Mul | Self::Div => PREC_MUL,
        }
    }
}

pub(crate) const PREC_OR: u8 = 1;
pub(crate) const PREC_AND: u8 = 2;
pub(crate) const PREC_NOT: u8 = 3;
pub(crate) const PREC_CMP: u8 = 4;
pub(crate) const PREC_CONCAT: u8 = 5;
pub(crate) const PREC_ADD: u8 = 6;
pub(crate) const PREC_MUL: u8 = 7;
pub(crate) const PREC_NEG: u8 = 8;
pub(crate) const PREC_ATOM: u8 = 9;

/// 子表彙總函式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateFunc {
    Sum,
    Avg,
    Count,
    Min,
    Max,
}

impl AggregateFunc {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sum => "SUM",
            Self::Avg => "AVG",
            Self::Count => "COUNT",
            Self::Min => "MIN",
            Self::Max => "MAX",
        }
    }
}

/// 一般函式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Function {
    If,
    Round,
    Abs,
    Today,
    Days,
    Concat,
    Coalesce,
}

/// 函式接受的參數個數。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// 剛好 n 個。
    Exact(usize),
    /// 至少 n 個。
    AtLeast(usize),
}

impl Arity {
    pub fn accepts(self, n: usize) -> bool {
        match self {
            Self::Exact(k) => n == k,
            Self::AtLeast(k) => n >= k,
        }
    }
}

impl fmt::Display for Arity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(n) => write!(f, "{n} 個"),
            Self::AtLeast(n) => write!(f, "至少 {n} 個"),
        }
    }
}

impl Function {
    pub fn name(self) -> &'static str {
        match self {
            Self::If => "IF",
            Self::Round => "ROUND",
            Self::Abs => "ABS",
            Self::Today => "TODAY",
            Self::Days => "DAYS",
            Self::Concat => "CONCAT",
            Self::Coalesce => "COALESCE",
        }
    }

    pub fn arity(self) -> Arity {
        match self {
            Self::If => Arity::Exact(3),
            Self::Round => Arity::Exact(2),
            Self::Abs => Arity::Exact(1),
            Self::Today => Arity::Exact(0),
            Self::Days => Arity::Exact(2),
            Self::Concat | Self::Coalesce => Arity::AtLeast(1),
        }
    }
}

/// 依名稱（不分大小寫）找函式。
pub(crate) enum FunctionName {
    Plain(Function),
    Aggregate(AggregateFunc),
}

pub(crate) fn lookup_function(name: &str) -> Option<FunctionName> {
    use FunctionName::{Aggregate, Plain};
    let f = match name.to_ascii_uppercase().as_str() {
        "IF" => Plain(Function::If),
        "ROUND" => Plain(Function::Round),
        "ABS" => Plain(Function::Abs),
        "TODAY" => Plain(Function::Today),
        "DAYS" => Plain(Function::Days),
        "CONCAT" => Plain(Function::Concat),
        "COALESCE" => Plain(Function::Coalesce),
        "SUM" => Aggregate(AggregateFunc::Sum),
        "AVG" => Aggregate(AggregateFunc::Avg),
        "COUNT" => Aggregate(AggregateFunc::Count),
        "MIN" => Aggregate(AggregateFunc::Min),
        "MAX" => Aggregate(AggregateFunc::Max),
        _ => return None,
    };
    Some(f)
}

// ---------------------------------------------------------------------------
// 正規化文字輸出
// ---------------------------------------------------------------------------

impl Expr {
    fn precedence(&self) -> u8 {
        match &self.kind {
            ExprKind::Binary { op, .. } => op.precedence(),
            ExprKind::Unary {
                op: UnaryOp::Not, ..
            } => PREC_NOT,
            ExprKind::Unary {
                op: UnaryOp::Neg, ..
            } => PREC_NEG,
            // 負數常數只會由程式建構（解析器不會產生），輸出時視同一元負號
            ExprKind::Number(n) if n.is_sign_negative() => PREC_NEG,
            _ => PREC_ATOM,
        }
    }

    /// `min` 以下的優先順序要加括號。
    fn fmt_operand(&self, f: &mut fmt::Formatter<'_>, min: u8) -> fmt::Result {
        if self.precedence() < min {
            write!(f, "({self})")
        } else {
            write!(f, "{self}")
        }
    }
}

/// 輸出正規化的公式文字：函式與關鍵字大寫、運算子前後一個空白、只保留必要的括號。
/// 輸出的文字再解析，會得到相等的 AST。
impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ExprKind::Number(n) => write!(f, "{n}"),
            ExprKind::String(s) => {
                f.write_str("\"")?;
                for c in s.chars() {
                    if c == '"' || c == '\\' {
                        f.write_str("\\")?;
                    }
                    write!(f, "{c}")?;
                }
                f.write_str("\"")
            }
            ExprKind::Bool(true) => f.write_str("TRUE"),
            ExprKind::Bool(false) => f.write_str("FALSE"),
            ExprKind::Field(key) => write!(f, "{{{key}}}"),
            ExprKind::RelationField { relation, field } => write!(f, "{{{relation}.{field}}}"),
            ExprKind::Aggregate {
                func,
                detail,
                field,
            } => write!(f, "{}({detail}.{field})", func.name()),
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand,
            } => {
                f.write_str("-")?;
                operand.fmt_operand(f, PREC_NEG)
            }
            ExprKind::Unary {
                op: UnaryOp::Not,
                operand,
            } => {
                f.write_str("NOT ")?;
                operand.fmt_operand(f, PREC_NOT)
            }
            ExprKind::Binary { op, left, right } => {
                // 二元運算子都是左結合：右邊同一層也要加括號
                let p = op.precedence();
                left.fmt_operand(f, p)?;
                write!(f, " {} ", op.symbol())?;
                right.fmt_operand(f, p + 1)
            }
            ExprKind::Call { func, args } => {
                write!(f, "{}(", func.name())?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                f.write_str(")")
            }
        }
    }
}
