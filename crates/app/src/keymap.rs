//! Keymap (SPEC §5.4, §5.8): any key combo plus double-press bindings
//! (same combo twice within 300 ms). Stored as JSON: only overrides are
//! written, so "reset to defaults" for one command removes its entry.

use crate::commands::{ALL, Cmd};
use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DOUBLE_PRESS_SECS: f64 = 0.3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Binding {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub double: bool,
}

impl Binding {
    pub fn new(key: Key, m: Modifiers) -> Self {
        Binding { key, ctrl: m.command || m.ctrl, shift: m.shift, alt: m.alt, double: false }
    }

    /// Parse `"Ctrl+Shift+Z"`, `"Q"`, `"L x2"` (double press), `"Ctrl+Comma"`.
    pub fn parse(s: &str) -> Option<Binding> {
        let s = s.trim();
        let (s, double) = match s.strip_suffix("x2") {
            Some(rest) => (rest.trim(), true),
            None => (s, false),
        };
        let mut b = Binding { key: Key::A, ctrl: false, shift: false, alt: false, double };
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key, mods) = parts.split_last()?;
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" | "command" => b.ctrl = true,
                "shift" => b.shift = true,
                "alt" | "option" => b.alt = true,
                _ => return None,
            }
        }
        b.key = Key::from_name(key)?;
        Some(b)
    }

    /// Storage form (inverse of [`Binding::parse`]).
    pub fn to_storage(self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        s.push_str(self.key.name());
        if self.double {
            s.push_str(" x2");
        }
        s
    }

    /// Display form, e.g. `Ctrl+,` or `L L`.
    pub fn display(self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str(if cfg!(target_os = "macos") { "Cmd+" } else { "Ctrl+" });
        }
        if self.shift {
            s.push_str("Shift+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        let k = match self.key {
            Key::Delete => "Del",
            Key::Backslash => "\\",
            k => k.symbol_or_name(),
        };
        s.push_str(k);
        if self.double {
            s = format!("{s} {s}");
        }
        s
    }

    pub fn same_combo(&self, o: &Binding) -> bool {
        self.key == o.key && self.ctrl == o.ctrl && self.shift == o.shift && self.alt == o.alt
    }
}

/// User overrides: command id → shortcuts (empty list = no shortcut).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Keymap {
    pub overrides: BTreeMap<String, Vec<String>>,
}

impl Keymap {
    pub fn bindings(&self, cmd: Cmd) -> Vec<Binding> {
        match self.overrides.get(cmd.id()) {
            Some(v) => v.iter().filter_map(|s| Binding::parse(s)).collect(),
            None => cmd.default_bindings().iter().filter_map(|s| Binding::parse(s)).collect(),
        }
    }

    pub fn set(&mut self, cmd: Cmd, bindings: &[Binding]) {
        let v: Vec<String> = bindings.iter().map(|b| b.to_storage()).collect();
        let default: Vec<String> =
            cmd.default_bindings().iter().filter_map(|s| Binding::parse(s)).map(|b| b.to_storage()).collect();
        if v == default {
            self.overrides.remove(cmd.id());
        } else {
            self.overrides.insert(cmd.id().to_string(), v);
        }
    }

    pub fn reset(&mut self, cmd: Cmd) {
        self.overrides.remove(cmd.id());
    }

    pub fn reset_all(&mut self) {
        self.overrides.clear();
    }

    pub fn is_default(&self, cmd: Cmd) -> bool {
        !self.overrides.contains_key(cmd.id())
    }

    /// First shortcut for display ("—" when none).
    pub fn label(&self, cmd: Cmd) -> String {
        let b = self.bindings(cmd);
        if b.is_empty() { String::new() } else { b.iter().map(|b| b.display()).collect::<Vec<_>>().join(" / ") }
    }

    /// Commands bound to exactly this binding (combo + double flag).
    pub fn lookup(&self, b: &Binding) -> Vec<Cmd> {
        ALL.iter().copied().filter(|c| self.bindings(*c).iter().any(|x| x.same_combo(b) && x.double == b.double)).collect()
    }

    /// Commands using this combo in any form (for conflict warnings).
    pub fn conflicts(&self, b: &Binding, except: Cmd) -> Vec<Cmd> {
        ALL.iter()
            .copied()
            .filter(|c| *c != except && self.bindings(*c).iter().any(|x| x.same_combo(b) && x.double == b.double))
            .collect()
    }
}

/// Turns key presses into commands, handling double presses.
#[derive(Default)]
pub struct Dispatcher {
    last: Option<(Binding, f64)>,
}

impl Dispatcher {
    /// A key was pressed at time `t` (seconds). Returns the command to run.
    /// On a double press the double binding wins if one exists; otherwise
    /// the single binding runs again.
    pub fn press(&mut self, keymap: &Keymap, b: Binding, t: f64) -> Option<Cmd> {
        let is_double = self.last.is_some_and(|(l, lt)| l.same_combo(&b) && t - lt <= DOUBLE_PRESS_SECS);
        if is_double {
            let dbl = Binding { double: true, ..b };
            if let Some(c) = keymap.lookup(&dbl).first() {
                self.last = None;
                return Some(*c);
            }
        }
        self.last = Some((b, t));
        keymap.lookup(&Binding { double: false, ..b }).first().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_binding_parses() {
        for c in ALL {
            for s in c.default_bindings() {
                let b = Binding::parse(s).unwrap_or_else(|| panic!("{} binding {s:?}", c.id()));
                assert_eq!(Binding::parse(&b.to_storage()), Some(b));
            }
        }
    }

    #[test]
    fn spec_table_defaults() {
        let k = Keymap::default();
        let b = |s: &str| Binding::parse(s).unwrap();
        assert_eq!(k.lookup(&b("Ctrl+K")), vec![Cmd::Palette]);
        assert_eq!(k.lookup(&b("Q")), vec![Cmd::Radial]);
        assert_eq!(k.lookup(&b("Ctrl+Comma")), vec![Cmd::Preferences]);
        assert_eq!(k.lookup(&b("Shift+V")), vec![Cmd::Wireframe]);
        assert_eq!(k.lookup(&b("Alt+C")), vec![Cmd::Collapse]);
        assert_eq!(k.lookup(&b("Slash")), vec![Cmd::Isolate]);
        assert_eq!(k.lookup(&b("Ctrl+Y")), vec![Cmd::Redo]);
        assert_eq!(k.lookup(&b("L x2")), vec![Cmd::LoopAlternate]);
        for (s, c) in [("1", Cmd::ModeVertex), ("2", Cmd::ModeEdge), ("3", Cmd::ModeFace), ("4", Cmd::ModeObject)] {
            assert_eq!(k.lookup(&b(s)), vec![c]);
        }
    }

    #[test]
    fn no_default_conflicts() {
        let k = Keymap::default();
        for c in ALL {
            for b in k.bindings(*c) {
                assert!(k.conflicts(&b, *c).is_empty(), "{} conflicts on {}", c.id(), b.display());
            }
        }
    }

    #[test]
    fn double_press() {
        let k = Keymap::default();
        let mut d = Dispatcher::default();
        let l = Binding::parse("L").unwrap();
        assert_eq!(d.press(&k, l, 1.0), Some(Cmd::Loop));
        assert_eq!(d.press(&k, l, 1.2), Some(Cmd::LoopAlternate));
        assert_eq!(d.press(&k, l, 1.3), Some(Cmd::Loop));
        assert_eq!(d.press(&k, l, 2.0), Some(Cmd::Loop));
        // A key without a double binding just repeats.
        let f = Binding::parse("F").unwrap();
        assert_eq!(d.press(&k, f, 3.0), Some(Cmd::Frame));
        assert_eq!(d.press(&k, f, 3.1), Some(Cmd::Frame));
    }

    #[test]
    fn overrides_and_reset() {
        let mut k = Keymap::default();
        k.set(Cmd::Frame, &[Binding::parse("Shift+F").unwrap()]);
        assert_eq!(k.lookup(&Binding::parse("Shift+F").unwrap()), vec![Cmd::Frame]);
        assert!(k.lookup(&Binding::parse("F").unwrap()).is_empty());
        let json = serde_json::to_string(&k).unwrap();
        let back: Keymap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, k);
        k.reset(Cmd::Frame);
        assert!(k.is_default(Cmd::Frame));
    }
}
