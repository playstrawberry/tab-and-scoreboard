//! What every player is sent, worked out without the host.
//!
//! The plugin feeds the planner what happened (joins, leaves, worlds, languages, pings, ranks,
//! TPS, settings, values from other plugins, players' choices) and asks it once per cycle for the
//! [`Action`]s to take. The planner keeps what each player was last sent and returns only what
//! changed: an idle server gets no actions and hardly any work, one player's new ping changes
//! only the texts that show it, and a join changes the texts that show `{online}`.
//!
//! How things are shown:
//!
//! - **Tab list name** (`tab-name`): one name per player, the same for every viewer
//!   (`player.set-tab-list-name`). The server still puts the platform badges in front of it for
//!   the viewers whose client draws them.
//! - **Order** (`sort`, `name-order`): the group weight as the tab-list order (26.3 sorts higher
//!   first) and in the team name (older clients sort by team name).
//! - **Nametag** (`nametag-prefix`, `nametag-suffix`, `color`): one team per distinct look, on
//!   each viewer's own scoreboard, created when a player needs it and removed when empty. The team
//!   shows only above the head: the tab list shows the name.
//! - **Scores** (`[tab-score]`, `[below-name]`): one objective per viewer, a score per player.
//! - **Header, footer, sidebar**: per viewer, from the first layout that fits the viewer's
//!   world and group, in the viewer's language.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use strawberry_plugin_api::common::NamedColor;

use crate::config::{Config, NameOrder, Score, Text};
use crate::placeholders::{Changes, Deps, Key, Op, Provided, Source, Store, Template, format_tps};
use crate::ranks::Rank;
use crate::render::{Context, Face, Policy, Rendered, render, render_lines, to_section_codes};
use crate::views::{Collision, Nametag, View};

/// Our team names start with this (other plugins' teams are never touched).
pub const TEAM_PREFIX: &str = "tas_";
/// The sidebar objective.
pub const OBJECTIVE: &str = "tas_sidebar";
/// The objective of the score in the tab list.
pub const TAB_OBJECTIVE: &str = "tas_tab";
/// The objective of the score below names.
pub const BELOW_OBJECTIVE: &str = "tas_below";
/// Most distinct nametag looks on a board. Over this a player gets the group's plain look.
pub const MAX_TEAMS: usize = 256;
/// Room kept for plain looks over [`MAX_TEAMS`]; a player over both shows no decoration.
const MAX_PLAIN_TEAMS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edition {
    Java,
    Bedrock,
}

/// What the planner knows of a player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub name: String,
    pub world: String,
    pub ping: u32,
    pub rank: Rank,
    /// Index into [`crate::lang::FILES`].
    pub lang: usize,
    pub edition: Edition,
}

/// The look of a nametag team.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeamStyle {
    pub prefix: Rendered,
    pub suffix: Rendered,
    pub color: NamedColor,
    pub collision: Collision,
    pub nametag: Nametag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    TabName {
        subject: String,
        name: Rendered,
    },
    TabNameReset {
        subject: String,
    },
    TabOrder {
        subject: String,
        order: i32,
    },
    HeaderFooter {
        viewer: String,
        header: Rendered,
        footer: Rendered,
    },
    /// Removes the teams and the objectives a previous run of this plugin left on the board.
    ClearBoard {
        viewer: String,
    },
    TeamCreate {
        viewer: String,
        team: String,
        style: TeamStyle,
    },
    TeamRemove {
        viewer: String,
        team: String,
    },
    TeamJoin {
        viewer: String,
        team: String,
        member: String,
    },
    TeamLeave {
        viewer: String,
        team: String,
        member: String,
    },
    SidebarShow {
        viewer: String,
        title: Rendered,
    },
    SidebarTitle {
        viewer: String,
        title: Rendered,
    },
    /// Line `index` (0 = top) appears.
    LineAdd {
        viewer: String,
        index: usize,
        text: Rendered,
    },
    LineSet {
        viewer: String,
        index: usize,
        text: Rendered,
    },
    LineRemove {
        viewer: String,
        index: usize,
    },
    /// Removes the sidebar and its `lines` lines.
    SidebarHide {
        viewer: String,
        lines: usize,
    },
    TabScoreShow {
        viewer: String,
    },
    TabScoreHide {
        viewer: String,
    },
    BelowShow {
        viewer: String,
        title: Rendered,
    },
    BelowHide {
        viewer: String,
    },
    /// `objective` is [`TAB_OBJECTIVE`] or [`BELOW_OBJECTIVE`].
    Score {
        viewer: String,
        objective: &'static str,
        entity: String,
        value: i32,
        shown: Option<Rendered>,
    },
    /// A player left: their scores go from this board.
    ForgetScores {
        viewer: String,
        entity: String,
    },
    /// A Bedrock sidebar: `old` lines replaced by `new` (section-coded text).
    BedrockSidebar {
        viewer: String,
        title: String,
        old: Vec<String>,
        new: Vec<String>,
        existed: bool,
    },
    BedrockSidebarHide {
        viewer: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Sidebar {
    title: Rendered,
    lines: Vec<Rendered>,
}

struct Seat {
    facts: Facts,
    face: Face,
    view: View,
    /// Own values that changed since the last plan.
    dirty: Deps,
    /// Joined since the last plan: nothing was sent yet.
    new: bool,
    /// The language changed: every viewer text again.
    relang: bool,
    /// The choices changed since the last plan.
    reviewed: bool,
    sent_name: Option<Rendered>,
    sent_order: i32,
    team: Option<String>,
    layout: Option<Option<usize>>,
    header: Option<(Rendered, Rendered)>,
    sidebar: Option<Sidebar>,
    bedrock: Option<(String, Vec<String>)>,
    /// The scores other viewers see of this player.
    tab_score: Option<i32>,
    below_score: Option<i32>,
    /// Whether this viewer's board has the objective.
    tab_shown: bool,
    below_shown: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Globals {
    max_players: u32,
    tps: String,
}

/// A nametag team and how many players are in it.
struct Team {
    style: TeamStyle,
    members: usize,
}

pub struct Planner {
    config: Arc<Config>,
    needs: Deps,
    reconfigured: bool,
    /// Something per player changed since the last plan.
    touched: bool,
    globals: Globals,
    changed: Deps,
    seats: BTreeMap<String, Seat>,
    teams: BTreeMap<String, Team>,
    store: Store,
    pending: Vec<Action>,
    team_overflow: bool,
}

struct Values<'a> {
    uuid: &'a str,
    facts: &'a Facts,
    online: usize,
    globals: &'a Globals,
    store: &'a Store,
    missing: &'a str,
}

impl Source for Values<'_> {
    fn value(&self, key: Key) -> String {
        match key {
            Key::Player => self.facts.name.clone(),
            Key::Ping => self.facts.ping.to_string(),
            Key::Online => self.online.to_string(),
            Key::MaxPlayers => self.globals.max_players.to_string(),
            Key::World => self.facts.world.clone(),
            Key::Group => self.facts.rank.group.clone(),
            Key::Prefix => self.facts.rank.prefix.clone(),
            Key::Suffix => self.facts.rank.suffix.clone(),
            Key::Tps => self.globals.tps.clone(),
            Key::Head => String::new(),
        }
    }

    fn provided(&self, name: &str) -> Option<&Provided> {
        self.store.get(self.uuid, name)
    }

    fn missing(&self) -> &str {
        self.missing
    }
}

/// No value from any plugin: a group's plain look.
struct Plain<'a>(&'a Values<'a>);

impl Source for Plain<'_> {
    fn value(&self, key: Key) -> String {
        self.0.value(key)
    }
}

/// How the name sorts inside one weight, for `name-order = exact-case`: the first characters as a
/// number, so a join moves nobody else and costs one update. Digits, then upper case, `_`, lower
/// case, as plain text comparison orders them; a shorter name first.
const NAME_BITS: u32 = 20;

fn name_symbol(c: Option<u8>) -> u32 {
    match c {
        None => 0,
        Some(b @ b'0'..=b'9') => 1 + u32::from(b - b'0'),
        Some(b @ b'A'..=b'Z') => 11 + u32::from(b - b'A'),
        Some(b'_') => 37,
        Some(b @ b'a'..=b'z') => 38 + u32::from(b - b'a'),
        // Anything else (a Bedrock name's mark) sorts after the letters.
        Some(_) => 63,
    }
}

/// The tab-list order of a player: 26.3 shows a higher order first. Without a weight or a name
/// order, 0, as in vanilla.
#[must_use]
pub fn list_order(config: &Config, rank: &Rank, name: &str) -> i32 {
    let weight = if config.sort { rank.weight } else { 0 };
    if config.name_order == NameOrder::Client {
        return weight;
    }
    let bytes = name.as_bytes();
    let symbol = |i: usize| name_symbol(bytes.get(i).copied());
    // Three whole characters and the top two bits of the fourth.
    let code = symbol(0) << 14 | symbol(1) << 8 | symbol(2) << 2 | symbol(3) >> 4;
    let top = (1_i64 << NAME_BITS) - 1;
    let weight = i64::from(weight.clamp(0, 2047));
    i32::try_from((weight << NAME_BITS) + (top - i64::from(code))).unwrap_or(i32::MAX)
}

/// The team of a look: higher weights sort first by name, then the look.
fn team_name(weight: i32, hash: u32) -> String {
    let order = 9999 - weight.clamp(0, 9999);
    format!("{TEAM_PREFIX}{order:04}{hash:08x}")
}

fn style_hash(style: &TeamStyle) -> u32 {
    format!("{style:?}").bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

/// The sidebar line team and the invisible entry it decorates.
#[must_use]
pub fn line_team(index: usize) -> (String, String) {
    const CODES: &[u8; 16] = b"0123456789abcdef";
    let code = char::from(CODES[index % 16]);
    (format!("{TEAM_PREFIX}line{index:02}"), format!("§{code}"))
}

/// The score of line `index`: the top line scores highest.
#[must_use]
pub fn line_score(index: usize) -> i32 {
    i32::try_from(crate::config::MAX_SIDEBAR_LINES.saturating_sub(index)).unwrap_or(0)
}

fn layout_deps(config: &Config, layout: Option<usize>) -> Deps {
    layout
        .and_then(|i| config.layouts.get(i))
        .map_or(Deps::NONE, crate::config::Layout::deps)
}

fn shown(score: &Score, lang: usize, value: i32) -> Option<Rendered> {
    score.shown(lang, value).map(|text| Rendered::text(&text))
}

fn over_threshold(sent: Option<i32>, value: i32, min_change: u32) -> bool {
    sent.is_none_or(|last| value != last && value.abs_diff(last) >= min_change.max(1))
}

impl Planner {
    #[must_use]
    pub fn new(config: Arc<Config>) -> Self {
        let needs = config.all_deps();
        Self {
            needs,
            config,
            reconfigured: false,
            touched: false,
            globals: Globals {
                max_players: 0,
                tps: format_tps(20.0),
            },
            changed: Deps::NONE,
            seats: BTreeMap::new(),
            teams: BTreeMap::new(),
            store: Store::default(),
            pending: Vec::new(),
            team_overflow: false,
        }
    }

    #[must_use]
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    pub fn set_config(&mut self, config: Arc<Config>) {
        self.config = config;
        self.needs = self.config.all_deps();
        self.reconfigured = true;
    }

    /// Whether any text shows `key` (so the plugin only reads values someone sees).
    #[must_use]
    pub fn needs(&self, key: Key) -> bool {
        self.needs.contains(key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.seats.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seats.is_empty()
    }

    #[must_use]
    pub fn contains(&self, uuid: &str) -> bool {
        self.seats.contains_key(uuid)
    }

    pub fn uuids(&self) -> impl Iterator<Item = &String> {
        self.seats.keys()
    }

    /// Whether a look had to fall back since the last call (for one line in the log).
    pub fn take_team_overflow(&mut self) -> bool {
        std::mem::take(&mut self.team_overflow)
    }

    /// The players whose ping is read in `cycle`: a slice of them each cycle, so every
    /// player's ping is read once every `every` cycles; nobody when no text shows a ping.
    #[must_use]
    pub fn ping_due(&self, cycle: u64, every: u64) -> Vec<String> {
        if !self.needs(Key::Ping) {
            return Vec::new();
        }
        let every = every.max(1);
        self.seats
            .keys()
            .enumerate()
            .filter(|(i, _)| (*i as u64) % every == cycle % every)
            .map(|(_, uuid)| uuid.clone())
            .collect()
    }

    pub fn join(&mut self, uuid: &str, facts: Facts, face: Face, view: View) {
        if self.seats.contains_key(uuid) {
            self.leave(uuid);
        }
        self.seats.insert(
            uuid.to_owned(),
            Seat {
                facts,
                face,
                view,
                dirty: Deps::NONE,
                new: true,
                relang: false,
                reviewed: false,
                sent_name: None,
                sent_order: 0,
                team: None,
                layout: None,
                header: None,
                sidebar: None,
                bedrock: None,
                tab_score: None,
                below_score: None,
                tab_shown: false,
                below_shown: None,
            },
        );
        self.touched = true;
        self.changed = self.changed.with(Key::Online);
    }

    pub fn leave(&mut self, uuid: &str) {
        let Some(seat) = self.seats.remove(uuid) else {
            return;
        };
        self.store.forget(uuid);
        self.changed = self.changed.with(Key::Online);
        let scored = seat.tab_score.is_some() || seat.below_score.is_some();
        let team = seat.team;
        if let Some(team) = &team
            && let Some(entry) = self.teams.get_mut(team)
        {
            entry.members -= 1;
        }
        for (viewer, other) in &self.seats {
            if other.new || other.facts.edition != Edition::Java {
                continue;
            }
            if let Some(team) = &team {
                self.pending.push(Action::TeamLeave {
                    viewer: viewer.clone(),
                    team: team.clone(),
                    member: seat.facts.name.clone(),
                });
            }
            if scored {
                self.pending.push(Action::ForgetScores {
                    viewer: viewer.clone(),
                    entity: seat.facts.name.clone(),
                });
            }
        }
    }

    fn update(&mut self, uuid: &str, change: impl FnOnce(&mut Facts) -> Deps) {
        if let Some(seat) = self.seats.get_mut(uuid) {
            let changed = change(&mut seat.facts);
            if !changed.is_empty() {
                seat.dirty = seat.dirty.union(changed);
                self.touched = true;
            }
        }
    }

    pub fn set_world(&mut self, uuid: &str, world: &str) {
        self.update(uuid, |facts| {
            if facts.world == world {
                return Deps::NONE;
            }
            world.clone_into(&mut facts.world);
            Deps::of(Key::World)
        });
    }

    pub fn set_ping(&mut self, uuid: &str, ping: u32) {
        self.update(uuid, |facts| {
            if facts.ping == ping {
                return Deps::NONE;
            }
            facts.ping = ping;
            Deps::of(Key::Ping)
        });
    }

    pub fn set_rank(&mut self, uuid: &str, rank: Rank) {
        self.update(uuid, |facts| {
            if facts.rank == rank {
                return Deps::NONE;
            }
            facts.rank = rank;
            Deps::RANK
        });
    }

    pub fn set_lang(&mut self, uuid: &str, lang: usize) {
        if let Some(seat) = self.seats.get_mut(uuid)
            && seat.facts.lang != lang
        {
            seat.facts.lang = lang;
            seat.relang = true;
            self.touched = true;
        }
    }

    pub fn set_face(&mut self, uuid: &str, face: Face) {
        if let Some(seat) = self.seats.get_mut(uuid)
            && seat.face != face
        {
            seat.face = face;
            seat.dirty = seat.dirty.with(Key::Head);
            self.touched = true;
        }
    }

    pub fn set_view(&mut self, uuid: &str, view: View) {
        if let Some(seat) = self.seats.get_mut(uuid)
            && seat.view != view
        {
            seat.view = view;
            seat.reviewed = true;
            self.touched = true;
        }
    }

    pub fn set_tps(&mut self, tps: f64) {
        let tps = format_tps(tps);
        if self.globals.tps != tps {
            self.globals.tps = tps;
            self.changed = self.changed.with(Key::Tps);
        }
    }

    pub fn set_max_players(&mut self, max: u32) {
        if self.globals.max_players != max {
            self.globals.max_players = max;
            self.changed = self.changed.with(Key::MaxPlayers);
        }
    }

    /// Values from another plugin. Keys nobody shows change nothing.
    pub fn apply_values(&mut self, plugin: &str, ops: Vec<Op>) {
        let changes = self.store.apply(plugin, ops);
        self.note_changes(changes);
    }

    fn note_changes(&mut self, changes: Changes) {
        if changes.everyone.meets(self.needs) {
            self.changed = self.changed.union(changes.everyone);
        }
        for (uuid, deps) in changes.players {
            if deps.meets(self.needs)
                && let Some(seat) = self.seats.get_mut(&uuid)
            {
                seat.dirty = seat.dirty.union(deps);
                self.touched = true;
            }
        }
    }

    /// A plugin is gone: its values become unset.
    pub fn forget_plugin(&mut self, plugin: &str) {
        let changes = self.store.apply(
            plugin,
            vec![Op::Clear {
                target: crate::placeholders::Target::Everyone,
                key: None,
            }],
        );
        self.note_changes(changes);
    }

    /// The plugins that have values now.
    #[must_use]
    pub fn providers(&self) -> HashSet<String> {
        self.store.plugins()
    }

    /// Forgets values of players who are not online and got nothing since the last call.
    pub fn sweep_values(&mut self, online: &HashSet<String>) {
        self.store.sweep(online);
    }

    fn java_viewers(&self) -> Vec<String> {
        self.seats
            .iter()
            .filter(|(_, s)| !s.new && s.facts.edition == Edition::Java)
            .map(|(uuid, _)| uuid.clone())
            .collect()
    }

    /// The team for `style`: an existing one with that look, or a new name. `None` when the board
    /// holds as many looks as it may; `bound` says how many.
    fn team_for(&self, weight: i32, style: &TeamStyle, bound: usize) -> Option<String> {
        let mut hash = style_hash(style);
        loop {
            let name = team_name(weight, hash);
            match self.teams.get(&name) {
                Some(team) if team.style == *style => return Some(name),
                Some(_) => hash = hash.wrapping_add(1),
                None if self.teams.len() < bound => return Some(name),
                None => return None,
            }
        }
    }

    fn style_of(
        &self,
        uuid: &str,
        seat: &Seat,
        online: usize,
        config: &Config,
        plain: bool,
    ) -> TeamStyle {
        let rank = &seat.facts.rank;
        let color = config
            .group(&rank.group)
            .and_then(|g| g.color)
            .unwrap_or(NamedColor::White);
        let (mut prefix, mut suffix) = (Rendered::default(), Rendered::default());
        if config.nametags {
            let values = Values {
                uuid,
                facts: &seat.facts,
                online,
                globals: &self.globals,
                store: &self.store,
                missing: &config.missing,
            };
            let cx = Context {
                group: &rank.group,
                config,
                policy: Policy::Java(config.icons_mode),
                lang: config.shared_lang(),
                face: None,
            };
            let (p, s) = config.nametag_for(&rank.group);
            if plain {
                let source = Plain(&values);
                prefix = render(p, &source, cx);
                suffix = render(s, &source, cx);
            } else {
                prefix = render(p, &values, cx);
                suffix = render(s, &values, cx);
            }
        }
        TeamStyle {
            prefix,
            suffix,
            color,
            collision: seat.view.collision,
            nametag: seat.view.nametag,
        }
    }

    /// Moves a player to the team of their look; the viewers follow.
    fn place_in_team(
        &mut self,
        uuid: &str,
        online: usize,
        config: &Config,
        viewers: &[String],
        out: &mut Vec<Action>,
    ) {
        let seat = &self.seats[uuid];
        let weight = seat.facts.rank.weight;
        let member = seat.facts.name.clone();
        let old = seat.team.clone();
        let style = self.style_of(uuid, seat, online, config, false);
        let chosen = match self.team_for(weight, &style, MAX_TEAMS) {
            Some(name) => Some((name, style)),
            None => {
                self.team_overflow = true;
                let seat = &self.seats[uuid];
                let plain = self.style_of(uuid, seat, online, config, true);
                self.team_for(weight, &plain, MAX_TEAMS + MAX_PLAIN_TEAMS)
                    .map(|name| (name, plain))
            }
        };
        if chosen.as_ref().map(|(name, _)| name) == old.as_ref() {
            return;
        }
        if let Some((name, style)) = &chosen {
            let team = self.teams.entry(name.clone()).or_insert_with(|| {
                for viewer in viewers {
                    out.push(Action::TeamCreate {
                        viewer: viewer.clone(),
                        team: name.clone(),
                        style: style.clone(),
                    });
                }
                Team {
                    style: style.clone(),
                    members: 0,
                }
            });
            team.members += 1;
        }
        if let Some(old) = &old
            && let Some(team) = self.teams.get_mut(old)
        {
            team.members -= 1;
        }
        for viewer in viewers {
            if let Some(old) = &old {
                out.push(Action::TeamLeave {
                    viewer: viewer.clone(),
                    team: old.clone(),
                    member: member.clone(),
                });
            }
            if let Some((name, _)) = &chosen {
                out.push(Action::TeamJoin {
                    viewer: viewer.clone(),
                    team: name.clone(),
                    member: member.clone(),
                });
            }
        }
        self.seats.get_mut(uuid).expect("known").team = chosen.map(|(name, _)| name);
    }

    /// The actions for everything that changed since the last plan.
    #[must_use]
    pub fn plan(&mut self) -> Vec<Action> {
        let mut out = std::mem::take(&mut self.pending);
        let reconfigured = std::mem::take(&mut self.reconfigured);
        let changed = std::mem::take(&mut self.changed);
        if !std::mem::take(&mut self.touched) && !reconfigured && !changed.meets(self.needs) {
            // Nothing anyone sees changed; a team may have emptied when a player left.
            self.remove_empty_teams(&mut out);
            return out;
        }
        let config = Arc::clone(&self.config);
        let online = self.seats.len();
        let viewers = self.java_viewers();

        self.plan_names_and_teams(&config, online, changed, reconfigured, &viewers, &mut out);
        self.remove_empty_teams(&mut out);
        self.plan_new_viewers(&mut out);
        self.plan_scores(&config, online, changed, reconfigured, &viewers, &mut out);
        self.plan_viewer_texts(&config, online, changed, reconfigured, &mut out);

        for seat in self.seats.values_mut() {
            seat.dirty = Deps::NONE;
            seat.new = false;
            seat.relang = false;
            seat.reviewed = false;
        }
        out
    }

    fn remove_empty_teams(&mut self, out: &mut Vec<Action>) {
        let empty: Vec<String> = self
            .teams
            .iter()
            .filter(|(_, team)| team.members == 0)
            .map(|(name, _)| name.clone())
            .collect();
        if empty.is_empty() {
            return;
        }
        let viewers = self.java_viewers();
        for team in empty {
            for viewer in &viewers {
                out.push(Action::TeamRemove {
                    viewer: viewer.clone(),
                    team: team.clone(),
                });
            }
            self.teams.remove(&team);
        }
    }

    fn plan_names_and_teams(
        &mut self,
        config: &Config,
        online: usize,
        changed: Deps,
        reconfigured: bool,
        viewers: &[String],
        out: &mut Vec<Action>,
    ) {
        let nametag_deps = config.nametag_deps();
        let uuids: Vec<String> = self.seats.keys().cloned().collect();
        for uuid in &uuids {
            let seat = &self.seats[uuid];
            let group = seat.facts.rank.group.clone();
            let template = config.tab_name_for(&group);
            let own = seat.dirty.union(changed);
            let due = seat.new
                || reconfigured
                || seat.dirty.meets(Deps::RANK)
                || own.meets(template.deps());
            let name = due.then(|| {
                let values = Values {
                    uuid,
                    facts: &seat.facts,
                    online,
                    globals: &self.globals,
                    store: &self.store,
                    missing: &config.missing,
                };
                render(
                    template,
                    &values,
                    Context {
                        group: &group,
                        config,
                        policy: Policy::Java(config.icons_mode),
                        lang: config.shared_lang(),
                        face: Some(&seat.face),
                    },
                )
            });
            let order = list_order(config, &seat.facts.rank, &seat.facts.name);
            let team_due = seat.new
                || reconfigured
                || seat.reviewed
                || seat.dirty.meets(Deps::RANK)
                || own.meets(nametag_deps);

            let seat = self.seats.get_mut(uuid).expect("listed above");
            if let Some(name) = name
                && seat.sent_name.as_ref() != Some(&name)
            {
                out.push(Action::TabName {
                    subject: uuid.clone(),
                    name: name.clone(),
                });
                seat.sent_name = Some(name);
            }
            if seat.sent_order != order {
                out.push(Action::TabOrder {
                    subject: uuid.clone(),
                    order,
                });
                seat.sent_order = order;
            }
            if team_due {
                self.place_in_team(uuid, online, config, viewers, out);
            }
        }
    }

    /// New Java viewers: every team, member and score at once.
    fn plan_new_viewers(&self, out: &mut Vec<Action>) {
        let new: Vec<String> = self
            .seats
            .iter()
            .filter(|(_, s)| s.new && s.facts.edition == Edition::Java)
            .map(|(uuid, _)| uuid.clone())
            .collect();
        if new.is_empty() {
            return;
        }
        let members: Vec<(String, String)> = self
            .seats
            .values()
            .filter_map(|s| s.team.clone().map(|t| (t, s.facts.name.clone())))
            .collect();
        for uuid in new {
            out.push(Action::ClearBoard {
                viewer: uuid.clone(),
            });
            for (team, entry) in &self.teams {
                out.push(Action::TeamCreate {
                    viewer: uuid.clone(),
                    team: team.clone(),
                    style: entry.style.clone(),
                });
            }
            for (team, member) in &members {
                out.push(Action::TeamJoin {
                    viewer: uuid.clone(),
                    team: team.clone(),
                    member: member.clone(),
                });
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn plan_scores(
        &mut self,
        config: &Config,
        online: usize,
        changed: Deps,
        reconfigured: bool,
        viewers: &[String],
        out: &mut Vec<Action>,
    ) {
        if config.tab_score.is_none() && config.below_name.is_none() {
            // Settings without scores: take down what the last ones showed.
            if reconfigured {
                self.take_down_scores(out);
            }
            return;
        }
        let tab_deps = config
            .tab_score
            .as_ref()
            .map_or(Deps::NONE, |s| s.value.deps());
        let below_deps = config
            .below_name
            .as_ref()
            .map_or(Deps::NONE, |b| b.score.value.deps());
        let mut tab_new: Vec<(String, i32)> = Vec::new();
        let mut below_new: Vec<(String, i32)> = Vec::new();
        for (uuid, seat) in &self.seats {
            let own = seat.dirty.union(changed);
            let values = Values {
                uuid,
                facts: &seat.facts,
                online,
                globals: &self.globals,
                store: &self.store,
                missing: &config.missing,
            };
            if let Some(score) = &config.tab_score
                && (seat.new || reconfigured || own.meets(tab_deps))
            {
                let value = score.value.number(&values);
                if over_threshold(seat.tab_score, value, score.min_change) {
                    tab_new.push((uuid.clone(), value));
                }
            }
            if let Some(below) = &config.below_name
                && (seat.new || reconfigured || own.meets(below_deps))
            {
                let value = below.score.value.number(&values);
                if over_threshold(seat.below_score, value, below.score.min_change) {
                    below_new.push((uuid.clone(), value));
                }
            }
        }
        for (uuid, value) in &tab_new {
            self.seats.get_mut(uuid).expect("known").tab_score = Some(*value);
        }
        for (uuid, value) in &below_new {
            self.seats.get_mut(uuid).expect("known").below_score = Some(*value);
        }

        // Changed scores reach the viewers that were already set up.
        let score_action = |viewer: &str,
                            lang: usize,
                            objective: &'static str,
                            score: &Score,
                            entity: &str,
                            value: i32| Action::Score {
            viewer: viewer.to_owned(),
            objective,
            entity: entity.to_owned(),
            value,
            shown: shown(score, lang, value),
        };
        for viewer in viewers {
            let seat = &self.seats[viewer];
            let lang = config.lang_for(seat.facts.lang);
            if let Some(score) = &config.tab_score
                && seat.tab_shown
            {
                for (uuid, value) in &tab_new {
                    let entity = &self.seats[uuid].facts.name;
                    out.push(score_action(
                        viewer,
                        lang,
                        TAB_OBJECTIVE,
                        score,
                        entity,
                        *value,
                    ));
                }
            }
            if let Some(below) = &config.below_name
                && seat.below_shown.is_some()
            {
                for (uuid, value) in &below_new {
                    let entity = &self.seats[uuid].facts.name;
                    out.push(score_action(
                        viewer,
                        lang,
                        BELOW_OBJECTIVE,
                        &below.score,
                        entity,
                        *value,
                    ));
                }
            }
        }

        // Viewers whose objectives change: new ones, a choice, a language or new settings.
        let names: Vec<(String, Option<i32>, Option<i32>)> = self
            .seats
            .values()
            .map(|s| (s.facts.name.clone(), s.tab_score, s.below_score))
            .collect();
        let uuids: Vec<String> = self.seats.keys().cloned().collect();
        for uuid in &uuids {
            let seat = self.seats.get_mut(uuid).expect("listed");
            if seat.facts.edition != Edition::Java {
                continue;
            }
            let lang = config.lang_for(seat.facts.lang);
            match &config.tab_score {
                Some(score) if !seat.tab_shown || reconfigured || seat.relang => {
                    if !seat.tab_shown {
                        out.push(Action::TabScoreShow {
                            viewer: uuid.clone(),
                        });
                    }
                    seat.tab_shown = true;
                    for (name, value, _) in &names {
                        if let Some(value) = value {
                            out.push(score_action(uuid, lang, TAB_OBJECTIVE, score, name, *value));
                        }
                    }
                }
                None if seat.tab_shown => {
                    out.push(Action::TabScoreHide {
                        viewer: uuid.clone(),
                    });
                    seat.tab_shown = false;
                }
                _ => {}
            }
            let want = config.below_name.as_ref().filter(|_| seat.view.below);
            match want {
                Some(below) if seat.below_shown != Some(lang) || reconfigured => {
                    let title = render(
                        below.title.get(lang),
                        &NoValues,
                        Context {
                            group: "",
                            config,
                            policy: Policy::Java(config.icons_mode),
                            lang,
                            face: None,
                        },
                    );
                    if seat.below_shown.is_some() {
                        out.push(Action::BelowHide {
                            viewer: uuid.clone(),
                        });
                    }
                    out.push(Action::BelowShow {
                        viewer: uuid.clone(),
                        title,
                    });
                    seat.below_shown = Some(lang);
                    for (name, _, value) in &names {
                        if let Some(value) = value {
                            out.push(score_action(
                                uuid,
                                lang,
                                BELOW_OBJECTIVE,
                                &below.score,
                                name,
                                *value,
                            ));
                        }
                    }
                }
                None if seat.below_shown.is_some() => {
                    out.push(Action::BelowHide {
                        viewer: uuid.clone(),
                    });
                    seat.below_shown = None;
                }
                _ => {}
            }
        }
        if config.tab_score.is_none() {
            for seat in self.seats.values_mut() {
                seat.tab_score = None;
            }
        }
        if config.below_name.is_none() {
            for seat in self.seats.values_mut() {
                seat.below_score = None;
            }
        }
    }

    fn take_down_scores(&mut self, out: &mut Vec<Action>) {
        for (uuid, seat) in &mut self.seats {
            if seat.tab_shown {
                out.push(Action::TabScoreHide {
                    viewer: uuid.clone(),
                });
                seat.tab_shown = false;
            }
            if seat.below_shown.take().is_some() {
                out.push(Action::BelowHide {
                    viewer: uuid.clone(),
                });
            }
            seat.tab_score = None;
            seat.below_score = None;
        }
    }

    fn plan_viewer_texts(
        &mut self,
        config: &Config,
        online: usize,
        changed: Deps,
        reconfigured: bool,
        out: &mut Vec<Action>,
    ) {
        for (uuid, seat) in &mut self.seats {
            let layout = config.layout_for(&seat.facts.world, &seat.facts.rank.group);
            let relayout = seat.new
                || reconfigured
                || seat.relang
                || seat.reviewed
                || seat.layout != Some(layout);
            let own = seat.dirty.union(changed);
            let due = relayout || own.meets(layout_deps(config, layout));
            seat.layout = Some(layout);
            if !due {
                continue;
            }
            let values = Values {
                uuid,
                facts: &seat.facts,
                online,
                globals: &self.globals,
                store: &self.store,
                missing: &config.missing,
            };
            let layout = layout.and_then(|i| config.layouts.get(i));
            let lang = config.lang_for(seat.facts.lang);
            let group = seat.facts.rank.group.clone();
            let visible = config.sidebar && seat.view.sidebar;
            let has_board =
                |l: &&crate::config::Layout| visible && (l.title.is_some() || !l.lines.is_empty());
            match seat.facts.edition {
                Edition::Java => {
                    let cx = Context {
                        group: &group,
                        config,
                        policy: Policy::Java(config.icons_mode),
                        lang,
                        face: Some(&seat.face),
                    };
                    let lines = |texts: &[Text]| {
                        render_lines(texts.iter().map(|t| t.get(lang)), &values, cx)
                    };
                    let want = layout.map_or_else(
                        || (Rendered::default(), Rendered::default()),
                        |l| (lines(&l.header), lines(&l.footer)),
                    );
                    match &seat.header {
                        None if want.0.is_empty() && want.1.is_empty() => {}
                        Some(sent) if *sent == want => {}
                        _ => out.push(Action::HeaderFooter {
                            viewer: uuid.clone(),
                            header: want.0.clone(),
                            footer: want.1.clone(),
                        }),
                    }
                    seat.header = Some(want);

                    let one = |t: &Template| render(t, &values, cx);
                    let want = layout.filter(has_board).map(|l| {
                        let sent = seat.sidebar.as_ref().filter(|_| !relayout);
                        Sidebar {
                            title: match (sent, &l.title) {
                                (Some(s), Some(t)) if !own.meets(t.deps()) => s.title.clone(),
                                (_, Some(t)) => one(t.get(lang)),
                                (_, None) => Rendered::default(),
                            },
                            // Only lines that show a changed value are rendered again.
                            lines: l
                                .lines
                                .iter()
                                .enumerate()
                                .map(|(i, t)| {
                                    sent.and_then(|s| s.lines.get(i))
                                        .filter(|_| !own.meets(t.deps()))
                                        .cloned()
                                        .unwrap_or_else(|| one(t.get(lang)))
                                })
                                .collect(),
                        }
                    });
                    diff_sidebar(out, uuid, seat.sidebar.as_ref(), want.as_ref());
                    seat.sidebar = want;
                }
                Edition::Bedrock => {
                    let cx = Context {
                        group: &group,
                        config,
                        policy: Policy::TextOnly,
                        lang,
                        face: None,
                    };
                    let one = |t: &Template| to_section_codes(&render(t, &values, cx));
                    let want = layout.filter(has_board).map(|l| {
                        (
                            l.title
                                .as_ref()
                                .map(|t| one(t.get(lang)))
                                .unwrap_or_default(),
                            l.lines.iter().map(|t| one(t.get(lang))).collect::<Vec<_>>(),
                        )
                    });
                    match (&seat.bedrock, &want) {
                        (Some(sent), Some(want)) if sent == want => {}
                        (None, None) => {}
                        (Some(_), None) => out.push(Action::BedrockSidebarHide {
                            viewer: uuid.clone(),
                        }),
                        (sent, Some((title, new))) => out.push(Action::BedrockSidebar {
                            viewer: uuid.clone(),
                            title: title.clone(),
                            old: sent.as_ref().map(|s| s.1.clone()).unwrap_or_default(),
                            new: new.clone(),
                            existed: sent.is_some(),
                        }),
                    }
                    seat.bedrock = want;
                }
            }
        }
    }

    /// The actions that take everything back (the plugin unloads).
    #[must_use]
    pub fn cleanup(&self) -> Vec<Action> {
        let mut out = Vec::new();
        for (uuid, seat) in &self.seats {
            if seat.sent_name.is_some() {
                out.push(Action::TabNameReset {
                    subject: uuid.clone(),
                });
            }
            if seat.sent_order != 0 {
                out.push(Action::TabOrder {
                    subject: uuid.clone(),
                    order: 0,
                });
            }
            if seat
                .header
                .as_ref()
                .is_some_and(|(h, f)| !h.is_empty() || !f.is_empty())
            {
                out.push(Action::HeaderFooter {
                    viewer: uuid.clone(),
                    header: Rendered::default(),
                    footer: Rendered::default(),
                });
            }
            if let Some(sidebar) = &seat.sidebar {
                out.push(Action::SidebarHide {
                    viewer: uuid.clone(),
                    lines: sidebar.lines.len(),
                });
            }
            if seat.bedrock.is_some() {
                out.push(Action::BedrockSidebarHide {
                    viewer: uuid.clone(),
                });
            }
            if seat.tab_shown {
                out.push(Action::TabScoreHide {
                    viewer: uuid.clone(),
                });
            }
            if seat.below_shown.is_some() {
                out.push(Action::BelowHide {
                    viewer: uuid.clone(),
                });
            }
            if seat.facts.edition == Edition::Java {
                for team in self.teams.keys() {
                    out.push(Action::TeamRemove {
                        viewer: uuid.clone(),
                        team: team.clone(),
                    });
                }
            }
        }
        out
    }
}

/// A text with no values (a below-name title).
struct NoValues;

impl Source for NoValues {
    fn value(&self, _key: Key) -> String {
        String::new()
    }
}

fn diff_sidebar(
    out: &mut Vec<Action>,
    viewer: &str,
    sent: Option<&Sidebar>,
    want: Option<&Sidebar>,
) {
    let viewer = viewer.to_owned();
    match (sent, want) {
        (None, None) => {}
        (Some(sent), None) => out.push(Action::SidebarHide {
            viewer,
            lines: sent.lines.len(),
        }),
        (None, Some(want)) => {
            out.push(Action::SidebarShow {
                viewer: viewer.clone(),
                title: want.title.clone(),
            });
            for (index, text) in want.lines.iter().enumerate() {
                out.push(Action::LineAdd {
                    viewer: viewer.clone(),
                    index,
                    text: text.clone(),
                });
            }
        }
        (Some(sent), Some(want)) => {
            if sent.title != want.title {
                out.push(Action::SidebarTitle {
                    viewer: viewer.clone(),
                    title: want.title.clone(),
                });
            }
            for index in 0..sent.lines.len().max(want.lines.len()) {
                match (sent.lines.get(index), want.lines.get(index)) {
                    (Some(a), Some(b)) if a == b => {}
                    (Some(_), Some(b)) => out.push(Action::LineSet {
                        viewer: viewer.clone(),
                        index,
                        text: b.clone(),
                    }),
                    (None, Some(b)) => out.push(Action::LineAdd {
                        viewer: viewer.clone(),
                        index,
                        text: b.clone(),
                    }),
                    (Some(_), None) => out.push(Action::LineRemove {
                        viewer: viewer.clone(),
                        index,
                    }),
                    (None, None) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
