//! Keyboard shortcuts: the commands that have them, their usual keys, and
//! the ones set in the settings instead. A key matches only with exactly
//! its modifiers (Ctrl+F is not Ctrl+Shift+F).

use std::collections::BTreeMap;

use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};

use crate::i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Cmd {
    RunStatement,
    RunScript,
    Explain,
    Cancel,
    Commit,
    Rollback,
    Format,
    Save,
    Hints,
    Comment,
    Duplicate,
    Find,
    Outline,
    ShowSpec,
    ShowBody,
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
}

impl Cmd {
    pub const ALL: [Cmd; 19] = [
        Cmd::RunStatement,
        Cmd::RunScript,
        Cmd::Explain,
        Cmd::Cancel,
        Cmd::Commit,
        Cmd::Rollback,
        Cmd::Format,
        Cmd::Save,
        Cmd::Hints,
        Cmd::Comment,
        Cmd::Duplicate,
        Cmd::Find,
        Cmd::Outline,
        Cmd::ShowSpec,
        Cmd::ShowBody,
        Cmd::NewTab,
        Cmd::CloseTab,
        Cmd::NextTab,
        Cmd::PrevTab,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Cmd::RunStatement => t("Выполнить запрос под курсором", "Run the statement"),
            Cmd::RunScript => t("Выполнить скрипт", "Run the script"),
            Cmd::Explain => t("План запроса", "Explain plan"),
            Cmd::Cancel => t("Остановить", "Stop"),
            Cmd::Commit => t("Зафиксировать (commit)", "Commit"),
            Cmd::Rollback => t("Откатить (rollback)", "Roll back"),
            Cmd::Format => t("Форматировать", "Format"),
            Cmd::Save => t("Сохранить файл", "Save the file"),
            Cmd::Hints => t("Подсказки", "Hints"),
            Cmd::Comment => t("Закомментировать строки", "Comment the lines"),
            Cmd::Duplicate => t("Повторить строку", "Duplicate the line"),
            Cmd::Find => t("Поиск", "Find"),
            Cmd::Outline => t("Структура скрипта", "Script outline"),
            Cmd::ShowSpec => t("Пакет: спецификация", "Package: specification"),
            Cmd::ShowBody => t("Пакет: тело", "Package: body"),
            Cmd::NewTab => t("Новая вкладка", "New tab"),
            Cmd::CloseTab => t("Закрыть вкладку", "Close the tab"),
            Cmd::NextTab => t("Следующая вкладка", "Next tab"),
            Cmd::PrevTab => t("Предыдущая вкладка", "Previous tab"),
        }
    }

    /// Where it's listed.
    pub fn group(self) -> &'static str {
        match self {
            Cmd::RunStatement | Cmd::RunScript | Cmd::Explain | Cmd::Cancel | Cmd::Commit | Cmd::Rollback => t("Выполнение", "Running"),
            Cmd::Format | Cmd::Save | Cmd::Hints | Cmd::Comment | Cmd::Duplicate | Cmd::Find => t("Редактор", "Editor"),
            _ => t("Окно и вкладки", "Window and tabs"),
        }
    }

    /// Its usual keys.
    pub fn defaults(self) -> Vec<Chord> {
        let c = |k| Chord::new(Modifiers::COMMAND, k);
        let cs = |k| Chord::new(Modifiers::COMMAND | Modifiers::SHIFT, k);
        let ca = |k| Chord::new(Modifiers::COMMAND | Modifiers::ALT, k);
        let n = |k| Chord::new(Modifiers::NONE, k);
        match self {
            Cmd::RunStatement => vec![c(Key::Enter)],
            Cmd::RunScript => vec![cs(Key::Enter), n(Key::F5)],
            Cmd::Explain => vec![c(Key::E), n(Key::F7)],
            Cmd::Cancel => vec![Chord::new(Modifiers::SHIFT, Key::Escape)],
            Cmd::Commit => vec![ca(Key::Enter)],
            Cmd::Rollback => vec![ca(Key::Backspace)],
            Cmd::Format => vec![cs(Key::F), Chord::new(Modifiers::ALT | Modifiers::SHIFT, Key::F)],
            Cmd::Save => vec![c(Key::S)],
            Cmd::Hints => vec![c(Key::Space)],
            Cmd::Comment => vec![c(Key::Slash)],
            Cmd::Duplicate => vec![c(Key::D)],
            Cmd::Find => vec![c(Key::F)],
            Cmd::Outline => vec![cs(Key::O)],
            Cmd::ShowSpec => vec![Chord::new(Modifiers::ALT, Key::Num1)],
            Cmd::ShowBody => vec![Chord::new(Modifiers::ALT, Key::Num2)],
            Cmd::NewTab => vec![c(Key::T), c(Key::N)],
            Cmd::CloseTab => vec![c(Key::W)],
            Cmd::NextTab => vec![c(Key::Tab), c(Key::PageDown)],
            Cmd::PrevTab => vec![cs(Key::Tab), c(Key::PageUp)],
        }
    }
}

/// A key with its modifiers.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// egui's name of the key ("Enter", "F5", "A").
    pub key: String,
}

impl Chord {
    pub fn new(m: Modifiers, k: Key) -> Self {
        Chord { ctrl: m.command || m.ctrl, shift: m.shift, alt: m.alt, key: k.name().to_string() }
    }

    pub fn key(&self) -> Option<Key> {
        Key::from_name(&self.key)
    }

    fn fits(&self, k: Key, m: Modifiers) -> bool {
        Some(k) == self.key() && (m.command || m.ctrl) == self.ctrl && m.shift == self.shift && m.alt == self.alt
    }

    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s += "Ctrl+";
        }
        if self.alt {
            s += "Alt+";
        }
        if self.shift {
            s += "Shift+";
        }
        s += &self.key().map_or_else(|| self.key.clone(), |k| k.symbol_or_name().to_string());
        s
    }
}

/// The keys set in the settings (a command not in it: its usual ones).
pub type Keymap = BTreeMap<Cmd, Vec<Chord>>;

pub fn chords(map: &Keymap, c: Cmd) -> Vec<Chord> {
    map.get(&c).cloned().unwrap_or_else(|| c.defaults())
}

/// Pressed (one of its keys, exactly): the key press is taken.
pub fn take(i: &mut egui::InputState, map: &Keymap, c: Cmd) -> bool {
    let ch = chords(map, c);
    let at = i.events.iter().position(|e| matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. } if ch.iter().any(|x| x.fits(*key, *modifiers))));
    at.map(|k| i.events.remove(k)).is_some()
}

/// Its keys as shown ("Ctrl+Enter / F5"; "" — none).
pub fn label(map: &Keymap, c: Cmd) -> String {
    chords(map, c).iter().map(Chord::label).collect::<Vec<_>>().join(" / ")
}

/// Another command with this key.
pub fn taken_by(map: &Keymap, ch: &Chord, but: Cmd) -> Option<Cmd> {
    Cmd::ALL.into_iter().find(|&c| c != but && chords(map, c).contains(ch))
}

/// A key pressed (not a modifier alone) as a chord.
pub fn pressed(i: &egui::InputState) -> Option<Chord> {
    i.events.iter().find_map(|e| match e {
        egui::Event::Key { key, pressed: true, modifiers, .. } => Some(Chord::new(*modifiers, *key)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_keys() {
        let mut map = Keymap::new();
        let ctrl_f = Chord::new(Modifiers::COMMAND, Key::F);
        assert!(ctrl_f.fits(Key::F, Modifiers::CTRL | Modifiers::COMMAND));
        assert!(!ctrl_f.fits(Key::F, Modifiers::CTRL | Modifiers::COMMAND | Modifiers::SHIFT));
        assert_eq!(label(&map, Cmd::RunScript), "Ctrl+Shift+Enter / F5");
        map.insert(Cmd::RunScript, vec![Chord::new(Modifiers::NONE, Key::F9)]);
        assert_eq!(label(&map, Cmd::RunScript), "F9");
        assert_eq!(taken_by(&map, &ctrl_f, Cmd::Format), Some(Cmd::Find));
        // Kept as names: any keyboard's.
        let j = serde_json::to_string(&map).unwrap();
        assert_eq!(serde_json::from_str::<Keymap>(&j).unwrap(), map);
    }
}
