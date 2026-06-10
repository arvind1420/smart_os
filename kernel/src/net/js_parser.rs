//! JavaScript Parser — Phase 38 (part 3a/3)
//! Recursive-descent parser — produces js_ast::Stmt/Expr trees.

#![allow(dead_code)]

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::{String, ToString};
use alloc::boxed::Box;
use super::js_lexer::Token;
use super::js_ast::*;

pub struct Parser {
    tokens: Vec<Token>,
    pos:    usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self { Parser { tokens, pos: 0 } }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }
    fn peek2(&self) -> &Token {
        self.tokens.get(self.pos + 1).unwrap_or(&Token::Eof)
    }
    fn advance(&mut self) -> Token {
        let t = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        if self.pos < self.tokens.len() { self.pos += 1; }
        t
    }
    fn eat(&mut self, tok: &Token) -> bool {
        if core::mem::discriminant(self.peek()) == core::mem::discriminant(tok) {
            self.advance(); true
        } else { false }
    }
    fn eat_semi(&mut self) { self.eat(&Token::Semi); }
    fn expect_ident(&mut self) -> String {
        if let Token::Ident(s) = self.advance() { s } else { String::new() }
    }

    // ─── Program ────────────────────────────────────────────────────────────

    pub fn parse_program(&mut self) -> Vec<Stmt> {
        let mut stmts = Vec::new();
        while *self.peek() != Token::Eof {
            stmts.push(self.parse_stmt());
        }
        stmts
    }

    // ─── Statements ─────────────────────────────────────────────────────────

    fn parse_stmt(&mut self) -> Stmt {
        match self.peek().clone() {
            Token::LBrace   => { self.advance(); let b = self.parse_block(); b }
            Token::Semi     => { self.advance(); Stmt::Empty }
            Token::Var | Token::Let | Token::Const => self.parse_var_decl(),
            Token::Function => self.parse_func_decl(),
            Token::Class    => self.parse_class_decl(),
            Token::Return   => self.parse_return(),
            Token::Throw    => { self.advance(); let e = self.parse_expr(); self.eat_semi(); Stmt::Throw(e) }
            Token::If       => self.parse_if(),
            Token::While    => self.parse_while(),
            Token::For      => self.parse_for(),
            Token::Break    => { self.advance(); self.eat_semi(); Stmt::Break(None) }
            Token::Continue => { self.advance(); self.eat_semi(); Stmt::Continue(None) }
            Token::Try      => self.parse_try(),
            Token::Import   => self.parse_import(),
            Token::Export   => { self.advance(); let s = self.parse_stmt(); Stmt::Export(Box::new(s)) }
            // `async function name() { }` → async FuncDecl
            Token::Ident(ref s) if s == "async" => {
                let saved = self.pos;
                self.advance(); // consume 'async'
                if *self.peek() == Token::Function {
                    return self.parse_func_decl_async(true);
                }
                // Not a function declaration — restore and fall through to expr
                self.pos = saved;
                let e = self.parse_expr();
                self.eat_semi();
                Stmt::Expr(e)
            }
            _ => {
                let e = self.parse_expr();
                self.eat_semi();
                Stmt::Expr(e)
            }
        }
    }

    fn parse_block(&mut self) -> Stmt {
        let mut stmts = Vec::new();
        while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
            stmts.push(self.parse_stmt());
        }
        self.eat(&Token::RBrace);
        Stmt::Block(stmts)
    }

    fn parse_var_decl(&mut self) -> Stmt {
        let kind = match self.advance() {
            Token::Var   => VarKind::Var,
            Token::Const => VarKind::Const,
            _            => VarKind::Let,
        };
        // Destructuring?
        if *self.peek() == Token::LBrace {
            self.advance();
            let mut keys: Vec<(String, Option<String>)> = Vec::new();
            while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
                let key = self.expect_ident();
                let alias = if *self.peek() == Token::Colon { self.advance(); Some(self.expect_ident()) } else { None };
                keys.push((key, alias));
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBrace);
            self.eat(&Token::Assign);
            let init = self.parse_expr();
            self.eat_semi();
            return Stmt::DestructDecl { kind, pattern: DestructPat::Object(keys), init };
        }
        if *self.peek() == Token::LBracket {
            self.advance();
            let mut elems: Vec<Option<String>> = Vec::new();
            while *self.peek() != Token::RBracket && *self.peek() != Token::Eof {
                if *self.peek() == Token::Comma { elems.push(None); }
                else { elems.push(Some(self.expect_ident())); }
                self.eat(&Token::Comma);
            }
            self.eat(&Token::RBracket);
            self.eat(&Token::Assign);
            let init = self.parse_expr();
            self.eat_semi();
            return Stmt::DestructDecl { kind, pattern: DestructPat::Array(elems), init };
        }

        let name = self.expect_ident();
        let init = if self.eat(&Token::Assign) { Some(self.parse_assign()) } else { None };
        self.eat_semi();
        Stmt::VarDecl { kind, name, init }
    }

    fn parse_func_decl(&mut self) -> Stmt { self.parse_func_decl_async(false) }

    fn parse_func_decl_async(&mut self, is_async: bool) -> Stmt {
        self.advance(); // 'function'
        let name = self.expect_ident();
        let (params, body) = self.parse_func_params_body();
        Stmt::FuncDecl { name, params, body, is_async }
    }

    fn parse_func_params_body(&mut self) -> (Vec<String>, Vec<Stmt>) {
        self.eat(&Token::LParen);
        let mut params = Vec::new();
        while *self.peek() != Token::RParen && *self.peek() != Token::Eof {
            if *self.peek() == Token::DotDotDot { self.advance(); }
            params.push(self.expect_ident());
            if *self.peek() == Token::Assign { self.advance(); self.parse_assign(); } // default
            self.eat(&Token::Comma);
        }
        self.eat(&Token::RParen);
        self.eat(&Token::LBrace);
        let mut body = Vec::new();
        while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
            body.push(self.parse_stmt());
        }
        self.eat(&Token::RBrace);
        (params, body)
    }

    fn parse_class_decl(&mut self) -> Stmt {
        self.advance(); // 'class'
        let name = self.expect_ident();
        let super_class = if self.eat(&Token::Extends) { Some(self.expect_ident()) } else { None };
        self.eat(&Token::LBrace);
        let mut methods = Vec::new();
        while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
            let is_static = if let Token::Ident(s) = self.peek() { if s == "static" { self.advance(); true } else { false } } else { false };
            let is_get = if let Token::Ident(s) = self.peek() { if s == "get" { self.advance(); true } else { false } } else { false };
            let is_set = if let Token::Ident(s) = self.peek() { if s == "set" { self.advance(); true } else { false } } else { false };
            let mname = match self.peek().clone() {
                Token::Ident(s) => { self.advance(); s }
                Token::Str(s)   => { self.advance(); s }
                _               => { self.advance(); String::new() }
            };
            let is_constructor = mname == "constructor";
            let (params, body) = self.parse_func_params_body();
            methods.push(ClassMethod { name: mname, params, body, is_static, is_get, is_set, is_constructor, is_async: false });
            self.eat(&Token::Semi);
        }
        self.eat(&Token::RBrace);
        Stmt::ClassDecl { name, super_class, methods }
    }

    fn parse_return(&mut self) -> Stmt {
        self.advance();
        if *self.peek() == Token::Semi || *self.peek() == Token::RBrace || *self.peek() == Token::Eof {
            self.eat_semi();
            return Stmt::Return(None);
        }
        let e = self.parse_expr();
        self.eat_semi();
        Stmt::Return(Some(e))
    }

    fn parse_if(&mut self) -> Stmt {
        self.advance(); // 'if'
        self.eat(&Token::LParen);
        let cond = self.parse_expr();
        self.eat(&Token::RParen);
        let then = Box::new(self.parse_stmt());
        let else_ = if self.eat(&Token::Else) { Some(Box::new(self.parse_stmt())) } else { None };
        Stmt::If { cond, then, else_ }
    }

    fn parse_while(&mut self) -> Stmt {
        self.advance();
        self.eat(&Token::LParen);
        let cond = self.parse_expr();
        self.eat(&Token::RParen);
        let body = Box::new(self.parse_stmt());
        Stmt::While { cond, body }
    }

    fn parse_for(&mut self) -> Stmt {
        self.advance(); // 'for'
        self.eat(&Token::LParen);

        // for..in / for..of
        // Peek: var/let/const ident in/of
        let saved = self.pos;
        if matches!(self.peek(), Token::Var | Token::Let | Token::Const) {
            let kind = match self.advance() { Token::Const => VarKind::Const, Token::Let => VarKind::Let, _ => VarKind::Var };
            let name = self.expect_ident();
            if *self.peek() == Token::In {
                self.advance();
                let obj = self.parse_expr();
                self.eat(&Token::RParen);
                let body = Box::new(self.parse_stmt());
                return Stmt::ForIn { kind, name, obj, body };
            }
            if let Token::Ident(s) = self.peek() { if s == "of" {
                self.advance();
                let iter = self.parse_expr();
                self.eat(&Token::RParen);
                let body = Box::new(self.parse_stmt());
                return Stmt::ForOf { kind, name, iter, body };
            }}
            // Restore and parse as normal for
            self.pos = saved;
        }

        let init = if *self.peek() == Token::Semi {
            None
        } else if matches!(self.peek(), Token::Var | Token::Let | Token::Const) {
            let kind = match self.advance() { Token::Const => VarKind::Const, Token::Let => VarKind::Let, _ => VarKind::Var };
            let n = self.expect_ident();
            let v = if self.eat(&Token::Assign) { Some(self.parse_assign()) } else { None };
            Some(ForInit::Var(kind, n, v))
        } else {
            Some(ForInit::Expr(self.parse_expr()))
        };
        self.eat(&Token::Semi);
        let cond   = if *self.peek() == Token::Semi { None } else { Some(self.parse_expr()) };
        self.eat(&Token::Semi);
        let update = if *self.peek() == Token::RParen { None } else { Some(self.parse_expr()) };
        self.eat(&Token::RParen);
        let body = Box::new(self.parse_stmt());
        Stmt::For { init, cond, update, body }
    }

    fn parse_try(&mut self) -> Stmt {
        self.advance();
        self.eat(&Token::LBrace);
        let mut body = Vec::new();
        while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
            body.push(self.parse_stmt());
        }
        self.eat(&Token::RBrace);

        let (param, catch) = if self.eat(&Token::Catch) {
            let p = if self.eat(&Token::LParen) {
                let n = self.expect_ident(); self.eat(&Token::RParen); Some(n)
            } else { None };
            self.eat(&Token::LBrace);
            let mut cb = Vec::new();
            while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
                cb.push(self.parse_stmt());
            }
            self.eat(&Token::RBrace);
            (p, Some(cb))
        } else { (None, None) };

        let finally = if self.eat(&Token::Finally) {
            self.eat(&Token::LBrace);
            let mut fb = Vec::new();
            while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
                fb.push(self.parse_stmt());
            }
            self.eat(&Token::RBrace);
            Some(fb)
        } else { None };

        Stmt::TryCatch { body, param, catch, finally }
    }

    fn parse_import(&mut self) -> Stmt {
        self.advance();
        // import 'module'  or  import x from 'module'  or  import {a,b} from 'module'
        let (what, from) = match self.peek().clone() {
            Token::Str(s) => { self.advance(); self.eat_semi(); (ImportSpec::Side, s) }
            Token::Star => {
                self.advance();
                self.eat(&Token::Ident("as".to_string()));
                let alias = self.expect_ident();
                self.eat(&Token::Ident("from".to_string()));
                let from = if let Token::Str(s) = self.advance() { s } else { String::new() };
                self.eat_semi();
                (ImportSpec::Namespace(alias), from)
            }
            Token::LBrace => {
                self.advance();
                let mut names = Vec::new();
                while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
                    let orig = self.expect_ident();
                    let alias = if let Token::Ident(s) = self.peek() { if s == "as" { self.advance(); self.expect_ident() } else { orig.clone() } } else { orig.clone() };
                    names.push((orig, alias));
                    self.eat(&Token::Comma);
                }
                self.eat(&Token::RBrace);
                self.eat(&Token::Ident("from".to_string()));
                let from = if let Token::Str(s) = self.advance() { s } else { String::new() };
                self.eat_semi();
                (ImportSpec::Named(names), from)
            }
            _ => {
                let name = self.expect_ident();
                self.eat(&Token::Ident("from".to_string()));
                let from = if let Token::Str(s) = self.advance() { s } else { String::new() };
                self.eat_semi();
                (ImportSpec::Default(name), from)
            }
        };
        Stmt::Import { what, from }
    }

    // ─── Expressions ────────────────────────────────────────────────────────

    pub fn parse_expr(&mut self) -> Expr {
        let e = self.parse_assign();
        if *self.peek() == Token::Comma {
            let mut exprs = vec![e];
            while self.eat(&Token::Comma) { exprs.push(self.parse_assign()); }
            Expr::Sequence(exprs)
        } else { e }
    }

    fn parse_assign(&mut self) -> Expr {
        let left = self.parse_ternary();
        let op = match self.peek() {
            Token::Assign        => AssignOp::Plain,
            Token::PlusAssign    => AssignOp::Add,
            Token::MinusAssign   => AssignOp::Sub,
            Token::StarAssign    => AssignOp::Mul,
            Token::SlashAssign   => AssignOp::Div,
            Token::PercentAssign => AssignOp::Rem,
            Token::AmpAssign     => AssignOp::BitAnd,
            Token::PipeAssign    => AssignOp::BitOr,
            Token::CaretAssign   => AssignOp::BitXor,
            _                    => return left,
        };
        self.advance();
        let right = self.parse_assign();
        Expr::Assign { op, target: Box::new(left), value: Box::new(right) }
    }

    fn parse_ternary(&mut self) -> Expr {
        let cond = self.parse_null_coalesce();
        if self.eat(&Token::Question) {
            let then = self.parse_assign();
            self.eat(&Token::Colon);
            let else_ = self.parse_assign();
            Expr::Ternary { cond: Box::new(cond), then: Box::new(then), else_: Box::new(else_) }
        } else { cond }
    }

    fn parse_null_coalesce(&mut self) -> Expr {
        let mut e = self.parse_or();
        while *self.peek() == Token::QuestionQuestion {
            self.advance();
            let r = self.parse_or();
            e = Expr::Logical { op: LogicOp::NullCoalesce, left: Box::new(e), right: Box::new(r) };
        }
        e
    }

    fn parse_or(&mut self) -> Expr {
        let mut e = self.parse_and();
        while *self.peek() == Token::PipePipe {
            self.advance();
            let r = self.parse_and();
            e = Expr::Logical { op: LogicOp::Or, left: Box::new(e), right: Box::new(r) };
        }
        e
    }

    fn parse_and(&mut self) -> Expr {
        let mut e = self.parse_bitor();
        while *self.peek() == Token::AmpAmp {
            self.advance();
            let r = self.parse_bitor();
            e = Expr::Logical { op: LogicOp::And, left: Box::new(e), right: Box::new(r) };
        }
        e
    }

    fn parse_bitor(&mut self) -> Expr  { self.parse_binop_left(&[Token::Pipe],  &[BinaryOp::BitOr],  Self::parse_bitxor) }
    fn parse_bitxor(&mut self) -> Expr { self.parse_binop_left(&[Token::Caret], &[BinaryOp::BitXor], Self::parse_bitand) }
    fn parse_bitand(&mut self) -> Expr { self.parse_binop_left(&[Token::Amp],   &[BinaryOp::BitAnd], Self::parse_eq) }

    fn parse_eq(&mut self) -> Expr {
        self.parse_binop_left(
            &[Token::Eq, Token::NotEq, Token::EqEq, Token::NotEqEq],
            &[BinaryOp::Eq, BinaryOp::NotEq, BinaryOp::StrictEq, BinaryOp::StrictNotEq],
            Self::parse_rel,
        )
    }

    fn parse_rel(&mut self) -> Expr {
        let mut e = self.parse_shift();
        loop {
            let op = match self.peek() {
                Token::Lt          => BinaryOp::Lt,
                Token::Gt          => BinaryOp::Gt,
                Token::LtEq        => BinaryOp::LtEq,
                Token::GtEq        => BinaryOp::GtEq,
                Token::In          => BinaryOp::In,
                Token::Instanceof  => BinaryOp::Instanceof,
                _ => break,
            };
            self.advance();
            let r = self.parse_shift();
            e = Expr::Binary { op, left: Box::new(e), right: Box::new(r) };
        }
        e
    }

    fn parse_shift(&mut self) -> Expr {
        self.parse_binop_left(
            &[Token::LShift, Token::RShift, Token::URShift],
            &[BinaryOp::Shl, BinaryOp::Shr, BinaryOp::UShr],
            Self::parse_add,
        )
    }

    fn parse_add(&mut self) -> Expr {
        self.parse_binop_left(&[Token::Plus, Token::Minus], &[BinaryOp::Add, BinaryOp::Sub], Self::parse_mul)
    }

    fn parse_mul(&mut self) -> Expr {
        self.parse_binop_left(
            &[Token::Star, Token::Slash, Token::Percent, Token::StarStar],
            &[BinaryOp::Mul, BinaryOp::Div, BinaryOp::Rem, BinaryOp::Pow],
            Self::parse_unary,
        )
    }

    fn parse_binop_left(
        &mut self,
        toks: &[Token],
        ops:  &[BinaryOp],
        next: fn(&mut Self) -> Expr,
    ) -> Expr {
        let mut e = next(self);
        loop {
            let mut found = false;
            for (tok, op) in toks.iter().zip(ops.iter()) {
                if core::mem::discriminant(self.peek()) == core::mem::discriminant(tok) {
                    self.advance();
                    let r = next(self);
                    e = Expr::Binary { op: op.clone(), left: Box::new(e), right: Box::new(r) };
                    found = true;
                    break;
                }
            }
            if !found { break; }
        }
        e
    }

    fn parse_unary(&mut self) -> Expr {
        match self.peek().clone() {
            Token::Bang   => { self.advance(); Expr::Unary { op: UnaryOp::Not,    expr: Box::new(self.parse_unary()) } }
            Token::Minus  => { self.advance(); Expr::Unary { op: UnaryOp::Neg,    expr: Box::new(self.parse_unary()) } }
            Token::Plus   => { self.advance(); Expr::Unary { op: UnaryOp::Pos,    expr: Box::new(self.parse_unary()) } }
            Token::Tilde  => { self.advance(); Expr::Unary { op: UnaryOp::BitNot, expr: Box::new(self.parse_unary()) } }
            Token::Typeof => { self.advance(); Expr::Typeof(Box::new(self.parse_unary())) }
            Token::Delete => { self.advance(); Expr::Delete(Box::new(self.parse_unary())) }
            Token::PlusPlus  => { self.advance(); Expr::Unary { op: UnaryOp::PreInc, expr: Box::new(self.parse_unary()) } }
            Token::MinusMinus => { self.advance(); Expr::Unary { op: UnaryOp::PreDec, expr: Box::new(self.parse_unary()) } }
            // `await expr` — treated as a synchronous unwrap in our cooperative kernel
            Token::Ident(ref s) if s == "await" => {
                self.advance();
                Expr::Await(Box::new(self.parse_unary()))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Expr {
        let mut e = self.parse_call();
        match self.peek() {
            Token::PlusPlus   => { self.advance(); e = Expr::Unary { op: UnaryOp::PostInc, expr: Box::new(e) }; }
            Token::MinusMinus => { self.advance(); e = Expr::Unary { op: UnaryOp::PostDec, expr: Box::new(e) }; }
            _ => {}
        }
        e
    }

    fn parse_call(&mut self) -> Expr {
        let mut e = self.parse_new();
        loop {
            match self.peek().clone() {
                Token::LParen => {
                    self.advance();
                    let args = self.parse_args();
                    self.eat(&Token::RParen);
                    e = Expr::Call { callee: Box::new(e), args };
                }
                Token::Dot | Token::Optional => {
                    self.advance();
                    let prop = match self.advance() {
                        Token::Ident(s) => Expr::Str(s),
                        Token::Number(n) => Expr::Number(n),
                        _ => Expr::Str(String::new()),
                    };
                    e = Expr::Member { obj: Box::new(e), prop: Box::new(prop), computed: false };
                }
                Token::LBracket => {
                    self.advance();
                    let prop = self.parse_expr();
                    self.eat(&Token::RBracket);
                    e = Expr::Member { obj: Box::new(e), prop: Box::new(prop), computed: true };
                }
                _ => break,
            }
        }
        e
    }

    fn parse_new(&mut self) -> Expr {
        if *self.peek() == Token::New {
            self.advance();
            let callee = self.parse_new(); // recursive for `new new Foo()`
            let args = if *self.peek() == Token::LParen {
                self.advance(); let a = self.parse_args(); self.eat(&Token::RParen); a
            } else { Vec::new() };
            return Expr::New { callee: Box::new(callee), args };
        }
        self.parse_primary()
    }

    fn parse_args(&mut self) -> Vec<Expr> {
        let mut args = Vec::new();
        while *self.peek() != Token::RParen && *self.peek() != Token::Eof {
            if *self.peek() == Token::DotDotDot { self.advance(); args.push(Expr::Spread(Box::new(self.parse_assign()))); }
            else { args.push(self.parse_assign()); }
            self.eat(&Token::Comma);
        }
        args
    }

    fn parse_primary(&mut self) -> Expr {
        match self.peek().clone() {
            Token::Number(n)   => { self.advance(); Expr::Number(n) }
            Token::Str(s)      => { self.advance(); Expr::Str(s) }
            Token::Template(s) => { self.advance(); Expr::Template(s) }
            Token::Bool(b)     => { self.advance(); Expr::Bool(b) }
            Token::Null        => { self.advance(); Expr::Null }
            Token::Undefined   => { self.advance(); Expr::Undefined }
            Token::This        => { self.advance(); Expr::This }
            Token::Ident(s)    => {
                self.advance();
                // Single-param arrow function without parens: x => expr  or  x => { ... }
                if *self.peek() == Token::Arrow {
                    self.advance(); // consume =>
                    let body = if *self.peek() == Token::LBrace {
                        self.advance();
                        let mut stmts = Vec::new();
                        while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
                            stmts.push(self.parse_stmt());
                        }
                        self.eat(&Token::RBrace);
                        ArrowBody::Block(stmts)
                    } else {
                        ArrowBody::Expr(Box::new(self.parse_assign()))
                    };
                    return Expr::Arrow { params: vec![s], body, is_async: false };
                }
                // `async function` expression  →  async anonymous function expression
                if s == "async" && *self.peek() == Token::Function {
                    self.advance(); // consume 'function'
                    // optional name
                    if let Token::Ident(_) = self.peek() { self.advance(); }
                    let (params, body) = self.parse_func_params_body();
                    return Expr::FuncExpr { params, body, is_async: true };
                }
                // `async (params) => body`  →  async arrow function
                if s == "async" && *self.peek() == Token::LParen {
                    let saved = self.pos;
                    self.advance(); // consume '('
                    let mut params = Vec::new();
                    let mut ok = true;
                    while *self.peek() != Token::RParen && *self.peek() != Token::Eof {
                        if *self.peek() == Token::DotDotDot { self.advance(); }
                        if let Token::Ident(p) = self.peek().clone() { self.advance(); params.push(p); }
                        else { ok = false; break; }
                        if *self.peek() == Token::Assign { self.advance(); self.parse_assign(); }
                        self.eat(&Token::Comma);
                    }
                    let at_rparen = *self.peek() == Token::RParen;
                    if at_rparen { self.advance(); }
                    if ok && at_rparen && *self.peek() == Token::Arrow {
                        self.advance(); // consume =>
                        let body = if *self.peek() == Token::LBrace {
                            self.advance();
                            let mut stmts = Vec::new();
                            while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
                                stmts.push(self.parse_stmt());
                            }
                            self.eat(&Token::RBrace);
                            ArrowBody::Block(stmts)
                        } else {
                            ArrowBody::Expr(Box::new(self.parse_assign()))
                        };
                        return Expr::Arrow { params, body, is_async: true };
                    }
                    // Not an async arrow — restore and treat `async` as plain ident
                    self.pos = saved;
                }
                Expr::Ident(s)
            }

            Token::LBracket => {
                self.advance();
                let mut elems = Vec::new();
                while *self.peek() != Token::RBracket && *self.peek() != Token::Eof {
                    if *self.peek() == Token::DotDotDot { self.advance(); elems.push(Expr::Spread(Box::new(self.parse_assign()))); }
                    else if *self.peek() == Token::Comma { elems.push(Expr::Undefined); }
                    else { elems.push(self.parse_assign()); }
                    self.eat(&Token::Comma);
                }
                self.eat(&Token::RBracket);
                Expr::Array(elems)
            }

            Token::LBrace => {
                self.advance();
                let mut props = Vec::new();
                while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
                    let key = match self.peek().clone() {
                        Token::Ident(s) => { self.advance(); ObjectKey::Ident(s) }
                        Token::Str(s)   => { self.advance(); ObjectKey::Str(s) }
                        Token::LBracket => { self.advance(); let e = self.parse_assign(); self.eat(&Token::RBracket); ObjectKey::Computed(Box::new(e)) }
                        Token::Number(n)=> { self.advance(); ObjectKey::Str(alloc::format!("{}", n)) }
                        _ => { self.advance(); ObjectKey::Ident(String::new()) }
                    };
                    if self.eat(&Token::Colon) {
                        props.push((key, self.parse_assign()));
                    } else {
                        // shorthand: { x } or method: { foo() {} }
                        match &key {
                            ObjectKey::Ident(name) => {
                                if *self.peek() == Token::LParen {
                                    let (params, body) = self.parse_func_params_body();
                                    props.push((ObjectKey::Str(name.clone()), Expr::FuncExpr { params, body, is_async: false }));
                                } else {
                                    props.push((ObjectKey::Str(name.clone()), Expr::Ident(name.clone())));
                                }
                            }
                            _ => { props.push((key, Expr::Undefined)); }
                        }
                    }
                    self.eat(&Token::Comma);
                }
                self.eat(&Token::RBrace);
                Expr::Object(props)
            }

            Token::Function => {
                self.advance();
                // optional name
                if let Token::Ident(_) = self.peek() { self.advance(); }
                let (params, body) = self.parse_func_params_body();
                Expr::FuncExpr { params, body, is_async: false }
            }

            Token::LParen => {
                self.advance();
                // Arrow function?  (params) =>
                // Collect params if followed by ) =>
                let saved = self.pos;
                let mut params = Vec::new();
                let mut ok = true;
                while *self.peek() != Token::RParen && *self.peek() != Token::Eof {
                    if *self.peek() == Token::DotDotDot { self.advance(); }
                    if let Token::Ident(s) = self.peek().clone() { self.advance(); params.push(s); }
                    else { ok = false; break; }
                    if *self.peek() == Token::Assign { self.advance(); self.parse_assign(); } // default
                    self.eat(&Token::Comma);
                }
                let at_rparen = *self.peek() == Token::RParen;
                if at_rparen { self.advance(); }
                if ok && at_rparen && *self.peek() == Token::Arrow {
                    self.advance(); // =>
                    let body = if *self.peek() == Token::LBrace {
                        self.advance();
                        let mut stmts = Vec::new();
                        while *self.peek() != Token::RBrace && *self.peek() != Token::Eof {
                            stmts.push(self.parse_stmt());
                        }
                        self.eat(&Token::RBrace);
                        ArrowBody::Block(stmts)
                    } else {
                        ArrowBody::Expr(Box::new(self.parse_assign()))
                    };
                    return Expr::Arrow { params, body, is_async: false };
                }
                // Not an arrow: restore and parse as grouped expression
                self.pos = saved;
                let e = self.parse_expr();
                self.eat(&Token::RParen);
                e
            }

            _ => { self.advance(); Expr::Undefined }
        }
    }
}

/// Parse a JS source string, return the program body.
pub fn parse(src: &str) -> Vec<Stmt> {
    let tokens = super::js_lexer::Lexer::tokenize(src);
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}
