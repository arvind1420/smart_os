/// Smart OS — Gaming & Entertainment Hub (Phase 77, v0.37.0)
///
/// Game launcher, achievement system, and leaderboard:
/// • `GameEntry`       — installed game (title, genre, playtime, rating)
/// • `Achievement`     — unlockable trophy with points and rarity
/// • `LeaderboardEntry`— high-score record (player, score, timestamp)
/// • `GameSession`     — active session tracker (start/stop, duration)
/// • `GameLibrary`     — full catalogue with filter/sort
/// GUI: game list, achievements panel, leaderboard, launch/quit controls

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use spin::Mutex;

use crate::gui::window::{Window, WindowId};
use crate::gui::desktop::DESKTOP;
use crate::gui::theme::*;
use crate::gui::widget::{
    Widget, WidgetKind, Button, StaticLabel, ScrollableText, AppCommand, WidgetAction,
};

// ─── Genre ────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Genre { Puzzle, Strategy, Arcade, Simulation, Adventure, Other }

impl Genre {
    pub fn name(self) -> &'static str {
        match self {
            Genre::Puzzle     => "Puzzle",
            Genre::Strategy   => "Strategy",
            Genre::Arcade     => "Arcade",
            Genre::Simulation => "Simulation",
            Genre::Adventure  => "Adventure",
            Genre::Other      => "Other",
        }
    }
    pub fn color(self) -> Color {
        match self {
            Genre::Puzzle     => ACCENT_CYAN,
            Genre::Strategy   => ACCENT_BLUE,
            Genre::Arcade     => ACCENT_ORANGE,
            Genre::Simulation => ACCENT_GREEN,
            Genre::Adventure  => ACCENT_MAGENTA,
            Genre::Other      => TEXT_SECONDARY,
        }
    }
}

// ─── Game entry ───────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct GameEntry {
    pub id:           u32,
    pub title:        String,
    pub genre:        Genre,
    pub version:      String,
    pub size_mb:      u32,
    pub playtime_min: u32,  // cumulative minutes played
    pub rating:       u8,   // 0-100
    pub installed:    bool,
    pub running:      bool,
}

impl GameEntry {
    pub fn summary(&self) -> String {
        let run_marker = if self.running { " ▶" } else { "" };
        format!("[{:10}] {:20}  v{}  {}★  {}h{}m{}",
            self.genre.name(),
            self.title,
            self.version,
            self.rating / 10,
            self.playtime_min / 60,
            self.playtime_min % 60,
            run_marker)
    }

    pub fn rating_stars(&self) -> String {
        let full  = (self.rating / 20) as usize;
        let empty = 5usize.saturating_sub(full);
        let mut s = String::new();
        for _ in 0..full  { s.push('★'); }
        for _ in 0..empty { s.push('☆'); }
        s
    }
}

// ─── Achievement ─────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Rarity { Common, Uncommon, Rare, Epic, Legendary }

impl Rarity {
    pub fn name(self) -> &'static str {
        match self {
            Rarity::Common    => "Common",
            Rarity::Uncommon  => "Uncommon",
            Rarity::Rare      => "Rare",
            Rarity::Epic      => "Epic",
            Rarity::Legendary => "Legendary",
        }
    }
    pub fn color(self) -> Color {
        match self {
            Rarity::Common    => TEXT_SECONDARY,
            Rarity::Uncommon  => ACCENT_GREEN,
            Rarity::Rare      => ACCENT_BLUE,
            Rarity::Epic      => ACCENT_MAGENTA,
            Rarity::Legendary => ACCENT_ORANGE,
        }
    }
    pub fn points(self) -> u32 {
        match self {
            Rarity::Common    => 10,
            Rarity::Uncommon  => 25,
            Rarity::Rare      => 50,
            Rarity::Epic      => 100,
            Rarity::Legendary => 250,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Achievement {
    pub id:          u32,
    pub title:       String,
    pub description: String,
    pub rarity:      Rarity,
    pub unlocked:    bool,
    pub game_id:     u32,
}

impl Achievement {
    pub fn summary(&self) -> String {
        let lock = if self.unlocked { "✓" } else { "⬡" };
        format!("{} [{:10}] {:25}  {}pts  — {}",
            lock, self.rarity.name(), self.title,
            self.rarity.points(), self.description)
    }
    pub fn points_earned(&self) -> u32 {
        if self.unlocked { self.rarity.points() } else { 0 }
    }
}

// ─── Leaderboard ─────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct LeaderboardEntry {
    pub rank:      u32,
    pub player:    String,
    pub score:     u64,
    pub game_id:   u32,
    pub timestamp: u64,
}

impl LeaderboardEntry {
    pub fn summary(&self) -> String {
        format!("#{:2}  {:16}  {:>12}pts  t={}s",
            self.rank, self.player, self.score, self.timestamp)
    }
}

// ─── Game session ─────────────────────────────────────────────────────────────
pub struct GameSession {
    pub game_id:     u32,
    pub start_tick:  u64,   // uptime_secs at launch
    pub score:       u64,
    pub active:      bool,
}

impl GameSession {
    pub fn new(game_id: u32) -> Self {
        GameSession {
            game_id,
            start_tick: crate::drivers::timer::uptime_secs(),
            score: 0,
            active: true,
        }
    }
    pub fn elapsed_secs(&self) -> u64 {
        crate::drivers::timer::uptime_secs().saturating_sub(self.start_tick)
    }
    pub fn elapsed_minutes(&self) -> u32 {
        (self.elapsed_secs() / 60) as u32
    }
}

// ─── Library ─────────────────────────────────────────────────────────────────
pub struct GameLibrary {
    pub games:    Vec<GameEntry>,
    pub filtered: Vec<usize>,
}

impl GameLibrary {
    pub fn new(games: Vec<GameEntry>) -> Self {
        let n = games.len();
        GameLibrary { games, filtered: (0..n).collect() }
    }
    pub fn filter_by_genre(&mut self, genre: Option<Genre>) {
        self.filtered = self.games.iter().enumerate()
            .filter(|(_, g)| genre.map_or(true, |gn| g.genre == gn))
            .map(|(i, _)| i)
            .collect();
    }
    pub fn total_playtime_min(&self) -> u32 {
        self.games.iter().map(|g| g.playtime_min).sum()
    }
    pub fn installed_count(&self) -> usize {
        self.games.iter().filter(|g| g.installed).count()
    }
}

// ─── Total achievement score ──────────────────────────────────────────────────
pub fn total_achievement_score(achievements: &[Achievement]) -> u32 {
    achievements.iter().map(|a| a.points_earned()).sum()
}

// ─── Sample data ─────────────────────────────────────────────────────────────
fn sample_games() -> Vec<GameEntry> {
    vec![
        GameEntry { id: 1, title: "BlockFall".to_string(),       genre: Genre::Puzzle,
            version: "1.0.0".to_string(), size_mb: 12,  playtime_min: 240, rating: 80, installed: true,  running: false },
        GameEntry { id: 2, title: "OsWars Strategy".to_string(), genre: Genre::Strategy,
            version: "2.1.0".to_string(), size_mb: 256, playtime_min: 1440, rating: 90, installed: true,  running: false },
        GameEntry { id: 3, title: "Retro Dash".to_string(),       genre: Genre::Arcade,
            version: "0.9.1".to_string(), size_mb: 8,   playtime_min: 60,  rating: 70, installed: true,  running: false },
        GameEntry { id: 4, title: "Farm Sim".to_string(),         genre: Genre::Simulation,
            version: "3.0.0".to_string(), size_mb: 512, playtime_min: 2160, rating: 85, installed: true,  running: false },
        GameEntry { id: 5, title: "Kernel Quest".to_string(),     genre: Genre::Adventure,
            version: "1.2.0".to_string(), size_mb: 64,  playtime_min: 360, rating: 95, installed: false, running: false },
    ]
}

fn sample_achievements() -> Vec<Achievement> {
    vec![
        Achievement { id: 1, title: "First Boot".to_string(),
            description: "Start your first game".to_string(),
            rarity: Rarity::Common, unlocked: true, game_id: 0 },
        Achievement { id: 2, title: "Puzzle Master".to_string(),
            description: "Complete 100 puzzle levels".to_string(),
            rarity: Rarity::Rare, unlocked: true, game_id: 1 },
        Achievement { id: 3, title: "Strategist".to_string(),
            description: "Win 10 strategy campaigns".to_string(),
            rarity: Rarity::Uncommon, unlocked: false, game_id: 2 },
        Achievement { id: 4, title: "Speed Demon".to_string(),
            description: "Score 100k in Retro Dash".to_string(),
            rarity: Rarity::Epic, unlocked: false, game_id: 3 },
        Achievement { id: 5, title: "OS Conqueror".to_string(),
            description: "Finish Kernel Quest on hard mode".to_string(),
            rarity: Rarity::Legendary, unlocked: false, game_id: 5 },
    ]
}

fn sample_leaderboard() -> Vec<LeaderboardEntry> {
    vec![
        LeaderboardEntry { rank: 1, player: "Alice".to_string(),    score: 99_500, game_id: 1, timestamp: 3600 },
        LeaderboardEntry { rank: 2, player: "Bob".to_string(),      score: 87_200, game_id: 1, timestamp: 7200 },
        LeaderboardEntry { rank: 3, player: "Charlie".to_string(),  score: 72_100, game_id: 3, timestamp: 1800 },
        LeaderboardEntry { rank: 4, player: "Diana".to_string(),    score: 65_000, game_id: 2, timestamp: 9000 },
        LeaderboardEntry { rank: 5, player: "SmartOS".to_string(),  score: 50_000, game_id: 1, timestamp: 600  },
    ]
}

// ─── GUI state ────────────────────────────────────────────────────────────────
pub struct GamingState {
    pub window_id:    WindowId,
    pub library:      GameLibrary,
    pub achievements: Vec<Achievement>,
    pub leaderboard:  Vec<LeaderboardEntry>,
    pub session:      Option<GameSession>,
    pub selected:     usize,
    pub tab:          u8,   // 0=Library 1=Achievements 2=Leaderboard
    pub status:       String,
    pub dirty:        bool,
}

pub static STATE: Mutex<Option<GamingState>> = Mutex::new(None);

fn create_window() -> WindowId {
    let mut dg = DESKTOP.lock();
    let desk = dg.as_mut().expect("desktop");
    let mut win = Window::new("Gaming", 180, 90, 780, 540, ACCENT_MAGENTA);
    win.use_widgets = true;

    // Title
    win.widgets.push(Widget::new(0, 4, 4, 760, 16,
        WidgetKind::Label(StaticLabel::new("Gaming & Entertainment Hub", TEXT_PRIMARY))));
    // Tab buttons
    win.widgets.push(Widget::new(1,   4, 24, 110, 26,
        WidgetKind::Button(Button::new("📚 Library",      ACCENT_BLUE,    AppCommand::ButtonClicked(1)))));
    win.widgets.push(Widget::new(2, 118, 24, 130, 26,
        WidgetKind::Button(Button::new("🏆 Achievements", ACCENT_ORANGE,  AppCommand::ButtonClicked(2)))));
    win.widgets.push(Widget::new(3, 252, 24, 120, 26,
        WidgetKind::Button(Button::new("📊 Leaderboard",  ACCENT_CYAN,    AppCommand::ButtonClicked(3)))));
    // Action buttons
    win.widgets.push(Widget::new(4, 400, 24, 100, 26,
        WidgetKind::Button(Button::new("▶ Launch",        ACCENT_GREEN,   AppCommand::ButtonClicked(4)))));
    win.widgets.push(Widget::new(5, 504, 24,  90, 26,
        WidgetKind::Button(Button::new("■ Stop",          ACCENT_RED,     AppCommand::ButtonClicked(5)))));
    win.widgets.push(Widget::new(6, 598, 24,  80, 26,
        WidgetKind::Button(Button::new("↑ Score",         ACCENT_MAGENTA, AppCommand::ButtonClicked(6)))));
    // Status bar
    win.widgets.push(Widget::new(7, 4, 54, 760, 16,
        WidgetKind::Label(StaticLabel::new("Library | Ready.", TEXT_SECONDARY))));
    // Main content scroll
    win.widgets.push(Widget::new(8, 4, 74, 760, 440,
        WidgetKind::ScrollText(ScrollableText::new(128))));

    let id = win.id;
    desk.wm.add(win);
    id
}

pub fn sync_to_window(win: &mut Window) {
    let guard = STATE.lock();
    let s = match guard.as_ref() { Some(x) => x, None => return };
    if !s.dirty { return; }

    // Status bar
    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 7) {
        if let WidgetKind::Label(ref mut l) = w.kind {
            let tab_name = match s.tab { 1 => "Achievements", 2 => "Leaderboard", _ => "Library" };
            let session_str = if let Some(ref sess) = s.session {
                format!("  |  ▶ Running ({}m)", sess.elapsed_minutes())
            } else {
                String::new()
            };
            l.text = format!("{}{}  |  {}", tab_name, session_str, s.status);
        }
    }

    // Content pane
    let lines: Vec<(String, Color)> = match s.tab {
        0 => {
            // Library
            let score = total_achievement_score(&s.achievements);
            let mut v: Vec<(String, Color)> = Vec::new();
            v.push((format!("  Installed: {}  |  Total playtime: {}h  |  Achievement score: {}pts",
                s.library.installed_count(),
                s.library.total_playtime_min() / 60,
                score), TEXT_MUTED));
            v.push(("─".repeat(80), TEXT_MUTED));
            for (i, idx) in s.library.filtered.iter().enumerate() {
                let g = &s.library.games[*idx];
                let sel = if i == s.selected { "►" } else { " " };
                let color = if g.running { ACCENT_GREEN } else if g.installed { TEXT_PRIMARY } else { TEXT_MUTED };
                v.push((format!("{} {}", sel, g.summary()), color));
            }
            v
        }
        1 => {
            // Achievements
            let unlocked = s.achievements.iter().filter(|a| a.unlocked).count();
            let total_pts = total_achievement_score(&s.achievements);
            let mut v: Vec<(String, Color)> = Vec::new();
            v.push((format!("  Unlocked: {}/{}  |  Total pts: {}", unlocked, s.achievements.len(), total_pts), TEXT_MUTED));
            v.push(("─".repeat(80), TEXT_MUTED));
            for a in &s.achievements {
                let color = if a.unlocked { a.rarity.color() } else { TEXT_MUTED };
                v.push((a.summary(), color));
            }
            v
        }
        _ => {
            // Leaderboard
            let mut v: Vec<(String, Color)> = Vec::new();
            v.push(("  Global High Scores".to_string(), TEXT_MUTED));
            v.push(("─".repeat(80), TEXT_MUTED));
            for (i, entry) in s.leaderboard.iter().enumerate() {
                let color = match i { 0 => ACCENT_ORANGE, 1 => TEXT_SECONDARY, 2 => ACCENT_CYAN, _ => TEXT_PRIMARY };
                v.push((entry.summary(), color));
            }
            v
        }
    };

    if let Some(w) = win.widgets.iter_mut().find(|w| w.id == 8) {
        if let WidgetKind::ScrollText(ref mut st) = w.kind { st.lines = lines; }
    }
    win.dirty = true;
}

pub fn run() {
    let window_id = create_window();
    let library = GameLibrary::new(sample_games());
    *STATE.lock() = Some(GamingState {
        window_id,
        library,
        achievements: sample_achievements(),
        leaderboard:  sample_leaderboard(),
        session:      None,
        selected:     0,
        tab:          0,
        status:       "Ready.".to_string(),
        dirty:        true,
    });

    loop {
        if let Some(action) = crate::gui::input::poll_action(window_id) {
            let mut guard = STATE.lock();
            if let Some(ref mut s) = *guard {
                match action {
                    WidgetAction::Execute(AppCommand::ButtonClicked(1)) => {
                        s.tab = 0; s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(2)) => {
                        s.tab = 1; s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(3)) => {
                        s.tab = 2; s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(4)) => {
                        // Launch selected game
                        if let Some(&idx) = s.library.filtered.get(s.selected) {
                            let game = &mut s.library.games[idx];
                            if game.installed && !game.running {
                                game.running = true;
                                s.session = Some(GameSession::new(game.id));
                                s.status = format!("Launched '{}'.", game.title.clone());
                            } else if !game.installed {
                                s.status = format!("'{}' not installed.", game.title.clone());
                            }
                        }
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(5)) => {
                        // Stop running game
                        if let Some(sess) = s.session.take() {
                            let elapsed_m = sess.elapsed_minutes();
                            for g in s.library.games.iter_mut() {
                                if g.id == sess.game_id {
                                    g.running = false;
                                    g.playtime_min += elapsed_m;
                                    s.status = format!("Stopped '{}' after {}m.", g.title.clone(), elapsed_m);
                                    break;
                                }
                            }
                        } else {
                            s.status = "No game running.".to_string();
                        }
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::ButtonClicked(6)) => {
                        // Award score to session
                        if let Some(ref mut sess) = s.session {
                            sess.score += 1000;
                            s.status = format!("Score: {}", sess.score);
                        } else {
                            s.status = "No active session.".to_string();
                        }
                        s.dirty = true;
                    }
                    WidgetAction::Execute(AppCommand::LineClicked(row)) => {
                        if row < s.library.filtered.len() {
                            s.selected = row;
                            s.dirty = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        crate::process::scheduler::yield_now();
    }
}

pub fn self_test() -> bool {
    let mut ok = true;

    // T1: Genre names non-empty
    for g in [Genre::Puzzle, Genre::Strategy, Genre::Arcade, Genre::Simulation, Genre::Adventure, Genre::Other] {
        if g.name().is_empty() { ok = false; }
    }

    // T2: Rarity points ordered correctly
    if !(Rarity::Common.points() < Rarity::Uncommon.points()
      && Rarity::Uncommon.points() < Rarity::Rare.points()
      && Rarity::Rare.points() < Rarity::Epic.points()
      && Rarity::Epic.points() < Rarity::Legendary.points()) {
        ok = false;
    }

    // T3: rating_stars length == 5
    let game = GameEntry { id: 0, title: "Test".to_string(), genre: Genre::Puzzle,
        version: "1.0".to_string(), size_mb: 0, playtime_min: 0,
        rating: 80, installed: true, running: false };
    if game.rating_stars().chars().count() != 5 { ok = false; }

    // T4: Achievement points_earned only if unlocked
    let ach_locked   = Achievement { id: 0, title: "A".to_string(), description: "D".to_string(),
        rarity: Rarity::Epic, unlocked: false, game_id: 0 };
    let ach_unlocked = Achievement { id: 1, title: "B".to_string(), description: "D".to_string(),
        rarity: Rarity::Epic, unlocked: true, game_id: 0 };
    if ach_locked.points_earned()   != 0   { ok = false; }
    if ach_unlocked.points_earned() != 100 { ok = false; }

    // T5: total_achievement_score
    let achs = [ach_locked.clone(), ach_unlocked.clone()];
    if total_achievement_score(&achs) != 100 { ok = false; }

    // T6: GameLibrary installed count
    let games = sample_games();
    let installed = games.iter().filter(|g| g.installed).count();
    let lib = GameLibrary::new(sample_games());
    if lib.installed_count() != installed { ok = false; }

    // T7: GameLibrary filter_by_genre
    let mut lib2 = GameLibrary::new(sample_games());
    lib2.filter_by_genre(Some(Genre::Arcade));
    // Only Retro Dash is Arcade
    if lib2.filtered.len() != 1 { ok = false; }

    // T8: GameLibrary filter None shows all
    lib2.filter_by_genre(None);
    if lib2.filtered.len() != sample_games().len() { ok = false; }

    // T9: LeaderboardEntry summary non-empty
    let lb = sample_leaderboard();
    for entry in &lb { if entry.summary().is_empty() { ok = false; } }

    // T10: sample data non-empty
    if sample_games().is_empty() { ok = false; }
    if sample_achievements().is_empty() { ok = false; }

    ok
}
