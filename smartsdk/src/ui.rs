/// Retained-Mode UI Framework for Smart OS.

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::String;
use crate::gui::{Window, EVENT_MOUSE_CLICK, GuiEvent};

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

pub trait Widget {
    fn draw(&self, win: &Window, rect: Rect);
    fn layout(&self, available: Rect) -> Rect;
    fn handle_event(&mut self, ev: &GuiEvent, rect: Rect) -> bool;
}

pub struct Label {
    pub text: String,
}

impl Label {
    pub fn new(text: &str) -> Self {
        Self { text: String::from(text) }
    }
}

impl Widget for Label {
    fn draw(&self, win: &Window, rect: Rect) {
        win.draw_text(rect.x, rect.y, &self.text);
    }
    fn layout(&self, available: Rect) -> Rect {
        Rect { x: available.x, y: available.y, w: (self.text.len() * 8) as u16, h: 20 }
    }
    fn handle_event(&mut self, _ev: &GuiEvent, _rect: Rect) -> bool { false }
}

pub struct Button {
    pub text: String,
    pub clicked: bool,
}

impl Button {
    pub fn new(text: &str) -> Self {
        Self { text: String::from(text), clicked: false }
    }
}

impl Widget for Button {
    fn draw(&self, win: &Window, rect: Rect) {
        win.fill_rect(rect.x, rect.y, rect.w, rect.h, if self.clicked { 0x666666 } else { 0x333333 });
        win.draw_text(rect.x + 5, rect.y + 5, &self.text);
    }
    fn layout(&self, available: Rect) -> Rect {
        Rect { x: available.x, y: available.y, w: (self.text.len() * 8 + 20) as u16, h: 30 }
    }
    fn handle_event(&mut self, ev: &GuiEvent, rect: Rect) -> bool {
        if ev.event_type == EVENT_MOUSE_CLICK {
            let mx = ev.data[0] as u16;
            let my = ev.data[1] as u16;
            if mx >= rect.x && mx <= rect.x + rect.w && my >= rect.y && my <= rect.y + rect.h {
                self.clicked = true;
                return true;
            }
        }
        self.clicked = false;
        false
    }
}

pub struct VBox {
    pub children: Vec<Box<dyn Widget>>,
    pub spacing: u16,
}

impl VBox {
    pub fn new() -> Self {
        Self { children: Vec::new(), spacing: 5 }
    }
    pub fn add<W: Widget + 'static>(&mut self, child: W) {
        self.children.push(Box::new(child));
    }
}

impl Widget for VBox {
    fn draw(&self, win: &Window, rect: Rect) {
        let mut y = rect.y;
        for child in &self.children {
            let child_pref = child.layout(Rect { x: rect.x, y, w: rect.w, h: 30 });
            child.draw(win, Rect { x: rect.x, y, w: rect.w, h: child_pref.h });
            y += child_pref.h + self.spacing;
        }
    }
    fn layout(&self, available: Rect) -> Rect {
        let mut h = 0;
        for child in &self.children {
            h += child.layout(available).h + self.spacing;
        }
        Rect { x: available.x, y: available.y, w: available.w, h }
    }
    fn handle_event(&mut self, ev: &GuiEvent, rect: Rect) -> bool {
        let mut y = rect.y;
        for child in &mut self.children {
            let child_pref = child.layout(Rect { x: rect.x, y, w: rect.w, h: 30 });
            if child.handle_event(ev, Rect { x: rect.x, y, w: rect.w, h: child_pref.h }) {
                return true;
            }
            y += child_pref.h + self.spacing;
        }
        false
    }
}

pub struct HBox {
    pub children: Vec<Box<dyn Widget>>,
    pub spacing: u16,
}

impl HBox {
    pub fn new() -> Self {
        Self { children: Vec::new(), spacing: 10 }
    }
    pub fn add<W: Widget + 'static>(&mut self, child: W) {
        self.children.push(Box::new(child));
    }
}

impl Widget for HBox {
    fn draw(&self, win: &Window, rect: Rect) {
        let mut x = rect.x;
        for child in &self.children {
            let child_pref = child.layout(Rect { x, y: rect.y, w: rect.w, h: rect.h });
            child.draw(win, Rect { x, y: rect.y, w: child_pref.w, h: rect.h });
            x += child_pref.w + self.spacing;
        }
    }
    fn layout(&self, available: Rect) -> Rect {
        let mut w = 0;
        for child in &self.children {
            w += child.layout(available).w + self.spacing;
        }
        Rect { x: available.x, y: available.y, w, h: available.h }
    }
    fn handle_event(&mut self, ev: &GuiEvent, rect: Rect) -> bool {
        let mut x = rect.x;
        for child in &mut self.children {
            let child_pref = child.layout(Rect { x, y: rect.y, w: rect.w, h: rect.h });
            if child.handle_event(ev, Rect { x, y: rect.y, w: child_pref.w, h: rect.h }) {
                return true;
            }
            x += child_pref.w + self.spacing;
        }
        false
    }
}

pub struct Icon {
    pub name: String,
    pub path: String,
    pub clicked: bool,
}

impl Icon {
    pub fn new(name: &str, path: &str) -> Self {
        Self { name: String::from(name), path: String::from(path), clicked: false }
    }
}

impl Widget for Icon {
    fn draw(&self, win: &Window, rect: Rect) {
        win.fill_rect(rect.x + 10, rect.y, 40, 40, if self.clicked { 0xAAAAAA } else { 0x4444FF });
        win.draw_text(rect.x, rect.y + 45, &self.name);
    }
    fn layout(&self, _available: Rect) -> Rect {
        Rect { x: 0, y: 0, w: 60, h: 70 }
    }
    fn handle_event(&mut self, ev: &GuiEvent, rect: Rect) -> bool {
        if ev.event_type == EVENT_MOUSE_CLICK {
            let mx = ev.data[0] as u16;
            let my = ev.data[1] as u16;
            if mx >= rect.x && mx <= rect.x + rect.w && my >= rect.y && my <= rect.y + rect.h {
                self.clicked = true;
                // Auto-spawn the app when clicked!
                crate::syscall::syscall2(
                    crate::syscall::SYS_SPAWN, 
                    self.path.as_ptr() as u64, 
                    self.path.len() as u64
                );
                return true;
            }
        }
        self.clicked = false;
        false
    }
}

pub struct UiApp {
    pub win: Window,
    pub root: Box<dyn Widget>,
    pub bg_color: u32,
}

impl UiApp {
    pub fn new(title: &str, width: u32, height: u32, root: Box<dyn Widget>) -> Option<Self> {
        let win = Window::new(width, height)?;
        Some(Self { win, root, bg_color: 0x111111 })
    }

    pub fn run<F>(&mut self, mut callback: F) 
    where F: FnMut(&mut Box<dyn Widget>) {
        let base_rect = Rect { x: 0, y: 0, w: 1024, h: 768 };
        
        loop {
            self.win.fill_rect(0, 0, 1024, 768, self.bg_color);
            self.root.draw(&self.win, base_rect);

            while let Some(ev) = Window::poll_event() {
                self.root.handle_event(&ev, base_rect);
                callback(&mut self.root);
            }

            for _ in 0..500_000 { core::hint::spin_loop(); }
        }
    }
}
