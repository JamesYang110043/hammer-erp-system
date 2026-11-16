//! 計算：依 crate 文件「計算」一節的語意，對 [`Expr`] 求值。

use std::cmp::Ordering;

use hammer_shared::decimal::Decimal;
use hammer_shared::schema::DataType;
use hammer_shared::time::{Date, TAIPEI_OFFSET};
use rust_decimal::RoundingStrategy;
use rust_decimal::prelude::ToPrimitive;

use crate::ast::{AggregateFunc, BinaryOp, Expr, ExprKind, Function, UnaryOp};
use crate::env::{Environment, FieldData, json_to_value};
use crate::error::EvalError;
use crate::value::{Value, ValueKind};

/// `ROUND` 小數位數的上限（`Decimal` 最多 28 位小數）。
pub const MAX_ROUND_DIGITS: u32 = 28;

/// 計算公式。失敗時公式的結果為空值，錯誤原因見 [`EvalError`]；不會 panic。
pub fn evaluate(expr: &Expr, env: &dyn Environment) -> Result<Value, EvalError> {
    Evaluator { env }.eval(expr)
}

struct Evaluator<'e> {
    env: &'e dyn Environment,
}

impl Evaluator<'_> {
    fn eval(&self, expr: &Expr) -> Result<Value, EvalError> {
        match &expr.kind {
            ExprKind::Number(n) => Ok(Value::Number(*n)),
            ExprKind::String(s) => Ok(Value::Text(s.clone())),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Field(key) => {
                let data = self
                    .env
                    .field(key)
                    .ok_or_else(|| EvalError::UnknownField(key.clone()))?;
                json_to_value(key, data)
            }
            ExprKind::RelationField { relation, field } => {
                let data = self.env.relation_field(relation, field).ok_or_else(|| {
                    EvalError::UnknownRelationField {
                        relation: relation.clone(),
                        field: field.clone(),
                    }
                })?;
                json_to_value(&format!("{relation}.{field}"), data)
            }
            ExprKind::Aggregate {
                func,
                detail,
                field,
            } => self.aggregate(*func, detail, field),
            ExprKind::Unary { op, operand } => {
                let v = self.eval(operand)?;
                unary(*op, v)
            }
            ExprKind::Binary { op, left, right } => self.binary(*op, left, right),
            ExprKind::Call { func, args } => self.call(*func, args),
        }
    }

    fn binary(&self, op: BinaryOp, left: &Expr, right: &Expr) -> Result<Value, EvalError> {
        let l = self.eval(left)?;
        let r = self.eval(right)?;
        match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => arithmetic(op, l, r),
            BinaryOp::Concat => Ok(Value::Text(l.to_text() + &r.to_text())),
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => compare(op, &l, &r),
            BinaryOp::And | BinaryOp::Or => logical(op, l, r),
        }
    }

    fn call(&self, func: Function, args: &[Expr]) -> Result<Value, EvalError> {
        match func {
            Function::If => {
                // 只計算選到的分支：IF({qty} = 0, 0, {total} / {qty}) 不會除以零
                let cond = self.eval(arg(args, 0)?)?;
                let branch = match cond {
                    Value::Bool(true) => 1,
                    Value::Bool(false) | Value::Null => 2,
                    other => return Err(mismatch(arg_context(func, 1), "布林", &other)),
                };
                self.eval(arg(args, branch)?)
            }
            Function::Round => {
                let x = self.eval(arg(args, 0)?)?;
                let n = self.eval(arg(args, 1)?)?;
                let (x, n) = match (x, n) {
                    (Value::Null, _) | (_, Value::Null) => return Ok(Value::Null),
                    (x, n) => (
                        number(x, || arg_context(func, 1))?,
                        number(n, || arg_context(func, 2))?,
                    ),
                };
                let digits = round_digits(n)?;
                Ok(Value::Number(x.round_dp_with_strategy(
                    digits,
                    RoundingStrategy::MidpointAwayFromZero,
                )))
            }
            Function::Abs => match self.eval(arg(args, 0)?)? {
                Value::Null => Ok(Value::Null),
                v => Ok(Value::Number(number(v, || arg_context(func, 1))?.abs())),
            },
            Function::Today => Ok(Value::Date(self.env.today())),
            Function::Days => {
                let a = self.eval(arg(args, 0)?)?;
                let b = self.eval(arg(args, 1)?)?;
                let (a, b) = match (a, b) {
                    (Value::Null, _) | (_, Value::Null) => return Ok(Value::Null),
                    (a, b) => (
                        date(a, || arg_context(func, 1))?,
                        date(b, || arg_context(func, 2))?,
                    ),
                };
                Ok(Value::Number((a - b).whole_days().into()))
            }
            Function::Concat => {
                let mut out = String::new();
                for a in args {
                    out.push_str(&self.eval(a)?.to_text());
                }
                Ok(Value::Text(out))
            }
            Function::Coalesce => {
                // 依序計算，遇到第一個非空值就停止
                for a in args {
                    let v = self.eval(a)?;
                    if !v.is_null() {
                        return Ok(v);
                    }
                }
                Ok(Value::Null)
            }
        }
    }

    fn aggregate(
        &self,
        func: AggregateFunc,
        detail: &str,
        field: &str,
    ) -> Result<Value, EvalError> {
        let column =
            self.env
                .detail_column(detail, field)
                .ok_or_else(|| EvalError::UnknownDetailField {
                    detail: detail.to_owned(),
                    field: field.to_owned(),
                })?;
        let label = format!("{detail}.{field}");
        let mut values = Vec::with_capacity(column.values.len());
        for json in &column.values {
            values.push(json_to_value(
                &label,
                FieldData::new(column.data_type, json),
            )?);
        }
        if func == AggregateFunc::Count {
            return Ok(Value::Number(values.len().into()));
        }

        let context = || format!("函式 {} 的明細欄位「{label}」", func.name());
        // 依欄位型態檢查，沒有明細時也一樣
        if !matches!(
            column.data_type,
            DataType::Integer | DataType::Decimal | DataType::Money
        ) {
            return Err(EvalError::TypeMismatch {
                context: context(),
                expected: "數字",
                found: values
                    .iter()
                    .find(|v| !v.is_null())
                    .map_or(ValueKind::Null, Value::kind),
            });
        }
        let mut numbers = Vec::with_capacity(values.len());
        for v in values {
            if !v.is_null() {
                numbers.push(number(v, context)?);
            }
        }

        match func {
            AggregateFunc::Sum => sum(&numbers).map(Value::Number),
            AggregateFunc::Avg => {
                if numbers.is_empty() {
                    return Ok(Value::Null);
                }
                let total = sum(&numbers)?;
                total
                    .checked_div(Decimal::from(numbers.len()))
                    .map(Value::Number)
                    .ok_or(EvalError::Overflow)
            }
            AggregateFunc::Min => Ok(numbers.into_iter().min().map_or(Value::Null, Value::Number)),
            AggregateFunc::Max => Ok(numbers.into_iter().max().map_or(Value::Null, Value::Number)),
            AggregateFunc::Count => Ok(Value::Null), // 已在上面處理
        }
    }
}

/// 解析器已檢查參數個數；這裡仍以錯誤取代 panic。
fn arg(args: &[Expr], i: usize) -> Result<&Expr, EvalError> {
    args.get(i).ok_or(EvalError::TypeMismatch {
        context: "函式參數".to_owned(),
        expected: "值",
        found: ValueKind::Null,
    })
}

fn arg_context(func: Function, position: usize) -> String {
    format!("函式 {} 的第 {position} 個參數", func.name())
}

fn op_context(symbol: &str) -> String {
    format!("運算子「{symbol}」")
}

fn mismatch(context: String, expected: &'static str, found: &Value) -> EvalError {
    EvalError::TypeMismatch {
        context,
        expected,
        found: found.kind(),
    }
}

fn number(v: Value, context: impl FnOnce() -> String) -> Result<Decimal, EvalError> {
    match v {
        Value::Number(n) => Ok(n),
        other => Err(mismatch(context(), "數字", &other)),
    }
}

/// 日期；日期時間以 Asia/Taipei 的日期計算（D13）。
fn date(v: Value, context: impl FnOnce() -> String) -> Result<Date, EvalError> {
    match v {
        Value::Date(d) => Ok(d),
        Value::DateTime(dt) => Ok(dt.to_offset(TAIPEI_OFFSET).date()),
        other => Err(mismatch(context(), "日期", &other)),
    }
}

fn round_digits(n: Decimal) -> Result<u32, EvalError> {
    if !n.fract().is_zero() {
        return Err(EvalError::InvalidRoundDigits);
    }
    n.to_u32()
        .filter(|d| *d <= MAX_ROUND_DIGITS)
        .ok_or(EvalError::InvalidRoundDigits)
}

fn sum(numbers: &[Decimal]) -> Result<Decimal, EvalError> {
    numbers.iter().try_fold(Decimal::ZERO, |acc, n| {
        acc.checked_add(*n).ok_or(EvalError::Overflow)
    })
}

fn unary(op: UnaryOp, v: Value) -> Result<Value, EvalError> {
    match (op, v) {
        (_, Value::Null) => Ok(Value::Null),
        (UnaryOp::Neg, Value::Number(n)) => Ok(Value::Number(-n)),
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnaryOp::Neg, other) => Err(mismatch(op_context("-"), "數字", &other)),
        (UnaryOp::Not, other) => Err(mismatch(op_context("NOT"), "布林", &other)),
    }
}

fn arithmetic(op: BinaryOp, l: Value, r: Value) -> Result<Value, EvalError> {
    let context = || op_context(op.symbol());
    // 型態不符優先於空值：`"a" * {空值}` 仍是錯誤
    for v in [&l, &r] {
        if !matches!(v, Value::Null | Value::Number(_)) {
            return Err(mismatch(context(), "數字", v));
        }
    }
    let (Value::Number(a), Value::Number(b)) = (l, r) else {
        return Ok(Value::Null);
    };
    let result = match op {
        BinaryOp::Add => a.checked_add(b),
        BinaryOp::Sub => a.checked_sub(b),
        BinaryOp::Mul => a.checked_mul(b),
        _ => {
            if b.is_zero() {
                return Err(EvalError::DivisionByZero);
            }
            a.checked_div(b)
        }
    };
    result.map(Value::Number).ok_or(EvalError::Overflow)
}

fn compare(op: BinaryOp, l: &Value, r: &Value) -> Result<Value, EvalError> {
    let ordering = match (l, r) {
        (Value::Null, _) | (_, Value::Null) => return Ok(Value::Null),
        (Value::Number(a), Value::Number(b)) => a.cmp(b),
        (Value::Text(a), Value::Text(b)) => a.cmp(b),
        (Value::Date(a), Value::Date(b)) => a.cmp(b),
        (Value::DateTime(a), Value::DateTime(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) if matches!(op, BinaryOp::Eq | BinaryOp::Ne) => a.cmp(b),
        _ => {
            return Err(EvalError::Incomparable {
                op: op.symbol(),
                left: l.kind(),
                right: r.kind(),
            });
        }
    };
    let result = match op {
        BinaryOp::Eq => ordering == Ordering::Equal,
        BinaryOp::Ne => ordering != Ordering::Equal,
        BinaryOp::Lt => ordering == Ordering::Less,
        BinaryOp::Le => ordering != Ordering::Greater,
        BinaryOp::Gt => ordering == Ordering::Greater,
        _ => ordering != Ordering::Less,
    };
    Ok(Value::Bool(result))
}

/// 三值邏輯：`FALSE AND 空值` 為 `FALSE`、`TRUE OR 空值` 為 `TRUE`，其餘有空值時為空值。
fn logical(op: BinaryOp, l: Value, r: Value) -> Result<Value, EvalError> {
    let to_bool = |v: &Value| match v {
        Value::Bool(b) => Ok(Some(*b)),
        Value::Null => Ok(None),
        other => Err(mismatch(op_context(op.symbol()), "布林", other)),
    };
    let (a, b) = (to_bool(&l)?, to_bool(&r)?);
    let result = if op == BinaryOp::And {
        match (a, b) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        }
    } else {
        match (a, b) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        }
    };
    Ok(result.map_or(Value::Null, Value::Bool))
}
