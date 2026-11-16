//! 型別檢查：定義公式時，依欄位定義推斷結果型態並檢查引用規則（不需要資料庫的部分）。
//!
//! 規則與計算時（[`evaluate`](crate::evaluate)）的型態檢查相同，另外要求 `IF` 的兩個分支、
//! `COALESCE` 的所有參數型態相同，讓公式的結果型態是固定的。

use std::collections::{HashMap, HashSet};
use std::fmt;

use hammer_shared::schema::DataType;

use crate::ast::{AggregateFunc, Arity, BinaryOp, Expr, ExprKind, Function, Span, UnaryOp};
use crate::eval::MAX_ROUND_DIGITS;
use crate::value::ValueKind;

/// 公式所在的位置。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scope {
    /// 主表（或沒有明細的資料類別）。
    #[default]
    Master,
    /// 明細子表。`master_fields` 是主表的欄位代碼，只用來產生較清楚的錯誤訊息
    /// （「明細的公式不能引用主表欄位」而不是「找不到欄位」）。
    Detail { master_fields: HashSet<String> },
}

/// 型別檢查需要的欄位定義，由呼叫端提供（本 crate 不讀取資料庫）。
///
/// 欄位型態依 §7.3；`formula` 欄位請傳它的 `formula_result_type`（與 [`FieldData`](crate::FieldData)
/// 相同），不要傳 [`DataType::Formula`]。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TypeSchema {
    pub scope: Scope,
    /// 公式所在資料表（主表或明細子表）的欄位：代碼 → 型態。
    pub fields: HashMap<String, DataType>,
    /// 關聯欄位代碼 → 目標資料類別的欄位（代碼 → 型態）。關聯欄位本身也要在 `fields` 中，型態為
    /// `relation`。
    pub relations: HashMap<String, HashMap<String, DataType>>,
    /// 明細子表代碼 → 子表的欄位（代碼 → 型態）。只在主表使用。
    pub details: HashMap<String, HashMap<String, DataType>>,
}

impl TypeSchema {
    /// 主表的公式。
    pub fn master() -> Self {
        Self::default()
    }

    /// 明細子表的公式；`master_fields` 是主表的欄位代碼。
    pub fn detail<I, S>(master_fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            scope: Scope::Detail {
                master_fields: master_fields.into_iter().map(Into::into).collect(),
            },
            ..Self::default()
        }
    }

    /// 加入一個欄位。
    pub fn with_field(mut self, key: impl Into<String>, data_type: DataType) -> Self {
        self.fields.insert(key.into(), data_type);
        self
    }

    /// 加入一個關聯欄位與它目標資料類別的欄位。
    pub fn with_relation<'k>(
        mut self,
        key: impl Into<String>,
        target_fields: impl IntoIterator<Item = (&'k str, DataType)>,
    ) -> Self {
        let key = key.into();
        self.fields.insert(key.clone(), DataType::Relation);
        self.relations.insert(key, to_map(target_fields));
        self
    }

    /// 加入一個明細子表與它的欄位。
    pub fn with_detail<'k>(
        mut self,
        key: impl Into<String>,
        fields: impl IntoIterator<Item = (&'k str, DataType)>,
    ) -> Self {
        self.details.insert(key.into(), to_map(fields));
        self
    }
}

fn to_map<'k>(fields: impl IntoIterator<Item = (&'k str, DataType)>) -> HashMap<String, DataType> {
    fields.into_iter().map(|(k, t)| (k.to_owned(), t)).collect()
}

/// 型別檢查失敗。`span` 是錯誤在公式文字中的位置（字元位移）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeError {
    pub kind: TypeErrorKind,
    pub span: Span,
}

impl TypeError {
    fn new(kind: TypeErrorKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// 顯示給使用者的訊息（繁體中文，不含位置）。
    pub fn message(&self) -> String {
        self.kind.to_string()
    }
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "第 {} 個字元：{}", self.span.start + 1, self.kind)
    }
}

impl std::error::Error for TypeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeErrorKind {
    /// 找不到欄位。
    UnknownField(String),
    /// 明細的公式引用了主表的欄位。
    MasterFieldInDetail(String),
    /// `{a.b}` 的 `a` 不是關聯欄位。
    NotRelation(String),
    /// 關聯欄位的目標資料類別沒有這個欄位。
    UnknownRelationField { relation: String, field: String },
    /// 明細的公式使用子表彙總。
    AggregateInDetail { func: &'static str },
    /// 找不到明細子表。
    UnknownDetail(String),
    /// 明細子表沒有這個欄位。
    UnknownDetailField { detail: String, field: String },
    /// 公式不支援此型態的欄位（`multi_select`、`relation`、`file`）。
    UnsupportedFieldType { field: String, data_type: DataType },
    /// `SUM`、`AVG`、`MIN`、`MAX` 的明細欄位不是數字。
    AggregateNotNumber {
        func: &'static str,
        field: String,
        found: ValueKind,
    },
    /// 運算子或函式參數的型態不對；`context` 例如「運算子「*」」、「函式 ROUND 的第 1 個參數」。
    TypeMismatch {
        context: String,
        expected: &'static str,
        found: ValueKind,
    },
    /// 比較運算子兩邊的型態無法比較。
    Incomparable {
        op: &'static str,
        left: ValueKind,
        right: ValueKind,
    },
    /// `IF` 的兩個分支、`COALESCE` 的參數型態不同。
    BranchMismatch {
        func: &'static str,
        first: ValueKind,
        found: ValueKind,
    },
    /// `ROUND` 的小數位數常數不是 0 到 [`MAX_ROUND_DIGITS`] 的整數。
    InvalidRoundDigits,
    /// 函式參數個數錯誤（解析器已檢查，只會發生在程式自行建構的 AST）。
    WrongArgCount {
        func: &'static str,
        expected: Arity,
        found: usize,
    },
    /// 公式的結果型態無法存成設定的 `formula_result_type`。
    ResultTypeMismatch {
        expected: DataType,
        found: ValueKind,
    },
    /// `formula_result_type` 不支援（`select`、`multi_select`、`relation`、`file`、`formula`）。
    UnsupportedResultType(DataType),
}

impl fmt::Display for TypeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownField(key) => write!(f, "找不到欄位「{key}」"),
            Self::MasterFieldInDetail(key) => {
                write!(f, "明細的公式不能引用主表欄位「{key}」")
            }
            Self::NotRelation(key) => write!(f, "欄位「{key}」不是關聯欄位"),
            Self::UnknownRelationField { relation, field } => {
                write!(f, "找不到關聯欄位「{relation}.{field}」")
            }
            Self::AggregateInDetail { func } => {
                write!(f, "明細的公式不能使用子表彙總 {func}")
            }
            Self::UnknownDetail(key) => write!(f, "找不到明細子表「{key}」"),
            Self::UnknownDetailField { detail, field } => {
                write!(f, "找不到明細欄位「{detail}.{field}」")
            }
            Self::UnsupportedFieldType { field, data_type } => write!(
                f,
                "公式不能使用欄位「{field}」（型態 {}）",
                data_type.as_str()
            ),
            Self::AggregateNotNumber { func, field, found } => write!(
                f,
                "函式 {func} 只能用於數字欄位，但明細欄位「{field}」是{found}"
            ),
            Self::TypeMismatch {
                context,
                expected,
                found,
            } => write!(f, "{context}需要{expected}，但收到{found}"),
            Self::Incomparable { op, left, right } => {
                write!(f, "運算子「{op}」無法比較{left}與{right}")
            }
            Self::BranchMismatch { func, first, found } => write!(
                f,
                "函式 {func} 的結果型態必須一致：前面是{first}，這裡是{found}"
            ),
            Self::InvalidRoundDigits => write!(
                f,
                "函式 ROUND 的小數位數必須是 0 到 {MAX_ROUND_DIGITS} 的整數"
            ),
            Self::WrongArgCount {
                func,
                expected,
                found,
            } => write!(f, "函式 {func} 需要 {expected}參數，但提供了 {found} 個"),
            Self::ResultTypeMismatch { expected, found } => {
                write!(
                    f,
                    "公式的結果是{found}，無法存成 {} 型態",
                    expected.as_str()
                )
            }
            Self::UnsupportedResultType(t) => {
                write!(f, "公式的結果型態不能是 {}", t.as_str())
            }
        }
    }
}

/// 推斷公式的結果型態。回傳值不會是 [`ValueKind::Null`]。
pub fn infer_type(expr: &Expr, schema: &TypeSchema) -> Result<ValueKind, TypeError> {
    Checker { schema }.infer(expr)
}

/// 推斷結果型態，並檢查能存成 `result_type`（公式欄位的 `formula_result_type`）：
///
/// | 推斷的型態 | 可以存成 |
/// |---|---|
/// | 數字 | `integer`、`decimal`、`money` |
/// | 文字 | `text`、`long_text` |
/// | 布林 | `boolean` |
/// | 日期 | `date` |
/// | 日期時間 | `datetime` |
///
/// 結果型態不相容時，錯誤位置是整個公式。
pub fn check_formula(
    expr: &Expr,
    schema: &TypeSchema,
    result_type: DataType,
) -> Result<ValueKind, TypeError> {
    if !matches!(
        result_type,
        DataType::Integer
            | DataType::Decimal
            | DataType::Money
            | DataType::Text
            | DataType::LongText
            | DataType::Boolean
            | DataType::Date
            | DataType::DateTime
    ) {
        return Err(TypeError::new(
            TypeErrorKind::UnsupportedResultType(result_type),
            expr.span,
        ));
    }
    let kind = infer_type(expr, schema)?;
    if kind_of(result_type) != Some(kind) {
        return Err(TypeError::new(
            TypeErrorKind::ResultTypeMismatch {
                expected: result_type,
                found: kind,
            },
            expr.span,
        ));
    }
    Ok(kind)
}

/// 欄位型態在公式中的值種類；`multi_select`、`relation`、`file`、`formula` 為 `None`。
fn kind_of(t: DataType) -> Option<ValueKind> {
    Some(match t {
        DataType::Integer | DataType::Decimal | DataType::Money => ValueKind::Number,
        DataType::Text | DataType::LongText | DataType::Select => ValueKind::Text,
        DataType::Boolean => ValueKind::Bool,
        DataType::Date => ValueKind::Date,
        DataType::DateTime => ValueKind::DateTime,
        DataType::MultiSelect | DataType::Relation | DataType::File | DataType::Formula => {
            return None;
        }
    })
}

struct Checker<'s> {
    schema: &'s TypeSchema,
}

impl Checker<'_> {
    fn infer(&self, expr: &Expr) -> Result<ValueKind, TypeError> {
        let err = |kind| Err(TypeError::new(kind, expr.span));
        match &expr.kind {
            ExprKind::Number(_) => Ok(ValueKind::Number),
            ExprKind::String(_) => Ok(ValueKind::Text),
            ExprKind::Bool(_) => Ok(ValueKind::Bool),
            ExprKind::Field(key) => {
                let t = self
                    .local_field(key)
                    .map_err(|k| TypeError::new(k, expr.span))?;
                field_kind(key, t).map_err(|k| TypeError::new(k, expr.span))
            }
            ExprKind::RelationField { relation, field } => {
                let t = self
                    .local_field(relation)
                    .map_err(|k| TypeError::new(k, expr.span))?;
                if t != DataType::Relation {
                    return err(TypeErrorKind::NotRelation(relation.clone()));
                }
                let Some(t) = self
                    .schema
                    .relations
                    .get(relation)
                    .and_then(|m| m.get(field))
                else {
                    return err(TypeErrorKind::UnknownRelationField {
                        relation: relation.clone(),
                        field: field.clone(),
                    });
                };
                field_kind(&format!("{relation}.{field}"), *t)
                    .map_err(|k| TypeError::new(k, expr.span))
            }
            ExprKind::Aggregate {
                func,
                detail,
                field,
            } => self.aggregate(expr.span, *func, detail, field),
            ExprKind::Unary { op, operand } => match op {
                UnaryOp::Neg => {
                    self.expect(operand, ValueKind::Number, || op_context("-"))?;
                    Ok(ValueKind::Number)
                }
                UnaryOp::Not => {
                    self.expect(operand, ValueKind::Bool, || op_context("NOT"))?;
                    Ok(ValueKind::Bool)
                }
            },
            ExprKind::Binary { op, left, right } => self.binary(expr.span, *op, left, right),
            ExprKind::Call { func, args } => self.call(expr.span, *func, args),
        }
    }

    /// 本筆資料的欄位型態。明細的公式引用主表欄位時，回傳較清楚的錯誤。
    fn local_field(&self, key: &str) -> Result<DataType, TypeErrorKind> {
        if let Some(t) = self.schema.fields.get(key) {
            return Ok(*t);
        }
        match &self.schema.scope {
            Scope::Detail { master_fields } if master_fields.contains(key) => {
                Err(TypeErrorKind::MasterFieldInDetail(key.to_owned()))
            }
            _ => Err(TypeErrorKind::UnknownField(key.to_owned())),
        }
    }

    fn aggregate(
        &self,
        span: Span,
        func: AggregateFunc,
        detail: &str,
        field: &str,
    ) -> Result<ValueKind, TypeError> {
        let err = |kind| Err(TypeError::new(kind, span));
        if matches!(self.schema.scope, Scope::Detail { .. }) {
            return err(TypeErrorKind::AggregateInDetail { func: func.name() });
        }
        let Some(table) = self.schema.details.get(detail) else {
            return err(TypeErrorKind::UnknownDetail(detail.to_owned()));
        };
        let Some(t) = table.get(field) else {
            return err(TypeErrorKind::UnknownDetailField {
                detail: detail.to_owned(),
                field: field.to_owned(),
            });
        };
        let label = format!("{detail}.{field}");
        let kind = field_kind(&label, *t).map_err(|k| TypeError::new(k, span))?;
        if func != AggregateFunc::Count && kind != ValueKind::Number {
            return err(TypeErrorKind::AggregateNotNumber {
                func: func.name(),
                field: label,
                found: kind,
            });
        }
        Ok(ValueKind::Number)
    }

    fn binary(
        &self,
        span: Span,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<ValueKind, TypeError> {
        let context = || op_context(op.symbol());
        match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                self.expect(left, ValueKind::Number, context)?;
                self.expect(right, ValueKind::Number, context)?;
                Ok(ValueKind::Number)
            }
            // 任何值都轉成文字
            BinaryOp::Concat => {
                self.infer(left)?;
                self.infer(right)?;
                Ok(ValueKind::Text)
            }
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => {
                let l = self.infer(left)?;
                let r = self.infer(right)?;
                let ordered = !matches!(op, BinaryOp::Eq | BinaryOp::Ne);
                if l != r || (ordered && l == ValueKind::Bool) {
                    return Err(TypeError::new(
                        TypeErrorKind::Incomparable {
                            op: op.symbol(),
                            left: l,
                            right: r,
                        },
                        span,
                    ));
                }
                Ok(ValueKind::Bool)
            }
            BinaryOp::And | BinaryOp::Or => {
                self.expect(left, ValueKind::Bool, context)?;
                self.expect(right, ValueKind::Bool, context)?;
                Ok(ValueKind::Bool)
            }
        }
    }

    fn call(&self, span: Span, func: Function, args: &[Expr]) -> Result<ValueKind, TypeError> {
        if !func.arity().accepts(args.len()) {
            return Err(TypeError::new(
                TypeErrorKind::WrongArgCount {
                    func: func.name(),
                    expected: func.arity(),
                    found: args.len(),
                },
                span,
            ));
        }
        let ctx = |i: usize| move || arg_context(func, i + 1);
        match func {
            Function::If => {
                self.expect(&args[0], ValueKind::Bool, ctx(0))?;
                self.same_type(func, &args[1..])
            }
            Function::Round => {
                self.expect(&args[0], ValueKind::Number, ctx(0))?;
                self.expect(&args[1], ValueKind::Number, ctx(1))?;
                if let ExprKind::Number(n) = &args[1].kind {
                    let valid = n.fract().is_zero()
                        && !n.is_sign_negative()
                        && *n <= MAX_ROUND_DIGITS.into();
                    if !valid {
                        return Err(TypeError::new(
                            TypeErrorKind::InvalidRoundDigits,
                            args[1].span,
                        ));
                    }
                }
                Ok(ValueKind::Number)
            }
            Function::Abs => {
                self.expect(&args[0], ValueKind::Number, ctx(0))?;
                Ok(ValueKind::Number)
            }
            Function::Today => Ok(ValueKind::Date),
            Function::Days => {
                for (i, a) in args.iter().enumerate() {
                    let k = self.infer(a)?;
                    if !matches!(k, ValueKind::Date | ValueKind::DateTime) {
                        return Err(TypeError::new(
                            TypeErrorKind::TypeMismatch {
                                context: arg_context(func, i + 1),
                                expected: "日期",
                                found: k,
                            },
                            a.span,
                        ));
                    }
                }
                Ok(ValueKind::Number)
            }
            Function::Concat => {
                for a in args {
                    self.infer(a)?;
                }
                Ok(ValueKind::Text)
            }
            Function::Coalesce => self.same_type(func, args),
        }
    }

    /// 所有參數的型態都要相同，回傳該型態。錯誤位置是第一個不同的參數。
    fn same_type(&self, func: Function, args: &[Expr]) -> Result<ValueKind, TypeError> {
        let mut first = None;
        for a in args {
            let k = self.infer(a)?;
            match first {
                None => first = Some(k),
                Some(f) if f != k => {
                    return Err(TypeError::new(
                        TypeErrorKind::BranchMismatch {
                            func: func.name(),
                            first: f,
                            found: k,
                        },
                        a.span,
                    ));
                }
                Some(_) => {}
            }
        }
        // 參數個數已檢查（IF 有兩個分支、COALESCE 至少一個）
        Ok(first.unwrap_or(ValueKind::Null))
    }

    /// 推斷 `expr` 的型態，必須是 `expected`；錯誤位置是 `expr`。
    fn expect(
        &self,
        expr: &Expr,
        expected: ValueKind,
        context: impl FnOnce() -> String,
    ) -> Result<(), TypeError> {
        let found = self.infer(expr)?;
        if found == expected {
            return Ok(());
        }
        Err(TypeError::new(
            TypeErrorKind::TypeMismatch {
                context: context(),
                expected: expected.name(),
                found,
            },
            expr.span,
        ))
    }
}

/// 欄位型態轉成值種類；公式不支援的型態回傳錯誤。
fn field_kind(label: &str, t: DataType) -> Result<ValueKind, TypeErrorKind> {
    kind_of(t).ok_or_else(|| TypeErrorKind::UnsupportedFieldType {
        field: label.to_owned(),
        data_type: t,
    })
}

fn arg_context(func: Function, position: usize) -> String {
    format!("函式 {} 的第 {position} 個參數", func.name())
}

fn op_context(symbol: &str) -> String {
    format!("運算子「{symbol}」")
}
