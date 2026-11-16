//! 詞法分析：公式文字 → token 序列。位置一律以字元位移計算。

use hammer_shared::decimal::Decimal;
use hammer_shared::schema::key::validate_key;

use crate::ast::Span;
use crate::error::{ParseError, ParseErrorKind};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    Number(Decimal),
    Str(String),
    /// `{field_key}`
    Field(String),
    /// `{relation_field.field_key}`
    RelationField(String, String),
    /// 函式名稱或關鍵字（`TRUE`、`AND`…），也用於彙總參數的明細子表代碼與欄位代碼。
    Ident(String),
    Dot,
    LParen,
    RParen,
    Comma,
    Plus,
    Minus,
    Star,
    Slash,
    Amp,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    /// 錯誤訊息中顯示的文字。
    pub fn describe(&self, chars: &[char]) -> String {
        chars[self.span.start..self.span.end].iter().collect()
    }
}

pub(crate) fn tokenize(chars: &[char]) -> Result<Vec<Token>, ParseError> {
    let mut lexer = Lexer { chars, pos: 0 };
    let mut tokens = Vec::new();
    loop {
        let token = lexer.next_token()?;
        let eof = token.kind == TokenKind::Eof;
        tokens.push(token);
        if eof {
            return Ok(tokens);
        }
    }
}

struct Lexer<'a> {
    chars: &'a [char],
    pos: usize,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn next_token(&mut self) -> Result<Token, ParseError> {
        while self.peek().is_some_and(char::is_whitespace) {
            self.pos += 1;
        }
        let start = self.pos;
        let Some(c) = self.peek() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                span: Span::new(start, start),
            });
        };

        let kind = match c {
            '0'..='9' => return self.number(),
            '"' => return self.string(),
            '{' => return self.field_ref(),
            c if c.is_ascii_alphabetic() || c == '_' => return Ok(self.ident()),
            '.' => TokenKind::Dot,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            ',' => TokenKind::Comma,
            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '&' => TokenKind::Amp,
            '=' => TokenKind::Eq,
            '<' => match self.peek_at(1) {
                Some('>') => {
                    self.pos += 1;
                    TokenKind::Ne
                }
                Some('=') => {
                    self.pos += 1;
                    TokenKind::Le
                }
                _ => TokenKind::Lt,
            },
            '>' => match self.peek_at(1) {
                Some('=') => {
                    self.pos += 1;
                    TokenKind::Ge
                }
                _ => TokenKind::Gt,
            },
            other => {
                return Err(ParseError::new(
                    ParseErrorKind::UnexpectedChar(other),
                    Span::new(start, start + 1),
                ));
            }
        };
        self.pos += 1;
        Ok(Token {
            kind,
            span: Span::new(start, self.pos),
        })
    }

    /// `123`、`0.05`。小數點後面沒有數字時，小數點不算在數字內（`1.` 會解析失敗）。
    fn number(&mut self) -> Result<Token, ParseError> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let span = Span::new(start, self.pos);
        let text: String = self.chars[start..self.pos].iter().collect();
        // 直接由十進位文字轉成 Decimal，不經過浮點數；超過精度或範圍視為錯誤
        let value = Decimal::from_str_exact(&text)
            .map_err(|_| ParseError::new(ParseErrorKind::InvalidNumber, span))?;
        Ok(Token {
            kind: TokenKind::Number(value),
            span,
        })
    }

    fn string(&mut self) -> Result<Token, ParseError> {
        let start = self.pos;
        self.pos += 1; // 開頭的 "
        let mut value = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::new(
                        ParseErrorKind::UnclosedString,
                        Span::new(start, self.pos),
                    ));
                }
                Some('"') => {
                    self.pos += 1;
                    return Ok(Token {
                        kind: TokenKind::Str(value),
                        span: Span::new(start, self.pos),
                    });
                }
                Some('\\') => {
                    let escape_start = self.pos;
                    match self.peek_at(1) {
                        Some(c @ ('"' | '\\')) => {
                            value.push(c);
                            self.pos += 2;
                        }
                        other => {
                            let end = escape_start + if other.is_some() { 2 } else { 1 };
                            return Err(ParseError::new(
                                ParseErrorKind::InvalidEscape(other),
                                Span::new(escape_start, end),
                            ));
                        }
                    }
                }
                Some(c) => {
                    value.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// `{field_key}` 或 `{relation_field.field_key}`。
    fn field_ref(&mut self) -> Result<Token, ParseError> {
        let start = self.pos;
        self.pos += 1; // {
        let content_start = self.pos;
        while self.peek().is_some_and(|c| c != '}') {
            self.pos += 1;
        }
        if self.peek().is_none() {
            return Err(ParseError::new(
                ParseErrorKind::UnclosedFieldRef,
                Span::new(start, start + 1),
            ));
        }
        let content_end = self.pos;
        self.pos += 1; // }
        let span = Span::new(start, self.pos);

        let content: String = self.chars[content_start..content_end].iter().collect();
        let parts: Vec<&str> = content.split('.').collect();
        if parts.len() > 2 {
            return Err(ParseError::new(ParseErrorKind::NestedRelation, span));
        }
        for part in &parts {
            validate_key(part).map_err(|reason| {
                ParseError::new(
                    ParseErrorKind::InvalidKey {
                        key: (*part).to_owned(),
                        reason,
                    },
                    span,
                )
            })?;
        }
        let kind = match parts.as_slice() {
            [field] => TokenKind::Field((*field).to_owned()),
            [relation, field] => {
                TokenKind::RelationField((*relation).to_owned(), (*field).to_owned())
            }
            _ => unreachable!("split 至少產生一段，且上面已排除超過兩段"),
        };
        Ok(Token { kind, span })
    }

    fn ident(&mut self) -> Token {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            self.pos += 1;
        }
        Token {
            kind: TokenKind::Ident(self.chars[start..self.pos].iter().collect()),
            span: Span::new(start, self.pos),
        }
    }
}
