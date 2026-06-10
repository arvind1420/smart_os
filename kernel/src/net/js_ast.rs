//! JavaScript AST — Phase 38 (part 2/3) / Phase 104 (async/await)

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;

// ─── Expressions ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Expr {
    Number(f64),
    Str(String),
    Bool(bool),
    Null,
    Undefined,
    This,

    Ident(String),

    Array(Vec<Expr>),
    Object(Vec<(ObjectKey, Expr)>),

    Unary  { op: UnaryOp,  expr: Box<Expr> },
    Binary { op: BinaryOp, left: Box<Expr>, right: Box<Expr> },
    Logical{ op: LogicOp,  left: Box<Expr>, right: Box<Expr> },
    Assign { op: AssignOp, target: Box<Expr>, value: Box<Expr> },
    Ternary{ cond: Box<Expr>, then: Box<Expr>, else_: Box<Expr> },

    /// `a.b` or `a[b]`
    Member { obj: Box<Expr>, prop: Box<Expr>, computed: bool },

    Call   { callee: Box<Expr>, args: Vec<Expr> },
    New    { callee: Box<Expr>, args: Vec<Expr> },

    /// `function(params) { body }` (anonymous); `is_async = true` for `async function`
    FuncExpr { params: Vec<String>, body: Vec<Stmt>, is_async: bool },
    /// `(params) => expr_or_block`; `is_async = true` for `async (p) => …`
    Arrow    { params: Vec<String>, body: ArrowBody, is_async: bool },

    Typeof(Box<Expr>),
    Delete(Box<Expr>),
    Spread(Box<Expr>),
    Await(Box<Expr>),

    Template(String), // simplified — no ${} substitution
    Sequence(Vec<Expr>),
}

#[derive(Debug, Clone)]
pub enum ObjectKey {
    Ident(String),
    Str(String),
    Computed(Box<Expr>),
}

#[derive(Debug, Clone)]
pub enum ArrowBody {
    Expr(Box<Expr>),
    Block(Vec<Stmt>),
}

// ─── Unary / Binary / Logical / Assign operators ────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum UnaryOp { Neg, Pos, Not, BitNot, PreInc, PreDec, PostInc, PostDec, Void }

#[derive(Debug, Clone, PartialEq)]
pub enum BinaryOp {
    Add, Sub, Mul, Div, Rem, Pow,
    BitAnd, BitOr, BitXor, Shl, Shr, UShr,
    Eq, NotEq, StrictEq, StrictNotEq,
    Lt, Gt, LtEq, GtEq,
    In, Instanceof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogicOp { And, Or, NullCoalesce }

#[derive(Debug, Clone, PartialEq)]
pub enum AssignOp {
    Plain,
    Add, Sub, Mul, Div, Rem,
    BitAnd, BitOr, BitXor,
}

// ─── Statements ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Stmt {
    Expr(Expr),
    Block(Vec<Stmt>),

    VarDecl { kind: VarKind, name: String, init: Option<Expr> },
    /// Destructuring: let { a, b } = obj  or  let [a, b] = arr
    DestructDecl { kind: VarKind, pattern: DestructPat, init: Expr },

    /// `is_async = true` for `async function name() {}`
    FuncDecl { name: String, params: Vec<String>, body: Vec<Stmt>, is_async: bool },
    ClassDecl { name: String, super_class: Option<String>, methods: Vec<ClassMethod> },

    Return(Option<Expr>),
    Throw(Expr),

    If { cond: Expr, then: Box<Stmt>, else_: Option<Box<Stmt>> },

    While { cond: Expr, body: Box<Stmt> },
    DoWhile { body: Box<Stmt>, cond: Expr },
    For {
        init:   Option<ForInit>,
        cond:   Option<Expr>,
        update: Option<Expr>,
        body:   Box<Stmt>,
    },
    ForIn  { kind: VarKind, name: String, obj:  Expr, body: Box<Stmt> },
    ForOf  { kind: VarKind, name: String, iter: Expr, body: Box<Stmt> },

    Break(Option<String>),
    Continue(Option<String>),
    Label(String, Box<Stmt>),

    TryCatch {
        body:    Vec<Stmt>,
        param:   Option<String>,
        catch:   Option<Vec<Stmt>>,
        finally: Option<Vec<Stmt>>,
    },

    Import { what: ImportSpec, from: String },
    Export(Box<Stmt>),

    Empty,
}

#[derive(Debug, Clone)]
pub enum ForInit {
    Var(VarKind, String, Option<Expr>),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum VarKind { Var, Let, Const }

#[derive(Debug, Clone)]
pub enum DestructPat {
    Object(Vec<(String, Option<String>)>), // (key, alias)
    Array(Vec<Option<String>>),
}

#[derive(Debug, Clone)]
pub struct ClassMethod {
    pub name:    String,
    pub params:  Vec<String>,
    pub body:    Vec<Stmt>,
    pub is_static: bool,
    pub is_get:  bool,
    pub is_set:  bool,
    pub is_constructor: bool,
    pub is_async: bool,
}

#[derive(Debug, Clone)]
pub enum ImportSpec {
    Default(String),
    Named(Vec<(String, String)>),  // (orig, alias)
    Namespace(String),
    Side,
}
