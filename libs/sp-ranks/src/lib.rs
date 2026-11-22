//! Ranks from perms-lite over `ipc`: contract `ranks/1` (the same text in perms-lite's README
//! and its `ipc` module; both sides test the same bytes). The one reader of it: Tab and Scoreboard,
//! list-lite and join-lite all use this crate.
//!
//! UTF-8 text, fields separated by TAB, records by LF:
//!
//! - `ranks/1<TAB>get<TAB><uuid>[<TAB><uuid>...]` (1 to [`MAX_UUIDS`] UUIDs) answers
//!   `ranks/1<TAB><rev>` and then one line per UUID, in the order asked:
//!   `<uuid><TAB><group><TAB><weight><TAB><prefix><TAB><suffix>`.
//! - `ranks/1<TAB>watch` answers `ranks/1<TAB><rev>` and makes perms-lite tell the sender of
//!   every later change: it sends `ranks/1<TAB>changed<TAB><rev>` and then one rank line per
//!   player whose rank changed; the watcher answers with nothing.
//! - `<rev>` grows with every change and starts somewhere new at each perms-lite load, so a
//!   watcher that sees a `rev` it was not told about reads every rank again.
//! - A plain UUID (no `ranks/1`) still answers the bare prefix, for chat-lite.
//!
//! Nothing here calls the host except [`get`] and [`watch`], which hold no lock.

#![forbid(unsafe_code)]

/// The plugin that owns the ranks.
pub const PERMS_PLUGIN: &str = "perms-lite";
/// The contract's first field.
pub const VERSION: &str = "ranks/1";
/// Most UUIDs in one `get`.
pub const MAX_UUIDS: usize = 128;
/// The longest prefix, suffix or group name kept, in characters.
pub const MAX_FIELD_CHARS: usize = 64;

/// A player's rank.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Rank {
    pub group: String,
    pub weight: i32,
    pub prefix: String,
    pub suffix: String,
}

impl Default for Rank {
    /// What a player has before perms-lite answers, or without perms-lite.
    fn default() -> Self {
        Self {
            group: "default".to_owned(),
            weight: 0,
            prefix: String::new(),
            suffix: String::new(),
        }
    }
}

/// The `get` request for `uuids` (the caller keeps it at most [`MAX_UUIDS`] long).
#[must_use]
pub fn get_request(uuids: &[String]) -> Vec<u8> {
    let mut text = format!("{VERSION}\tget");
    for uuid in uuids {
        text.push('\t');
        text.push_str(uuid);
    }
    text.into_bytes()
}

/// The `watch` request.
#[must_use]
pub fn watch_request() -> Vec<u8> {
    format!("{VERSION}\twatch").into_bytes()
}

/// What a message from perms-lite says.
#[derive(Debug, PartialEq, Eq)]
pub struct Answer {
    pub rev: u64,
    pub ranks: Vec<(String, Rank)>,
}

/// Reads an answer to `get` or `watch`; `None` when it is not one.
#[must_use]
pub fn parse_answer(bytes: &[u8]) -> Option<Answer> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.split('\n');
    let mut head = lines.next()?.split('\t');
    if head.next()? != VERSION {
        return None;
    }
    let rev = head.next()?.parse().ok()?;
    if head.next().is_some() {
        return None;
    }
    Some(Answer {
        rev,
        ranks: parse_ranks(lines)?,
    })
}

/// Reads a `changed` message; `None` when it is not one.
#[must_use]
pub fn parse_changed(bytes: &[u8]) -> Option<Answer> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.split('\n');
    let mut head = lines.next()?.split('\t');
    if head.next()? != VERSION || head.next()? != "changed" {
        return None;
    }
    let rev = head.next()?.parse().ok()?;
    if head.next().is_some() {
        return None;
    }
    Some(Answer {
        rev,
        ranks: parse_ranks(lines)?,
    })
}

fn parse_ranks<'a>(lines: impl Iterator<Item = &'a str>) -> Option<Vec<(String, Rank)>> {
    let mut ranks = Vec::new();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let [uuid, group, weight, prefix, suffix] = fields.as_slice() else {
            return None;
        };
        if uuid.is_empty() || group.is_empty() || ranks.len() == MAX_UUIDS {
            return None;
        }
        ranks.push((
            (*uuid).to_owned(),
            Rank {
                group: field(group),
                weight: weight.parse().ok()?,
                prefix: field(prefix),
                suffix: field(suffix),
            },
        ));
    }
    Some(ranks)
}

/// A field as it is kept: no control characters, at most [`MAX_FIELD_CHARS`] characters.
fn field(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_FIELD_CHARS)
        .collect()
}

/// Asks perms-lite for the ranks of `uuids`; `None` when perms-lite is not there or answers
/// something else.
#[must_use]
pub fn get(uuids: &[String]) -> Option<Answer> {
    send(&get_request(uuids))
}

/// Asks perms-lite to tell this plugin of changes; the answer carries perms-lite's `rev`.
#[must_use]
pub fn watch() -> Option<u64> {
    send(&watch_request()).map(|answer| answer.rev)
}

fn send(request: &[u8]) -> Option<Answer> {
    match strawberry_plugin_api::ipc::send_ipc_message(PERMS_PLUGIN, request) {
        Ok(Ok(bytes)) => parse_answer(&bytes),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALEX: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
    const BO: &str = "00000000-0000-0000-0000-000000000002";

    #[test]
    fn requests_are_the_contract_bytes() {
        assert_eq!(
            get_request(&[ALEX.into(), BO.into()]),
            format!("ranks/1\tget\t{ALEX}\t{BO}").into_bytes()
        );
        assert_eq!(watch_request(), b"ranks/1\twatch".to_vec());
    }

    #[test]
    fn an_answer_gives_rev_and_ranks_in_order() {
        let bytes = format!("ranks/1\t77\n{ALEX}\tvip\t50\tVIP\t\n{BO}\tdefault\t0\t\t");
        let answer = parse_answer(bytes.as_bytes()).expect("an answer");
        assert_eq!(answer.rev, 77);
        assert_eq!(
            answer.ranks,
            vec![
                (
                    ALEX.to_owned(),
                    Rank {
                        group: "vip".into(),
                        weight: 50,
                        prefix: "VIP".into(),
                        suffix: String::new()
                    }
                ),
                (BO.to_owned(), Rank::default()),
            ]
        );
        let watch = parse_answer(b"ranks/1\t9").expect("a watch answer");
        assert_eq!((watch.rev, watch.ranks.len()), (9, 0));
    }

    #[test]
    fn a_changed_message_is_read() {
        let bytes = format!("ranks/1\tchanged\t78\n{ALEX}\tadmin\t100\tADMIN\t*");
        let changed = parse_changed(bytes.as_bytes()).expect("a change");
        assert_eq!(changed.rev, 78);
        assert_eq!(changed.ranks[0].1.group, "admin");
        assert_eq!(changed.ranks[0].1.suffix, "*");
        // A change is not an answer and an answer is not a change.
        assert!(parse_answer(bytes.as_bytes()).is_none());
        assert!(parse_changed(b"ranks/1\t9").is_none());
    }

    #[test]
    fn anything_else_is_refused() {
        for bad in [
            &b""[..],
            b"VIP",
            b"ranks/2\t1",
            b"ranks/1\tnot-a-number",
            b"ranks/1\t1\nonly\tthree\tfields",
            b"ranks/1\t1\nu\tg\tnot-a-weight\tp\ts",
            b"ranks/1\t1\nu\tg\t1\tp\ts\textra",
            b"\xff\xfe",
        ] {
            assert!(parse_answer(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn long_fields_are_cut_and_control_characters_dropped() {
        let long = "x".repeat(200);
        let bytes = format!("ranks/1\t1\n{ALEX}\tvip\t5\t{long}\ta\u{7}b");
        let answer = parse_answer(bytes.as_bytes()).expect("an answer");
        assert_eq!(answer.ranks[0].1.prefix.chars().count(), MAX_FIELD_CHARS);
        assert_eq!(answer.ranks[0].1.suffix, "ab");
    }
}
