//! What each player chose and what other plugins hold for them, without the host: the stored
//! sidebar and below-name settings (`data/players.tsv`) and, for as long as the player is online,
//! the reasons other plugins hold (`sidebar/1` suppress, `teams/1`).
//!
//! This lives apart from the planner, behind its own lock, so a plugin's question is answered at
//! once even while a cycle runs.

use std::collections::{BTreeMap, HashMap};

/// The largest `players.tsv` read.
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
/// Most reasons one player can be held for (per kind).
pub const MAX_REASONS: usize = 16;
/// Most online players reasons are kept for; a bound against a plugin's bad ids.
pub const MAX_HELD_PLAYERS: usize = 4096;

/// A team's collision rule, strongest last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Collision {
    #[default]
    Always,
    PushOwn,
    PushOther,
    Never,
}

/// A team's nametag visibility, strongest last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Nametag {
    #[default]
    Always,
    HideOwn,
    HideOther,
    Never,
}

/// What the planner needs to know of a player's choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub sidebar: bool,
    pub below: bool,
    pub collision: Collision,
    pub nametag: Nametag,
}

#[derive(Default)]
struct Held {
    /// `(plugin, reason)`.
    sidebar: Vec<(String, String)>,
    team: BTreeMap<(String, String), (Option<Collision>, Option<Nametag>)>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Stored {
    sidebar: Option<bool>,
    below: Option<bool>,
}

pub struct Views {
    sidebar_default: bool,
    below_default: bool,
    stored: BTreeMap<String, Stored>,
    held: HashMap<String, Held>,
    dirty: bool,
}

impl Views {
    #[must_use]
    pub fn new(sidebar_default: bool, below_default: bool) -> Self {
        Self {
            sidebar_default,
            below_default,
            stored: BTreeMap::new(),
            held: HashMap::new(),
            dirty: false,
        }
    }

    /// New defaults (the settings were read again); what players chose stays.
    pub fn set_defaults(&mut self, sidebar: bool, below: bool) {
        self.sidebar_default = sidebar;
        self.below_default = below;
    }

    #[must_use]
    pub fn sidebar_setting(&self, uuid: &str) -> bool {
        self.stored
            .get(uuid)
            .and_then(|s| s.sidebar)
            .unwrap_or(self.sidebar_default)
    }

    #[must_use]
    pub fn below_setting(&self, uuid: &str) -> bool {
        self.stored
            .get(uuid)
            .and_then(|s| s.below)
            .unwrap_or(self.below_default)
    }

    pub fn set_sidebar(&mut self, uuid: &str, on: bool) {
        if self.sidebar_setting(uuid) != on || self.stored.contains_key(uuid) {
            self.stored.entry(uuid.to_owned()).or_default().sidebar = Some(on);
            self.dirty = true;
        }
    }

    pub fn set_below(&mut self, uuid: &str, on: bool) {
        if self.below_setting(uuid) != on || self.stored.contains_key(uuid) {
            self.stored.entry(uuid.to_owned()).or_default().below = Some(on);
            self.dirty = true;
        }
    }

    fn held_for(&mut self, uuid: &str) -> Option<&mut Held> {
        if !self.held.contains_key(uuid) && self.held.len() >= MAX_HELD_PLAYERS {
            return None;
        }
        Some(self.held.entry(uuid.to_owned()).or_default())
    }

    /// Whether the reason was taken; `false` when a bound refused it.
    pub fn suppress(&mut self, uuid: &str, plugin: &str, reason: &str, on: bool) -> bool {
        let key = (plugin.to_owned(), reason.to_owned());
        if !on {
            if let Some(held) = self.held.get_mut(uuid) {
                held.sidebar.retain(|k| *k != key);
            }
            return true;
        }
        let Some(held) = self.held_for(uuid) else {
            return false;
        };
        if held.sidebar.contains(&key) {
            return true;
        }
        if held.sidebar.len() == MAX_REASONS {
            return false;
        }
        held.sidebar.push(key);
        true
    }

    /// Whether the options were taken; `false` when a bound refused them.
    pub fn team_set(
        &mut self,
        uuid: &str,
        plugin: &str,
        reason: &str,
        collision: Option<Collision>,
        nametag: Option<Nametag>,
    ) -> bool {
        let key = (plugin.to_owned(), reason.to_owned());
        let Some(held) = self.held_for(uuid) else {
            return false;
        };
        if !held.team.contains_key(&key) && held.team.len() == MAX_REASONS {
            return false;
        }
        held.team.insert(key, (collision, nametag));
        true
    }

    pub fn team_clear(&mut self, uuid: &str, plugin: &str, reason: &str) {
        if let Some(held) = self.held.get_mut(uuid) {
            held.team.remove(&(plugin.to_owned(), reason.to_owned()));
        }
    }

    /// The player left: reasons end there, the stored settings stay.
    pub fn forget(&mut self, uuid: &str) {
        self.held.remove(uuid);
    }

    /// What a player sees now. Several plugins' team options: the strongest of each wins.
    #[must_use]
    pub fn view(&self, uuid: &str) -> View {
        let held = self.held.get(uuid);
        let suppressed = held.is_some_and(|h| !h.sidebar.is_empty());
        let options = held.map(|h| h.team.values());
        let collision = options
            .clone()
            .into_iter()
            .flatten()
            .filter_map(|(c, _)| *c)
            .max()
            .unwrap_or_default();
        let nametag = options
            .into_iter()
            .flatten()
            .filter_map(|(_, n)| *n)
            .max()
            .unwrap_or_default();
        View {
            sidebar: self.sidebar_setting(uuid) && !suppressed,
            below: self.below_setting(uuid),
            collision,
            nametag,
        }
    }

    /// The file text when it needs writing; the caller writes it off the tick.
    pub fn take_dirty(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let mut out = String::new();
        for (uuid, stored) in &self.stored {
            out.push_str(uuid);
            for flag in [stored.sidebar, stored.below] {
                out.push('\t');
                out.push(match flag {
                    Some(true) => '1',
                    Some(false) => '0',
                    None => '-',
                });
            }
            out.push('\n');
        }
        Some(out)
    }

    /// Marks the settings as not written yet, after a failed write.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Reads `players.tsv`: `<uuid><TAB><sidebar><TAB><below>`, each `1`, `0` or `-` (the
    /// default). A line that breaks this is skipped and counted: one bad line loses one choice,
    /// never the file.
    pub fn load(&mut self, text: &str) -> usize {
        let mut skipped = 0;
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            let flag = |field: &str| match field {
                "1" => Ok(Some(true)),
                "0" => Ok(Some(false)),
                "-" => Ok(None),
                _ => Err(()),
            };
            let parsed = match fields.as_slice() {
                [uuid, sidebar, below] if uuid.len() == 36 => {
                    flag(sidebar).and_then(|s| flag(below).map(|b| (*uuid, s, b)))
                }
                _ => Err(()),
            };
            match parsed {
                Ok((uuid, sidebar, below)) => {
                    self.stored
                        .insert(uuid.to_ascii_lowercase(), Stored { sidebar, below });
                }
                Err(()) => skipped += 1,
            }
        }
        skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "00000000-0000-0000-0000-00000000000a";
    const B: &str = "00000000-0000-0000-0000-00000000000b";

    #[test]
    fn visible_is_the_setting_and_no_reason_held() {
        let mut views = Views::new(true, false);
        assert!(views.view(A).sidebar);
        assert!(views.suppress(A, "core", "mode", true));
        assert!(!views.view(A).sidebar);
        // The setting is still on, and another plugin's same word is its own reason.
        assert!(views.sidebar_setting(A));
        assert!(views.suppress(A, "other", "mode", true));
        assert!(views.suppress(A, "core", "mode", false));
        assert!(!views.view(A).sidebar);
        assert!(views.suppress(A, "other", "mode", false));
        assert!(views.view(A).sidebar);
        // Off by the player: a reason ending does not bring it back.
        views.set_sidebar(A, false);
        assert!(views.suppress(A, "core", "mode", true));
        assert!(views.suppress(A, "core", "mode", false));
        assert!(!views.view(A).sidebar);
    }

    #[test]
    fn reasons_end_when_the_player_leaves_and_settings_stay() {
        let mut views = Views::new(true, true);
        views.set_sidebar(A, false);
        views.set_below(A, false);
        assert!(views.suppress(B, "core", "mode", true));
        views.forget(A);
        views.forget(B);
        assert!(views.view(B).sidebar);
        assert!(!views.view(A).sidebar);
        assert!(!views.view(A).below);
    }

    #[test]
    fn the_strongest_team_option_wins_and_each_plugin_keeps_its_own() {
        let mut views = Views::new(true, true);
        assert!(views.team_set(A, "core", "boat", Some(Collision::Never), None));
        assert!(views.team_set(
            A,
            "pvp",
            "arena",
            Some(Collision::PushOwn),
            Some(Nametag::HideOther)
        ));
        let view = views.view(A);
        assert_eq!(view.collision, Collision::Never);
        assert_eq!(view.nametag, Nametag::HideOther);
        views.team_clear(A, "core", "boat");
        assert_eq!(views.view(A).collision, Collision::PushOwn);
        views.team_clear(A, "pvp", "arena");
        assert_eq!(views.view(A).collision, Collision::Always);
        assert_eq!(views.view(A).nametag, Nametag::Always);
    }

    #[test]
    fn reasons_are_bounded() {
        let mut views = Views::new(true, true);
        for i in 0..MAX_REASONS {
            assert!(views.suppress(A, "core", &format!("r{i}"), true));
            assert!(views.team_set(A, "core", &format!("r{i}"), None, None));
        }
        assert!(!views.suppress(A, "core", "more", true));
        assert!(!views.team_set(A, "core", "more", None, None));
        assert!(views.suppress(A, "core", "r0", true), "already held");
        assert!(views.suppress(A, "core", "more", false), "ending is free");
    }

    #[test]
    fn the_file_keeps_choices_only_and_survives_a_bad_line() {
        let mut views = Views::new(true, true);
        assert_eq!(views.take_dirty(), None);
        views.set_sidebar(A, true);
        assert_eq!(views.take_dirty(), None, "the default needs no line");
        views.set_sidebar(A, false);
        let text = views.take_dirty().expect("changed");
        assert_eq!(text, format!("{A}\t0\t-\n"));
        assert_eq!(views.take_dirty(), None);

        let mut read = Views::new(true, true);
        let skipped = read.load(&format!(
            "{text}garbage\n{B}\t1\tmaybe\n{}\t-\t0\n",
            B.to_ascii_uppercase()
        ));
        assert_eq!(skipped, 2);
        assert!(!read.sidebar_setting(A));
        assert!(!read.below_setting(B));
        assert!(read.sidebar_setting(B));
    }

    #[test]
    fn a_new_default_reaches_only_those_who_chose_nothing() {
        let mut views = Views::new(true, true);
        views.set_sidebar(A, true);
        views.set_sidebar(A, false);
        views.set_defaults(false, false);
        assert!(!views.sidebar_setting(B));
        assert!(!views.below_setting(B));
        views.set_defaults(true, true);
        assert!(views.sidebar_setting(B));
        assert!(!views.sidebar_setting(A));
    }
}
