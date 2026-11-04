use super::*;
use crate::config::parse;
use crate::render::Segment;

const A: &str = "00000000-0000-0000-0000-00000000000a";
const B: &str = "00000000-0000-0000-0000-00000000000b";
const C: &str = "00000000-0000-0000-0000-00000000000c";

fn facts(name: &str, edition: Edition) -> Facts {
    Facts {
        name: name.into(),
        world: "world".into(),
        ping: 0,
        rank: Rank::default(),
        lang: 0,
        edition,
    }
}

fn view() -> View {
    View {
        sidebar: true,
        below: true,
        collision: Collision::Always,
        nametag: Nametag::Always,
    }
}

fn join(p: &mut Planner, uuid: &str, f: Facts) {
    p.join(uuid, f, Face::default(), view());
}

fn vip() -> Rank {
    Rank {
        group: "vip".into(),
        weight: 50,
        prefix: "VIP".into(),
        suffix: String::new(),
    }
}

fn planner(text: &str) -> Planner {
    Planner::new(Arc::new(parse(text).expect("parses")))
}

fn builtin() -> Planner {
    Planner::new(Arc::new(Config::builtin()))
}

fn text(r: &Rendered) -> String {
    r.0.iter()
        .map(|s| match s {
            Segment::Text(t) | Segment::Literal(t) => t.clone(),
            Segment::Glyph { text, .. } => format!("<glyph {text}>"),
            Segment::Head { name, .. } => format!("<head {name}>"),
        })
        .collect()
}

fn count(actions: &[Action], pred: impl Fn(&Action) -> bool) -> usize {
    actions.iter().filter(|a| pred(a)).count()
}

fn team_of(actions: &[Action], viewer: &str, member: &str) -> Option<String> {
    actions.iter().find_map(|a| match a {
        Action::TeamJoin {
            viewer: v,
            team,
            member: m,
        } if v == viewer && m == member => Some(team.clone()),
        _ => None,
    })
}

fn set(p: &mut Planner, plugin: &str, uuid: &str, key: &str, value: &str) {
    p.apply_values(
        plugin,
        vec![Op::Set {
            target: crate::placeholders::Target::Player(uuid.into()),
            key: key.into(),
            value: Provided::Text(value.into()),
        }],
    );
}

#[test]
fn a_join_sets_everything_up_once_and_an_idle_cycle_sends_nothing() {
    let mut p = builtin();
    p.set_max_players(20);
    join(&mut p, A, facts("Alex", Edition::Java));
    let first = p.plan();
    assert!(first.contains(&Action::TabName {
        subject: A.into(),
        name: Rendered(vec![Segment::Text("Alex".into())])
    }));
    assert!(first.contains(&Action::ClearBoard { viewer: A.into() }));
    assert!(team_of(&first, A, "Alex").is_some());
    assert_eq!(
        count(&first, |a| matches!(a, Action::HeaderFooter { .. })),
        1
    );
    assert_eq!(
        count(&first, |a| matches!(a, Action::SidebarShow { .. })),
        1
    );
    assert_eq!(count(&first, |a| matches!(a, Action::LineAdd { .. })), 4);
    // Nothing changed: nothing goes out, however many cycles pass.
    assert!(p.plan().is_empty());
    p.set_tps(20.0);
    p.set_ping(A, 0);
    assert!(p.plan().is_empty());
}

#[test]
fn a_new_ping_changes_only_that_players_own_texts() {
    let mut p = builtin();
    join(&mut p, A, facts("Alex", Edition::Java));
    join(&mut p, B, facts("Bo", Edition::Java));
    let _ = p.plan();
    p.set_ping(A, 42);
    let actions = p.plan();
    assert!(!actions.is_empty());
    for action in &actions {
        match action {
            Action::HeaderFooter { viewer, footer, .. } => {
                assert_eq!(viewer, A);
                assert!(text(footer).contains("42 ms"));
            }
            Action::LineSet {
                viewer,
                index,
                text: line,
            } => {
                assert_eq!((viewer.as_str(), *index), (A, 3));
                assert!(text(line).contains("42"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[test]
fn a_second_join_adds_the_player_to_the_first_viewers_teams_and_updates_online() {
    let mut p = builtin();
    join(&mut p, A, facts("Alex", Edition::Java));
    let first = p.plan();
    let team = team_of(&first, A, "Alex").expect("joined");
    join(&mut p, B, facts("Bo", Edition::Java));
    let actions = p.plan();
    assert!(actions.contains(&Action::TeamJoin {
        viewer: A.into(),
        team: team.clone(),
        member: "Bo".into()
    }));
    // The same look: no second team on Alex's board.
    assert_eq!(
        count(
            &actions,
            |a| matches!(a, Action::TeamCreate { viewer, .. } if viewer == A)
        ),
        0
    );
    // Bo's board gets both members.
    assert_eq!(
        count(
            &actions,
            |a| matches!(a, Action::TeamJoin { viewer, .. } if viewer == B)
        ),
        2
    );
    // Alex's {online} line changed.
    assert!(actions.iter().any(|a| matches!(a,
        Action::LineSet { viewer, index: 2, text: line } if viewer == A && text(line).contains('2'))));
    // Alex's name did not change.
    assert_eq!(
        count(
            &actions,
            |a| matches!(a, Action::TabName { subject, .. } if subject == A)
        ),
        0
    );
}

#[test]
fn a_rank_change_moves_the_player_to_the_new_team_everywhere_and_sorts_it() {
    let mut p = builtin();
    join(&mut p, A, facts("Alex", Edition::Java));
    join(&mut p, B, facts("Bo", Edition::Java));
    let first = p.plan();
    let old_team = team_of(&first, A, "Bo").expect("joined");
    p.set_rank(B, vip());
    let actions = p.plan();
    let new_team = team_of(&actions, A, "Bo").expect("moved");
    assert_ne!(old_team, new_team);
    assert!(actions.contains(&Action::TabName {
        subject: B.into(),
        name: Rendered(vec![Segment::Text("&6[VIP]&r Bo".into())])
    }));
    assert!(actions.contains(&Action::TabOrder {
        subject: B.into(),
        order: 50
    }));
    for viewer in [A, B] {
        assert!(actions.iter().any(|a| matches!(a,
            Action::TeamCreate { viewer: v, team, style } if v == viewer && *team == new_team
                && text(&style.prefix) == "&6[VIP]&r ")));
        assert!(actions.contains(&Action::TeamLeave {
            viewer: viewer.into(),
            team: old_team.clone(),
            member: "Bo".into()
        }));
    }
    // Bo's "Rank" line shows the group.
    assert!(actions.iter().any(|a| matches!(a,
        Action::LineSet { viewer, index: 0, text: line } if viewer == B && text(line).contains("vip"))));
}

#[test]
fn an_empty_team_is_removed_after_its_last_member_moved_or_left() {
    let mut p = builtin();
    join(&mut p, A, facts("Alex", Edition::Java));
    join(&mut p, B, facts("Bo", Edition::Java));
    p.set_rank(B, vip());
    let first = p.plan();
    let vip_team = team_of(&first, A, "Bo").expect("joined");
    let default_team = team_of(&first, A, "Alex").expect("joined");
    // Alex moves to Bo's look: Alex's team is empty.
    p.set_rank(A, vip());
    let moved = p.plan();
    assert!(moved.contains(&Action::TeamRemove {
        viewer: B.into(),
        team: default_team,
    }));
    assert_eq!(team_of(&moved, B, "Alex").as_ref(), Some(&vip_team));
    // Both leave one after the other; the last one empties it, on nobody's board any more.
    p.leave(A);
    let left = p.plan();
    assert!(left.contains(&Action::TeamLeave {
        viewer: B.into(),
        team: vip_team.clone(),
        member: "Alex".into(),
    }));
    assert!(!left.iter().any(|a| matches!(a, Action::TeamRemove { .. })));
    p.leave(B);
    assert!(p.plan().is_empty());
    assert!(p.teams.is_empty());
}

#[test]
fn higher_weights_sort_first_by_team_name() {
    assert!(team_name(100, 0) < team_name(50, 0));
    assert!(team_name(50, u32::MAX) < team_name(0, 0));
    assert_eq!(team_name(50, u32::MAX).len(), 16);
    assert_eq!(team_name(99_999, 0), team_name(9999, 0));
}

#[test]
fn a_leave_takes_the_player_out_of_the_other_boards() {
    let mut p = builtin();
    join(&mut p, A, facts("Alex", Edition::Java));
    join(&mut p, B, facts("Bo", Edition::Java));
    let first = p.plan();
    let team = team_of(&first, A, "Bo").expect("joined");
    p.leave(B);
    let actions = p.plan();
    assert!(actions.contains(&Action::TeamLeave {
        viewer: A.into(),
        team,
        member: "Bo".into()
    }));
    assert!(!actions.iter().any(|a| format!("{a:?}").contains(B)));
}

#[test]
fn values_nobody_shows_are_not_read_and_change_nothing() {
    let mut p = planner("[layout a]\nheader = {player}\n");
    assert!(!p.needs(Key::Tps));
    assert!(!p.needs(Key::Ping));
    assert!(p.ping_due(0, 5).is_empty());
    join(&mut p, A, facts("Alex", Edition::Java));
    let _ = p.plan();
    p.set_tps(12.0);
    p.set_ping(A, 99);
    set(&mut p, "other", A, "unused", "1");
    assert!(p.plan().is_empty());
}

#[test]
fn pings_are_read_in_slices() {
    let mut p = builtin();
    for (uuid, name) in [(A, "Alex"), (B, "Bo"), (C, "Cy")] {
        join(&mut p, uuid, facts(name, Edition::Java));
    }
    let mut seen: Vec<String> = (0..3).flat_map(|cycle| p.ping_due(cycle, 3)).collect();
    seen.sort();
    assert_eq!(seen, [A, B, C]);
    assert_eq!(p.ping_due(0, 3).len(), 1);
}

#[test]
fn layouts_follow_world_and_group() {
    let mut p = planner(
        "[layout nether]\nworlds = world_nether\nline = hot\n\
         [layout rest]\nheader = hello\n",
    );
    join(&mut p, A, facts("Alex", Edition::Java));
    let first = p.plan();
    assert_eq!(
        count(&first, |a| matches!(a, Action::SidebarShow { .. })),
        0
    );
    p.set_world(A, "world_nether");
    let actions = p.plan();
    assert!(actions.contains(&Action::HeaderFooter {
        viewer: A.into(),
        header: Rendered::default(),
        footer: Rendered::default()
    }));
    assert_eq!(count(&actions, |a| matches!(a, Action::LineAdd { .. })), 1);
    p.set_world(A, "world");
    assert!(p.plan().contains(&Action::SidebarHide {
        viewer: A.into(),
        lines: 1
    }));
}

#[test]
fn images_follow_the_pack_rule_and_bedrock_gets_text() {
    let icons = "[icon vip]\nglyph = U+E100\nfont = myserver:ranks\nfallback = &6[VIP]\n\
                 [layout a]\nline = {icon}{player}\n";
    let mut text_mode = planner(&format!("[settings]\nicons = text\n{icons}"));
    let mut pack = planner(&format!("[settings]\nicons = pack-required\n{icons}"));
    for p in [&mut text_mode, &mut pack] {
        join(p, A, facts("Alex", Edition::Java));
        join(p, B, facts("Bo", Edition::Bedrock));
        p.set_rank(A, vip());
        p.set_rank(B, vip());
    }
    let line_of = |actions: &[Action]| {
        actions.iter().find_map(|a| match a {
            Action::LineAdd {
                viewer, text: line, ..
            } if viewer == A => Some(text(line)),
            _ => None,
        })
    };
    assert_eq!(line_of(&text_mode.plan()).as_deref(), Some("&6[VIP]Alex"));
    let actions = pack.plan();
    assert_eq!(line_of(&actions).as_deref(), Some("<glyph \u{E100}>Alex"));
    // The Bedrock player gets text, no header (Java only) and no teams.
    assert!(actions.iter().any(|a| matches!(a,
        Action::BedrockSidebar { viewer, new, .. } if viewer == B && new == &["§6[VIP]Bo".to_owned()])));
    assert!(!actions.iter().any(|a| matches!(a,
        Action::HeaderFooter { viewer, .. } | Action::ClearBoard { viewer } | Action::TeamCreate { viewer, .. }
            if viewer == B)));
}

#[test]
fn new_settings_make_a_new_team_and_new_texts() {
    let mut p = builtin();
    join(&mut p, A, facts("Alex", Edition::Java));
    p.set_rank(A, vip());
    let first = p.plan();
    let old = team_of(&first, A, "Alex").expect("joined");
    p.set_config(Arc::new(
        parse("[settings]\nnametag-prefix = \"&c{prefix} \"\n[layout a]\nheader = new\n")
            .expect("parses"),
    ));
    let actions = p.plan();
    assert!(actions.iter().any(|a| matches!(a,
        Action::TeamCreate { viewer, style, .. } if viewer == A && text(&style.prefix) == "&cVIP ")));
    assert!(actions.contains(&Action::TeamRemove {
        viewer: A.into(),
        team: old,
    }));
    assert!(actions.iter().any(|a| matches!(a,
        Action::HeaderFooter { header, .. } if text(header) == "new")));
    assert!(actions.contains(&Action::SidebarHide {
        viewer: A.into(),
        lines: 4
    }));
}

#[test]
fn a_new_language_rewrites_the_viewers_texts() {
    let mut p = builtin();
    join(&mut p, A, facts("Alex", Edition::Java));
    let _ = p.plan();
    p.set_lang(A, 3);
    let actions = p.plan();
    assert!(actions.iter().any(|a| matches!(a,
        Action::HeaderFooter { header, .. } if text(header).contains("Hoş geldin"))));
    assert!(!actions.iter().any(|a| matches!(a, Action::TabName { .. })));
}

#[test]
fn a_fixed_language_is_every_viewers_language() {
    let mut p = planner("[settings]\nlanguage = tr\n[layout a]\nheader = @default.header\n");
    join(&mut p, A, facts("Alex", Edition::Java));
    let actions = p.plan();
    assert!(actions.iter().any(|a| matches!(a,
        Action::HeaderFooter { header, .. } if text(header).contains("Hoş geldin"))));
    p.set_lang(A, 1);
    assert!(p.plan().is_empty(), "the client's language does not matter");
}

#[test]
fn cleanup_takes_back_what_was_sent() {
    let mut p = builtin();
    join(&mut p, A, facts("Alex", Edition::Java));
    p.set_rank(A, vip());
    let first = p.plan();
    let team = team_of(&first, A, "Alex").expect("joined");
    let actions = p.cleanup();
    assert!(actions.contains(&Action::TabNameReset { subject: A.into() }));
    assert!(actions.contains(&Action::TabOrder {
        subject: A.into(),
        order: 0
    }));
    assert!(actions.contains(&Action::SidebarHide {
        viewer: A.into(),
        lines: 4
    }));
    assert!(actions.contains(&Action::TeamRemove {
        viewer: A.into(),
        team,
    }));
}

#[test]
fn line_entries_are_distinct_and_invisible() {
    let entries: Vec<String> = (0..crate::config::MAX_SIDEBAR_LINES)
        .map(|i| line_team(i).1)
        .collect();
    let mut unique = entries.clone();
    unique.dedup();
    assert_eq!(unique.len(), entries.len());
    assert!(
        entries
            .iter()
            .all(|e| e.starts_with('§') && e.chars().count() == 2)
    );
    assert!(line_score(0) > line_score(14));
    assert!(line_team(14).0.len() <= 16);
}

// Values from other plugins.

#[test]
fn a_pushed_value_rerenders_only_the_lines_that_show_it() {
    let mut p = planner(
        "[settings]\nmissing = &8none\n\
         [layout a]\nline = Plain\nline = Tax: {core:tax}\nline = Hi {player}\n",
    );
    join(&mut p, A, facts("Alex", Edition::Java));
    join(&mut p, B, facts("Bo", Edition::Java));
    let first = p.plan();
    assert!(first.iter().any(|a| matches!(a,
        Action::LineAdd { index: 1, text: line, .. } if text(line) == "Tax: &8none")));
    set(&mut p, "core", A, "tax", "&c5%");
    let actions = p.plan();
    assert_eq!(actions.len(), 1, "{actions:?}");
    let Action::LineSet {
        viewer,
        index,
        text: line,
    } = &actions[0]
    else {
        panic!("{actions:?}");
    };
    assert_eq!((viewer.as_str(), *index), (A, 1));
    assert_eq!(
        line.0,
        vec![
            Segment::Text("Tax: ".into()),
            Segment::Literal("&c5%".into()),
        ],
        "the value stays literal"
    );
    // The same value again: nothing.
    set(&mut p, "core", A, "tax", "&c5%");
    assert!(p.plan().is_empty());
}

#[test]
fn a_value_for_everyone_reaches_every_viewer_and_a_gone_plugin_blanks_it() {
    let mut p = planner("[layout a]\nline = {core:motd}|\n");
    join(&mut p, A, facts("Alex", Edition::Java));
    join(&mut p, B, facts("Bo", Edition::Java));
    let _ = p.plan();
    p.apply_values(
        "core",
        vec![Op::Set {
            target: crate::placeholders::Target::Everyone,
            key: "motd".into(),
            value: Provided::Text("hi".into()),
        }],
    );
    let actions = p.plan();
    assert_eq!(
        count(&actions, |a| matches!(a, Action::LineSet { .. })),
        2,
        "{actions:?}"
    );
    assert_eq!(p.providers(), HashSet::from(["core".to_owned()]));
    p.forget_plugin("core");
    let actions = p.plan();
    assert!(actions.iter().all(|a| matches!(a,
        Action::LineSet { text: line, .. } if text(line) == "|")));
    assert_eq!(actions.len(), 2);
    assert!(p.providers().is_empty());
}

#[test]
fn a_players_values_go_with_the_player() {
    let mut p = planner("[layout a]\nline = {core:tax}\n");
    join(&mut p, A, facts("Alex", Edition::Java));
    set(&mut p, "core", A, "tax", "1");
    let _ = p.plan();
    p.leave(A);
    let _ = p.plan();
    join(&mut p, A, facts("Alex", Edition::Java));
    let actions = p.plan();
    assert!(actions.iter().any(|a| matches!(a,
        Action::LineAdd { text: line, .. } if text(line).is_empty())));
}

#[test]
fn values_pushed_before_the_join_are_there_at_the_join() {
    let mut p = planner("[layout a]\nline = {core:tax}\n");
    set(&mut p, "core", A, "tax", "7");
    join(&mut p, A, facts("Alex", Edition::Java));
    let actions = p.plan();
    assert!(actions.iter().any(|a| matches!(a,
        Action::LineAdd { text: line, .. } if text(line) == "7")));
}

// Per-player teams.

#[test]
fn a_pushed_value_in_a_nametag_gives_one_team_per_look() {
    let mut p = planner("[settings]\nnametag-prefix = {core:rank}\n");
    join(&mut p, A, facts("Alex", Edition::Java));
    join(&mut p, B, facts("Bo", Edition::Java));
    join(&mut p, C, facts("Cy", Edition::Java));
    set(&mut p, "core", A, "rank", "x");
    set(&mut p, "core", B, "rank", "x");
    set(&mut p, "core", C, "rank", "y");
    let first = p.plan();
    let a = team_of(&first, A, "Alex").expect("joined");
    assert_eq!(team_of(&first, A, "Bo").as_ref(), Some(&a), "the same look");
    assert_ne!(team_of(&first, A, "Cy").as_ref(), Some(&a));
    assert_eq!(p.teams.len(), 2);
    // Cy takes the shared look: Cy's old team goes.
    set(&mut p, "core", C, "rank", "x");
    let actions = p.plan();
    assert_eq!(team_of(&actions, B, "Cy").as_ref(), Some(&a));
    assert_eq!(
        count(&actions, |x| matches!(x, Action::TeamRemove { .. })),
        3
    );
    assert_eq!(p.teams.len(), 1);
}

#[test]
fn looks_are_bounded_and_the_players_over_it_get_the_groups_plain_look() {
    let mut p = planner("[settings]\nnametag-prefix = {core:rank}\nnametag-suffix = \"!\"\n");
    let uuids: Vec<String> = (0..=MAX_TEAMS)
        .map(|i| format!("00000000-0000-0000-0000-{i:012}"))
        .collect();
    for (i, uuid) in uuids.iter().enumerate() {
        join(&mut p, uuid, facts(&format!("P{i}"), Edition::Java));
        set(&mut p, "core", uuid, "rank", &format!("look{i}"));
    }
    let actions = p.plan();
    assert!(p.take_team_overflow());
    assert!(!p.take_team_overflow(), "told once");
    assert_eq!(p.teams.len(), MAX_TEAMS + 1, "one plain look on top");
    let viewer = &uuids[0];
    let last = format!("P{MAX_TEAMS}");
    let team = team_of(&actions, viewer, &last).expect("placed");
    assert!(actions.iter().any(|a| matches!(a,
        Action::TeamCreate { team: t, style, .. } if *t == team && style.prefix.is_empty()
            && text(&style.suffix) == "!")));
}

#[test]
fn another_plugins_team_options_are_part_of_the_look() {
    let mut p = builtin();
    join(&mut p, A, facts("Alex", Edition::Java));
    join(&mut p, B, facts("Bo", Edition::Java));
    let first = p.plan();
    let shared = team_of(&first, A, "Alex").expect("joined");
    p.set_view(
        A,
        View {
            collision: Collision::Never,
            ..view()
        },
    );
    let actions = p.plan();
    let own = team_of(&actions, B, "Alex").expect("moved");
    assert_ne!(own, shared);
    assert!(actions.iter().any(|a| matches!(a,
        Action::TeamCreate { team, style, .. } if *team == own
            && style.collision == Collision::Never)));
    // Bo keeps the shared team, and nothing else changes for anybody.
    assert!(!actions.iter().any(|a| format!("{a:?}").contains("\"Bo\"")));
    p.set_view(A, view());
    let back = p.plan();
    assert_eq!(team_of(&back, B, "Alex").as_ref(), Some(&shared));
    assert!(back.contains(&Action::TeamRemove {
        viewer: B.into(),
        team: own,
    }));
}

// Scores.

fn score_of(actions: &[Action], viewer: &str, objective: &str, entity: &str) -> Option<i32> {
    actions.iter().find_map(|a| match a {
        Action::Score {
            viewer: v,
            objective: o,
            entity: e,
            value,
            ..
        } if v == viewer && *o == objective && e == entity => Some(*value),
        _ => None,
    })
}

#[test]
fn the_tab_score_reaches_every_viewer_only_when_it_changes() {
    let mut p = planner("[tab-score]\nvalue = {ping}\nformat = &7Ping: {value}\n");
    join(&mut p, A, facts("Alex", Edition::Java));
    let first = p.plan();
    assert!(first.contains(&Action::TabScoreShow { viewer: A.into() }));
    assert_eq!(score_of(&first, A, TAB_OBJECTIVE, "Alex"), Some(0));
    join(&mut p, B, facts("Bo", Edition::Java));
    let second = p.plan();
    assert_eq!(score_of(&second, B, TAB_OBJECTIVE, "Alex"), Some(0));
    assert_eq!(score_of(&second, B, TAB_OBJECTIVE, "Bo"), Some(0));
    assert_eq!(score_of(&second, A, TAB_OBJECTIVE, "Bo"), Some(0));
    assert_eq!(score_of(&second, A, TAB_OBJECTIVE, "Alex"), None);
    assert!(p.plan().is_empty());

    p.set_ping(A, 42);
    let actions = p.plan();
    assert_eq!(actions.len(), 2, "one score per viewer: {actions:?}");
    assert!(actions.contains(&Action::Score {
        viewer: B.into(),
        objective: TAB_OBJECTIVE,
        entity: "Alex".into(),
        value: 42,
        shown: Some(Rendered::text("&7Ping: 42")),
    }));
}

#[test]
fn a_small_change_is_not_sent_under_the_threshold() {
    let mut p = planner("[tab-score]\nvalue = {ping}\nmin-change = 10\n");
    join(&mut p, A, facts("Alex", Edition::Java));
    let _ = p.plan();
    p.set_ping(A, 9);
    assert!(p.plan().is_empty());
    p.set_ping(A, 10);
    assert_eq!(score_of(&p.plan(), A, TAB_OBJECTIVE, "Alex"), Some(10));
    p.set_ping(A, 19);
    assert!(p.plan().is_empty(), "measured from what was shown");
    p.set_ping(A, 0);
    assert_eq!(score_of(&p.plan(), A, TAB_OBJECTIVE, "Alex"), Some(0));
}

#[test]
fn below_name_follows_each_viewers_own_choice() {
    let mut p = planner("[below-name]\ntitle = \" cookies\"\nvalue = {core:cookies}\n");
    join(&mut p, A, facts("Alex", Edition::Java));
    p.join(
        B,
        facts("Bo", Edition::Java),
        Face::default(),
        View {
            below: false,
            ..view()
        },
    );
    set(&mut p, "core", A, "cookies", "5");
    let first = p.plan();
    assert!(first.contains(&Action::BelowShow {
        viewer: A.into(),
        title: Rendered::text(" cookies"),
    }));
    assert_eq!(score_of(&first, A, BELOW_OBJECTIVE, "Alex"), Some(5));
    assert_eq!(score_of(&first, B, BELOW_OBJECTIVE, "Alex"), None);

    set(&mut p, "core", A, "cookies", "6");
    let changed = p.plan();
    assert_eq!(
        changed.len(),
        1,
        "only the viewer who has it on: {changed:?}"
    );
    assert_eq!(score_of(&changed, A, BELOW_OBJECTIVE, "Alex"), Some(6));

    // Bo turns it on and gets every score; Alex turns it off.
    p.set_view(B, view());
    p.set_view(
        A,
        View {
            below: false,
            ..view()
        },
    );
    let toggled = p.plan();
    assert_eq!(score_of(&toggled, B, BELOW_OBJECTIVE, "Alex"), Some(6));
    assert!(toggled.contains(&Action::BelowHide { viewer: A.into() }));
}

#[test]
fn a_leaving_players_scores_go_from_the_other_boards() {
    let mut p = planner("[tab-score]\nvalue = {ping}\n");
    join(&mut p, A, facts("Alex", Edition::Java));
    join(&mut p, B, facts("Bo", Edition::Java));
    let _ = p.plan();
    p.leave(B);
    assert!(p.plan().contains(&Action::ForgetScores {
        viewer: A.into(),
        entity: "Bo".into(),
    }));
}

#[test]
fn scores_are_taken_down_when_the_settings_drop_them() {
    let mut p = planner("[tab-score]\nvalue = {ping}\n[below-name]\ntitle = x\nvalue = {ping}\n");
    join(&mut p, A, facts("Alex", Edition::Java));
    let _ = p.plan();
    p.set_config(Arc::new(parse("[settings]\nsort = on\n").expect("parses")));
    let actions = p.plan();
    assert!(actions.contains(&Action::TabScoreHide { viewer: A.into() }));
    assert!(actions.contains(&Action::BelowHide { viewer: A.into() }));
    assert!(
        p.cleanup()
            .iter()
            .all(|a| !matches!(a, Action::TabScoreHide { .. } | Action::BelowHide { .. }))
    );
}

#[test]
fn bedrock_viewers_get_no_scores() {
    let mut p = planner("[tab-score]\nvalue = {ping}\n");
    join(&mut p, B, facts("Bo", Edition::Bedrock));
    join(&mut p, A, facts("Alex", Edition::Java));
    let actions = p.plan();
    assert!(!actions.iter().any(|a| matches!(a,
        Action::TabScoreShow { viewer } | Action::Score { viewer, .. } if viewer == B)));
    // A Java viewer does see the Bedrock player's score.
    assert_eq!(score_of(&actions, A, TAB_OBJECTIVE, "Bo"), Some(0));
}

// The sidebar flag.

#[test]
fn a_hidden_sidebar_keeps_the_header_and_comes_back_whole() {
    let mut p = builtin();
    join(&mut p, A, facts("Alex", Edition::Java));
    let _ = p.plan();
    p.set_view(
        A,
        View {
            sidebar: false,
            ..view()
        },
    );
    let hidden = p.plan();
    assert_eq!(
        hidden,
        vec![Action::SidebarHide {
            viewer: A.into(),
            lines: 4,
        }]
    );
    // While hidden a new ping changes the footer only.
    p.set_ping(A, 5);
    let actions = p.plan();
    assert_eq!(actions.len(), 1);
    assert!(matches!(&actions[0], Action::HeaderFooter { .. }));
    p.set_view(A, view());
    let shown = p.plan();
    assert_eq!(
        count(&shown, |a| matches!(a, Action::SidebarShow { .. })),
        1
    );
    assert_eq!(count(&shown, |a| matches!(a, Action::LineAdd { .. })), 4);
}

// Order inside one weight.

fn ordered(config: &str, weight: i32, name: &str) -> i32 {
    let config = parse(config).expect("parses");
    let rank = Rank {
        weight,
        ..Rank::default()
    };
    list_order(&config, &rank, name)
}

#[test]
fn exact_case_order_sorts_as_plain_text_and_the_weight_comes_first() {
    let exact = "[settings]\nname-order = exact-case\n";
    // A higher order is shown first: so `Zed` before `alex`, as plain text compares.
    let names = ["1a", "Al", "Alex", "Zed", "_x", "alex", "zed"];
    for pair in names.windows(2) {
        assert!(
            ordered(exact, 0, pair[0]) > ordered(exact, 0, pair[1]),
            "{pair:?}"
        );
    }
    // Names alike further on tie, and the client orders those without case: the named limit.
    assert_eq!(ordered(exact, 0, "alexa"), ordered(exact, 0, "alexb"));
    assert!(ordered(exact, 1, "zed") > ordered(exact, 0, "Alex"));
    assert!(ordered(exact, 2047, "zzzz") > ordered(exact, 2046, "0000"));
    // Equal names give equal orders, so nothing moves without a change.
    assert_eq!(ordered(exact, 3, "Alex"), ordered(exact, 3, "Alex"));
    // Without the setting: the weight alone, as before.
    assert_eq!(ordered("[settings]\nsort = on\n", 7, "Alex"), 7);
    assert_eq!(ordered("[settings]\nsort = off\n", 7, "Alex"), 0);
    // A weight outside the range still sorts, and never overflows.
    assert!(ordered(exact, 100_000, "a") > ordered(exact, 2046, "a"));
    assert!(ordered(exact, -5, "a") >= 0);
}

#[test]
fn a_join_moves_nobody_else_with_exact_case_order() {
    let mut p = planner("[settings]\nname-order = exact-case\n");
    join(&mut p, A, facts("Bob", Edition::Java));
    let _ = p.plan();
    join(&mut p, B, facts("Alex", Edition::Java));
    let actions = p.plan();
    assert_eq!(
        count(
            &actions,
            |a| matches!(a, Action::TabOrder { subject, .. } if subject == A)
        ),
        0
    );
    assert_eq!(
        count(
            &actions,
            |a| matches!(a, Action::TabOrder { subject, .. } if subject == B)
        ),
        1
    );
}

// The player's own head.

#[test]
fn a_new_skin_changes_the_tab_name_only_when_a_text_shows_a_head() {
    let mut p = planner("[settings]\ntab-name = {head}{player}\n");
    assert!(p.needs(Key::Head));
    let face = |textures: &str| Face {
        id: A.into(),
        textures: Some(textures.into()),
        signature: None,
    };
    p.join(A, facts("Alex", Edition::Java), face("one"), view());
    let first = p.plan();
    assert!(first.iter().any(|a| matches!(a,
        Action::TabName { name, .. } if text(name) == "<head .>Alex")));
    p.set_face(A, face("one"));
    assert!(p.plan().is_empty());
    p.set_face(A, face("two"));
    assert_eq!(count(&p.plan(), |a| matches!(a, Action::TabName { .. })), 1);

    let plain = builtin();
    assert!(!plain.needs(Key::Head));
}

// Pixel-exact panels of glyphs.

#[test]
fn glyph_panels_have_nothing_added_around_them() {
    let mut p = planner(
        "[settings]\nicons = pack-required\n\
         [icon panel]\nglyph = U+E000\nfont = minecraft:default\nfallback =\n\
         [layout a]\ntitle =\nline = {icon:panel}x\nline =\nline =\nline = {icon:panel}x\n",
    );
    join(&mut p, A, facts("Alex", Edition::Java));
    let actions = p.plan();
    // No width from the title.
    assert!(actions.contains(&Action::SidebarShow {
        viewer: A.into(),
        title: Rendered::default(),
    }));
    let line = |wanted: usize| {
        actions.iter().find_map(|a| match a {
            Action::LineAdd { index, text, .. } if *index == wanted => Some(text.clone()),
            _ => None,
        })
    };
    // The glyph is the very first thing on its line, with nothing before it.
    assert!(matches!(
        line(0).expect("sent").0.first(),
        Some(Segment::Glyph { .. })
    ));
    // Two blank lines stay two lines; equal lines stay apart, by their own entries.
    assert!(line(1).expect("sent").is_empty());
    assert!(line(2).expect("sent").is_empty());
    assert_ne!(line_team(1).1, line_team(2).1);
    assert_eq!(line(0), line(3));
    assert_ne!(line_team(0).1, line_team(3).1);
    // Without the pack the empty fallback draws nothing at all, never a box.
    let mut text_mode = planner(
        "[icon panel]\nglyph = U+E000\nfont = minecraft:default\nfallback =\n\
         [layout a]\nline = {icon:panel}\n",
    );
    join(&mut text_mode, A, facts("Alex", Edition::Java));
    assert!(text_mode.plan().iter().any(|a| matches!(a,
        Action::LineAdd { text: t, .. } if t.is_empty())));
}

#[test]
fn an_idle_plan_is_empty_after_every_kind_of_setup() {
    let mut p = planner(
        "[settings]\nname-order = exact-case\nnametag-prefix = {core:rank}\n\
         [tab-score]\nvalue = {ping}\nformat = {value} ms\n\
         [below-name]\ntitle = x\nvalue = {core:n}\n\
         [layout a]\nheader = {player}\nline = {core:n}\n",
    );
    join(&mut p, A, facts("Alex", Edition::Java));
    join(&mut p, B, facts("Bo", Edition::Bedrock));
    set(&mut p, "core", A, "n", "1");
    let _ = p.plan();
    assert!(p.plan().is_empty());
    assert!(p.plan().is_empty());
}
