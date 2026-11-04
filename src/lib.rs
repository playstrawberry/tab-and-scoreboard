//! Tab and Scoreboard: the tab list (names, order, header, footer and a score),
//! the nametags, a sidebar and a score below names, with ranks from perms-lite, images,
//! placeholders and values from other plugins.
//!
//! - Settings: `data/config.txt` ([`config`]), written with the built-in settings on first
//!   start, and the owner's language files `data/lang/<code>.lang` ([`lang`]); `/tab reload` reads
//!   them again.
//! - Ranks: perms-lite over `ipc`, contract `ranks/1` ([`ranks`]); perms-lite tells this plugin
//!   of every change.
//! - Other plugins: contracts `placeholders/1`, `sidebar/1`, `below-name/1` and `teams/1`
//!   ([`contracts`]).
//! - Placeholders: [`placeholders`], one module.
//! - Players' choices: `data/players.tsv` ([`views`]).
//! - Images and colours: [`render`] (glyphs only where the server's pack is required, heads
//!   everywhere, text otherwise).
//! - Work: the event handlers only write notes; one async task every `refresh-seconds` reads
//!   the notes, the pings due and the TPS, asks [`engine::Planner`] what changed and sends only
//!   that. The game tick never waits for this plugin.

pub mod config;
pub mod contracts;
pub mod engine;
pub mod lang;
pub mod placeholders;
pub mod ranks;
pub mod render;
pub mod views;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use strawberry_plugin_api::{
    Context, Player, Plugin, PluginMetadata, Result, Server,
    command::{Command, CommandError, CommandNode, CommandSender, ConsumedArgs},
    commands::CommandHandler,
    common::{ArgbColor, NamedColor, RgbColor},
    events::{
        EventData, EventHandler, FailureVerdict, HandlerOptions, HandlerPriority,
        PlayerChangedWorldEvent, PlayerJoinEvent, PlayerLeaveEvent, PlayerLocaleChangeEvent,
    },
    logging::{LogLevel, log},
    permission::{Permission, PermissionDefault, PermissionLevel},
    permissions,
    scheduler::SchedulerExt,
    scoreboard::{
        BedrockDisplaySlot, BedrockScoreboard, BedrockSortOrder, CollisionRule, DisplaySlot,
        NametagVisibility, NumberFormat, RenderType, Scoreboard, TeamSettings,
    },
    text::{HeadProfile, TextComponent},
    wit,
};

use config::Config;
use contracts::{Flag, Request};
use engine::{
    Action, BELOW_OBJECTIVE, Edition, Facts, OBJECTIVE, Planner, TAB_OBJECTIVE, TEAM_PREFIX,
    TeamStyle, line_score, line_team,
};
use lang::OwnerLang;
use placeholders::{Key, KeyLimit, Op};
use ranks::Rank;
use render::{Color, Face, Rendered, Run, Segment, Style, runs};
use views::{Collision, Nametag, Views};

const CONFIG_FILE: &str = "data/config.txt";
const VIEWS_FILE: &str = "data/players.tsv";
const ADMIN_NODE: &str = "tab-and-scoreboard:admin";
const SIDEBAR_NODE: &str = "tab-and-scoreboard:sidebar";
/// Notes kept while the task has not read them; beyond this the task looks at every player.
const MAX_NOTES: usize = 4096;
/// How often the online players are compared with the planner's, perms-lite is asked, the
/// plugins that pushed values are looked for, and players' choices are written.
const RECONCILE_SECONDS: u64 = 30;
const WATCH_SECONDS: u64 = 10;
const TPS_SECONDS: u64 = 10;
/// How many cycles a join waits for the server to list the player.
const JOIN_TRIES: u8 = 10;

/// What a handler saw, for the task to act on.
enum Note {
    Join(String, u8),
    Leave(String),
    World(String, String),
    Locale(String, String),
    Ranks(u64, Vec<(String, Rank)>),
    Config(Box<Config>),
    Values(String, Vec<Op>),
    /// A player's choices changed: read them again.
    View(String),
}

#[derive(Default)]
struct Inbox {
    notes: Vec<Note>,
    overflow: bool,
}

struct Handles {
    player: Player,
    board: Option<Scoreboard>,
    bedrock: Option<BedrockScoreboard>,
}

/// Everything the task owns. Taken out of its mutex for a cycle, so a call that comes back into
/// this plugin during a host call never waits on it.
struct Runtime {
    planner: Planner,
    handles: HashMap<String, Handles>,
    cycle: u64,
    rev: Option<u64>,
    perms: bool,
    warned_teams: bool,
    warned_notes: bool,
}

#[derive(Clone, Copy, Default)]
struct Status {
    players: usize,
    layouts: usize,
    perms: bool,
}

/// What commands say things with.
#[derive(Clone, Default)]
struct Messages {
    owner: Arc<OwnerLang>,
    language: Option<usize>,
}

impl Messages {
    fn text(&self, sender: &CommandSender, key: &str, args: &[&str]) -> String {
        let lang = self.language.unwrap_or_else(|| sender_lang(sender));
        self.owner.text(lang, key, args)
    }
}

struct Shared {
    inbox: Mutex<Inbox>,
    runtime: Mutex<Option<Runtime>>,
    status: Mutex<Status>,
    views: Mutex<Views>,
    key_limit: Mutex<KeyLimit>,
    /// Plugins already told about in the log, for what would otherwise repeat.
    warned: Mutex<HashSet<String>>,
    messages: Mutex<Messages>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn note(&self, note: Note) {
        let mut inbox = lock(&self.inbox);
        if inbox.notes.len() < MAX_NOTES {
            inbox.notes.push(note);
        } else {
            inbox.overflow = true;
        }
    }

    fn warn_once(&self, key: &str, text: &str) {
        if lock(&self.warned).insert(key.to_owned()) {
            log(LogLevel::Warn, text);
        }
    }

    /// Writes players' choices if they changed. Runs off the tick: from the task and at unload.
    fn save_views(&self) {
        let Some(text) = lock(&self.views).take_dirty() else {
            return;
        };
        let tmp = "data/players.tsv.tmp";
        let written = std::fs::write(tmp, text).and_then(|()| std::fs::rename(tmp, VIEWS_FILE));
        if let Err(e) = written {
            lock(&self.views).mark_dirty();
            log(
                LogLevel::Warn,
                &format!("tab-and-scoreboard: could not write {VIEWS_FILE}: {e}"),
            );
        }
    }
}

/// Writes the built-in settings when there is no settings file, so the owner can edit them.
fn write_default_config() {
    if !matches!(std::fs::metadata(CONFIG_FILE), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    {
        return;
    }
    let tmp = "data/config.txt.tmp";
    let written =
        std::fs::write(tmp, config::DEFAULT_TEXT).and_then(|()| std::fs::rename(tmp, CONFIG_FILE));
    if let Err(e) = written {
        log(
            LogLevel::Warn,
            &format!("tab-and-scoreboard: could not write {CONFIG_FILE}: {e}"),
        );
    }
}

/// Reads the owner's language files; a missing one is fine.
fn read_owner_lang(lang: usize) -> std::result::Result<OwnerLang, String> {
    let mut owner = OwnerLang::default();
    for (index, (code, _)) in lang::FILES.iter().enumerate() {
        let path = format!("data/lang/{code}.lang");
        let name = format!("lang/{code}.lang");
        match std::fs::metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Ok(meta) if meta.len() > lang::MAX_OWNER_FILE_BYTES as u64 => {
                return Err(lang::text(lang, "err.read", &[&name]));
            }
            _ => {}
        }
        let text =
            std::fs::read_to_string(&path).map_err(|_| lang::text(lang, "err.read", &[&name]))?;
        owner
            .read(index, &text)
            .map_err(|line| lang::text(lang, "err.lang", &[&name, &line.to_string()]))?;
    }
    Ok(owner)
}

/// Reads the settings file (the built-in settings when there is none) and the language files; the
/// message in language `lang` when they are refused.
fn read_config(lang: usize) -> std::result::Result<(Config, OwnerLang), String> {
    let owner = read_owner_lang(lang)?;
    let too_big = || {
        config::ConfigError {
            line: 0,
            kind: config::ErrorKind::TooBig,
        }
        .message(lang)
    };
    match std::fs::metadata(CONFIG_FILE) {
        Ok(meta) if meta.len() > config::MAX_FILE_BYTES as u64 => return Err(too_big()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Config::builtin(), owner));
        }
        _ => {}
    }
    let text = std::fs::read_to_string(CONFIG_FILE)
        .map_err(|_| lang::text(lang, "err.read", &[CONFIG_FILE]))?;
    if text.len() > config::MAX_FILE_BYTES {
        return Err(too_big());
    }
    let config = config::parse_with(&text, &owner).map_err(|e| e.message(lang))?;
    Ok((config, owner))
}

fn face_of(player: &Player, uuid: &str, planner: &Planner) -> Face {
    let skin = planner
        .needs(Key::Head)
        .then(|| player.get_skin())
        .flatten();
    Face {
        id: uuid.to_owned(),
        signature: skin.as_ref().and_then(|s| s.signature.clone()),
        textures: skin.map(|s| s.value),
    }
}

fn add_player(shared: &Shared, rt: &mut Runtime, player: Player, joined: &mut Vec<String>) {
    let uuid = player.get_id().to_string();
    let world = player.get_world().get_name();
    let lang = lang::index(&player.get_locale());
    let ping = if rt.planner.needs(Key::Ping) {
        player.get_ping()
    } else {
        0
    };
    let (edition, board, bedrock) = if let Some(java) = player.as_java() {
        (Edition::Java, Some(java.get_scoreboard()), None)
    } else if let Some(bedrock) = player.as_bedrock() {
        (Edition::Bedrock, None, Some(bedrock.get_scoreboard()))
    } else {
        (Edition::Java, None, None)
    };
    let face = face_of(&player, &uuid, &rt.planner);
    let view = lock(&shared.views).view(&uuid);
    rt.planner.join(
        &uuid,
        Facts {
            name: player.get_name(),
            world,
            ping,
            rank: Rank::default(),
            lang,
            edition,
        },
        face,
        view,
    );
    rt.handles.insert(
        uuid.clone(),
        Handles {
            player,
            board,
            bedrock,
        },
    );
    joined.push(uuid);
}

/// Compares the online players with the planner's and fixes any difference.
fn reconcile(shared: &Shared, rt: &mut Runtime, server: &Server, joined: &mut Vec<String>) {
    let mut online = HashSet::new();
    for player in server.get_all_players() {
        let uuid = player.get_id().to_string();
        online.insert(uuid.clone());
        if !rt.planner.contains(&uuid) {
            add_player(shared, rt, player, joined);
        }
    }
    let gone: Vec<String> = rt
        .planner
        .uuids()
        .filter(|uuid| !online.contains(*uuid))
        .cloned()
        .collect();
    for uuid in gone {
        rt.planner.leave(&uuid);
        rt.handles.remove(&uuid);
    }
    rt.planner.set_max_players(server.get_max_players());
    rt.planner.sweep_values(&online);

    // A skin can change after a join (a skin plugin); only read when a text shows a head.
    if rt.planner.needs(Key::Head) {
        for (uuid, handles) in &rt.handles {
            let face = face_of(&handles.player, uuid, &rt.planner);
            rt.planner.set_face(uuid, face);
        }
    }

    // A plugin that pushed values and is gone: its values become unset. The server answers that
    // no such plugin exists without asking anybody.
    for plugin in rt.planner.providers() {
        let probe = strawberry_plugin_api::ipc::send_ipc_message(
            &plugin,
            format!("{}\talive", contracts::VALUES).as_bytes(),
        );
        if probe.is_err() {
            rt.planner.forget_plugin(&plugin);
        }
    }
}

fn fetch_ranks(rt: &mut Runtime, uuids: &[String]) {
    for chunk in uuids.chunks(ranks::MAX_UUIDS) {
        match ranks::get(chunk) {
            Some(answer) => {
                rt.perms = true;
                for (uuid, rank) in answer.ranks {
                    rt.planner.set_rank(&uuid, rank);
                }
            }
            None => {
                rt.perms = false;
                return;
            }
        }
    }
}

fn take_notes(shared: &Shared, rt: &mut Runtime) -> (Vec<Note>, bool) {
    let (notes, overflow) = {
        let mut inbox = lock(&shared.inbox);
        let overflow = std::mem::take(&mut inbox.overflow);
        (std::mem::take(&mut inbox.notes), overflow)
    };
    if overflow && !std::mem::replace(&mut rt.warned_notes, true) {
        log(
            LogLevel::Warn,
            "tab-and-scoreboard: more changes arrived in one cycle than it keeps; players are \
             checked again, but values other plugins pushed in that moment are lost.",
        );
    }
    (notes, overflow)
}

fn run_notes(
    shared: &Shared,
    rt: &mut Runtime,
    server: &Server,
    notes: Vec<Note>,
    joined: &mut Vec<String>,
) -> Vec<Note> {
    let mut retry = Vec::new();
    for note in notes {
        match note {
            Note::Join(uuid, tries) => {
                if rt.planner.contains(&uuid) {
                    continue;
                }
                let found = strawberry_plugin_api::uuid::parse(&uuid)
                    .and_then(|id| server.get_player_by_uuid(id));
                match found {
                    Some(player) => add_player(shared, rt, player, joined),
                    None if tries > 1 => retry.push(Note::Join(uuid, tries - 1)),
                    None => {}
                }
            }
            Note::Leave(uuid) => {
                rt.planner.leave(&uuid);
                rt.handles.remove(&uuid);
            }
            Note::World(uuid, world) => rt.planner.set_world(&uuid, &world),
            Note::Locale(uuid, locale) => rt.planner.set_lang(&uuid, lang::index(&locale)),
            Note::Ranks(rev, list) => {
                rt.rev = Some(rev);
                for (uuid, rank) in list {
                    rt.planner.set_rank(&uuid, rank);
                }
            }
            Note::Config(config) => {
                rt.planner.set_config(Arc::new(*config));
                // New defaults reach those who chose nothing.
                let uuids: Vec<String> = rt.planner.uuids().cloned().collect();
                let views = lock(&shared.views);
                for uuid in uuids {
                    rt.planner.set_view(&uuid, views.view(&uuid));
                }
            }
            Note::Values(plugin, ops) => rt.planner.apply_values(&plugin, ops),
            Note::View(uuid) => {
                let view = lock(&shared.views).view(&uuid);
                rt.planner.set_view(&uuid, view);
            }
        }
    }
    retry
}

fn run_cycle(shared: &Shared, server: &Server) {
    // Another cycle still runs (a call came back in): skip this one.
    let Some(mut rt) = lock(&shared.runtime).take() else {
        return;
    };
    let (notes, overflow) = take_notes(shared, &mut rt);
    rt.cycle += 1;
    let refresh = u64::from(rt.planner.config().refresh_seconds);
    let cycles = |seconds: u64| (seconds / refresh).max(1);
    let mut joined = Vec::new();
    let check = rt.cycle == 1 || overflow || rt.cycle.is_multiple_of(cycles(RECONCILE_SECONDS));

    if check {
        reconcile(shared, &mut rt, server, &mut joined);
        shared.save_views();
    }
    let retry = run_notes(shared, &mut rt, server, notes, &mut joined);

    // perms-lite: the ranks of new players, and every rank again when perms-lite changed
    // without telling (it was reloaded).
    if rt.cycle == 1 || rt.cycle.is_multiple_of(cycles(WATCH_SECONDS)) {
        match ranks::watch() {
            Some(rev) => {
                rt.perms = true;
                if rt.rev != Some(rev) {
                    rt.rev = Some(rev);
                    joined = rt.planner.uuids().cloned().collect();
                }
            }
            None => rt.perms = false,
        }
    }
    if !joined.is_empty() && (rt.perms || rt.cycle == 1) {
        fetch_ranks(&mut rt, &joined);
    }

    let every = cycles(u64::from(rt.planner.config().ping_seconds));
    for uuid in rt.planner.ping_due(rt.cycle, every) {
        if let Some(handles) = rt.handles.get(&uuid) {
            let ping = handles.player.get_ping();
            rt.planner.set_ping(&uuid, ping);
        }
    }
    // Not every cycle: a loaded server's TPS moves all the time, and each move is a new text for
    // every viewer who sees it.
    if rt.planner.needs(Key::Tps) && (rt.cycle == 1 || rt.cycle.is_multiple_of(cycles(TPS_SECONDS)))
    {
        rt.planner.set_tps(server.get_tps());
    }

    let actions = rt.planner.plan();
    if rt.planner.take_team_overflow() && !std::mem::replace(&mut rt.warned_teams, true) {
        log(
            LogLevel::Warn,
            &format!(
                "tab-and-scoreboard: more than {} different nametag looks; players over that get \
                 their group's plain look.",
                engine::MAX_TEAMS
            ),
        );
    }
    apply(&rt.handles, actions);

    *lock(&shared.status) = Status {
        players: rt.planner.len(),
        layouts: rt.planner.config().layouts.len(),
        perms: rt.perms,
    };
    lock(&shared.inbox).notes.extend(retry);
    *lock(&shared.runtime) = Some(rt);
}

const fn named(code: char) -> NamedColor {
    match code {
        '0' => NamedColor::Black,
        '1' => NamedColor::DarkBlue,
        '2' => NamedColor::DarkGreen,
        '3' => NamedColor::DarkAqua,
        '4' => NamedColor::DarkRed,
        '5' => NamedColor::DarkPurple,
        '6' => NamedColor::Gold,
        '7' => NamedColor::Gray,
        '8' => NamedColor::DarkGray,
        '9' => NamedColor::Blue,
        'a' => NamedColor::Green,
        'b' => NamedColor::Aqua,
        'c' => NamedColor::Red,
        'd' => NamedColor::LightPurple,
        'e' => NamedColor::Yellow,
        _ => NamedColor::White,
    }
}

fn styled(text: &str, style: &Style) -> TextComponent {
    let mut out = TextComponent::text(text);
    match style.color {
        Some(Color::Code(code)) => out = out.color_named(named(code)),
        Some(Color::Rgb(r, g, b)) => out = out.color_rgb(RgbColor { r, g, b }),
        None => {}
    }
    if style.bold {
        out = out.bold(true);
    }
    if style.italic {
        out = out.italic(true);
    }
    if style.underlined {
        out = out.underlined(true);
    }
    if style.strikethrough {
        out = out.strikethrough(true);
    }
    if style.obfuscated {
        out = out.obfuscated(true);
    }
    out
}

fn image(segment: &Segment) -> TextComponent {
    match segment {
        Segment::Glyph { text, font } => TextComponent::text(text)
            .font(font)
            .color_rgb(RgbColor {
                r: 255,
                g: 255,
                b: 255,
            })
            .bold(false)
            .italic(false)
            .underlined(false)
            .strikethrough(false)
            .obfuscated(false),
        Segment::Head {
            textures,
            signature,
            name,
            id,
            hat,
            shadow,
        } => {
            let profile = HeadProfile {
                name: Some(name.clone()),
                id: id.clone(),
                textures: textures.clone(),
                textures_signature: signature.clone(),
            };
            TextComponent::player_head(&profile, *hat).map_or_else(
                |_| TextComponent::text(name),
                |head| {
                    let head = head.color_named(NamedColor::White);
                    if *shadow {
                        head
                    } else {
                        head.shadow_color(ArgbColor {
                            a: 0,
                            r: 0,
                            g: 0,
                            b: 0,
                        })
                    }
                },
            )
        }
        Segment::Text(text) | Segment::Literal(text) => TextComponent::text(text),
    }
}

fn component(rendered: &Rendered) -> TextComponent {
    let piece = |run: &Run<'_>| match run {
        Run::Text { text, style } => styled(text, style),
        Run::Image(segment) => image(segment),
    };
    let runs = runs(rendered);
    match runs.as_slice() {
        [] => TextComponent::text(""),
        [one] => piece(one),
        many => many.iter().fold(TextComponent::text(""), |root, run| {
            root.add_child(piece(run))
        }),
    }
}

const fn collision_rule(collision: Collision) -> CollisionRule {
    match collision {
        Collision::Always => CollisionRule::Always,
        Collision::Never => CollisionRule::Never,
        Collision::PushOther => CollisionRule::PushOtherTeams,
        Collision::PushOwn => CollisionRule::PushOwnTeam,
    }
}

const fn nametag_rule(nametag: Nametag) -> NametagVisibility {
    match nametag {
        Nametag::Always => NametagVisibility::Always,
        Nametag::Never => NametagVisibility::Never,
        Nametag::HideOther => NametagVisibility::HideForOtherTeams,
        Nametag::HideOwn => NametagVisibility::HideForOwnTeam,
    }
}

fn style_settings(team: &str, style: &TeamStyle) -> TeamSettings {
    // Vanilla's team defaults, except that teammates do not see each other when invisible
    // (without a team they do not either): the plugin's teams change nothing but the names,
    // unless another plugin asked for an option.
    TeamSettings {
        display_name: TextComponent::text(team),
        friendly_fire: true,
        see_friendly_invisibles: false,
        nametag_visibility: nametag_rule(style.nametag),
        collision_rule: collision_rule(style.collision),
        color: style.color,
        prefix: component(&style.prefix),
        suffix: component(&style.suffix),
    }
}

fn line_settings(team: &str, text: &Rendered) -> TeamSettings {
    style_settings(
        team,
        &TeamStyle {
            prefix: text.clone(),
            suffix: Rendered::default(),
            color: NamedColor::White,
            collision: Collision::Always,
            nametag: Nametag::Always,
        },
    )
}

fn bedrock_entry(index: usize, line: &str) -> String {
    // The text is the entry's name, so two equal lines need a difference nobody sees.
    format!("{line}{}", line_team(index).1)
}

#[allow(clippy::too_many_lines)]
fn apply(handles: &HashMap<String, Handles>, actions: Vec<Action>) {
    let player = |uuid: &str| handles.get(uuid).map(|h| &h.player);
    let board = |uuid: &str| handles.get(uuid).and_then(|h| h.board.as_ref());
    for action in actions {
        match action {
            Action::TabName { subject, name } => {
                if let Some(p) = player(&subject) {
                    p.set_tab_list_name(Some(component(&name)));
                }
            }
            Action::TabNameReset { subject } => {
                if let Some(p) = player(&subject) {
                    p.set_tab_list_name(None);
                }
            }
            Action::TabOrder { subject, order } => {
                if let Some(p) = player(&subject) {
                    p.set_tab_list_order(order);
                }
            }
            Action::HeaderFooter {
                viewer,
                header,
                footer,
            } => {
                if let Some(p) = player(&viewer) {
                    p.set_tab_list_header_footer(component(&header), component(&footer));
                }
            }
            Action::ClearBoard { viewer } => {
                if let Some(b) = board(&viewer) {
                    let ours: Vec<String> = b
                        .get_teams()
                        .into_iter()
                        .filter(|t| t.starts_with(TEAM_PREFIX))
                        .collect();
                    // Left by an earlier run of this plugin (it stopped without its unload).
                    if !ours.is_empty() {
                        b.remove_objective(OBJECTIVE);
                        b.remove_objective(TAB_OBJECTIVE);
                        b.remove_objective(BELOW_OBJECTIVE);
                        for team in ours {
                            b.remove_team(&team);
                        }
                    }
                }
            }
            Action::TeamCreate {
                viewer,
                team,
                style,
            } => {
                if let Some(b) = board(&viewer) {
                    b.create_team(&team, style_settings(&team, &style));
                }
            }
            Action::TeamRemove { viewer, team } => {
                if let Some(b) = board(&viewer) {
                    b.remove_team(&team);
                }
            }
            Action::TeamJoin {
                viewer,
                team,
                member,
            } => {
                if let Some(b) = board(&viewer) {
                    b.add_player_to_team(&team, &member);
                }
            }
            Action::TeamLeave {
                viewer,
                team,
                member,
            } => {
                if let Some(b) = board(&viewer) {
                    b.remove_player_from_team(&team, &member);
                }
            }
            Action::SidebarShow { viewer, title } => {
                if let Some(b) = board(&viewer) {
                    b.add_objective(
                        OBJECTIVE,
                        component(&title),
                        RenderType::Integer,
                        Some(NumberFormat::Blank),
                    );
                    b.set_display_slot(DisplaySlot::Sidebar, OBJECTIVE);
                }
            }
            Action::SidebarTitle { viewer, title } => {
                if let Some(b) = board(&viewer) {
                    b.update_objective(
                        OBJECTIVE,
                        component(&title),
                        RenderType::Integer,
                        Some(NumberFormat::Blank),
                    );
                }
            }
            Action::LineAdd {
                viewer,
                index,
                text,
            } => {
                if let Some(b) = board(&viewer) {
                    let (team, entry) = line_team(index);
                    b.create_team(&team, line_settings(&team, &text));
                    b.add_player_to_team(&team, &entry);
                    b.update_score(
                        &entry,
                        OBJECTIVE,
                        line_score(index),
                        Some(NumberFormat::Blank),
                    );
                }
            }
            Action::LineSet {
                viewer,
                index,
                text,
            } => {
                if let Some(b) = board(&viewer) {
                    let (team, _) = line_team(index);
                    b.update_team(&team, line_settings(&team, &text));
                }
            }
            Action::LineRemove { viewer, index } => {
                if let Some(b) = board(&viewer) {
                    let (team, entry) = line_team(index);
                    b.remove_score(&entry, OBJECTIVE);
                    b.remove_team(&team);
                }
            }
            Action::SidebarHide { viewer, lines } => {
                if let Some(b) = board(&viewer) {
                    b.remove_objective(OBJECTIVE);
                    for index in 0..lines {
                        b.remove_team(&line_team(index).0);
                    }
                }
            }
            Action::TabScoreShow { viewer } => {
                if let Some(b) = board(&viewer) {
                    b.add_objective(
                        TAB_OBJECTIVE,
                        TextComponent::text(""),
                        RenderType::Integer,
                        None,
                    );
                    b.set_display_slot(DisplaySlot::PlayerList, TAB_OBJECTIVE);
                }
            }
            Action::TabScoreHide { viewer } => {
                if let Some(b) = board(&viewer) {
                    b.remove_objective(TAB_OBJECTIVE);
                }
            }
            Action::BelowShow { viewer, title } => {
                if let Some(b) = board(&viewer) {
                    b.add_objective(
                        BELOW_OBJECTIVE,
                        component(&title),
                        RenderType::Integer,
                        None,
                    );
                    b.set_display_slot(DisplaySlot::BelowName, BELOW_OBJECTIVE);
                }
            }
            Action::BelowHide { viewer } => {
                if let Some(b) = board(&viewer) {
                    b.remove_objective(BELOW_OBJECTIVE);
                }
            }
            Action::Score {
                viewer,
                objective,
                entity,
                value,
                shown,
            } => {
                if let Some(b) = board(&viewer) {
                    b.update_score(
                        &entity,
                        objective,
                        value,
                        shown.map(|text| NumberFormat::Fixed(component(&text))),
                    );
                }
            }
            Action::ForgetScores { viewer, entity } => {
                if let Some(b) = board(&viewer) {
                    b.reset_entity_scores(&entity);
                }
            }
            Action::BedrockSidebar {
                viewer,
                title,
                old,
                new,
                existed,
            } => {
                let Some(b) = handles.get(&viewer).and_then(|h| h.bedrock.as_ref()) else {
                    continue;
                };
                if existed {
                    b.update_objective(OBJECTIVE, &title, BedrockSortOrder::Descending);
                    for (index, line) in old.iter().enumerate() {
                        b.remove_score(&bedrock_entry(index, line), OBJECTIVE);
                    }
                } else {
                    b.add_objective(OBJECTIVE, &title, BedrockSortOrder::Descending);
                    b.set_display_slot(BedrockDisplaySlot::Sidebar, OBJECTIVE);
                }
                for (index, line) in new.iter().enumerate() {
                    b.update_score(&bedrock_entry(index, line), OBJECTIVE, line_score(index));
                }
            }
            Action::BedrockSidebarHide { viewer } => {
                if let Some(b) = handles.get(&viewer).and_then(|h| h.bedrock.as_ref()) {
                    b.remove_objective(OBJECTIVE);
                }
            }
        }
    }
}

fn observer() -> HandlerOptions {
    // Observers only: the event never waits for this plugin, and a failure changes nothing.
    HandlerOptions {
        priority: HandlerPriority::Monitor,
        blocking: false,
        ignore_cancelled: true,
        on_failure: FailureVerdict::Allow,
    }
}

struct OnJoin(Arc<Shared>);
impl EventHandler<PlayerJoinEvent> for OnJoin {
    fn handle(
        &self,
        _server: Server,
        event: EventData<PlayerJoinEvent>,
    ) -> EventData<PlayerJoinEvent> {
        self.0
            .note(Note::Join(event.player.get_id().to_string(), JOIN_TRIES));
        event
    }
}

struct OnLeave(Arc<Shared>);
impl EventHandler<PlayerLeaveEvent> for OnLeave {
    fn handle(
        &self,
        _server: Server,
        event: EventData<PlayerLeaveEvent>,
    ) -> EventData<PlayerLeaveEvent> {
        let uuid = event.player.get_id().to_string();
        // Here and not in the task, so a reason set by the next join is not lost.
        lock(&self.0.views).forget(&uuid);
        self.0.note(Note::Leave(uuid));
        event
    }
}

struct OnWorld(Arc<Shared>);
impl EventHandler<PlayerChangedWorldEvent> for OnWorld {
    fn handle(
        &self,
        _server: Server,
        event: EventData<PlayerChangedWorldEvent>,
    ) -> EventData<PlayerChangedWorldEvent> {
        self.0.note(Note::World(
            event.player.get_id().to_string(),
            event.to_world.get_name(),
        ));
        event
    }
}

struct OnLocale(Arc<Shared>);
impl EventHandler<PlayerLocaleChangeEvent> for OnLocale {
    fn handle(
        &self,
        _server: Server,
        event: EventData<PlayerLocaleChangeEvent>,
    ) -> EventData<PlayerLocaleChangeEvent> {
        self.0.note(Note::Locale(
            event.player.get_id().to_string(),
            event.new_locale.clone(),
        ));
        event
    }
}

fn sender_lang(sender: &CommandSender) -> usize {
    lang::index(&format!("{:?}", sender.get_locale()))
}

fn say(sender: &CommandSender, text: &str) {
    sender.send_message(TextComponent::from_legacy_string_with_code(text, '&'));
}

struct StatusCommand(Arc<Shared>);
impl CommandHandler for StatusCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let messages = lock(&self.0.messages).clone();
        let status = *lock(&self.0.status);
        let key = if status.perms {
            "status"
        } else {
            "status.no_perms"
        };
        say(
            &sender,
            &messages.text(
                &sender,
                key,
                &[
                    env!("CARGO_PKG_VERSION"),
                    &status.players.to_string(),
                    &status.layouts.to_string(),
                ],
            ),
        );
        say(&sender, &messages.text(&sender, "usage", &[]));
        Ok(1)
    }
}

struct ReloadCommand(Arc<Shared>);
impl CommandHandler for ReloadCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let lang = sender_lang(&sender);
        match read_config(lang) {
            Ok((config, owner)) => {
                let layouts = config.layouts.len();
                *lock(&self.0.messages) = Messages {
                    owner: Arc::new(owner),
                    language: config.language,
                };
                lock(&self.0.views).set_defaults(
                    config.sidebar_default,
                    config.below_name.as_ref().is_some_and(|b| b.default_on),
                );
                self.0.note(Note::Config(Box::new(config)));
                let messages = lock(&self.0.messages).clone();
                say(
                    &sender,
                    &messages.text(&sender, "reloaded", &[&layouts.to_string()]),
                );
                Ok(1)
            }
            Err(message) => {
                say(&sender, &message);
                say(&sender, &lang::text(lang, "kept", &[]));
                Ok(0)
            }
        }
    }
}

/// The words of the player's sidebar command, for its messages.
#[derive(Clone)]
struct SidebarWords {
    label: String,
    on: String,
    off: String,
}

struct SidebarStatus(Arc<Shared>, SidebarWords);
impl CommandHandler for SidebarStatus {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let messages = lock(&self.0.messages).clone();
        let SidebarWords { label, on, off } = &self.1;
        if let Some(player) = sender.as_player() {
            let uuid = player.get_id().to_string();
            let state = lock(&self.0.views).sidebar_setting(&uuid);
            let word = if state { on } else { off };
            say(&sender, &messages.text(&sender, "sidebar.state", &[word]));
        }
        say(
            &sender,
            &messages.text(&sender, "sidebar.usage", &[label, on, off]),
        );
        Ok(1)
    }
}

struct SidebarSet(Arc<Shared>, bool);
impl CommandHandler for SidebarSet {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let messages = lock(&self.0.messages).clone();
        let Some(player) = sender.as_player() else {
            return Ok(0);
        };
        let uuid = player.get_id().to_string();
        lock(&self.0.views).set_sidebar(&uuid, self.1);
        self.0.note(Note::View(uuid));
        let key = if self.1 { "sidebar.on" } else { "sidebar.off" };
        say(&sender, &messages.text(&sender, key, &[]));
        Ok(1)
    }
}

/// Answers one message from another plugin.
fn answer(shared: &Shared, from: &str, request: Request) -> std::result::Result<String, String> {
    let on_off = |on: bool| if on { "on" } else { "off" };
    match request {
        Request::Values(ops) => {
            if !lock(&shared.key_limit).admit(from, &ops) {
                shared.warn_once(
                    from,
                    &format!(
                        "tab-and-scoreboard: refused values from {from}: more than {} keys, or \
                         more than {} plugins.",
                        placeholders::MAX_KEYS_PER_PLUGIN,
                        placeholders::MAX_PLUGINS
                    ),
                );
                return Err("too many keys or plugins".to_owned());
            }
            if !ops.is_empty() {
                shared.note(Note::Values(from.to_owned(), ops));
            }
            Ok(format!("{}\tok", contracts::VALUES))
        }
        Request::Alive => Ok(format!("{}\tok", contracts::VALUES)),
        Request::Get(Flag::Sidebar, uuid) => Ok(format!(
            "sidebar/1\t{}",
            on_off(lock(&shared.views).sidebar_setting(&uuid))
        )),
        Request::Get(Flag::BelowName, uuid) => Ok(format!(
            "below-name/1\t{}",
            on_off(lock(&shared.views).below_setting(&uuid))
        )),
        Request::Set(Flag::Sidebar, uuid, on) => {
            lock(&shared.views).set_sidebar(&uuid, on);
            shared.note(Note::View(uuid));
            Ok(format!("sidebar/1\t{}", on_off(on)))
        }
        Request::Set(Flag::BelowName, uuid, on) => {
            lock(&shared.views).set_below(&uuid, on);
            shared.note(Note::View(uuid));
            Ok(format!("below-name/1\t{}", on_off(on)))
        }
        Request::Suppress { uuid, reason, on } => {
            let taken = lock(&shared.views).suppress(&uuid, from, &reason, on);
            if !taken {
                return Err("too many reasons".to_owned());
            }
            let setting = lock(&shared.views).sidebar_setting(&uuid);
            shared.note(Note::View(uuid));
            Ok(format!("sidebar/1\t{}", on_off(setting)))
        }
        Request::TeamSet {
            uuid,
            reason,
            collision,
            nametag,
        } => {
            if !lock(&shared.views).team_set(&uuid, from, &reason, collision, nametag) {
                return Err("too many reasons".to_owned());
            }
            shared.note(Note::View(uuid));
            Ok("teams/1\tok".to_owned())
        }
        Request::TeamClear { uuid, reason } => {
            lock(&shared.views).team_clear(&uuid, from, &reason);
            shared.note(Note::View(uuid));
            Ok("teams/1\tok".to_owned())
        }
    }
}

pub struct TabAndScoreboard {
    shared: Arc<Shared>,
}

impl TabAndScoreboard {
    fn load_views(&self) {
        match std::fs::metadata(VIEWS_FILE) {
            Ok(meta) if meta.len() > views::MAX_FILE_BYTES as u64 => {
                log(
                    LogLevel::Warn,
                    &format!("tab-and-scoreboard: {VIEWS_FILE} is too large and was not read."),
                );
                return;
            }
            Ok(_) => {}
            Err(_) => return,
        }
        let Ok(text) = std::fs::read_to_string(VIEWS_FILE) else {
            return;
        };
        let skipped = lock(&self.shared.views).load(&text);
        if skipped > 0 {
            log(
                LogLevel::Warn,
                &format!("tab-and-scoreboard: skipped {skipped} bad lines in {VIEWS_FILE}."),
            );
        }
    }

    fn register_sidebar_command(&self, context: &Context, config: &Config) -> Result<()> {
        let Some(label) = config.sidebar_command.clone() else {
            return Ok(());
        };
        if !config.sidebar {
            return Ok(());
        }
        context.register_permission(&Permission {
            node: SIDEBAR_NODE.into(),
            description: "Turn your own sidebar on or off (Tab and Scoreboard).".into(),
            default: PermissionDefault::Allow,
            children: vec![],
        })?;
        let words = SidebarWords {
            label: label.clone(),
            on: config.sidebar_on_words[0].clone(),
            off: config.sidebar_off_words[0].clone(),
        };
        let mut command = Command::new(&[label], "Turn your sidebar on or off.")
            .execute(SidebarStatus(Arc::clone(&self.shared), words));
        for (list, on) in [
            (&config.sidebar_on_words, true),
            (&config.sidebar_off_words, false),
        ] {
            for word in list {
                command = command.then(
                    CommandNode::literal(word).execute(SidebarSet(Arc::clone(&self.shared), on)),
                );
            }
        }
        context.register_command(command, SIDEBAR_NODE);
        Ok(())
    }
}

impl Plugin for TabAndScoreboard {
    fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                inbox: Mutex::new(Inbox::default()),
                runtime: Mutex::new(None),
                status: Mutex::new(Status::default()),
                views: Mutex::new(Views::new(true, false)),
                key_limit: Mutex::new(KeyLimit::default()),
                warned: Mutex::new(HashSet::new()),
                messages: Mutex::new(Messages::default()),
            }),
        }
    }

    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "tab-and-scoreboard".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["Strawberry Interactive LLC".into()],
            description: "Tab list, nametags and sidebar with ranks, images and placeholders."
                .into(),
            dependencies: vec![],
            permissions: vec![
                permissions::FS_READ_DATA.into(),
                permissions::FS_WRITE_DATA.into(),
            ],
        }
    }

    fn on_load(&self, context: Context) -> Result<()> {
        write_default_config();
        let (config, owner) = read_config(0).unwrap_or_else(|message| {
            log(
                LogLevel::Error,
                &format!("tab-and-scoreboard: {message} The built-in settings are used."),
            );
            (Config::builtin(), OwnerLang::default())
        });
        if config.icons_mode == config::IconsMode::PackRequired {
            log(
                LogLevel::Info,
                "tab-and-scoreboard: icons = pack-required: glyph images go to every Java player. \
                 The server resource pack with their fonts must be set to required.",
            );
        }
        lock(&self.shared.views).set_defaults(
            config.sidebar_default,
            config.below_name.as_ref().is_some_and(|b| b.default_on),
        );
        self.load_views();
        *lock(&self.shared.messages) = Messages {
            owner: Arc::new(owner),
            language: config.language,
        };
        self.register_sidebar_command(&context, &config)?;

        let refresh = u64::from(config.refresh_seconds) * 20;
        *lock(&self.shared.runtime) = Some(Runtime {
            planner: Planner::new(Arc::new(config)),
            handles: HashMap::new(),
            cycle: 0,
            rev: None,
            perms: false,
            warned_teams: false,
            warned_notes: false,
        });

        context.register_permission(&Permission {
            node: ADMIN_NODE.into(),
            description: "Use /tab and /tab reload (Tab and Scoreboard).".into(),
            default: PermissionDefault::Op(PermissionLevel::Three),
            children: vec![],
        })?;
        let command = Command::new(
            &["tab".to_string()],
            "Tab and Scoreboard: status and reload.",
        )
        .execute(StatusCommand(Arc::clone(&self.shared)))
        .then(CommandNode::literal("reload").execute(ReloadCommand(Arc::clone(&self.shared))));
        context.register_command(command, ADMIN_NODE);

        context.register_event_handler_with_options::<PlayerJoinEvent, _>(
            OnJoin(Arc::clone(&self.shared)),
            observer(),
        )?;
        context.register_event_handler_with_options::<PlayerLeaveEvent, _>(
            OnLeave(Arc::clone(&self.shared)),
            observer(),
        )?;
        context.register_event_handler_with_options::<PlayerChangedWorldEvent, _>(
            OnWorld(Arc::clone(&self.shared)),
            observer(),
        )?;
        context.register_event_handler_with_options::<PlayerLocaleChangeEvent, _>(
            OnLocale(Arc::clone(&self.shared)),
            observer(),
        )?;

        // The period is fixed at load: a reload with another refresh-seconds takes effect at the
        // next server start (the task's own cycle count follows the new value at once).
        let shared = Arc::clone(&self.shared);
        let _task = context.schedule_repeating_async_task(1, refresh, move |server: Server| {
            run_cycle(&shared, &server);
        });
        Ok(())
    }

    fn on_unload(&self, _context: Context) -> Result<()> {
        if let Some(rt) = lock(&self.shared.runtime).take() {
            apply(&rt.handles, rt.planner.cleanup());
        }
        self.shared.save_views();
        Ok(())
    }

    /// Other plugins: perms-lite tells of rank changes (contract `ranks/1`), any plugin may use
    /// the contracts in [`contracts`]. Nothing here calls the host: changes are noted for the
    /// next cycle, and questions are answered from [`views`].
    fn handle_ipc_message(
        &self,
        from: wit::PluginId,
        message: wit::IpcMessage,
    ) -> std::result::Result<wit::IpcMessage, String> {
        if message.starts_with(ranks::VERSION.as_bytes()) {
            if from != ranks::PERMS_PLUGIN {
                return Err("ranks/1 is taken only from perms-lite".to_owned());
            }
            let changed = ranks::parse_changed(&message).ok_or("not a ranks/1 change")?;
            self.shared.note(Note::Ranks(changed.rev, changed.ranks));
            return Ok(Vec::new());
        }
        let request = contracts::parse(&message)?;
        answer(&self.shared, &from, request).map(String::into_bytes)
    }
}

strawberry_plugin_api::register_plugin!(TabAndScoreboard);
