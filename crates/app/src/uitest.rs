//! Scripted UI runs (`MDM_UI_SCRIPT`), for screenshots and checks.
//!
//! A script feeds pointer and keyboard events into egui's input before the frame is processed,
//! so a test never moves the real pointer, types on the real keyboard, or raises and focuses the
//! window. Coordinates are client points, the same space the macOS main window lays out in:
//!
//! ```text
//! # comments are ignored
//! click 120 200      # one click
//! dblclick 400 330   # two clicks, close enough in time for egui's double click
//! rclick 400 330     # a right click (a menu)
//! move 400 330
//! key cmd+n
//! text https://example.com/file.zip
//! wait 10            # let the app draw ten frames
//! ```
//!
//! Only debug builds install it, and only the root viewport is driven: dialogs open from the
//! root's shortcuts, and their content is checked in the screenshot, not by typing into them.

use std::sync::Mutex;

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, ViewportId, pos2};

/// Installs the driver when `MDM_UI_SCRIPT` names a script file.
pub fn install(options: &mut eframe::NativeOptions) {
    let Ok(path) = std::env::var("MDM_UI_SCRIPT") else {
        return;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("MDM_UI_SCRIPT: cannot read {path}: {error}");
            return;
        }
    };
    let steps = parse(&text);
    if steps.is_empty() {
        eprintln!("MDM_UI_SCRIPT: {path} has no steps");
        return;
    }
    eprintln!("MDM_UI_SCRIPT: {} step(s) from {path}", steps.len());
    options.viewport_hooks = Some(std::sync::Arc::new(Driver {
        state: Mutex::new(State {
            steps,
            at: 0,
            wait: 0,
        }),
    }));
}

struct Driver {
    state: Mutex<State>,
}

impl eframe::ViewportHooks for Driver {
    fn raw_input(&self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if raw.viewport_id != ViewportId::ROOT {
            return;
        }
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };
        if state.at >= state.steps.len() {
            return;
        }
        // An unfocused window still receives the script: it must not ask for focus to be tested.
        raw.focused = true;
        state.step(raw);
        ctx.request_repaint();
    }
}

struct State {
    steps: Vec<Step>,
    at: usize,
    wait: u32,
}

impl State {
    fn step(&mut self, raw: &mut egui::RawInput) {
        if self.wait > 0 {
            self.wait -= 1;
            return;
        }
        let Some(step) = self.steps.get(self.at).cloned() else {
            return;
        };
        match step {
            Step::Wait(frames) => {
                self.wait = frames;
                self.at += 1;
            }
            Step::Move(pos) => {
                raw.events.push(Event::PointerMoved(pos));
                self.at += 1;
            }
            Step::Click {
                pos,
                double,
                button,
            } => self.click(raw, pos, double, button),
            Step::Key { key, modifiers } => {
                raw.events.push(Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers,
                });
                raw.events.push(Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: false,
                    repeat: false,
                    modifiers,
                });
                self.at += 1;
            }
            Step::Text(text) => {
                raw.events.push(Event::Text(text));
                self.at += 1;
            }
        }
    }

    /// A click is delivered as one frame's worth of events: the press and the release
    /// together. Splitting them across frames let a popup that opened on the press see the
    /// release as an outside click and close again, which no real click does.
    fn click(&mut self, raw: &mut egui::RawInput, pos: Pos2, double: bool, button: PointerButton) {
        let press = |pressed: bool| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::default(),
        };
        raw.events.push(Event::PointerMoved(pos));
        raw.events.push(press(true));
        raw.events.push(press(false));
        if double {
            raw.events.push(press(true));
            raw.events.push(press(false));
        }
        self.at += 1;
    }
}

#[derive(Clone)]
enum Step {
    Move(Pos2),
    Click {
        pos: Pos2,
        double: bool,
        button: PointerButton,
    },
    Key {
        key: Key,
        modifiers: Modifiers,
    },
    Text(String),
    Wait(u32),
}

fn parse(text: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    for (line, source) in text.lines().enumerate() {
        let source = source.split('#').next().unwrap_or("").trim();
        if source.is_empty() {
            continue;
        }
        let (command, rest) = match source.split_once(' ') {
            Some((command, rest)) => (command, rest.trim()),
            None => (source, ""),
        };
        let step = match command {
            "click" | "dblclick" | "rclick" => point(rest).map(|pos| Step::Click {
                pos,
                double: command == "dblclick",
                button: if command == "rclick" {
                    PointerButton::Secondary
                } else {
                    PointerButton::Primary
                },
            }),
            "move" => point(rest).map(Step::Move),
            "key" => key(rest).map(|(key, modifiers)| Step::Key { key, modifiers }),
            "text" => Some(Step::Text(rest.to_owned())),
            "wait" => rest.parse().ok().map(Step::Wait),
            _ => None,
        };
        match step {
            Some(step) => steps.push(step),
            None => eprintln!("MDM_UI_SCRIPT: line {}: cannot read {source:?}", line + 1),
        }
    }
    steps
}

fn point(text: &str) -> Option<Pos2> {
    let mut parts = text.split_whitespace();
    let x: f32 = parts.next()?.parse().ok()?;
    let y: f32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(pos2(x, y))
}

fn key(text: &str) -> Option<(Key, Modifiers)> {
    let mut modifiers = Modifiers::default();
    let mut parts = text.split('+');
    let name = parts.next_back()?;
    for part in parts {
        match part.to_ascii_lowercase().as_str() {
            "cmd" | "command" => {
                modifiers.command = true;
                modifiers.mac_cmd = true;
            }
            "ctrl" => modifiers.ctrl = true,
            "alt" | "option" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            _ => return None,
        }
    }
    let key = match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Key::Enter,
        "esc" | "escape" => Key::Escape,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "comma" => Key::Comma,
        "period" => Key::Period,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        _ => Key::from_name(name)?,
    };
    Some((key, modifiers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_reads_into_steps() {
        let steps = parse("click 10 20\ndblclick 300 40\nkey cmd+shift+n\ntext hi there\nwait 5");
        assert_eq!(steps.len(), 5);
        assert!(matches!(
            steps[0],
            Step::Click {
                double: false,
                button: PointerButton::Primary,
                ..
            }
        ));
        assert!(matches!(
            steps[1],
            Step::Click {
                double: true,
                button: PointerButton::Primary,
                ..
            }
        ));
        match &steps[2] {
            Step::Key { key, modifiers } => {
                assert_eq!(*key, Key::N);
                assert!(modifiers.command && modifiers.shift);
            }
            _ => panic!("expected a key step"),
        }
        assert!(matches!(&steps[3], Step::Text(text) if text == "hi there"));
        assert!(matches!(steps[4], Step::Wait(5)));
    }

    #[test]
    fn a_bad_line_is_skipped_and_the_rest_still_runs() {
        let steps = parse("click nope\nkey cmd+\nwait 2");
        assert_eq!(steps.len(), 1);
        assert!(matches!(steps[0], Step::Wait(2)));
    }

    #[test]
    fn comments_and_empty_lines_are_ignored() {
        assert!(parse("# nothing here\n\n   \n").is_empty());
        assert_eq!(parse("wait 1 # one frame").len(), 1);
    }
}
