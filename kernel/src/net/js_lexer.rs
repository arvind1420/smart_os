//! JavaScript Lexer — Phase 38 (part 1/3)

#![allow(dead_code)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Number(f64),
    Str(String),
    Bool(bool),
    Null,
    Undefined,
    Template(String),   // `...` (no substitutions for simplicity)

    // Identifiers & keywords
    Ident(String),
    Var, Let, Const,
    Function, Return, Arrow,   // =>
    If, Else,
    While, For, In, Of,
    Break, Continue,
    New, This, Delete,
    Typeof, Instanceof,
    Class, Extends, Super,
    Import, Export, Default,
    Throw, Try, Catch, Finally,

    // Punctuation
    LParen, RParen,
    LBrace, RBrace,
    LBracket, RBracket,
    Semi, Comma, Dot, DotDotDot, Question, Colon,
    Optional,   // ?.

    // Arithmetic
    Plus, Minus, Star, Slash, Percent, StarStar,
    // Bitwise
    Amp, Pipe, Caret, Tilde, LShift, RShift, URShift,
    // Logical
    AmpAmp, PipePipe, Bang, QuestionQuestion,
    // Comparison
    Eq, NotEq, EqEq, NotEqEq,
    Lt, Gt, LtEq, GtEq,
    // Assignment
    Assign,
    PlusAssign, MinusAssign, StarAssign, SlashAssign, PercentAssign,
    AmpAssign, PipeAssign, CaretAssign,
    // Increment / Decrement
    PlusPlus, MinusMinus,

    Eof,
}

pub struct Lexer<'a> {
    src:  &'a [u8],
    pos:  usize,
    pub line: u32,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer { src: src.as_bytes(), pos: 0, line: 1 }
    }

    fn peek(&self) -> u8     { if self.pos < self.src.len() { self.src[self.pos] } else { 0 } }
    fn peek2(&self) -> u8    { if self.pos+1 < self.src.len() { self.src[self.pos+1] } else { 0 } }
    fn peek3(&self) -> u8    { if self.pos+2 < self.src.len() { self.src[self.pos+2] } else { 0 } }
    fn advance(&mut self) -> u8 {
        let c = self.peek();
        if c == b'\n' { self.line += 1; }
        self.pos += 1;
        c
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == c { self.advance(); true } else { false }
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Whitespace
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
                self.advance();
            }
            // // comment
            if self.peek() == b'/' && self.peek2() == b'/' {
                while self.pos < self.src.len() && self.peek() != b'\n' { self.advance(); }
                continue;
            }
            // /* comment */
            if self.peek() == b'/' && self.peek2() == b'*' {
                self.pos += 2;
                while self.pos + 1 < self.src.len() {
                    if self.src[self.pos] == b'*' && self.src[self.pos+1] == b'/' {
                        self.pos += 2; break;
                    }
                    self.advance();
                }
                continue;
            }
            break;
        }
    }

    fn read_string(&mut self, quote: u8) -> Token {
        let mut s = String::new();
        loop {
            let c = self.advance();
            if c == 0 || c == quote { break; }
            if c == b'\\' {
                let esc = self.advance();
                match esc {
                    b'n'  => s.push('\n'),
                    b'r'  => s.push('\r'),
                    b't'  => s.push('\t'),
                    b'\'' => s.push('\''),
                    b'"'  => s.push('"'),
                    b'\\' => s.push('\\'),
                    _     => { s.push('\\'); s.push(esc as char); }
                }
            } else {
                s.push(c as char);
            }
        }
        Token::Str(s)
    }

    fn read_template(&mut self) -> Token {
        // Back-tick string, no ${ } interpolation — treat as plain string
        let mut s = String::new();
        loop {
            let c = self.advance();
            if c == 0 || c == b'`' { break; }
            if c == b'\\' { let esc = self.advance(); s.push('\\'); s.push(esc as char); }
            else { s.push(c as char); }
        }
        Token::Template(s)
    }

    fn read_number(&mut self, first: u8) -> Token {
        let mut s = String::new();
        s.push(first as char);
        // Hex
        if first == b'0' && (self.peek() == b'x' || self.peek() == b'X') {
            s.push(self.advance() as char);
            while self.peek().is_ascii_hexdigit() { s.push(self.advance() as char); }
            let v = u64::from_str_radix(&s[2..], 16).unwrap_or(0);
            return Token::Number(v as f64);
        }
        while self.peek().is_ascii_digit() { s.push(self.advance() as char); }
        if self.peek() == b'.' && self.peek2().is_ascii_digit() {
            s.push(self.advance() as char);
            while self.peek().is_ascii_digit() { s.push(self.advance() as char); }
        }
        if self.peek() == b'e' || self.peek() == b'E' {
            s.push(self.advance() as char);
            if self.peek() == b'+' || self.peek() == b'-' { s.push(self.advance() as char); }
            while self.peek().is_ascii_digit() { s.push(self.advance() as char); }
        }
        let v: f64 = s.parse().unwrap_or(0.0);
        Token::Number(v)
    }

    fn read_ident(&mut self, first: u8) -> Token {
        let mut s = String::new();
        s.push(first as char);
        while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' || self.peek() == b'$' {
            s.push(self.advance() as char);
        }
        match s.as_str() {
            "var"        => Token::Var,
            "let"        => Token::Let,
            "const"      => Token::Const,
            "function"   => Token::Function,
            "return"     => Token::Return,
            "if"         => Token::If,
            "else"       => Token::Else,
            "while"      => Token::While,
            "for"        => Token::For,
            "in"         => Token::In,
            "of"         => Token::Of,
            "break"      => Token::Break,
            "continue"   => Token::Continue,
            "new"        => Token::New,
            "this"       => Token::This,
            "delete"     => Token::Delete,
            "typeof"     => Token::Typeof,
            "instanceof" => Token::Instanceof,
            "class"      => Token::Class,
            "extends"    => Token::Extends,
            "super"      => Token::Super,
            "import"     => Token::Import,
            "export"     => Token::Export,
            "default"    => Token::Default,
            "throw"      => Token::Throw,
            "try"        => Token::Try,
            "catch"      => Token::Catch,
            "finally"    => Token::Finally,
            "true"       => Token::Bool(true),
            "false"      => Token::Bool(false),
            "null"       => Token::Null,
            "undefined"  => Token::Undefined,
            _            => Token::Ident(s),
        }
    }

    pub fn next(&mut self) -> Token {
        self.skip_whitespace_and_comments();
        if self.pos >= self.src.len() { return Token::Eof; }
        let c = self.advance();
        match c {
            b'"' | b'\'' => self.read_string(c),
            b'`'          => self.read_template(),
            b'0'..=b'9'  => self.read_number(c),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => self.read_ident(c),
            b'(' => Token::LParen,  b')' => Token::RParen,
            b'{' => Token::LBrace,  b'}' => Token::RBrace,
            b'[' => Token::LBracket, b']' => Token::RBracket,
            b';' => Token::Semi,    b',' => Token::Comma,
            b'~' => Token::Tilde,   b'^' => { if self.eat(b'=') { Token::CaretAssign } else { Token::Caret } },
            b'?' => {
                if self.peek() == b'?' { self.advance(); Token::QuestionQuestion }
                else if self.peek() == b'.' { self.advance(); Token::Optional }
                else { Token::Question }
            },
            b':' => Token::Colon,
            b'.' => {
                if self.peek() == b'.' && self.peek2() == b'.' { self.pos += 2; Token::DotDotDot }
                else { Token::Dot }
            },
            b'+' => {
                if self.eat(b'+') { Token::PlusPlus }
                else if self.eat(b'=') { Token::PlusAssign }
                else { Token::Plus }
            },
            b'-' => {
                if self.eat(b'-') { Token::MinusMinus }
                else if self.eat(b'=') { Token::MinusAssign }
                else { Token::Minus }
            },
            b'*' => {
                if self.eat(b'*') { Token::StarStar }
                else if self.eat(b'=') { Token::StarAssign }
                else { Token::Star }
            },
            b'/' => {
                if self.eat(b'=') { Token::SlashAssign }
                else { Token::Slash }
            },
            b'%' => { if self.eat(b'=') { Token::PercentAssign } else { Token::Percent } },
            b'&' => {
                if self.eat(b'&') { Token::AmpAmp }
                else if self.eat(b'=') { Token::AmpAssign }
                else { Token::Amp }
            },
            b'|' => {
                if self.eat(b'|') { Token::PipePipe }
                else if self.eat(b'=') { Token::PipeAssign }
                else { Token::Pipe }
            },
            b'!' => {
                if self.peek() == b'=' {
                    self.advance();
                    if self.eat(b'=') { Token::NotEqEq } else { Token::NotEq }
                } else { Token::Bang }
            },
            b'=' => {
                if self.peek() == b'>' { self.advance(); Token::Arrow }
                else if self.peek() == b'=' {
                    self.advance();
                    if self.eat(b'=') { Token::EqEq } else { Token::Eq }
                } else { Token::Assign }
            },
            b'<' => {
                if self.peek() == b'<' { self.advance(); Token::LShift }
                else if self.eat(b'=') { Token::LtEq }
                else { Token::Lt }
            },
            b'>' => {
                if self.peek() == b'>' {
                    self.advance();
                    if self.peek() == b'>' { self.advance(); Token::URShift }
                    else { Token::RShift }
                } else if self.eat(b'=') { Token::GtEq }
                else { Token::Gt }
            },
            _ => Token::Ident(alloc::format!("<{}>", c as char)),
        }
    }

    /// Tokenise entire source into a Vec (for look-ahead parser).
    pub fn tokenize(src: &'a str) -> Vec<Token> {
        let mut lex = Lexer::new(src);
        let mut out = Vec::new();
        loop {
            let t = lex.next();
            let done = t == Token::Eof;
            out.push(t);
            if done { break; }
        }
        out
    }
}
