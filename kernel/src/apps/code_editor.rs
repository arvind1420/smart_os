/// Phase 56: Code Editor v2 for Smart OS.
///
/// Extends the basic editor with:
///   • Syntax highlighting — JS / Rust / C tokeniser, per-line dominant colour
///   • Multi-tab          — Vec<Tab>, Ctrl+T new tab, Ctrl+W close tab, Ctrl+← /→ switch
///   • Find + Replace     — incremental search (Ctrl+F), replace-one (Ctrl+H)
///   • Line numbers       — gutter prefix "  N│ " rendered per display line
///   • Language detection — by file extension (.js .rs .c .h .py .txt)

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

use crate::gui::theme::*;
use crate::gui::window::{Window, WindowId};
use crate::gui::widget::*;
use crate::gui::desktop::DESKTOP;

// ─────────────────────────────────────────────────────────────────────────────
//  Language / syntax
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang { Rust, Js, C, Python, Plain }

impl Lang {
    pub fn from_ext(path: &str) -> Self {
        if path.ends_with(".rs")              { Self::Rust   }
        else if path.ends_with(".js") || path.ends_with(".ts") { Self::Js }
        else if path.ends_with(".c") || path.ends_with(".h") || path.ends_with(".cpp") { Self::C }
        else if path.ends_with(".py")         { Self::Python }
        else                                   { Self::Plain  }
    }
}

/// Token kind produced by the lexer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokKind {
    Keyword,
    TypeKw,      // type keywords (fn, struct, class, …)
    String,
    Comment,
    Number,
    Operator,
    Macro,       // Rust macro! / C preprocessor
    Normal,
}

impl TokKind {
    pub fn color(self) -> Color {
        match self {
            Self::Keyword  => ACCENT_BLUE,
            Self::TypeKw   => ACCENT_CYAN,
            Self::String   => ACCENT_GREEN,
            Self::Comment  => TEXT_SECONDARY,
            Self::Number   => ACCENT_ORANGE,
            Self::Operator => ACCENT_MAGENTA,
            Self::Macro    => ACCENT_PURPLE,
            Self::Normal   => TEXT_PRIMARY,
        }
    }
}

/// One syntax token.
#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokKind,
    pub text: String,
}

// ─────────────────────────────────────────────────────────────────────────────
//  Tokeniser
// ─────────────────────────────────────────────────────────────────────────────

const RUST_KW: &[&str] = &[
    "let", "mut", "fn", "pub", "use", "mod", "struct", "enum", "impl",
    "trait", "for", "in", "while", "loop", "if", "else", "match", "return",
    "break", "continue", "self", "Self", "super", "crate", "extern", "as",
    "type", "where", "const", "static", "unsafe", "async", "await", "dyn",
    "move", "ref", "box", "true", "false",
];

const JS_KW: &[&str] = &[
    "var", "let", "const", "function", "return", "if", "else", "for", "while",
    "do", "break", "continue", "new", "this", "class", "extends", "import",
    "export", "from", "of", "in", "typeof", "instanceof", "null", "undefined",
    "true", "false", "async", "await", "try", "catch", "throw", "finally",
    "switch", "case", "default", "delete", "void", "yield",
];

const C_KW: &[&str] = &[
    "if", "else", "for", "while", "do", "return", "break", "continue",
    "switch", "case", "default", "goto", "typedef", "struct", "union", "enum",
    "sizeof", "const", "static", "extern", "inline", "void", "int", "char",
    "short", "long", "float", "double", "unsigned", "signed", "NULL",
];

const PY_KW: &[&str] = &[
    "def", "class", "if", "elif", "else", "for", "while", "in", "not", "and",
    "or", "return", "import", "from", "as", "pass", "break", "continue",
    "with", "yield", "lambda", "try", "except", "finally", "raise", "del",
    "global", "nonlocal", "True", "False", "None", "is", "assert",
];

fn keywords_for(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Rust   => RUST_KW,
        Lang::Js     => JS_KW,
        Lang::C      => C_KW,
        Lang::Python => PY_KW,
        Lang::Plain  => &[],
    }
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Tokenise one source line. Returns a flat vec of tokens.
pub fn tokenise_line(line: &str, lang: Lang) -> Vec<Token> {
    if lang == Lang::Plain {
        return alloc::vec![Token { kind: TokKind::Normal, text: line.to_string() }];
    }

    let keywords = keywords_for(lang);
    let mut tokens: Vec<Token> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        // Line comment: // or # or ;; (Rust doc /// too)
        let rest: String = chars[i..].iter().collect();
        if rest.starts_with("//") || (lang == Lang::Python && chars[i] == '#') {
            tokens.push(Token { kind: TokKind::Comment, text: rest });
            break;
        }
        // C preprocessor
        if lang == Lang::C && chars[i] == '#' {
            tokens.push(Token { kind: TokKind::Macro, text: rest });
            break;
        }
        // Rust macro call (word followed by '!')
        if lang == Lang::Rust && i + 1 < n {
            // Detect identifiers ending in '!'
            let (word, advance) = read_identifier(&chars, i);
            if !word.is_empty() && i + advance < n && chars[i + advance] == '!' {
                tokens.push(Token { kind: TokKind::Macro, text: format!("{}!", word) });
                i += advance + 1;
                continue;
            }
        }
        // String literals: "..." or '...' (single char in C/Rust)
        if chars[i] == '"' || chars[i] == '\'' {
            let quote = chars[i];
            let mut s = String::new();
            s.push(quote);
            i += 1;
            while i < n {
                let c = chars[i];
                s.push(c);
                i += 1;
                if c == '\\' && i < n { s.push(chars[i]); i += 1; continue; }
                if c == quote { break; }
            }
            tokens.push(Token { kind: TokKind::String, text: s });
            continue;
        }
        // Number literals.
        if chars[i].is_ascii_digit()
            || (chars[i] == '-' && i + 1 < n && chars[i+1].is_ascii_digit())
        {
            let (num, adv) = read_number(&chars, i);
            tokens.push(Token { kind: TokKind::Number, text: num });
            i += adv;
            continue;
        }
        // Identifier or keyword.
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let (word, adv) = read_identifier(&chars, i);
            let kind = if keywords.contains(&word.as_str()) {
                TokKind::Keyword
            } else {
                TokKind::Normal
            };
            tokens.push(Token { kind, text: word });
            i += adv;
            continue;
        }
        // Operators.
        if "+-*/=<>!&|^~%?:.@".contains(chars[i]) {
            tokens.push(Token { kind: TokKind::Operator, text: chars[i].to_string() });
            i += 1;
            continue;
        }
        // Whitespace / punctuation → normal.
        tokens.push(Token { kind: TokKind::Normal, text: chars[i].to_string() });
        i += 1;
    }
    tokens
}

fn read_identifier(chars: &[char], start: usize) -> (String, usize) {
    let mut s = String::new();
    let mut i = start;
    while i < chars.len() && is_identifier_char(chars[i]) {
        s.push(chars[i]);
        i += 1;
    }
    let adv = i - start;
    (s, adv)
}

fn read_number(chars: &[char], start: usize) -> (String, usize) {
    let mut s = String::new();
    let mut i = start;
    // Optional leading minus already checked by caller.
    if i < chars.len() && chars[i] == '-' { s.push('-'); i += 1; }
    while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
        s.push(chars[i]);
        i += 1;
    }
    (s, i - start)
}

/// Compute the dominant colour for a line by finding the most-common token kind.
/// Used to pick the `display_lines` colour for the RawInputWidget.
pub fn dominant_color(tokens: &[Token]) -> Color {
    if tokens.is_empty() { return TEXT_PRIMARY; }
    // Priority: Comment > Keyword > String > Number > rest.
    if tokens.iter().any(|t| t.kind == TokKind::Comment) { return TEXT_SECONDARY; }
    if tokens.iter().any(|t| t.kind == TokKind::Macro)   { return ACCENT_PURPLE; }
    if tokens.iter().any(|t| t.kind == TokKind::String)  { return ACCENT_GREEN; }
    let keyword_chars: usize = tokens.iter().filter(|t| t.kind == TokKind::Keyword)
        .map(|t| t.text.len()).sum();
    let total_chars: usize = tokens.iter().map(|t| t.text.len()).sum::<usize>().max(1);
    if keyword_chars * 2 > total_chars { return ACCENT_BLUE; }
    TEXT_PRIMARY
}

/// Build a gutter-prefixed display line: "  42│ <content>"
pub fn format_line(line_no: usize, content: &str) -> String {
    format!("{:4}\u{2502} {}", line_no, content)
}

// ─────────────────────────────────────────────────────────────────────────────
//  Tab state
// ─────────────────────────────────────────────────────────────────────────────

pub struct Tab {
    pub path:        String,
    pub lang:        Lang,
    pub lines:       Vec<String>,
    pub cursor_line: usize,
    pub cursor_col:  usize,
    pub scroll:      usize,
    pub modified:    bool,
}

impl Tab {
    pub fn new_empty() -> Self {
        Self {
            path:        String::from("[new]"),
            lang:        Lang::Plain,
            lines:       alloc::vec![String::new()],
            cursor_line: 0, cursor_col: 0, scroll: 0, modified: false,
        }
    }

    pub fn from_path(path: &str) -> Self {
        let lang = Lang::from_ext(path);
        let lines = match crate::vfs::read_file_full(path) {
            Ok(data) => {
                let text = core::str::from_utf8(&data).unwrap_or("").to_string();
                if text.is_empty() { alloc::vec![String::new()] }
                else { text.lines().map(|l| l.to_string()).collect() }
            }
            Err(_) => alloc::vec![String::new()],
        };
        Self {
            path: path.to_string(), lang, lines,
            cursor_line: 0, cursor_col: 0, scroll: 0, modified: false,
        }
    }

    /// Save tab content to VFS.
    pub fn save(&mut self) -> Result<(), &'static str> {
        if self.path == "[new]" { return Err("no path"); }
        let mut content = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            content.push_str(line);
            if i + 1 < self.lines.len() { content.push('\n'); }
        }
        crate::vfs::create_and_write(&self.path, content.as_bytes())?;
        self.modified = false;
        Ok(())
    }

    /// Build display lines (line-number gutter + syntax colour).
    pub fn display_lines(&self, visible_rows: usize) -> Vec<(String, Color)> {
        let end = (self.scroll + visible_rows).min(self.lines.len());
        self.lines[self.scroll..end].iter().enumerate().map(|(i, line)| {
            let lineno = self.scroll + i + 1;
            let tokens = tokenise_line(line, self.lang);
            let color  = dominant_color(&tokens);
            let text   = format_line(lineno, line);
            (text, color)
        }).collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Find + Replace
// ─────────────────────────────────────────────────────────────────────────────

pub struct FindReplace {
    pub query:       String,
    pub replacement: String,
    pub active:      bool,
    /// Line indices of current matches.
    pub matches:     Vec<(usize, usize)>, // (line, col)
    pub match_idx:   usize,
}

impl FindReplace {
    pub fn new() -> Self {
        Self { query: String::new(), replacement: String::new(), active: false, matches: Vec::new(), match_idx: 0 }
    }

    pub fn search(&mut self, lines: &[String]) {
        self.matches.clear();
        self.match_idx = 0;
        if self.query.is_empty() { return; }
        for (li, line) in lines.iter().enumerate() {
            let mut col = 0;
            while let Some(pos) = line[col..].find(self.query.as_str()) {
                self.matches.push((li, col + pos));
                col += pos + self.query.len();
            }
        }
    }

    pub fn current_match(&self) -> Option<(usize, usize)> {
        self.matches.get(self.match_idx).copied()
    }

    pub fn next_match(&mut self) {
        if !self.matches.is_empty() {
            self.match_idx = (self.match_idx + 1) % self.matches.len();
        }
    }

    /// Replace the current match.
    pub fn replace_one(&mut self, lines: &mut Vec<String>) -> bool {
        if let Some((li, col)) = self.current_match() {
            let line = &lines[li];
            if col + self.query.len() <= line.len() {
                let before = line[..col].to_string();
                let after  = line[col + self.query.len()..].to_string();
                lines[li] = format!("{}{}{}", before, self.replacement, after);
                self.search(lines);
                return true;
            }
        }
        false
    }

    /// Replace all matches.
    pub fn replace_all(&mut self, lines: &mut Vec<String>) -> usize {
        let mut count = 0;
        // Iterate backwards to preserve indices.
        for li in (0..lines.len()).rev() {
            let line = lines[li].clone();
            let replaced = line.replace(self.query.as_str(), self.replacement.as_str());
            if replaced != line {
                let reps = (line.len() - replaced.len())
                    .checked_div(self.query.len().max(1)).unwrap_or(0);
                count += if self.replacement.len() >= self.query.len() {
                    (replaced.len() - line.len()) / self.replacement.len().max(1)
                } else { reps };
                lines[li] = replaced;
            }
        }
        self.search(lines);
        count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Editor state
// ─────────────────────────────────────────────────────────────────────────────

pub struct CodeEditorState {
    pub window_id:   WindowId,
    pub tabs:        Vec<Tab>,
    pub current_tab: usize,
    pub find:        FindReplace,
    pub status_msg:  String,
    pub dirty:       bool,
}

pub static STATE: Mutex<Option<CodeEditorState>> = Mutex::new(None);

// Widget IDs
const W_STATUS:   u8 = 0;
const W_TAB_BAR:  u8 = 1;
const W_CONTENT:  u8 = 2;
const W_FIND_BAR: u8 = 3;

const VISIBLE_ROWS: usize = 24;

// ─────────────────────────────────────────────────────────────────────────────
//  GUI sync
// ─────────────────────────────────────────────────────────────────────────────

fn sync_gui(state: &CodeEditorState, win: &mut Window) {
    let tab = match state.tabs.get(state.current_tab) {
        Some(t) => t,
        None => return,
    };

    // Status bar.
    let status = format!(
        " {} | {}:{} | {} | {} {}",
        tab.path,
        tab.cursor_line + 1, tab.cursor_col + 1,
        match tab.lang { Lang::Rust => "Rust", Lang::Js => "JS", Lang::C => "C", Lang::Python => "Python", Lang::Plain => "Text" },
        if tab.modified { "[+]" } else { "   " },
        &state.status_msg,
    );
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == W_STATUS) {
        if let WidgetKind::Label(ref mut lbl) = w.kind {
            lbl.text = status;
        }
    }

    // Tab bar.
    let tab_bar: String = state.tabs.iter().enumerate().map(|(i, t)| {
        let name = t.path.rsplit('/').next().unwrap_or(&t.path);
        let mark = if t.modified { "•" } else { "" };
        if i == state.current_tab {
            format!("[{}{}] ", name, mark)
        } else {
            format!(" {}{}  ", name, mark)
        }
    }).collect();
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == W_TAB_BAR) {
        if let WidgetKind::Label(ref mut lbl) = w.kind {
            lbl.text = tab_bar;
        }
    }

    // Content area (RawInputWidget with syntax-coloured lines).
    let display = tab.display_lines(VISIBLE_ROWS);
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == W_CONTENT) {
        if let WidgetKind::RawInput(ref mut ri) = w.kind {
            ri.display_lines = display;
            ri.scroll_offset = 0;
        }
    }

    // Find bar.
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == W_FIND_BAR) {
        if let WidgetKind::Label(ref mut lbl) = w.kind {
            lbl.text = if state.find.active {
                format!("Find: \"{}\" ({} matches)", state.find.query, state.find.matches.len())
            } else {
                String::from("Ctrl+S save | Ctrl+T new tab | Ctrl+W close | Ctrl+F find | Ctrl+H replace")
            };
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Key handling
// ─────────────────────────────────────────────────────────────────────────────

/// Handle a raw key event from the RawInputWidget.
///
/// `ascii`  – raw ASCII byte from the keyboard driver
/// `sc`     – PS/2 scancode (used to distinguish Backspace from Ctrl+H)
///
/// Ctrl+letter detection: ASCII control chars 0x01–0x1A (Ctrl+A … Ctrl+Z),
/// excluding:
///   0x08 with scancode 0x0E  — physical Backspace key
///   0x09                     — Tab (treated as literal insert)
///   0x0A / 0x0D              — Enter / Return
fn handle_key(state: &mut CodeEditorState, ascii: u8, sc: u8) {
    // Detect Ctrl+letter combos.
    let is_ctrl = ascii >= 1 && ascii <= 26
        && ascii != 0x09            // Tab
        && ascii != 0x0A            // LF / Enter
        && ascii != 0x0D            // CR / Enter
        && !(ascii == 0x08 && sc == 0x0E); // physical Backspace key

    if is_ctrl {
        // Convert control char back to lower-case letter (Ctrl+S = 0x13 = 's'-'a'+1 = 19)
        let letter = (b'a' + ascii - 1) as char;
        match letter {
            's' => {
                if let Some(tab) = state.tabs.get_mut(state.current_tab) {
                    match tab.save() {
                        Ok(_)  => state.status_msg = String::from("Saved."),
                        Err(e) => state.status_msg = format!("Save error: {}", e),
                    }
                }
            }
            't' => {
                state.tabs.push(Tab::new_empty());
                state.current_tab = state.tabs.len() - 1;
                state.status_msg = String::from("New tab.");
            }
            'w' => {
                if state.tabs.len() > 1 {
                    state.tabs.remove(state.current_tab);
                    state.current_tab = state.current_tab.min(state.tabs.len() - 1);
                    state.status_msg = String::from("Tab closed.");
                }
            }
            'f' => {
                state.find.active = !state.find.active;
                if state.find.active {
                    state.status_msg = String::from("Find mode: type query.");
                } else {
                    state.find.query.clear();
                    state.find.matches.clear();
                }
            }
            'n' => { state.find.next_match(); }
            'h' => {
                if state.find.active {
                    if let Some(tab) = state.tabs.get_mut(state.current_tab) {
                        state.find.replace_one(&mut tab.lines);
                        tab.modified = true;
                        state.status_msg = String::from("Replaced 1.");
                    }
                }
            }
            'l' => {
                // Ctrl+L — switch to next tab (Ctrl+Tab would both be 0x09).
                if !state.tabs.is_empty() {
                    state.current_tab = (state.current_tab + 1) % state.tabs.len();
                }
            }
            _ => {}
        }
        state.dirty = true;
        return;
    }

    // Regular / navigation key — insert into current line.
    let Some(tab) = state.tabs.get_mut(state.current_tab) else { return };

    match ascii {
        b'\n' | b'\r' => {
            let line = tab.lines.get(tab.cursor_line).cloned().unwrap_or_default();
            let before = line[..tab.cursor_col.min(line.len())].to_string();
            let after  = line[tab.cursor_col.min(line.len())..].to_string();
            tab.lines[tab.cursor_line] = before;
            tab.cursor_line += 1;
            tab.lines.insert(tab.cursor_line, after);
            tab.cursor_col = 0;
            tab.modified = true;
        }
        0x08 | 0x7F => {
            // Backspace / Delete.
            let cl = tab.cursor_line;
            let cc = tab.cursor_col;
            if cc > 0 {
                let line = &mut tab.lines[cl];
                if cc <= line.len() {
                    line.remove(cc - 1);
                    tab.cursor_col -= 1;
                }
                tab.modified = true;
            } else if cl > 0 {
                let removed = tab.lines.remove(cl);
                tab.cursor_line -= 1;
                let prev_len = tab.lines[tab.cursor_line].len();
                tab.lines[tab.cursor_line].push_str(&removed);
                tab.cursor_col = prev_len;
                tab.modified = true;
            }
        }
        c if c >= 32 => {
            // Printable character.
            if tab.cursor_line < tab.lines.len() {
                let line = &mut tab.lines[tab.cursor_line];
                let cc = tab.cursor_col.min(line.len());
                line.insert(cc, c as char);
                tab.cursor_col += 1;
                tab.modified = true;

                // Rebuild incremental find.
                if state.find.active {
                    state.find.search(&tab.lines);
                }
            }
        }
        _ => {}
    }
    state.dirty = true;
}

// ─────────────────────────────────────────────────────────────────────────────
//  Run
// ─────────────────────────────────────────────────────────────────────────────

pub fn run() {
    let window_id = {
        let mut desktop = DESKTOP.lock();
        let desk = match desktop.as_mut() { Some(d) => d, None => return };

        let mut win = Window::new("Code", 60, 40, 700, 520, ACCENT_BLUE);
        win.use_widgets = true;

        win.add_widget(Widget::new(W_STATUS,  0, 0,   700, 18,
            WidgetKind::Label(StaticLabel::new(" [new] | 1:1 | Text", ACCENT_CYAN))));
        win.add_widget(Widget::new(W_TAB_BAR, 0, 20,  700, 16,
            WidgetKind::Label(StaticLabel::new("[new]", TEXT_SECONDARY))));

        // Main RawInput text area.
        let mut raw = Widget::new(W_CONTENT, 0, 38, 700, 440,
            WidgetKind::RawInput(RawInputWidget::new()));
        raw.focused = true;
        win.add_widget(raw);

        win.add_widget(Widget::new(W_FIND_BAR, 0, 480, 700, 18,
            WidgetKind::Label(StaticLabel::new("Ctrl+S save | Ctrl+T tab | Ctrl+F find", TEXT_SECONDARY))));

        win.focused_widget = Some(W_CONTENT);
        let id = win.id;
        desk.wm.add(win);
        id
    };

    *STATE.lock() = Some(CodeEditorState {
        window_id,
        tabs: alloc::vec![Tab::new_empty()],
        current_tab: 0,
        find: FindReplace::new(),
        status_msg: String::new(),
        dirty: true,
    });

    loop {
        let action = crate::gui::input::poll_action(window_id);

        let mut need_sync = false;
        {
            let mut guard = STATE.lock();
            let state = match guard.as_mut() { Some(s) => s, None => break };

            match action {
                Some(WidgetAction::Execute(AppCommand::RawKey(ascii, sc))) => {
                    handle_key(state, ascii, sc);
                }
                Some(WidgetAction::Execute(AppCommand::ButtonClicked(_))) => {}
                _ => {}
            }

            if state.dirty {
                state.dirty = false;
                need_sync = true;
            }
        }

        if need_sync {
            let mut desktop = DESKTOP.lock();
            if let Some(desk) = desktop.as_mut() {
                if let Some(win) = desk.wm.get_mut(window_id) {
                    if let Some(st) = STATE.lock().as_ref() {
                        sync_gui(st, win);
                        win.dirty = true;
                    }
                }
            }
        }

        crate::process::scheduler::yield_now();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Self-test
// ─────────────────────────────────────────────────────────────────────────────

pub fn self_test() -> bool {
    let mut ok = true;

    // ── Test 1: Rust keyword detected ────────────────────────────────────────
    let toks = tokenise_line("let x = 42;", Lang::Rust);
    let has_kw = toks.iter().any(|t| t.kind == TokKind::Keyword && t.text == "let");
    if !has_kw { crate::serial_println!("[code-test] FAIL: 'let' not detected as keyword"); ok = false; }

    // ── Test 2: Number literal detected ──────────────────────────────────────
    let has_num = toks.iter().any(|t| t.kind == TokKind::Number && t.text == "42");
    if !has_num { crate::serial_println!("[code-test] FAIL: '42' not detected as number"); ok = false; }

    // ── Test 3: String literal detected ──────────────────────────────────────
    let toks2 = tokenise_line(r#"let s = "hello world";"#, Lang::Rust);
    let has_str = toks2.iter().any(|t| t.kind == TokKind::String && t.text.contains("hello"));
    if !has_str { crate::serial_println!("[code-test] FAIL: string literal not detected"); ok = false; }

    // ── Test 4: Comment detected ──────────────────────────────────────────────
    let toks3 = tokenise_line("// this is a comment", Lang::Rust);
    let has_comment = toks3.iter().any(|t| t.kind == TokKind::Comment);
    if !has_comment { crate::serial_println!("[code-test] FAIL: comment not detected"); ok = false; }

    // ── Test 5: dominant_color for comments → TEXT_SECONDARY ─────────────────
    let color = dominant_color(&toks3);
    if color != TEXT_SECONDARY {
        crate::serial_println!("[code-test] FAIL: comment line should be TEXT_SECONDARY");
        ok = false;
    }

    // ── Test 6: JS keywords ───────────────────────────────────────────────────
    let toks4 = tokenise_line("function hello(x) { return x + 1; }", Lang::Js);
    let has_fn = toks4.iter().any(|t| t.kind == TokKind::Keyword && t.text == "function");
    let has_ret = toks4.iter().any(|t| t.kind == TokKind::Keyword && t.text == "return");
    if !has_fn || !has_ret { crate::serial_println!("[code-test] FAIL: JS keywords not detected"); ok = false; }

    // ── Test 7: format_line adds gutter ──────────────────────────────────────
    let formatted = format_line(5, "hello");
    if !formatted.contains("5") || !formatted.contains("hello") {
        crate::serial_println!("[code-test] FAIL: format_line missing content");
        ok = false;
    }

    // ── Test 8: Lang::from_ext ────────────────────────────────────────────────
    if Lang::from_ext("main.rs")  != Lang::Rust  { crate::serial_println!("[code-test] FAIL: .rs detection"); ok = false; }
    if Lang::from_ext("app.js")   != Lang::Js    { crate::serial_println!("[code-test] FAIL: .js detection"); ok = false; }
    if Lang::from_ext("main.c")   != Lang::C     { crate::serial_println!("[code-test] FAIL: .c detection"); ok = false; }
    if Lang::from_ext("main.py")  != Lang::Python{ crate::serial_println!("[code-test] FAIL: .py detection"); ok = false; }
    if Lang::from_ext("notes.txt")!= Lang::Plain { crate::serial_println!("[code-test] FAIL: .txt detection"); ok = false; }

    // ── Test 9: FindReplace search ────────────────────────────────────────────
    let mut fr = FindReplace::new();
    fr.query = String::from("hello");
    let lines = alloc::vec![
        String::from("say hello to hello world"),
        String::from("goodbye"),
    ];
    fr.search(&lines);
    if fr.matches.len() != 2 {
        crate::serial_println!("[code-test] FAIL: expected 2 matches, got {}", fr.matches.len());
        ok = false;
    }

    // ── Test 10: FindReplace replace_all ─────────────────────────────────────
    let mut lines2 = alloc::vec![String::from("foo bar foo"), String::from("baz foo")];
    let mut fr2 = FindReplace::new();
    fr2.query = String::from("foo");
    fr2.replacement = String::from("qux");
    fr2.search(&lines2);
    fr2.replace_all(&mut lines2);
    if lines2[0] != "qux bar qux" || lines2[1] != "baz qux" {
        crate::serial_println!("[code-test] FAIL: replace_all wrong: {:?}", lines2);
        ok = false;
    }

    if ok { crate::serial_println!("[code-test] All 10 code editor tests PASSED"); }
    ok
}
