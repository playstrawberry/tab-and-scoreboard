//! The placeholders: the built-in `{player}`, `{ping}`, `{online}`, `{max_players}`, `{world}`,
//! `{group}`, `{prefix}`, `{suffix}` and `{tps}`, the images `{icon}` (the image of the player's
//! group), `{icon:<id>}` (a named image) and `{head}` (the player's own face), and the values
//! other plugins push, `{<plugin>:<key>}` (contract `placeholders/1`, [`Store`]).
//!
//! Everything about placeholders is in this one module and never calls the host: a template is
//! parsed once, says which values it depends on ([`Deps`]), and is filled in from a [`Source`].
//!
//! Text that is not a placeholder stays as written (`{x}` shows as `{x}`), so a server owner's
//! braces never disappear. A value pushed by a plugin is literal text: its `&` and braces are
//! never read.

use std::collections::{BTreeMap, HashMap, HashSet};

/// A value a template can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Player,
    Ping,
    Online,
    MaxPlayers,
    World,
    Group,
    Prefix,
    Suffix,
    Tps,
    /// `{head}`: an image, not text, and not in [`Key::ALL`].
    Head,
}

impl Key {
    pub const ALL: [Self; 9] = [
        Self::Player,
        Self::Ping,
        Self::Online,
        Self::MaxPlayers,
        Self::World,
        Self::Group,
        Self::Prefix,
        Self::Suffix,
        Self::Tps,
    ];

    /// The name written between the braces.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Ping => "ping",
            Self::Online => "online",
            Self::MaxPlayers => "max_players",
            Self::World => "world",
            Self::Group => "group",
            Self::Prefix => "prefix",
            Self::Suffix => "suffix",
            Self::Tps => "tps",
            Self::Head => "head",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.name() == name)
    }

    const fn bit(self) -> u128 {
        1 << self as u128
    }
}

/// Bits from here up stand for the values of other plugins.
const FIRST_PROVIDED_BIT: u32 = 16;

/// The bit of a pushed value. Two names may share one: a text is then rendered again without
/// need, compares equal, and nothing is sent.
fn provided_bit(name: &str) -> u128 {
    let hash = name.bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    1 << (FIRST_PROVIDED_BIT + hash % (128 - FIRST_PROVIDED_BIT))
}

/// A set of [`Key`]s and pushed values: what a template depends on, or what changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Deps(u128);

impl Deps {
    pub const NONE: Self = Self(0);
    /// What a player's rank (group, prefix, suffix) changes.
    pub const RANK: Self = Self(Key::Group.bit() | Key::Prefix.bit() | Key::Suffix.bit());
    /// Every value another plugin can push.
    pub const PROVIDED: Self = Self(!0 << FIRST_PROVIDED_BIT);

    /// The pushed value `<plugin>:<key>`.
    #[must_use]
    pub fn provided(name: &str) -> Self {
        Self(provided_bit(name))
    }

    #[must_use]
    pub const fn of(key: Key) -> Self {
        Self(key.bit())
    }

    #[must_use]
    pub const fn with(self, key: Key) -> Self {
        Self(self.0 | key.bit())
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn contains(self, key: Key) -> bool {
        self.0 & key.bit() != 0
    }

    #[must_use]
    pub const fn meets(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every key of `self` is in `allowed`.
    #[must_use]
    pub const fn within(self, allowed: Self) -> bool {
        self.0 & !allowed.0 == 0
    }
}

/// Which image an `{icon}` part names.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconRef {
    /// `{icon}`: the image of the player's group, if the group has one.
    Own,
    /// `{icon:<id>}`: the image with this id.
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Text(String),
    Value(Key),
    Icon(IconRef),
    /// `{<plugin>:<key>}`.
    Provided(String),
    /// `{head}`.
    Head,
}

/// A parsed template.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Template {
    parts: Vec<Part>,
    deps: Deps,
}

/// The longest template, in characters.
pub const MAX_TEMPLATE_CHARS: usize = 256;

impl Template {
    /// Parses `text`. Never fails: anything that is not a placeholder stays text.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut parts = Vec::new();
        let mut deps = Deps::NONE;
        let mut literal = String::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            literal.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let Some(close) = after.find('}') else {
                literal.push_str(&rest[open..]);
                rest = "";
                break;
            };
            let name = &after[..close];
            let part = if let Some(key) = Key::from_name(name) {
                deps = deps.with(key);
                Some(Part::Value(key))
            } else if name == "icon" {
                Some(Part::Icon(IconRef::Own))
            } else if name == "head" {
                deps = deps.with(Key::Head);
                Some(Part::Head)
            } else if let Some(id) = name.strip_prefix("icon:") {
                valid_id(id).then(|| Part::Icon(IconRef::Named(id.to_owned())))
            } else if valid_provided(name) {
                deps = deps.union(Deps::provided(name));
                Some(Part::Provided(name.to_owned()))
            } else {
                None
            };
            match part {
                Some(part) => {
                    if !literal.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut literal)));
                    }
                    parts.push(part);
                    rest = &after[close + 1..];
                }
                None => {
                    // Not ours: keep the brace and look for the next one after it.
                    literal.push('{');
                    rest = after;
                }
            }
        }
        literal.push_str(rest);
        if !literal.is_empty() {
            parts.push(Part::Text(literal));
        }
        Self { parts, deps }
    }

    #[must_use]
    pub const fn deps(&self) -> Deps {
        self.deps
    }

    #[must_use]
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// The named images the template uses.
    pub fn named_icons(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().filter_map(|part| match part {
            Part::Icon(IconRef::Named(id)) => Some(id.as_str()),
            _ => None,
        })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// The template filled in from `source`: text and the images it names.
    #[must_use]
    pub fn fill(&self, source: &dyn Source) -> Vec<Filled> {
        let mut out: Vec<Filled> = Vec::with_capacity(self.parts.len());
        for part in &self.parts {
            match part {
                Part::Text(text) => push_text(&mut out, text),
                Part::Value(key) => push_text(&mut out, &source.value(*key)),
                Part::Icon(icon) => out.push(Filled::Icon(icon.clone())),
                Part::Head => out.push(Filled::Head),
                Part::Provided(name) => match source.provided(name) {
                    Some(Provided::Text(text)) => out.push(Filled::Literal(text.clone())),
                    Some(Provided::Icon(id)) => {
                        out.push(Filled::Icon(IconRef::Named(id.clone())));
                    }
                    Some(Provided::Glyphs { font, text }) => out.push(Filled::Glyphs {
                        font: font.clone(),
                        text: text.clone(),
                    }),
                    None => push_text(&mut out, source.missing()),
                },
            }
        }
        out
    }

    /// The number the template shows; 0 when it shows anything else, so a score never fails.
    #[must_use]
    pub fn number(&self, source: &dyn Source) -> i32 {
        let mut text = String::new();
        for piece in self.fill(source) {
            if let Filled::Text(t) | Filled::Literal(t) = piece {
                text.push_str(&t);
            }
        }
        text.trim().parse::<i64>().map_or(0, |n| {
            i32::try_from(n).unwrap_or(if n < 0 { i32::MIN } else { i32::MAX })
        })
    }
}

/// A piece of a filled template.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Filled {
    /// Text with `&` codes.
    Text(String),
    /// Text shown as it is.
    Literal(String),
    Icon(IconRef),
    /// Characters of a pushed font.
    Glyphs {
        font: String,
        text: String,
    },
    Head,
}

fn push_text(out: &mut Vec<Filled>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Filled::Text(last)) = out.last_mut() {
        last.push_str(text);
    } else {
        out.push(Filled::Text(text.to_owned()));
    }
}

/// An id of an image or a section: 1 to 32 of `a-z`, `0-9`, `_` and `-`.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// `<plugin>:<key>` of a pushed value.
#[must_use]
pub fn valid_provided(name: &str) -> bool {
    name.split_once(':')
        .is_some_and(|(plugin, key)| valid_plugin(plugin) && valid_key(key))
}

/// A plugin's name as the server knows it: 1 to 64 letters, digits, `_`, `-` or `.`.
#[must_use]
pub fn valid_plugin(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}

/// A pushed value's key: 1 to 64 of `a-z`, `0-9`, `_`, `-` and `.`.
#[must_use]
pub fn valid_key(key: &str) -> bool {
    (1..=64).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-.".contains(&b))
}

/// Where a template's values come from.
pub trait Source {
    fn value(&self, key: Key) -> String;

    /// A value pushed by a plugin, by `<plugin>:<key>`.
    fn provided(&self, _name: &str) -> Option<&Provided> {
        None
    }

    /// What an unset pushed value shows (`&` codes allowed: the server owner's text).
    fn missing(&self) -> &str {
        ""
    }
}

/// A pushed value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provided {
    /// Shown as it is: no `&` codes, no braces.
    Text(String),
    /// A named image.
    Icon(String),
    /// Characters of a font of the server's own resource pack (a picture made of glyphs, such as
    /// a map), drawn only where the owner says the pack is required.
    Glyphs { font: String, text: String },
}

/// The longest pushed value, in characters.
pub const MAX_VALUE_CHARS: usize = 256;
/// Most keys one plugin may use.
pub const MAX_KEYS_PER_PLUGIN: usize = 64;
/// Most plugins that may push values.
pub const MAX_PLUGINS: usize = 16;

/// Who a pushed value is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Everyone,
    Player(String),
}

/// One line of a `placeholders/1` message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Set {
        target: Target,
        key: String,
        value: Provided,
    },
    /// No key: everything of that target from this plugin; for [`Target::Everyone`], everything
    /// this plugin ever pushed, for all players too.
    Clear { target: Target, key: Option<String> },
}

/// What a push changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub everyone: Deps,
    pub players: Vec<(String, Deps)>,
}

impl Changes {
    fn player(&mut self, uuid: &str, deps: Deps) {
        if let Some((_, known)) = self.players.iter_mut().find(|(u, _)| u == uuid) {
            *known = known.union(deps);
        } else {
            self.players.push((uuid.to_owned(), deps));
        }
    }
}

#[derive(Default)]
struct PlayerValues {
    values: BTreeMap<String, Provided>,
    /// Pushed since the last check: the player may be joining right now.
    fresh: bool,
}

/// The values other plugins pushed. Keys are written `<plugin>:<key>`, so a plugin can only ever
/// touch its own: the plugin's name comes from the server, not from the message.
#[derive(Default)]
pub struct Store {
    everyone: BTreeMap<String, Provided>,
    players: HashMap<String, PlayerValues>,
}

impl Store {
    /// The value of `name` for the player `uuid`: the player's own, else the one for everyone.
    #[must_use]
    pub fn get(&self, uuid: &str, name: &str) -> Option<&Provided> {
        self.players
            .get(uuid)
            .and_then(|p| p.values.get(name))
            .or_else(|| self.everyone.get(name))
    }

    /// Applies `ops` from `plugin` and says what changed.
    pub fn apply(&mut self, plugin: &str, ops: Vec<Op>) -> Changes {
        let mut changes = Changes::default();
        for op in ops {
            match op {
                Op::Set { target, key, value } => {
                    let name = format!("{plugin}:{key}");
                    let deps = Deps::provided(&name);
                    match target {
                        Target::Everyone => {
                            if self.everyone.get(&name) != Some(&value) {
                                self.everyone.insert(name, value);
                                changes.everyone = changes.everyone.union(deps);
                            }
                        }
                        Target::Player(uuid) => {
                            let entry = self.players.entry(uuid.clone()).or_default();
                            entry.fresh = true;
                            if entry.values.get(&name) != Some(&value) {
                                entry.values.insert(name, value);
                                changes.player(&uuid, deps);
                            }
                        }
                    }
                }
                Op::Clear { target, key } => self.clear(plugin, target, key, &mut changes),
            }
        }
        changes
    }

    fn clear(&mut self, plugin: &str, target: Target, key: Option<String>, out: &mut Changes) {
        let prefix = format!("{plugin}:");
        let wanted = key.map(|k| format!("{prefix}{k}"));
        let drop_from = |values: &mut BTreeMap<String, Provided>| {
            let mut deps = Deps::NONE;
            values.retain(|name, _| {
                let hit = wanted
                    .as_ref()
                    .map_or_else(|| name.starts_with(&prefix), |w| w == name);
                if hit {
                    deps = deps.union(Deps::provided(name));
                }
                !hit
            });
            deps
        };
        match target {
            Target::Player(uuid) => {
                if let Some(entry) = self.players.get_mut(&uuid) {
                    let deps = drop_from(&mut entry.values);
                    if !deps.is_empty() {
                        out.player(&uuid, deps);
                    }
                }
            }
            Target::Everyone => {
                out.everyone = out.everyone.union(drop_from(&mut self.everyone));
                if wanted.is_none() {
                    for (uuid, entry) in &mut self.players {
                        let deps = drop_from(&mut entry.values);
                        if !deps.is_empty() {
                            out.player(uuid, deps);
                        }
                    }
                }
            }
        }
    }

    /// Forgets a player who left.
    pub fn forget(&mut self, uuid: &str) {
        self.players.remove(uuid);
    }

    /// Forgets values of players who are not `online` and got nothing since the last call.
    pub fn sweep(&mut self, online: &HashSet<String>) {
        self.players.retain(|uuid, entry| {
            let keep = online.contains(uuid) || entry.fresh;
            entry.fresh = false;
            keep
        });
    }

    /// The plugins that have values now.
    #[must_use]
    pub fn plugins(&self) -> HashSet<String> {
        self.everyone
            .keys()
            .chain(self.players.values().flat_map(|p| p.values.keys()))
            .filter_map(|name| name.split_once(':').map(|(plugin, _)| plugin.to_owned()))
            .collect()
    }
}

/// The keys each plugin used: the bound on what a plugin can make this one remember.
#[derive(Default)]
pub struct KeyLimit {
    plugins: BTreeMap<String, HashSet<String>>,
}

impl KeyLimit {
    /// Whether `plugin` may use every key of `ops`; if so they count from now on.
    pub fn admit(&mut self, plugin: &str, ops: &[Op]) -> bool {
        let known = self.plugins.get(plugin);
        if known.is_none() && self.plugins.len() == MAX_PLUGINS {
            return false;
        }
        let mut all = known.cloned().unwrap_or_default();
        for op in ops {
            if let Op::Set { key, .. } = op {
                all.insert(key.clone());
            }
        }
        if all.len() > MAX_KEYS_PER_PLUGIN {
            return false;
        }
        self.plugins.insert(plugin.to_owned(), all);
        true
    }
}

/// TPS as Paper's `/tps` writes it: one decimal, at most 20.0, never negative.
#[must_use]
pub fn format_tps(tps: f64) -> String {
    let tps = if tps.is_finite() {
        tps.clamp(0.0, 20.0)
    } else {
        0.0
    };
    format!("{tps:.1}")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed;
    impl Source for Fixed {
        fn value(&self, key: Key) -> String {
            match key {
                Key::Player => "Alex".into(),
                Key::Ping => "42".into(),
                Key::Online => "3".into(),
                Key::Prefix => String::new(),
                other => other.name().to_uppercase(),
            }
        }
    }

    fn text(filled: &[Filled]) -> String {
        filled
            .iter()
            .map(|f| match f {
                Filled::Text(t) | Filled::Literal(t) => t.clone(),
                Filled::Icon(IconRef::Own) => "<icon>".into(),
                Filled::Icon(IconRef::Named(id)) => format!("<{id}>"),
                Filled::Head => "<head>".into(),
                Filled::Glyphs { font, .. } => format!("<{font}>"),
            })
            .collect()
    }

    #[test]
    fn known_placeholders_are_filled_and_listed_as_deps() {
        let t = Template::parse("&7Ping: {ping} ms, {online} online, hi {player}");
        assert_eq!(text(&t.fill(&Fixed)), "&7Ping: 42 ms, 3 online, hi Alex");
        assert!(t.deps().contains(Key::Ping));
        assert!(t.deps().contains(Key::Online));
        assert!(t.deps().contains(Key::Player));
        assert!(!t.deps().contains(Key::Tps));
    }

    #[test]
    fn unknown_and_unclosed_braces_stay_as_written() {
        let t = Template::parse("{nope} {player} {icon:Bad} {");
        assert_eq!(text(&t.fill(&Fixed)), "{nope} Alex {icon:Bad} {");
        assert_eq!(t.deps(), Deps::of(Key::Player));
        let nested = Template::parse("{{player}}");
        assert_eq!(text(&nested.fill(&Fixed)), "{Alex}");
    }

    #[test]
    fn icons_are_parts_not_text() {
        let t = Template::parse("{icon}&6[{prefix}] {icon:star}{player}");
        assert_eq!(text(&t.fill(&Fixed)), "<icon>&6[] <star>Alex");
        assert_eq!(t.named_icons().collect::<Vec<_>>(), ["star"]);
        assert!(t.deps().within(Deps::RANK.with(Key::Player)));
    }

    #[test]
    fn neighbouring_text_merges_and_empty_values_vanish() {
        let t = Template::parse("a{prefix}b");
        assert_eq!(t.fill(&Fixed), vec![Filled::Text("ab".into())]);
        assert!(Template::parse("").is_empty());
        assert!(Template::parse("{prefix}").fill(&Fixed).is_empty());
    }

    #[test]
    fn deps_sets() {
        let d = Deps::of(Key::Ping).with(Key::Tps);
        assert!(d.meets(Deps::of(Key::Tps)));
        assert!(!d.meets(Deps::RANK));
        assert!(!d.within(Deps::RANK));
        assert!(Deps::NONE.is_empty());
        assert!(d.union(Deps::RANK).contains(Key::Group));
    }

    #[test]
    fn tps_is_written_like_paper() {
        assert_eq!(format_tps(20.0), "20.0");
        assert_eq!(format_tps(21.3), "20.0");
        assert_eq!(format_tps(19.94), "19.9");
        assert_eq!(format_tps(-1.0), "0.0");
        assert_eq!(format_tps(f64::NAN), "0.0");
    }

    struct Pushed(Store, &'static str);
    impl Source for Pushed {
        fn value(&self, key: Key) -> String {
            key.name().to_owned()
        }
        fn provided(&self, name: &str) -> Option<&Provided> {
            self.0.get("u1", name)
        }
        fn missing(&self) -> &str {
            self.1
        }
    }

    fn set(target: Target, key: &str, value: &str) -> Op {
        Op::Set {
            target,
            key: key.into(),
            value: Provided::Text(value.into()),
        }
    }

    #[test]
    fn pushed_values_are_literal_and_unset_ones_show_the_missing_text() {
        let t = Template::parse("&7Rate: {eco:rate}%");
        assert!(t.deps().meets(Deps::provided("eco:rate")));
        assert!(t.deps().within(Deps::PROVIDED));
        let mut store = Store::default();
        let change = store.apply(
            "eco",
            vec![set(Target::Player("u1".into()), "rate", "&c{player}")],
        );
        assert_eq!(change.players[0].0, "u1");
        let filled = t.fill(&Pushed(store, ""));
        assert_eq!(
            filled,
            vec![
                Filled::Text("&7Rate: ".into()),
                Filled::Literal("&c{player}".into()),
                Filled::Text("%".into()),
            ]
        );
        let unset = t.fill(&Pushed(Store::default(), "&8-"));
        assert_eq!(unset, vec![Filled::Text("&7Rate: &8-%".into())]);
    }

    #[test]
    fn a_plugin_only_touches_its_own_keys_and_a_players_value_wins() {
        let mut store = Store::default();
        let _ = store.apply("a", vec![set(Target::Everyone, "k", "all")]);
        let _ = store.apply("b", vec![set(Target::Everyone, "k", "other")]);
        let _ = store.apply("a", vec![set(Target::Player("u1".into()), "k", "mine")]);
        assert_eq!(store.get("u1", "a:k"), Some(&Provided::Text("mine".into())));
        assert_eq!(store.get("u2", "a:k"), Some(&Provided::Text("all".into())));
        let cleared = store.apply(
            "b",
            vec![Op::Clear {
                target: Target::Everyone,
                key: None,
            }],
        );
        assert!(cleared.everyone.meets(Deps::provided("b:k")));
        assert_eq!(store.get("u2", "b:k"), None);
        assert_eq!(store.get("u2", "a:k"), Some(&Provided::Text("all".into())));
    }

    #[test]
    fn the_same_value_again_changes_nothing() {
        let mut store = Store::default();
        let op = || vec![set(Target::Player("u1".into()), "k", "1")];
        assert!(!store.apply("a", op()).players.is_empty());
        assert_eq!(store.apply("a", op()), Changes::default());
    }

    #[test]
    fn clearing_everything_reaches_every_player_and_says_who() {
        let mut store = Store::default();
        let _ = store.apply("a", vec![set(Target::Player("u1".into()), "k", "1")]);
        let _ = store.apply("b", vec![set(Target::Player("u1".into()), "k", "1")]);
        let change = store.apply(
            "a",
            vec![Op::Clear {
                target: Target::Everyone,
                key: None,
            }],
        );
        assert_eq!(change.players.len(), 1);
        assert_eq!(store.get("u1", "a:k"), None);
        assert!(store.get("u1", "b:k").is_some());
        assert_eq!(store.plugins(), HashSet::from(["b".to_owned()]));
    }

    #[test]
    fn values_of_players_never_seen_are_swept_but_a_joining_player_keeps_them() {
        let mut store = Store::default();
        let _ = store.apply("a", vec![set(Target::Player("late".into()), "k", "1")]);
        let none = HashSet::new();
        store.sweep(&none);
        assert!(store.get("late", "a:k").is_some(), "one check of grace");
        store.sweep(&none);
        assert!(store.get("late", "a:k").is_none());
        let _ = store.apply("a", vec![set(Target::Player("on".into()), "k", "1")]);
        let online = HashSet::from(["on".to_owned()]);
        store.sweep(&online);
        store.sweep(&online);
        assert!(store.get("on", "a:k").is_some());
    }

    #[test]
    fn plugins_and_keys_are_bounded() {
        let mut limit = KeyLimit::default();
        let many: Vec<Op> = (0..MAX_KEYS_PER_PLUGIN)
            .map(|i| set(Target::Everyone, &format!("k{i}"), "v"))
            .collect();
        assert!(limit.admit("p", &many));
        assert!(limit.admit("p", &many), "the same keys again are fine");
        assert!(!limit.admit("p", &[set(Target::Everyone, "new", "v")]));
        for i in 1..MAX_PLUGINS {
            assert!(limit.admit(&format!("p{i}"), &[]));
        }
        assert!(!limit.admit("one-too-many", &[]));
    }

    #[test]
    fn numbers() {
        struct N(&'static str);
        impl Source for N {
            fn value(&self, _key: Key) -> String {
                self.0.to_owned()
            }
        }
        let t = Template::parse("{ping}");
        assert_eq!(t.number(&N("42")), 42);
        assert_eq!(t.number(&N(" -7 ")), -7);
        assert_eq!(t.number(&N("many")), 0);
        assert_eq!(t.number(&N("99999999999")), i32::MAX);
    }

    #[test]
    fn head_is_an_image_and_foreign_names_are_values() {
        let t = Template::parse("{head}{player}");
        assert!(t.deps().contains(Key::Head));
        assert_eq!(text(&t.fill(&Fixed)), "<head>Alex");
        assert!(Template::parse("{a b:c}").deps().is_empty());
        assert!(Template::parse("{Eco:rate}").deps().meets(Deps::PROVIDED));
        assert!(Template::parse("{icon:Bad}").deps().is_empty());
        // Built-in values and images are never counted as pushed ones.
        assert!(!Deps::of(Key::Head).meets(Deps::PROVIDED));
    }

    #[test]
    fn ids() {
        assert!(valid_id("vip_star-2"));
        assert!(!valid_id(""));
        assert!(!valid_id("Vip"));
        assert!(!valid_id(&"a".repeat(33)));
    }
}
