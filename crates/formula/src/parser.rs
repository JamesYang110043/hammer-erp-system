//! 語法解析：token 序列 → AST。遞迴下降，每一層優先順序一個函式。

use hammer_shared::schema::key::validate_key;

use crate::ast::{
    AggregateFunc, BinaryOp, Expr, ExprKind, FunctionName, Span, UnaryOp, lookup_function,
};
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::{Token, TokenKind, tokenize};

/// 公式長度上限（字元數）。
pub const MAX_FORMULA_LEN: usize = 2000;

/// 巢狀深度上限：括號、函式參數、`NOT`、一元負號每一層各算一層。
pub const MAX_DEPTH: usize = 50;

/// 解析公式文字。
pub fn parse(source: &str) -> Result<Expr, ParseError> {
    let chars: Vec<char> = source.chars().collect();
    if chars.len() > MAX_FORMULA_LEN {
        return Err(ParseError::new(
            ParseErrorKind::TooLong {
                max: MAX_FORMULA_LEN,
            },
            Span::new(MAX_FORMULA_LEN, chars.len()),
        ));
    }
    if chars.iter().all(|c| c.is_whitespace()) {
        return Err(ParseError::new(
            ParseErrorKind::Empty,
            Span::new(0, chars.len()),
        ));
    }

    let tokens = tokenize(&chars)?;
    let mut parser = Parser {
        chars: &chars,
        tokens,
        pos: 0,
        depth: 0,
    };
    let expr = parser.expr()?;
    let next = parser.peek();
    if next.kind != TokenKind::Eof {
        return Err(parser.unexpected(next.clone()));
    }
    Ok(expr)
}

struct Parser<'a> {
    chars: &'a [char],
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        // tokenize 保證最後一個是 Eof，且不會越過 Eof
        &self.tokens[self.pos]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
        token
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(name) if name.eq_ignore_ascii_case(keyword))
    }

    fn unexpected(&self, token: Token) -> ParseError {
        if token.kind == TokenKind::Eof {
            ParseError::new(ParseErrorKind::UnexpectedEnd, token.span)
        } else {
            ParseError::new(
                ParseErrorKind::UnexpectedToken {
                    found: token.describe(self.chars),
                },
                token.span,
            )
        }
    }

    /// 進入一層巢狀；超過上限時回傳錯誤，避免遞迴過深。
    fn enter(&mut self, span: Span) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ParseError::new(
                ParseErrorKind::TooDeep { max: MAX_DEPTH },
                span,
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn expr(&mut self) -> Result<Expr, ParseError> {
        self.or()
    }

    /// 左結合的二元運算層。
    fn binary_level(
        &mut self,
        next: fn(&mut Self) -> Result<Expr, ParseError>,
        op_of: fn(&Self) -> Option<BinaryOp>,
    ) -> Result<Expr, ParseError> {
        let mut left = next(self)?;
        while let Some(op) = op_of(self) {
            self.advance();
            let right = next(self)?;
            let span = left.span.to(right.span);
            left = Expr::new(
                ExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }
        Ok(left)
    }

    fn or(&mut self) -> Result<Expr, ParseError> {
        self.binary_level(Self::and, |p| p.peek_keyword("OR").then_some(BinaryOp::Or))
    }

    fn and(&mut self) -> Result<Expr, ParseError> {
        self.binary_level(Self::not, |p| {
            p.peek_keyword("AND").then_some(BinaryOp::And)
        })
    }

    fn not(&mut self) -> Result<Expr, ParseError> {
        if !self.peek_keyword("NOT") {
            return self.comparison();
        }
        let op_token = self.advance();
        self.enter(op_token.span)?;
        let operand = self.not()?;
        self.leave();
        let span = op_token.span.to(operand.span);
        Ok(Expr::new(
            ExprKind::Unary {
                op: UnaryOp::Not,
                operand: Box::new(operand),
            },
            span,
        ))
    }

    fn comparison(&mut self) -> Result<Expr, ParseError> {
        self.binary_level(Self::concat, |p| match p.peek().kind {
            TokenKind::Eq => Some(BinaryOp::Eq),
            TokenKind::Ne => Some(BinaryOp::Ne),
            TokenKind::Lt => Some(BinaryOp::Lt),
            TokenKind::Le => Some(BinaryOp::Le),
            TokenKind::Gt => Some(BinaryOp::Gt),
            TokenKind::Ge => Some(BinaryOp::Ge),
            _ => None,
        })
    }

    fn concat(&mut self) -> Result<Expr, ParseError> {
        self.binary_level(Self::additive, |p| {
            (p.peek().kind == TokenKind::Amp).then_some(BinaryOp::Concat)
        })
    }

    fn additive(&mut self) -> Result<Expr, ParseError> {
        self.binary_level(Self::multiplicative, |p| match p.peek().kind {
            TokenKind::Plus => Some(BinaryOp::Add),
            TokenKind::Minus => Some(BinaryOp::Sub),
            _ => None,
        })
    }

    fn multiplicative(&mut self) -> Result<Expr, ParseError> {
        self.binary_level(Self::unary, |p| match p.peek().kind {
            TokenKind::Star => Some(BinaryOp::Mul),
            TokenKind::Slash => Some(BinaryOp::Div),
            _ => None,
        })
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        if self.peek().kind != TokenKind::Minus {
            return self.primary();
        }
        let op_token = self.advance();
        self.enter(op_token.span)?;
        let operand = self.unary()?;
        self.leave();
        let span = op_token.span.to(operand.span);
        Ok(Expr::new(
            ExprKind::Unary {
                op: UnaryOp::Neg,
                operand: Box::new(operand),
            },
            span,
        ))
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let token = self.advance();
        let kind = match token.kind {
            TokenKind::Number(n) => ExprKind::Number(n),
            TokenKind::Str(ref s) => ExprKind::String(s.clone()),
            TokenKind::Field(ref key) => ExprKind::Field(key.clone()),
            TokenKind::RelationField(ref relation, ref field) => ExprKind::RelationField {
                relation: relation.clone(),
                field: field.clone(),
            },
            TokenKind::LParen => return self.parenthesized(token),
            TokenKind::Ident(ref name) => return self.ident(name.clone(), token.span),
            _ => return Err(self.unexpected(token)),
        };
        Ok(Expr::new(kind, token.span))
    }

    fn parenthesized(&mut self, open: Token) -> Result<Expr, ParseError> {
        self.enter(open.span)?;
        let mut inner = self.expr()?;
        self.leave();
        self.expect_close(&open)?;
        // 括號只影響結合，不留在 AST；span 擴大到含括號，方便錯誤標示
        inner.span = open.span.to(self.tokens[self.pos - 1].span);
        Ok(inner)
    }

    /// 期待 `)`；遇到公式結尾時回報「括號沒有閉合」，位置指向對應的 `(`。
    fn expect_close(&mut self, open: &Token) -> Result<Token, ParseError> {
        let token = self.advance();
        match token.kind {
            TokenKind::RParen => Ok(token),
            TokenKind::Eof => Err(ParseError::new(ParseErrorKind::UnclosedParen, open.span)),
            _ => Err(self.unexpected(token)),
        }
    }

    fn ident(&mut self, name: String, span: Span) -> Result<Expr, ParseError> {
        if name.eq_ignore_ascii_case("TRUE") {
            return Ok(Expr::new(ExprKind::Bool(true), span));
        }
        if name.eq_ignore_ascii_case("FALSE") {
            return Ok(Expr::new(ExprKind::Bool(false), span));
        }
        if ["AND", "OR", "NOT"]
            .iter()
            .any(|k| name.eq_ignore_ascii_case(k))
        {
            return Err(ParseError::new(
                ParseErrorKind::UnexpectedToken { found: name },
                span,
            ));
        }
        if self.peek().kind != TokenKind::LParen {
            return Err(ParseError::new(ParseErrorKind::BareIdentifier(name), span));
        }
        let Some(func) = lookup_function(&name) else {
            return Err(ParseError::new(ParseErrorKind::UnknownFunction(name), span));
        };
        let open = self.advance();
        match func {
            FunctionName::Aggregate(func) => self.aggregate(func, span, &open),
            FunctionName::Plain(func) => {
                self.enter(open.span)?;
                let args = self.arguments(&open)?;
                self.leave();
                let span = span.to(self.tokens[self.pos - 1].span);
                if !func.arity().accepts(args.len()) {
                    return Err(ParseError::new(
                        ParseErrorKind::WrongArgCount {
                            func: func.name(),
                            expected: func.arity(),
                            found: args.len(),
                        },
                        span,
                    ));
                }
                Ok(Expr::new(ExprKind::Call { func, args }, span))
            }
        }
    }

    /// 以逗號分隔的參數，直到 `)`（已消耗 `(`）。
    fn arguments(&mut self, open: &Token) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if self.peek().kind == TokenKind::RParen {
            self.advance();
            return Ok(args);
        }
        loop {
            args.push(self.expr()?);
            if self.peek().kind == TokenKind::Comma {
                self.advance();
                continue;
            }
            self.expect_close(open)?;
            return Ok(args);
        }
    }

    /// `SUM(detail.field)` 等子表彙總（已消耗 `(`）。參數只能是一個「明細子表代碼.欄位代碼」。
    fn aggregate(
        &mut self,
        func: AggregateFunc,
        name_span: Span,
        open: &Token,
    ) -> Result<Expr, ParseError> {
        let invalid = |span: Span| {
            ParseError::new(
                ParseErrorKind::InvalidAggregateArg { func: func.name() },
                span,
            )
        };

        let detail = self.advance();
        let dot = self.advance();
        let field = self.advance();
        let (TokenKind::Ident(detail_key), TokenKind::Dot, TokenKind::Ident(field_key)) =
            (&detail.kind, &dot.kind, &field.kind)
        else {
            // 錯誤範圍標到 `)` 之前；先遇到公式結尾則是括號沒有閉合
            let mut span = detail.span;
            for token in [&detail, &dot, &field] {
                match token.kind {
                    TokenKind::Eof => {
                        return Err(ParseError::new(ParseErrorKind::UnclosedParen, open.span));
                    }
                    TokenKind::RParen => break,
                    _ => span = span.to(token.span),
                }
            }
            return Err(invalid(span));
        };
        let arg_span = detail.span.to(field.span);
        // 「.」前後不可有空白
        if detail.span.end != dot.span.start || dot.span.end != field.span.start {
            return Err(invalid(arg_span));
        }
        for key in [detail_key, field_key] {
            validate_key(key).map_err(|reason| {
                ParseError::new(
                    ParseErrorKind::InvalidKey {
                        key: key.clone(),
                        reason,
                    },
                    arg_span,
                )
            })?;
        }

        match self.peek().kind {
            TokenKind::RParen => {}
            TokenKind::Eof => {
                return Err(ParseError::new(ParseErrorKind::UnclosedParen, open.span));
            }
            _ => return Err(invalid(arg_span.to(self.peek().span))),
        }
        let close = self.advance();
        Ok(Expr::new(
            ExprKind::Aggregate {
                func,
                detail: detail_key.clone(),
                field: field_key.clone(),
            },
            name_span.to(close.span),
        ))
    }
}
