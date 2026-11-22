//! The settings file `data/config.txt`: sections and `key = value` lines.
//!
//! ```text
//! [settings]            refresh-seconds, ping-seconds, icons (text | pack-required), sidebar,
//!                       nametags, sort (on | off), name-order (client | exact-case),
//!                       language (client | en | fr | es | tr), missing, sidebar-default,
//!                       sidebar-command, sidebar-on-words, sidebar-off-words,
//!                       tab-name, nametag-prefix, nametag-suffix
//! [group <name>]        tab-name, nametag-prefix, nametag-suffix, color (for a perms-lite group)
//! [icon <id>]           glyph + font, or head (+ head-name, hat); fallback
//! [layout <name>]       worlds, groups, header*, footer*, title, line*   (* = may repeat)
//! [tab-score]           value, format, min-change
//! [below-name]          title, value, format, min-change, default
//! ```
//!
//! A value may be written in double quotes to keep spaces at its ends. A value starting with `@`
//! names a text in the language files (`@default.header`, or the owner's `@my.key`), shown in each
//! viewer's language. Lines starting with `#` are notes. A file that breaks a rule is refused as a
//! whole, with the line number, and the settings in use stay.

use strawberry_plugin_api::common::NamedColor;

use crate::lang::{self, OwnerLang};
use crate::placeholders::{Deps, Key, MAX_TEMPLATE_CHARS, Part, Template, valid_id};

/// The largest settings file read.
pub const MAX_FILE_BYTES: usize = 64 * 1024;
pub const MAX_HEADER_LINES: usize = 10;
pub const MAX_SIDEBAR_LINES: usize = 15;
pub const MAX_LAYOUTS: usize = 32;
pub const MAX_GROUPS: usize = 64;
pub const MAX_ICONS: usize = 64;
pub const MAX_LIST_ITEMS: usize = 64;
/// The longest `head` value (26.3 `ExtraCodecs.PROPERTY`).
pub const MAX_TEXTURES_CHARS: usize = 32767;

/// Font namespaces that belong to the platform, and its badge font.
pub const RESERVED_NAMESPACES: [&str; 7] = [
    "strawberry",
    "strawberry_core",
    "strawberry_cosmetics",
    "strawberry_items",
    "playstrawberry",
    "realms",
    "c",
];

/// The values a tab name may use: they belong to the named player.
pub const NAME_KEYS: Deps = SCORE_KEYS.with(Key::Head);

/// The values a score may use.
pub const SCORE_KEYS: Deps = Deps::RANK
    .union(Deps::PROVIDED)
    .with(Key::Player)
    .with(Key::Ping)
    .with(Key::World);

/// The values a nametag may use: a nametag is a team, so a text that changes often costs a new
/// team on every viewer's board.
pub const NAMETAG_KEYS: Deps = Deps::RANK.union(Deps::PROVIDED);

/// The longest command label or word.
const MAX_WORD_CHARS: usize = 32;

/// How players of one order value are sorted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameOrder {
    /// As the client does by itself: A to Z without case.
    Client,
    /// A to Z with case (`Zed` before `alex`), by the start of the name.
    ExactCase,
}

/// A score next to or below a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Score {
    pub value: Template,
    /// The text shown in place of the number, with `{value}`.
    pub format: Option<Text>,
    /// A new value closer than this to the shown one is not sent.
    pub min_change: u32,
}

impl Score {
    /// The text a viewer in language `lang` is shown for `value`.
    #[must_use]
    pub fn shown(&self, lang: usize, value: i32) -> Option<String> {
        let number = value.to_string();
        self.format.as_ref().map(|format| {
            format
                .get(lang)
                .parts()
                .iter()
                .map(|part| match part {
                    Part::Text(text) => text.replace("{value}", &number),
                    _ => String::new(),
                })
                .collect()
        })
    }
}

/// `[below-name]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BelowName {
    pub title: Text,
    pub score: Score,
    /// Whether a viewer who chose nothing sees it.
    pub default_on: bool,
}

/// Whether images that need the server's pack are sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconsMode {
    /// Never: glyph images show their fallback text.
    Text,
    /// The server owner says the server's resource pack that has the fonts is required, so
    /// every Java player in the game has loaded it (the engine disconnects those who did not).
    PackRequired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconKind {
    /// Characters drawn by a font of the server's own resource pack.
    Glyph { text: String, font: String },
    /// A head (26.3's object text): any 8x8 face from a skin `textures` value. Needs no pack.
    /// Clients before 26.1 see `name` as text.
    Head {
        textures: String,
        name: String,
        hat: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    pub id: String,
    pub kind: IconKind,
    /// What a client that cannot draw the image sees, per language (`&` codes allowed; empty for
    /// images that only move other images).
    pub fallback: [String; 4],
}

impl Icon {
    #[must_use]
    pub fn fallback(&self, lang: usize) -> &str {
        &self.fallback[lang.min(3)]
    }
}

/// A text per language: the owner's own (the same for all) or one from the language files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    by_lang: [Template; 4],
    deps: Deps,
}

impl Text {
    #[must_use]
    pub fn own(template: Template) -> Self {
        let deps = template.deps();
        Self {
            by_lang: [
                template.clone(),
                template.clone(),
                template.clone(),
                template,
            ],
            deps,
        }
    }

    /// The text `key` of the language files; `None` when there is no such text.
    #[must_use]
    pub fn named(key: &str, owner: &OwnerLang) -> Option<Self> {
        if !owner.has(key) {
            return None;
        }
        let by_lang = [0, 1, 2, 3].map(|i| Template::parse(&owner.text(i, key, &[])));
        let deps = by_lang
            .iter()
            .fold(Deps::NONE, |deps, t| deps.union(t.deps()));
        Some(Self { by_lang, deps })
    }

    #[must_use]
    pub fn get(&self, lang: usize) -> &Template {
        &self.by_lang[lang.min(3)]
    }

    #[must_use]
    pub const fn deps(&self) -> Deps {
        self.deps
    }

    fn named_icons(&self) -> impl Iterator<Item = &str> {
        self.by_lang.iter().flat_map(Template::named_icons)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Match {
    Any,
    /// Names compared without case.
    Only(Vec<String>),
}

impl Match {
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Only(names) => names.iter().any(|n| n.eq_ignore_ascii_case(name)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupStyle {
    /// The perms-lite group name, lower case.
    pub name: String,
    pub tab_name: Option<Text>,
    pub nametag_prefix: Option<Text>,
    pub nametag_suffix: Option<Text>,
    pub color: Option<NamedColor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub name: String,
    pub worlds: Match,
    pub groups: Match,
    pub header: Vec<Text>,
    pub footer: Vec<Text>,
    pub title: Option<Text>,
    pub lines: Vec<Text>,
}

impl Layout {
    /// What any text of the layout depends on.
    #[must_use]
    pub fn deps(&self) -> Deps {
        self.header
            .iter()
            .chain(&self.footer)
            .chain(&self.title)
            .chain(&self.lines)
            .fold(Deps::NONE, |deps, text| deps.union(text.deps()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub refresh_seconds: u32,
    pub ping_seconds: u32,
    pub icons_mode: IconsMode,
    pub sidebar: bool,
    pub nametags: bool,
    pub sort: bool,
    pub name_order: NameOrder,
    /// `None`: each viewer's client language. Otherwise that language for everyone.
    pub language: Option<usize>,
    /// What an unset value from another plugin shows.
    pub missing: String,
    pub sidebar_default: bool,
    /// The label of the player's on/off command; `None`: no command. Read at start.
    pub sidebar_command: Option<String>,
    pub sidebar_on_words: Vec<String>,
    pub sidebar_off_words: Vec<String>,
    pub tab_name: Text,
    pub nametag_prefix: Text,
    pub nametag_suffix: Text,
    pub groups: Vec<GroupStyle>,
    pub icons: Vec<Icon>,
    pub layouts: Vec<Layout>,
    pub tab_score: Option<Score>,
    pub below_name: Option<BelowName>,
}

impl Config {
    /// The built-in settings ([`DEFAULT_TEXT`]).
    ///
    /// # Panics
    ///
    /// Never: a unit test parses the built-in text.
    #[must_use]
    pub fn builtin() -> Self {
        parse(DEFAULT_TEXT).unwrap_or_else(|e| panic!("the built-in settings: {e:?}"))
    }

    #[must_use]
    pub fn group(&self, name: &str) -> Option<&GroupStyle> {
        self.groups
            .iter()
            .find(|g| g.name.eq_ignore_ascii_case(name))
    }

    #[must_use]
    pub fn icon(&self, id: &str) -> Option<&Icon> {
        self.icons.iter().find(|i| i.id == id)
    }

    /// The language of a viewer whose client is set to `client`.
    #[must_use]
    pub fn lang_for(&self, client: usize) -> usize {
        self.language.unwrap_or(client)
    }

    /// The language of what every viewer shares (tab names, nametags).
    #[must_use]
    pub fn shared_lang(&self) -> usize {
        self.language.unwrap_or(0)
    }

    #[must_use]
    pub fn tab_name_for(&self, group: &str) -> &Template {
        self.group(group)
            .and_then(|g| g.tab_name.as_ref())
            .unwrap_or(&self.tab_name)
            .get(self.shared_lang())
    }

    #[must_use]
    pub fn nametag_for(&self, group: &str) -> (&Template, &Template) {
        let style = self.group(group);
        let lang = self.shared_lang();
        (
            style
                .and_then(|g| g.nametag_prefix.as_ref())
                .unwrap_or(&self.nametag_prefix)
                .get(lang),
            style
                .and_then(|g| g.nametag_suffix.as_ref())
                .unwrap_or(&self.nametag_suffix)
                .get(lang),
        )
    }

    /// The first layout for a viewer in `world` and `group`.
    #[must_use]
    pub fn layout_for(&self, world: &str, group: &str) -> Option<usize> {
        self.layouts
            .iter()
            .position(|l| l.worlds.matches(world) && l.groups.matches(group))
    }

    /// What any tab name depends on.
    #[must_use]
    pub fn name_deps(&self) -> Deps {
        self.groups
            .iter()
            .filter_map(|g| g.tab_name.as_ref())
            .fold(self.tab_name.deps(), |deps, t| deps.union(t.deps()))
    }

    /// What any nametag depends on.
    #[must_use]
    pub fn nametag_deps(&self) -> Deps {
        self.groups
            .iter()
            .flat_map(|g| [g.nametag_prefix.as_ref(), g.nametag_suffix.as_ref()])
            .flatten()
            .fold(
                self.nametag_prefix.deps().union(self.nametag_suffix.deps()),
                |deps, t| deps.union(t.deps()),
            )
    }

    /// What any text at all depends on.
    #[must_use]
    pub fn all_deps(&self) -> Deps {
        let mut deps = self
            .layouts
            .iter()
            .fold(self.name_deps().union(self.nametag_deps()), |deps, l| {
                deps.union(l.deps())
            });
        if let Some(score) = &self.tab_score {
            deps = deps.union(score.value.deps());
        }
        if let Some(below) = &self.below_name {
            deps = deps.union(below.score.value.deps());
        }
        deps
    }
}

/// The settings written on first start and used while no file exists.
pub const DEFAULT_TEXT: &str = "\
[settings]
refresh-seconds = 1
ping-seconds = 5
icons = text
sidebar = on
nametags = on
sort = on
tab-name = \"{icon}&6[{prefix}]&r {player}\"
nametag-prefix = \"{icon}&6[{prefix}]&r \"
nametag-suffix =

[group default]
tab-name = {player}
nametag-prefix =

[layout default]
worlds = *
groups = *
header = @default.header
footer = @default.footer
title = @default.title
line = @default.line.rank
line = @default.line.world
line = @default.line.online
line = @default.line.ping
";

/// Why a settings file was refused, at which line (1-based; 0 for the whole file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub line: usize,
    pub kind: ErrorKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    TooBig,
    Syntax,
    Unknown(String),
    Duplicate(String),
    Value(String),
    Limit(String, usize),
    Font(String),
    Icon(String),
}

impl ConfigError {
    /// The message in language `lang`.
    #[must_use]
    pub fn message(&self, lang: usize) -> String {
        let line = self.line.to_string();
        match &self.kind {
            ErrorKind::TooBig => {
                lang::text(lang, "err.too_big", &[&(MAX_FILE_BYTES / 1024).to_string()])
            }
            ErrorKind::Syntax => lang::text(lang, "err.syntax", &[&line]),
            ErrorKind::Unknown(name) => lang::text(lang, "err.unknown", &[&line, name]),
            ErrorKind::Duplicate(name) => lang::text(lang, "err.duplicate", &[&line, name]),
            ErrorKind::Value(key) => lang::text(lang, "err.value", &[&line, key]),
            ErrorKind::Limit(key, max) => {
                lang::text(lang, "err.limit", &[&line, key, &max.to_string()])
            }
            ErrorKind::Font(font) => lang::text(lang, "err.font", &[&line, font]),
            ErrorKind::Icon(id) => lang::text(lang, "err.icon", &[&line, id]),
        }
    }
}

enum Section {
    Settings,
    Group(usize),
    Icon(IconDraft),
    Layout(usize),
    Score(ScoreDraft),
    Below(ScoreDraft),
}

#[derive(Default)]
struct IconDraft {
    id: String,
    line: usize,
    glyph: Option<String>,
    font: Option<String>,
    head: Option<String>,
    head_name: Option<String>,
    hat: bool,
    fallback: Option<[String; 4]>,
}

#[derive(Default)]
struct ScoreDraft {
    line: usize,
    title: Option<Text>,
    value: Option<Template>,
    format: Option<Text>,
    min_change: u32,
    default_off: bool,
}

/// Reads a settings file with no owner language files.
///
/// # Errors
///
/// The first rule the file breaks.
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    parse_with(text, &OwnerLang::default())
}

/// Reads a settings file.
///
/// # Errors
///
/// The first rule the file breaks.
pub fn parse_with(text: &str, owner: &OwnerLang) -> Result<Config, ConfigError> {
    if text.len() > MAX_FILE_BYTES {
        return Err(err(0, ErrorKind::TooBig));
    }
    let mut config = Config {
        refresh_seconds: 1,
        ping_seconds: 5,
        icons_mode: IconsMode::Text,
        sidebar: true,
        nametags: true,
        sort: true,
        name_order: NameOrder::Client,
        language: None,
        missing: String::new(),
        sidebar_default: true,
        sidebar_command: Some("sidebar".to_owned()),
        sidebar_on_words: vec!["on".to_owned()],
        sidebar_off_words: vec!["off".to_owned()],
        tab_name: Text::own(Template::parse("{player}")),
        nametag_prefix: Text::own(Template::default()),
        nametag_suffix: Text::own(Template::default()),
        groups: Vec::new(),
        icons: Vec::new(),
        layouts: Vec::new(),
        tab_score: None,
        below_name: None,
    };
    let mut section: Option<Section> = None;
    let mut seen_sections: Vec<String> = Vec::new();
    let mut seen_keys: Vec<&str> = Vec::new();
    // Named images used, with their line, checked once every [icon] is known.
    let mut used_icons: Vec<(usize, String)> = Vec::new();
    // Language-file texts in what every viewer shares: only with a fixed language.
    let mut shared_keys: Vec<(usize, String)> = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(header) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            finish(&mut config, section.take())?;
            let header = header.trim();
            let (kind, name) = header
                .split_once(char::is_whitespace)
                .map_or((header, ""), |(k, n)| (k, n.trim()));
            let tag = format!("{kind} {name}");
            if seen_sections.contains(&tag) {
                return Err(err(line, ErrorKind::Duplicate(format!("[{header}]"))));
            }
            seen_sections.push(tag);
            seen_keys.clear();
            section = Some(match (kind, name) {
                ("settings", "") => Section::Settings,
                ("group", name) if valid_group(name) => {
                    if config.groups.len() == MAX_GROUPS {
                        return Err(err(line, ErrorKind::Limit("[group]".into(), MAX_GROUPS)));
                    }
                    config.groups.push(GroupStyle {
                        name: name.to_ascii_lowercase(),
                        tab_name: None,
                        nametag_prefix: None,
                        nametag_suffix: None,
                        color: None,
                    });
                    Section::Group(config.groups.len() - 1)
                }
                ("icon", id) if valid_id(id) => {
                    if config.icons.len() == MAX_ICONS {
                        return Err(err(line, ErrorKind::Limit("[icon]".into(), MAX_ICONS)));
                    }
                    Section::Icon(IconDraft {
                        id: id.to_owned(),
                        line,
                        ..IconDraft::default()
                    })
                }
                ("layout", name) if valid_id(name) => {
                    if config.layouts.len() == MAX_LAYOUTS {
                        return Err(err(line, ErrorKind::Limit("[layout]".into(), MAX_LAYOUTS)));
                    }
                    config.layouts.push(Layout {
                        name: name.to_owned(),
                        worlds: Match::Any,
                        groups: Match::Any,
                        header: Vec::new(),
                        footer: Vec::new(),
                        title: None,
                        lines: Vec::new(),
                    });
                    Section::Layout(config.layouts.len() - 1)
                }
                ("tab-score", "") => Section::Score(ScoreDraft {
                    line,
                    ..ScoreDraft::default()
                }),
                ("below-name", "") => Section::Below(ScoreDraft {
                    line,
                    ..ScoreDraft::default()
                }),
                _ => return Err(err(line, ErrorKind::Unknown(format!("[{header}]")))),
            });
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(err(line, ErrorKind::Syntax));
        };
        let key = key.trim();
        let value = unquote(value.trim());
        let Some(current) = section.as_mut() else {
            return Err(err(line, ErrorKind::Syntax));
        };
        let repeatable =
            matches!(current, Section::Layout(_)) && matches!(key, "header" | "footer" | "line");
        if !repeatable && let Some(known) = KNOWN_KEYS.iter().find(|k| **k == key) {
            if seen_keys.contains(known) {
                return Err(err(line, ErrorKind::Duplicate(key.to_owned())));
            }
            seen_keys.push(known);
        }
        if value.starts_with('@') && matches!(key, "tab-name" | "nametag-prefix" | "nametag-suffix")
        {
            shared_keys.push((line, key.to_owned()));
        }
        match current {
            Section::Settings => settings_key(&mut config, line, key, value, owner)?,
            Section::Score(draft) | Section::Below(draft) => match key {
                "value" => {
                    let t = plain_template(line, key, value, SCORE_KEYS)?;
                    note_icons(&mut used_icons, line, t.named_icons());
                    draft.value = Some(t);
                }
                "format" => {
                    let t = text_value(line, key, value, owner)?;
                    if !t.deps().is_empty() || t.named_icons().next().is_some() {
                        return Err(err(line, ErrorKind::Value(key.into())));
                    }
                    draft.format = Some(t);
                }
                "min-change" => {
                    draft.min_change = value
                        .parse()
                        .map_err(|_| err(line, ErrorKind::Value(key.into())))?;
                }
                "title" => {
                    let t = text_value(line, key, value, owner)?;
                    if !t.deps().is_empty() {
                        return Err(err(line, ErrorKind::Value(key.into())));
                    }
                    note_icons(&mut used_icons, line, t.named_icons());
                    draft.title = Some(t);
                }
                "default" => draft.default_off = !switch(line, key, value)?,
                _ => return Err(err(line, ErrorKind::Unknown(key.to_owned()))),
            },
            Section::Group(i) => {
                let group = &mut config.groups[*i];
                match key {
                    "tab-name" => {
                        let t = name_text(line, key, value, NAME_KEYS, owner)?;
                        note_icons(&mut used_icons, line, t.named_icons());
                        group.tab_name = Some(t);
                    }
                    "nametag-prefix" | "nametag-suffix" => {
                        let t = name_text(line, key, value, NAMETAG_KEYS, owner)?;
                        note_icons(&mut used_icons, line, t.named_icons());
                        if key == "nametag-prefix" {
                            group.nametag_prefix = Some(t);
                        } else {
                            group.nametag_suffix = Some(t);
                        }
                    }
                    "color" => {
                        group.color = Some(
                            color(value).ok_or_else(|| err(line, ErrorKind::Value(key.into())))?,
                        );
                    }
                    _ => return Err(err(line, ErrorKind::Unknown(key.to_owned()))),
                }
            }
            Section::Icon(draft) => {
                let text = || Some(value.to_owned());
                match key {
                    "glyph" => {
                        draft.glyph = Some(
                            glyph(value).ok_or_else(|| err(line, ErrorKind::Value(key.into())))?,
                        );
                    }
                    "font" => {
                        let font =
                            font(value).ok_or_else(|| err(line, ErrorKind::Value(key.into())))?;
                        let namespace = font.split(':').next().unwrap_or("");
                        if RESERVED_NAMESPACES.contains(&namespace) {
                            return Err(err(line, ErrorKind::Font(font)));
                        }
                        draft.font = Some(font);
                    }
                    "head" => {
                        if !valid_textures(value) {
                            return Err(err(line, ErrorKind::Value(key.into())));
                        }
                        draft.head = text();
                    }
                    "head-name" => {
                        if !valid_profile_name(value) {
                            return Err(err(line, ErrorKind::Value(key.into())));
                        }
                        draft.head_name = text();
                    }
                    "hat" => draft.hat = switch(line, key, value)?,
                    "fallback" => {
                        if value.chars().count() > MAX_TEMPLATE_CHARS {
                            return Err(err(
                                line,
                                ErrorKind::Limit(key.into(), MAX_TEMPLATE_CHARS),
                            ));
                        }
                        draft.fallback = Some(if let Some(name) = value.strip_prefix('@') {
                            if !owner.has(name) {
                                return Err(err(line, ErrorKind::Unknown(value.to_owned())));
                            }
                            [0, 1, 2, 3].map(|i| owner.text(i, name, &[]))
                        } else {
                            [0, 1, 2, 3].map(|_| value.to_owned())
                        });
                    }
                    _ => return Err(err(line, ErrorKind::Unknown(key.to_owned()))),
                }
            }
            Section::Layout(i) => {
                let layout = &mut config.layouts[*i];
                match key {
                    "worlds" | "groups" => {
                        let names = list(line, key, value)?;
                        if key == "worlds" {
                            layout.worlds = names;
                        } else {
                            layout.groups = names;
                        }
                    }
                    "header" | "footer" | "line" => {
                        let (lines, max) = match key {
                            "header" => (&mut layout.header, MAX_HEADER_LINES),
                            "footer" => (&mut layout.footer, MAX_HEADER_LINES),
                            _ => (&mut layout.lines, MAX_SIDEBAR_LINES),
                        };
                        if lines.len() == max {
                            return Err(err(line, ErrorKind::Limit(key.into(), max)));
                        }
                        let t = text_value(line, key, value, owner)?;
                        note_icons(&mut used_icons, line, t.named_icons());
                        lines.push(t);
                    }
                    "title" => {
                        let t = text_value(line, key, value, owner)?;
                        note_icons(&mut used_icons, line, t.named_icons());
                        layout.title = Some(t);
                    }
                    _ => return Err(err(line, ErrorKind::Unknown(key.to_owned()))),
                }
            }
        }
        if matches!(key, "tab-name" | "nametag-prefix" | "nametag-suffix")
            && matches!(section, Some(Section::Settings))
        {
            let t = match key {
                "tab-name" => &config.tab_name,
                "nametag-prefix" => &config.nametag_prefix,
                _ => &config.nametag_suffix,
            };
            note_icons(&mut used_icons, line, t.named_icons());
        }
    }
    finish(&mut config, section.take())?;
    for (line, id) in used_icons {
        if config.icon(&id).is_none() {
            return Err(err(line, ErrorKind::Icon(id)));
        }
    }
    // Tab names and nametags are one text for all viewers, so they cannot follow each client.
    if config.language.is_none()
        && let Some((line, key)) = shared_keys.into_iter().next()
    {
        return Err(err(line, ErrorKind::Value(key)));
    }
    if let Some(word) = config
        .sidebar_on_words
        .iter()
        .find(|w| config.sidebar_off_words.contains(w))
    {
        return Err(err(0, ErrorKind::Value(word.clone())));
    }
    Ok(config)
}

const KNOWN_KEYS: [&str; 33] = [
    "refresh-seconds",
    "ping-seconds",
    "icons",
    "sidebar",
    "nametags",
    "sort",
    "name-order",
    "language",
    "missing",
    "sidebar-default",
    "sidebar-command",
    "sidebar-on-words",
    "sidebar-off-words",
    "value",
    "format",
    "min-change",
    "default",
    "tab-name",
    "nametag-prefix",
    "nametag-suffix",
    "color",
    "glyph",
    "font",
    "head",
    "head-name",
    "hat",
    "fallback",
    "worlds",
    "groups",
    "title",
    "header",
    "footer",
    "line",
];

fn err(line: usize, kind: ErrorKind) -> ConfigError {
    ConfigError { line, kind }
}

fn settings_key(
    config: &mut Config,
    line: usize,
    key: &str,
    value: &str,
    owner: &OwnerLang,
) -> Result<(), ConfigError> {
    match key {
        "name-order" => {
            config.name_order = match value {
                "client" => NameOrder::Client,
                "exact-case" => NameOrder::ExactCase,
                _ => return Err(err(line, ErrorKind::Value(key.into()))),
            };
        }
        "language" => {
            config.language = match value {
                "client" => None,
                code => Some(
                    lang::FILES
                        .iter()
                        .position(|(c, _)| *c == code)
                        .ok_or_else(|| err(line, ErrorKind::Value(key.into())))?,
                ),
            };
        }
        "missing" => {
            if value.chars().count() > MAX_TEMPLATE_CHARS {
                return Err(err(line, ErrorKind::Limit(key.into(), MAX_TEMPLATE_CHARS)));
            }
            value.clone_into(&mut config.missing);
        }
        "sidebar-default" => config.sidebar_default = switch(line, key, value)?,
        "sidebar-command" => {
            config.sidebar_command = if value.is_empty() {
                None
            } else if valid_word(value) && value.is_ascii() {
                Some(value.to_owned())
            } else {
                return Err(err(line, ErrorKind::Value(key.into())));
            };
        }
        "sidebar-on-words" | "sidebar-off-words" => {
            let Match::Only(words) = list(line, key, value)? else {
                return Err(err(line, ErrorKind::Value(key.into())));
            };
            if !words.iter().all(|w| valid_word(w)) {
                return Err(err(line, ErrorKind::Value(key.into())));
            }
            if key == "sidebar-on-words" {
                config.sidebar_on_words = words;
            } else {
                config.sidebar_off_words = words;
            }
        }
        "refresh-seconds" | "ping-seconds" => {
            let seconds = value
                .parse::<u32>()
                .ok()
                .filter(|s| (1..=60).contains(s))
                .ok_or_else(|| err(line, ErrorKind::Value(key.into())))?;
            if key == "refresh-seconds" {
                config.refresh_seconds = seconds;
            } else {
                config.ping_seconds = seconds;
            }
        }
        "icons" => {
            config.icons_mode = match value {
                "text" => IconsMode::Text,
                "pack-required" => IconsMode::PackRequired,
                _ => return Err(err(line, ErrorKind::Value(key.into()))),
            };
        }
        "sidebar" => config.sidebar = switch(line, key, value)?,
        "nametags" => config.nametags = switch(line, key, value)?,
        "sort" => config.sort = switch(line, key, value)?,
        "tab-name" => config.tab_name = name_text(line, key, value, NAME_KEYS, owner)?,
        "nametag-prefix" => {
            config.nametag_prefix = name_text(line, key, value, NAMETAG_KEYS, owner)?;
        }
        "nametag-suffix" => {
            config.nametag_suffix = name_text(line, key, value, NAMETAG_KEYS, owner)?;
        }
        _ => return Err(err(line, ErrorKind::Unknown(key.to_owned()))),
    }
    Ok(())
}

/// A command word: 1 to 32 characters with no space or control character, in lower case.
fn valid_word(word: &str) -> bool {
    (1..=MAX_WORD_CHARS).contains(&word.chars().count())
        && word
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !c.is_uppercase())
}

fn finish_score(draft: ScoreDraft) -> Result<Score, ConfigError> {
    let value = draft
        .value
        .ok_or_else(|| err(draft.line, ErrorKind::Value("value".into())))?;
    Ok(Score {
        value,
        format: draft.format,
        min_change: draft.min_change,
    })
}

/// Closes a section: an `[icon]` becomes an [`Icon`] once it is complete, and so on.
fn finish(config: &mut Config, section: Option<Section>) -> Result<(), ConfigError> {
    let draft = match section {
        Some(Section::Icon(draft)) => draft,
        Some(Section::Score(draft)) => {
            if draft.title.is_some() || draft.default_off {
                return Err(err(draft.line, ErrorKind::Value("[tab-score]".into())));
            }
            config.tab_score = Some(finish_score(draft)?);
            return Ok(());
        }
        Some(Section::Below(mut draft)) => {
            let title = draft
                .title
                .take()
                .ok_or_else(|| err(draft.line, ErrorKind::Value("title".into())))?;
            let default_on = !draft.default_off;
            config.below_name = Some(BelowName {
                title,
                score: finish_score(draft)?,
                default_on,
            });
            return Ok(());
        }
        _ => return Ok(()),
    };
    let bad = |key: &str| err(draft.line, ErrorKind::Value(key.into()));
    let fallback = draft.fallback.clone().ok_or_else(|| bad("fallback"))?;
    let kind = match (draft.glyph, draft.font, draft.head) {
        (Some(text), Some(font), None) => IconKind::Glyph { text, font },
        (None, None, Some(textures)) => {
            let name = match draft.head_name {
                Some(name) => name,
                None => profile_name_from(&fallback[0]).ok_or_else(|| bad("head-name"))?,
            };
            IconKind::Head {
                textures,
                name,
                hat: draft.hat,
            }
        }
        (Some(_), None, None) => return Err(bad("font")),
        (None, Some(_), None) => return Err(bad("glyph")),
        _ => return Err(bad("head")),
    };
    config.icons.push(Icon {
        id: draft.id,
        kind,
        fallback,
    });
    Ok(())
}

fn note_icons<'a>(
    used: &mut Vec<(usize, String)>,
    line: usize,
    ids: impl Iterator<Item = &'a str>,
) {
    used.extend(ids.map(|id| (line, id.to_owned())));
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

/// A text every viewer shares; with a language-file text the parser checks the language later.
fn name_text(
    line: usize,
    key: &str,
    value: &str,
    allowed: Deps,
    owner: &OwnerLang,
) -> Result<Text, ConfigError> {
    let text = text_value(line, key, value, owner)?;
    if !text.deps().within(allowed) {
        return Err(err(line, ErrorKind::Value(key.into())));
    }
    Ok(text)
}

/// A template that is never a language-file text.
fn plain_template(
    line: usize,
    key: &str,
    value: &str,
    allowed: Deps,
) -> Result<Template, ConfigError> {
    if value.starts_with('@') {
        return Err(err(line, ErrorKind::Value(key.into())));
    }
    if value.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(err(line, ErrorKind::Limit(key.into(), MAX_TEMPLATE_CHARS)));
    }
    let template = Template::parse(value);
    if !template.deps().within(allowed) {
        return Err(err(line, ErrorKind::Value(key.into())));
    }
    Ok(template)
}

fn text_value(line: usize, key: &str, value: &str, owner: &OwnerLang) -> Result<Text, ConfigError> {
    if let Some(name) = value.strip_prefix('@') {
        return Text::named(name, owner)
            .ok_or_else(|| err(line, ErrorKind::Unknown(value.to_owned())));
    }
    if value.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(err(line, ErrorKind::Limit(key.into(), MAX_TEMPLATE_CHARS)));
    }
    Ok(Text::own(Template::parse(value)))
}

fn switch(line: usize, key: &str, value: &str) -> Result<bool, ConfigError> {
    match value {
        "on" | "true" | "yes" => Ok(true),
        "off" | "false" | "no" => Ok(false),
        _ => Err(err(line, ErrorKind::Value(key.into()))),
    }
}

fn list(line: usize, key: &str, value: &str) -> Result<Match, ConfigError> {
    if value == "*" {
        return Ok(Match::Any);
    }
    let names: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect();
    if names.is_empty() || names.iter().any(|n| n.chars().count() > 64) {
        return Err(err(line, ErrorKind::Value(key.into())));
    }
    if names.len() > MAX_LIST_ITEMS {
        return Err(err(line, ErrorKind::Limit(key.into(), MAX_LIST_ITEMS)));
    }
    Ok(Match::Only(names))
}

/// A perms-lite group name: 1 to 32 letters, digits, `_` or `-`.
fn valid_group(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `U+E100`, `\uE100`, or up to four characters as they are; never a control character.
fn glyph(value: &str) -> Option<String> {
    let mut out = String::new();
    for item in value.split_whitespace() {
        let code = item
            .strip_prefix("U+")
            .or_else(|| item.strip_prefix("u+"))
            .or_else(|| item.strip_prefix("\\u"));
        match code {
            Some(hex) => out.push(char::from_u32(u32::from_str_radix(hex, 16).ok()?)?),
            None => out.push_str(item),
        }
    }
    let count = out.chars().count();
    ((1..=4).contains(&count) && !out.chars().any(char::is_control)).then_some(out)
}

/// `namespace:path` as a resource location; a bare path is in `minecraft`.
pub(crate) fn font(value: &str) -> Option<String> {
    let (namespace, path) = value.split_once(':').unwrap_or(("minecraft", value));
    let ns_ok = !namespace.is_empty()
        && namespace
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b));
    let path_ok = !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-/".contains(&b));
    (ns_ok && path_ok && value.len() <= 128).then(|| format!("{namespace}:{path}"))
}

fn valid_textures(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXTURES_CHARS
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
}

/// A name 26.3 accepts in a profile: 1 to 16 letters, digits or `_`.
fn valid_profile_name(name: &str) -> bool {
    (1..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The letters, digits and `_` of `fallback` (colour codes left out), at most 16.
fn profile_name_from(fallback: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = fallback.chars();
    while let Some(c) = chars.next() {
        if c == '&' || c == '§' {
            chars.next();
        } else if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        }
    }
    out.truncate(16);
    valid_profile_name(&out).then_some(out)
}

fn color(value: &str) -> Option<NamedColor> {
    Some(match value {
        "black" => NamedColor::Black,
        "dark_blue" => NamedColor::DarkBlue,
        "dark_green" => NamedColor::DarkGreen,
        "dark_aqua" => NamedColor::DarkAqua,
        "dark_red" => NamedColor::DarkRed,
        "dark_purple" => NamedColor::DarkPurple,
        "gold" => NamedColor::Gold,
        "gray" => NamedColor::Gray,
        "dark_gray" => NamedColor::DarkGray,
        "blue" => NamedColor::Blue,
        "green" => NamedColor::Green,
        "aqua" => NamedColor::Aqua,
        "red" => NamedColor::Red,
        "light_purple" => NamedColor::LightPurple,
        "yellow" => NamedColor::Yellow,
        "white" => NamedColor::White,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bad(text: &str) -> ConfigError {
        parse(text).expect_err("refused")
    }

    #[test]
    fn the_builtin_settings_parse_and_have_one_layout_for_everyone() {
        let config = Config::builtin();
        assert_eq!(config.refresh_seconds, 1);
        assert_eq!(config.icons_mode, IconsMode::Text);
        assert_eq!(config.layouts.len(), 1);
        assert_eq!(config.layout_for("world_nether", "vip"), Some(0));
        let layout = &config.layouts[0];
        assert_eq!(layout.lines.len(), 4);
        assert!(layout.deps().contains(Key::Ping));
        assert!(layout.deps().contains(Key::Online));
        assert_eq!(config.tab_name_for("default").deps(), Deps::of(Key::Player));
        assert!(config.tab_name_for("vip").deps().contains(Key::Prefix));
        assert_eq!(config.sidebar_command.as_deref(), Some("sidebar"));
        assert_eq!(config.name_order, NameOrder::Client);
        assert_eq!(config.language, None);
        assert!(config.nametag_for("default").0.is_empty());
        // Built-in texts follow the viewer's language.
        let header = &layout.header[0];
        assert_ne!(header.get(0), header.get(3));
    }

    #[test]
    fn a_full_file_parses() {
        let config = parse(
            "# notes are skipped\n\
             [settings]\n\
             icons = pack-required\n\
             sort = off\n\
             [group VIP]\n\
             tab-name = \"{icon} &6{player}\"\n\
             color = gold\n\
             [icon vip]\n\
             glyph = U+E100\n\
             font = myserver:ranks\n\
             fallback = &6[VIP]\n\
             [icon star]\n\
             head = eyJ0ZXh0dXJlcyI6e319\n\
             fallback = [*]\n\
             head-name = star\n\
             [layout lobby]\n\
             worlds = lobby, Lobby2\n\
             groups = vip\n\
             header = Hi {player} {icon:star}\n\
             header = second\n\
             line = {ping}\n",
        )
        .expect("parses");
        assert_eq!(config.icons_mode, IconsMode::PackRequired);
        assert!(!config.sort);
        assert_eq!(
            config.group("vip").and_then(|g| g.color),
            Some(NamedColor::Gold)
        );
        assert_eq!(
            config.icon("vip").map(|i| &i.kind),
            Some(&IconKind::Glyph {
                text: "\u{E100}".into(),
                font: "myserver:ranks".into()
            })
        );
        assert!(matches!(
            config.icon("star").map(|i| &i.kind),
            Some(IconKind::Head { name, .. }) if name == "star"
        ));
        assert_eq!(config.layout_for("lobby2", "VIP"), Some(0));
        assert_eq!(config.layout_for("world", "vip"), None);
        assert_eq!(config.layouts[0].header.len(), 2);
    }

    #[test]
    fn quotes_keep_spaces_and_empty_values_are_empty() {
        let config = parse("[settings]\nnametag-prefix = \" {prefix} \"\nnametag-suffix =\n")
            .expect("parses");
        assert_eq!(config.nametag_prefix.get(0).parts().len(), 3);
        assert!(config.nametag_suffix.get(0).is_empty());
    }

    #[test]
    fn rules_are_enforced_with_the_line_number() {
        assert_eq!(bad("key = value").kind, ErrorKind::Syntax);
        assert_eq!(bad("[settings]\nno equals sign").line, 2);
        assert_eq!(
            bad("[settings]\nwhat = 1").kind,
            ErrorKind::Unknown("what".into())
        );
        assert_eq!(bad("[nope]").kind, ErrorKind::Unknown("[nope]".into()));
        assert_eq!(
            bad("[settings]\n[settings]").kind,
            ErrorKind::Duplicate("[settings]".into())
        );
        assert_eq!(
            bad("[settings]\nsort = on\nsort = off").kind,
            ErrorKind::Duplicate("sort".into())
        );
        assert_eq!(
            bad("[settings]\nrefresh-seconds = 0").kind,
            ErrorKind::Value("refresh-seconds".into())
        );
        assert_eq!(bad("[settings]\nicons = maybe").line, 2);
    }

    #[test]
    fn names_take_only_the_named_players_values() {
        // {online} in a name would send every name again at each join.
        assert_eq!(
            bad("[settings]\ntab-name = {player} {online}").kind,
            ErrorKind::Value("tab-name".into())
        );
        // A nametag is one team for the whole group, so the player's own values cannot be in it.
        assert_eq!(
            bad("[settings]\nnametag-prefix = {player}").kind,
            ErrorKind::Value("nametag-prefix".into())
        );
        // A name is the same for every viewer: no built-in (per-language) text.
        assert_eq!(
            bad("[settings]\ntab-name = @default.title").kind,
            ErrorKind::Value("tab-name".into())
        );
        assert_eq!(
            bad("[layout a]\nheader = @default.nothing").kind,
            ErrorKind::Unknown("@default.nothing".into())
        );
    }

    #[test]
    fn icons_need_a_complete_definition_and_their_own_font() {
        assert_eq!(
            bad("[icon a]\nglyph = U+E100\nfallback = x").kind,
            ErrorKind::Value("font".into())
        );
        assert_eq!(
            bad("[icon a]\nglyph = U+E100\nfont = x:y").kind,
            ErrorKind::Value("fallback".into())
        );
        assert_eq!(
            bad("[icon a]\nglyph = U+E100\nfont = strawberry_core:badge\nfallback = x").kind,
            ErrorKind::Font("strawberry_core:badge".into())
        );
        assert_eq!(
            bad("[icon a]\nglyph = \\u0007\nfont = a:b\nfallback = x").line,
            2
        );
        assert_eq!(bad("[icon a]\nhead = not base64!\nfallback = x").line, 2);
        // A head needs a name older clients can show: from the fallback, or given.
        assert_eq!(
            bad("[icon a]\nhead = abc=\nfallback = &6[*]").kind,
            ErrorKind::Value("head-name".into())
        );
        assert_eq!(
            bad("[layout a]\nline = {icon:missing}").kind,
            ErrorKind::Icon("missing".into())
        );
        assert_eq!(bad("[settings]\ntab-name = {icon:x} {player}").line, 2);
    }

    #[test]
    fn limits() {
        let many = "line = x\n".repeat(MAX_SIDEBAR_LINES + 1);
        assert_eq!(
            bad(&format!("[layout a]\n{many}")).kind,
            ErrorKind::Limit("line".into(), MAX_SIDEBAR_LINES)
        );
        let big = "#".repeat(MAX_FILE_BYTES + 1);
        assert_eq!(bad(&big).kind, ErrorKind::TooBig);
        let long = format!("[layout a]\ntitle = {}", "x".repeat(MAX_TEMPLATE_CHARS + 1));
        assert_eq!(
            bad(&long).kind,
            ErrorKind::Limit("title".into(), MAX_TEMPLATE_CHARS)
        );
    }

    #[test]
    fn the_builtin_settings_show_nothing_that_changes_for_everybody_every_second() {
        // A loaded server's TPS moves every second: in a built-in text it would send every
        // viewer a new footer each cycle.
        assert!(!Config::builtin().all_deps().contains(Key::Tps));
    }

    #[test]
    fn scores_need_a_value_and_take_only_the_named_players_values() {
        let config = parse(
            "[tab-score]\nvalue = {ping}\nformat = &7Ping: {value}\nmin-change = 10\n\
             [below-name]\ntitle = \" hearts\"\nvalue = {shop:cookies}\ndefault = off\n",
        )
        .expect("parses");
        let score = config.tab_score.as_ref().expect("set");
        assert_eq!(score.min_change, 10);
        assert_eq!(score.shown(0, 42).as_deref(), Some("&7Ping: 42"));
        let below = config.below_name.as_ref().expect("set");
        assert!(!below.default_on);
        assert_eq!(below.score.shown(0, 1), None);
        assert!(config.all_deps().contains(Key::Ping));
        assert!(config.all_deps().meets(Deps::provided("shop:cookies")));

        assert_eq!(
            bad("[tab-score]\nformat = x").kind,
            ErrorKind::Value("value".into())
        );
        assert_eq!(
            bad("[below-name]\nvalue = {ping}").kind,
            ErrorKind::Value("title".into())
        );
        assert_eq!(
            bad("[tab-score]\nvalue = {online}").kind,
            ErrorKind::Value("value".into())
        );
        assert_eq!(
            bad("[tab-score]\nvalue = {ping}\ntitle = x").kind,
            ErrorKind::Value("[tab-score]".into())
        );
        assert_eq!(
            bad("[tab-score]\nvalue = {ping}\nformat = {ping}").kind,
            ErrorKind::Value("format".into())
        );
    }

    #[test]
    fn values_of_other_plugins_are_allowed_in_names_and_nametags() {
        let config = parse(
            "[settings]\ntab-name = {head}{a:rank} {player}\nnametag-prefix = {a:rank}\n\
             [group vip]\nnametag-suffix = {a:tag}\n",
        )
        .expect("parses");
        assert!(config.nametag_deps().meets(Deps::provided("a:rank")));
        assert!(config.nametag_deps().meets(Deps::provided("a:tag")));
        assert!(config.name_deps().contains(Key::Head));
        // A face above the head would need a team per player: not offered.
        assert_eq!(
            bad("[settings]\nnametag-prefix = {head}").kind,
            ErrorKind::Value("nametag-prefix".into())
        );
    }

    #[test]
    fn owner_texts_follow_the_language_and_shared_ones_need_a_fixed_one() {
        let mut owner = OwnerLang::default();
        owner
            .read(0, "my.title=Hello\nmy.rank=Rank\n")
            .expect("reads");
        owner
            .read(3, "my.title=Merhaba\nmy.rank=Rütbe\n")
            .expect("reads");
        let config = parse_with(
            "[icon vip]\nglyph = U+E100\nfont = a:b\nfallback = @my.rank\n\
             [layout a]\ntitle = @my.title\n",
            &owner,
        )
        .expect("parses");
        let title = config.layouts[0].title.as_ref().expect("set");
        assert_ne!(title.get(0), title.get(3));
        let icon = config.icon("vip").expect("set");
        assert_eq!((icon.fallback(0), icon.fallback(3)), ("Rank", "Rütbe"));
        assert_eq!(icon.fallback(1), "Rank", "French falls back to English");

        // Unknown keys are refused, and a shared text needs one language for everybody.
        assert_eq!(
            parse_with("[layout a]\ntitle = @my.none\n", &owner)
                .expect_err("refused")
                .kind,
            ErrorKind::Unknown("@my.none".into())
        );
        assert_eq!(
            parse_with("[settings]\ntab-name = @my.title\n", &owner)
                .expect_err("refused")
                .kind,
            ErrorKind::Value("tab-name".into())
        );
        let fixed = parse_with("[settings]\ntab-name = @my.title\nlanguage = tr\n", &owner)
            .expect("parses");
        assert_eq!(fixed.lang_for(0), 3);
        assert_eq!(fixed.shared_lang(), 3);
        assert_eq!(
            fixed.tab_name_for("any"),
            &Template::parse("Merhaba"),
            "the fixed language"
        );
        assert_eq!(
            parse("[settings]\nlanguage = de")
                .map(|_| ())
                .expect_err("no")
                .line,
            2
        );
    }

    #[test]
    fn the_command_and_its_words() {
        let config = parse(
            "[settings]\nsidebar-command = pano\nsidebar-on-words = on, aç, ac\n\
             sidebar-off-words = off, kapat\nsidebar-default = off\nname-order = exact-case\n\
             missing = &8-\n",
        )
        .expect("parses");
        assert_eq!(config.sidebar_command.as_deref(), Some("pano"));
        assert_eq!(config.sidebar_on_words, ["on", "aç", "ac"]);
        assert!(!config.sidebar_default);
        assert_eq!(config.name_order, NameOrder::ExactCase);
        assert_eq!(config.missing, "&8-");
        let none = parse("[settings]\nsidebar-command =\n").expect("parses");
        assert_eq!(none.sidebar_command, None);
        assert_eq!(bad("[settings]\nsidebar-command = two words").line, 2);
        assert_eq!(bad("[settings]\nsidebar-on-words = Big").line, 2);
        assert_eq!(
            bad("[settings]\nsidebar-on-words = same\nsidebar-off-words = same").kind,
            ErrorKind::Value("same".into())
        );
    }

    #[test]
    fn panels_made_of_glyphs_are_allowed() {
        // An image that only moves other images has no text, and a sidebar may have no title.
        let config = parse(
            "[settings]\nicons = pack-required\n\
             [icon gap]\nglyph = U+E001\nfont = minecraft:default\nfallback =\n\
             [layout a]\ntitle =\nline = {icon:gap}x\nline = {icon:gap}x\n",
        )
        .expect("parses");
        assert_eq!(config.icon("gap").expect("set").fallback(0), "");
        let layout = &config.layouts[0];
        assert!(layout.title.as_ref().expect("set").get(0).is_empty());
        assert_eq!(layout.lines.len(), 2);
    }

    #[test]
    fn error_messages_are_translated() {
        let e = bad("[settings]\nwhat = 1");
        assert!(e.message(0).contains('2') && e.message(0).contains("what"));
        assert_ne!(e.message(0), e.message(3));
    }
}
