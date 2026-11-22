//! What other plugins ask this plugin over `ipc`. UTF-8 text, fields separated by TAB, records by
//! LF (a last LF is allowed), in the style of `ranks/1`. The sender's name comes from the
//! server.
//!
//! - `placeholders/1`, then lines: `set<TAB><uuid or *><TAB><key><TAB><value>` (literal text),
//!   `icon<TAB><uuid or *><TAB><key><TAB><image id>`,
//!   `glyphs<TAB><uuid or *><TAB><key><TAB><font><TAB><text>` (characters of a font of the server's
//!   own pack, never a platform font; drawn only with `icons = pack-required`),
//!   `clear<TAB><uuid or *>[<TAB><key>]`. Answer
//!   `placeholders/1<TAB>ok`. `clear<TAB>*` removes everything the sender ever pushed.
//! - `sidebar/1<TAB>get<TAB><uuid>`, `sidebar/1<TAB>set<TAB><uuid><TAB>on|off` (stored; nothing is
//!   written to the player), `sidebar/1<TAB>suppress<TAB><uuid><TAB><reason><TAB>on|off` (hidden
//!   while any reason is held; forgotten when the player leaves). Answer `sidebar/1<TAB>on|off`:
//!   the stored setting.
//! - `below-name/1<TAB>get<TAB><uuid>`, `below-name/1<TAB>set<TAB><uuid><TAB>on|off`: whether this
//!   viewer sees the scores below names.
//! - `teams/1<TAB>set<TAB><uuid><TAB><reason><TAB><collision><TAB><nametag>`,
//!   `teams/1<TAB>clear<TAB><uuid><TAB><reason>`. `<collision>`: `always`, `never`, `push-other`,
//!   `push-own`, or `-`; `<nametag>`: `always`, `never`, `hide-other`, `hide-own`, or `-`.
//!
//! A message that breaks a rule is refused as a whole, with the reason, and changes nothing.

use crate::placeholders::{MAX_VALUE_CHARS, Op, Provided, Target, valid_id, valid_key};
use crate::views::{Collision, Nametag};

/// The first word of a values message.
pub const VALUES: &str = "placeholders/1";
/// The largest message read.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// Most lines in one `placeholders/1` message.
pub const MAX_LINES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Flag {
    Sidebar,
    BelowName,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Values(Vec<Op>),
    /// `placeholders/1<TAB>alive`: a plugin that pushes values may be asked this, and any answer
    /// will do; the server tells the asker when no such plugin exists.
    Alive,
    Get(Flag, String),
    Set(Flag, String, bool),
    Suppress {
        uuid: String,
        reason: String,
        on: bool,
    },
    TeamSet {
        uuid: String,
        reason: String,
        collision: Option<Collision>,
        nametag: Option<Nametag>,
    },
    TeamClear {
        uuid: String,
        reason: String,
    },
}

/// Reads a message; the reason in English when it is refused.
///
/// # Errors
///
/// The first rule the message breaks.
pub fn parse(bytes: &[u8]) -> Result<Request, String> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("message too long".to_owned());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8".to_owned())?;
    // a last LF ends the last record, as user_interface reads the same contract
    let text = text.strip_suffix('\n').unwrap_or(text);
    let (head, rest) = text.split_once('\n').unwrap_or((text, ""));
    if head == "placeholders/1" {
        return parse_values(rest).map(Request::Values);
    }
    if !rest.is_empty() {
        return Err("only placeholders/1 has more lines".to_owned());
    }
    let fields: Vec<&str> = head.split('\t').collect();
    match fields.as_slice() {
        ["placeholders/1", "alive"] => Ok(Request::Alive),
        ["sidebar/1", tail @ ..] => parse_flag(Flag::Sidebar, tail),
        ["below-name/1", tail @ ..] => parse_flag(Flag::BelowName, tail),
        ["teams/1", tail @ ..] => parse_teams(tail),
        _ => Err("unknown contract".to_owned()),
    }
}

fn parse_flag(flag: Flag, fields: &[&str]) -> Result<Request, String> {
    match fields {
        ["get", uuid] => Ok(Request::Get(flag, player(uuid)?)),
        ["set", uuid, value] => Ok(Request::Set(flag, player(uuid)?, on_off(value)?)),
        ["suppress", uuid, why, value] if flag == Flag::Sidebar => Ok(Request::Suppress {
            uuid: player(uuid)?,
            reason: reason(why)?,
            on: on_off(value)?,
        }),
        _ => Err("unknown request".to_owned()),
    }
}

fn parse_teams(fields: &[&str]) -> Result<Request, String> {
    match fields {
        ["set", uuid, why, collision, nametag] => Ok(Request::TeamSet {
            uuid: player(uuid)?,
            reason: reason(why)?,
            collision: match *collision {
                "-" => None,
                "always" => Some(Collision::Always),
                "never" => Some(Collision::Never),
                "push-other" => Some(Collision::PushOther),
                "push-own" => Some(Collision::PushOwn),
                _ => return Err("unknown collision rule".to_owned()),
            },
            nametag: match *nametag {
                "-" => None,
                "always" => Some(Nametag::Always),
                "never" => Some(Nametag::Never),
                "hide-other" => Some(Nametag::HideOther),
                "hide-own" => Some(Nametag::HideOwn),
                _ => return Err("unknown nametag rule".to_owned()),
            },
        }),
        ["clear", uuid, why] => Ok(Request::TeamClear {
            uuid: player(uuid)?,
            reason: reason(why)?,
        }),
        _ => Err("unknown request".to_owned()),
    }
}

fn parse_values(lines: &str) -> Result<Vec<Op>, String> {
    let mut ops = Vec::new();
    if lines.is_empty() {
        return Ok(ops);
    }
    for line in lines.split('\n') {
        if ops.len() == MAX_LINES {
            return Err("too many lines".to_owned());
        }
        let fields: Vec<&str> = line.split('\t').collect();
        ops.push(match fields.as_slice() {
            ["set", who, name, value] => Op::Set {
                target: target(who)?,
                key: key(name)?,
                value: Provided::Text(value_text(value)?),
            },
            ["icon", who, name, id] => {
                if !valid_id(id) {
                    return Err("bad image id".to_owned());
                }
                Op::Set {
                    target: target(who)?,
                    key: key(name)?,
                    value: Provided::Icon((*id).to_owned()),
                }
            }
            ["glyphs", who, name, font, text] => {
                let font = crate::config::font(font)
                    .filter(|f| {
                        let namespace = f.split(':').next().unwrap_or("");
                        !crate::config::RESERVED_NAMESPACES.contains(&namespace)
                    })
                    .ok_or_else(|| "bad font".to_owned())?;
                Op::Set {
                    target: target(who)?,
                    key: key(name)?,
                    value: Provided::Glyphs {
                        font,
                        text: value_text(text)?,
                    },
                }
            }
            ["clear", who] => Op::Clear {
                target: target(who)?,
                key: None,
            },
            ["clear", who, name] => Op::Clear {
                target: target(who)?,
                key: Some(key(name)?),
            },
            _ => return Err("unknown line".to_owned()),
        });
    }
    Ok(ops)
}

fn target(field: &str) -> Result<Target, String> {
    if field == "*" {
        Ok(Target::Everyone)
    } else {
        player(field).map(Target::Player)
    }
}

/// A UUID as this plugin writes them: 36 characters, lower case.
fn player(field: &str) -> Result<String, String> {
    let ok = field.len() == 36
        && field.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        });
    if ok {
        Ok(field.to_ascii_lowercase())
    } else {
        Err("bad player id".to_owned())
    }
}

fn key(field: &str) -> Result<String, String> {
    if valid_key(field) {
        Ok(field.to_owned())
    } else {
        Err("bad key".to_owned())
    }
}

fn reason(field: &str) -> Result<String, String> {
    if valid_key(field) {
        Ok(field.to_owned())
    } else {
        Err("bad reason".to_owned())
    }
}

fn on_off(field: &str) -> Result<bool, String> {
    match field {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err("expected on or off".to_owned()),
    }
}

/// A value without control characters; too long a value is refused, never cut in silence.
fn value_text(field: &str) -> Result<String, String> {
    if field.chars().count() > MAX_VALUE_CHARS {
        return Err("value too long".to_owned());
    }
    Ok(field.chars().filter(|c| !c.is_control()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALEX: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

    fn text(s: &str) -> Result<Request, String> {
        parse(s.as_bytes())
    }

    #[test]
    fn a_batch_of_values_is_read_in_order() {
        let message = format!(
            "placeholders/1\nset\t{ALEX}\trate\t12%\nset\t*\tmotd\t&c{{x}}\nicon\t{ALEX}\trank\tvip\n\
             clear\t{ALEX}\trate\nclear\t*"
        );
        let Ok(Request::Values(ops)) = text(&message) else {
            panic!("read");
        };
        assert_eq!(ops.len(), 5);
        assert_eq!(
            ops[1],
            Op::Set {
                target: Target::Everyone,
                key: "motd".into(),
                value: Provided::Text("&c{x}".into()),
            }
        );
        assert_eq!(
            ops[2],
            Op::Set {
                target: Target::Player(ALEX.into()),
                key: "rank".into(),
                value: Provided::Icon("vip".into()),
            }
        );
        assert_eq!(
            ops[4],
            Op::Clear {
                target: Target::Everyone,
                key: None,
            }
        );
    }

    #[test]
    fn glyphs_name_their_font_and_platform_fonts_are_refused() {
        let message =
            format!("placeholders/1\nglyphs\t{ALEX}\tmap\tminimap:map\t\u{E120}\u{F0000}");
        let Ok(Request::Values(ops)) = text(&message) else {
            panic!("read");
        };
        assert_eq!(
            ops[0],
            Op::Set {
                target: Target::Player(ALEX.into()),
                key: "map".into(),
                value: Provided::Glyphs {
                    font: "minimap:map".into(),
                    text: "\u{E120}\u{F0000}".into()
                },
            }
        );
        for bad in ["strawberry_core:badge", "Bad:Font", "", "a:b c"] {
            let message = format!("placeholders/1\nglyphs\t*\tmap\t{bad}\tx");
            assert!(text(&message).is_err(), "{bad}");
        }
        let long = "x".repeat(MAX_VALUE_CHARS + 1);
        assert!(text(&format!("placeholders/1\nglyphs\t*\tmap\ta:b\t{long}")).is_err());
    }

    #[test]
    fn a_message_ending_with_a_line_feed_is_read() {
        // as the minimap sends its Tab map: every line ends with LF
        let message = format!(
            "placeholders/1\nglyphs\t{ALEX}\ttab\tminimap:map\t\u{F0000}\u{E400}\nclear\t{ALEX}\tx\n"
        );
        let Ok(Request::Values(ops)) = text(&message) else {
            panic!("read");
        };
        assert_eq!(ops.len(), 2);
        assert!(
            matches!(&ops[0], Op::Set { value: Provided::Glyphs { font, .. }, .. } if font == "minimap:map")
        );
        assert_eq!(
            text(&format!("sidebar/1\tget\t{ALEX}\n")),
            Ok(Request::Get(Flag::Sidebar, ALEX.into()))
        );
        // one LF ends a record; an empty line is still refused
        assert!(text(&format!("placeholders/1\nclear\t{ALEX}\n\n")).is_err());
    }

    #[test]
    fn an_empty_value_is_a_value_and_control_characters_go() {
        let message = format!("placeholders/1\nset\t{ALEX}\tk\t\nset\t*\tk\ta\u{7}b");
        let Ok(Request::Values(ops)) = text(&message) else {
            panic!("read");
        };
        assert!(matches!(&ops[0], Op::Set { value: Provided::Text(v), .. } if v.is_empty()));
        assert!(matches!(&ops[1], Op::Set { value: Provided::Text(v), .. } if v == "ab"));
    }

    #[test]
    fn bad_messages_are_refused_as_a_whole() {
        let long = "x".repeat(MAX_VALUE_CHARS + 1);
        for bad in [
            String::new(),
            "ranks/1\tget".to_owned(),
            "placeholders/2\nset\t*\tk\tv".to_owned(),
            "placeholders/1\nset\t*\tBad Key\tv".to_owned(),
            "placeholders/1\nset\tnot-a-uuid\tk\tv".to_owned(),
            format!("placeholders/1\nset\t*\tk\t{long}"),
            "placeholders/1\nicon\t*\tk\tNope!".to_owned(),
            "placeholders/1\nset\t*\tk\tv\nwhat".to_owned(),
            format!("sidebar/1\tset\t{ALEX}\tmaybe"),
            format!("sidebar/1\tget\t{ALEX}\nmore"),
            format!("below-name/1\tsuppress\t{ALEX}\tmode\ton"),
            format!("teams/1\tset\t{ALEX}\tboat\tsideways\t-"),
        ] {
            assert!(text(&bad).is_err(), "{bad}");
        }
        assert!(parse(&[0xff, 0xfe]).is_err());
        let many = format!(
            "placeholders/1\n{}",
            vec!["clear\t*"; MAX_LINES + 1].join("\n")
        );
        assert_eq!(text(&many), Err("too many lines".to_owned()));
    }

    #[test]
    fn flags_and_reasons() {
        assert_eq!(
            text(&format!("sidebar/1\tset\t{ALEX}\toff")),
            Ok(Request::Set(Flag::Sidebar, ALEX.into(), false))
        );
        assert_eq!(
            text(&format!("below-name/1\tget\t{ALEX}")),
            Ok(Request::Get(Flag::BelowName, ALEX.into()))
        );
        assert_eq!(
            text(&format!("sidebar/1\tsuppress\t{ALEX}\tmode-menu\ton")),
            Ok(Request::Suppress {
                uuid: ALEX.into(),
                reason: "mode-menu".into(),
                on: true,
            })
        );
    }

    #[test]
    fn team_options() {
        assert_eq!(
            text(&format!("teams/1\tset\t{ALEX}\tboat\tnever\t-")),
            Ok(Request::TeamSet {
                uuid: ALEX.into(),
                reason: "boat".into(),
                collision: Some(Collision::Never),
                nametag: None,
            })
        );
        assert_eq!(
            text(&format!(
                "teams/1\tclear\t{}\tboat",
                ALEX.to_ascii_uppercase()
            )),
            Ok(Request::TeamClear {
                uuid: ALEX.into(),
                reason: "boat".into(),
            })
        );
    }
}
