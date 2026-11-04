//! A filled template becomes [`Rendered`] segments: text with colour codes, literal text, glyph
//! images and head images. Which image a viewer gets is decided here, without the host:
//!
//! - a glyph needs a font from the server's own resource pack, so it is sent only when the
//!   server owner set `icons = pack-required` (the server lets no Java player in who did not
//!   load a required pack); otherwise its fallback text is sent;
//! - a head needs no pack (26.3 object text); older clients see its name as text;
//! - a Bedrock viewer gets fallback text for every image ([`Policy::TextOnly`]).
//!
//! A client is never sent a glyph it may lack, so nobody sees a missing-glyph box.
//!
//! [`runs`] reads the codes: `&0`-`&f`, `&k`-`&o`, `&r`, `&#RRGGBB` and the gradient
//! `<#RRGGBB>text</#RRGGBB>`. A style carries from one segment to the next, as it does in the
//! text the owner wrote. A colour code ends bold, italic and the like, as in vanilla.

use crate::config::{Config, IconKind, IconsMode};
use crate::placeholders::{Filled, IconRef, Source, Template};

/// The profile name of a player's own `{head}`: what a client too old to draw heads shows in its
/// place (one mark, not the player's name a second time).
pub const OWN_HEAD_TEXT: &str = ".";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Segment {
    /// Text with colour codes.
    Text(String),
    /// Text shown as it is (a value from another plugin), in the style around it.
    Literal(String),
    Glyph {
        text: String,
        font: String,
    },
    Head {
        textures: Option<String>,
        signature: Option<String>,
        name: String,
        id: Option<String>,
        hat: bool,
        shadow: bool,
    },
}

/// What a viewer is sent: compared with what it was sent last, so only changes go out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rendered(pub Vec<Segment>);

impl Rendered {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(Segment::Text(last)) = self.0.last_mut() {
            last.push_str(text);
        } else {
            self.0.push(Segment::Text(text.to_owned()));
        }
    }

    /// Plain text with codes.
    #[must_use]
    pub fn text(text: &str) -> Self {
        let mut out = Self::default();
        out.push_text(text);
        out
    }
}

/// A player's own face, for `{head}`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Face {
    pub id: String,
    pub textures: Option<String>,
    pub signature: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Java(IconsMode),
    TextOnly,
}

/// What a template is rendered with, besides its values.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub group: &'a str,
    pub config: &'a Config,
    pub policy: Policy,
    /// Index into the language files, for image fallbacks.
    pub lang: usize,
    pub face: Option<&'a Face>,
}

/// `template` filled from `source`; `{icon}` is the image whose id is the group, if there is one.
#[must_use]
pub fn render(template: &Template, source: &dyn Source, cx: Context<'_>) -> Rendered {
    let mut out = Rendered::default();
    add(&mut out, template, source, cx);
    out
}

/// `templates` one per line.
#[must_use]
pub fn render_lines<'a>(
    templates: impl IntoIterator<Item = &'a Template>,
    source: &dyn Source,
    cx: Context<'_>,
) -> Rendered {
    let mut out = Rendered::default();
    for (i, template) in templates.into_iter().enumerate() {
        if i > 0 {
            // A colour code must not run into the next line, so the line ends with a reset.
            out.push_text("&r\n");
        }
        add(&mut out, template, source, cx);
    }
    out
}

fn add(out: &mut Rendered, template: &Template, source: &dyn Source, cx: Context<'_>) {
    for piece in template.fill(source) {
        match piece {
            Filled::Text(text) => out.push_text(&text),
            Filled::Literal(text) => {
                if !text.is_empty() {
                    out.0.push(Segment::Literal(text));
                }
            }
            Filled::Head => {
                if let (Some(face), Policy::Java(_)) = (cx.face, cx.policy) {
                    out.0.push(Segment::Head {
                        textures: face.textures.clone(),
                        signature: face.signature.clone(),
                        name: OWN_HEAD_TEXT.to_owned(),
                        id: Some(face.id.clone()),
                        hat: true,
                        shadow: false,
                    });
                }
            }
            Filled::Icon(icon) => {
                let id = match &icon {
                    IconRef::Own => cx.group.to_ascii_lowercase(),
                    IconRef::Named(id) => id.clone(),
                };
                let Some(icon) = cx.config.icon(&id) else {
                    continue;
                };
                match (&icon.kind, cx.policy) {
                    (IconKind::Glyph { text, font }, Policy::Java(IconsMode::PackRequired)) => {
                        out.0.push(Segment::Glyph {
                            text: text.clone(),
                            font: font.clone(),
                        });
                    }
                    (
                        IconKind::Head {
                            textures,
                            name,
                            hat,
                        },
                        Policy::Java(_),
                    ) => {
                        out.0.push(Segment::Head {
                            textures: Some(textures.clone()),
                            signature: None,
                            name: name.clone(),
                            id: None,
                            hat: *hat,
                            shadow: true,
                        });
                    }
                    _ => out.push_text(icon.fallback(cx.lang)),
                }
            }
        }
    }
}

/// A text's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// A legacy code, `0` to `f`.
    Code(char),
    Rgb(u8, u8, u8),
}

/// A text's style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub color: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underlined: bool,
    pub strikethrough: bool,
    pub obfuscated: bool,
}

impl Style {
    fn apply(&mut self, code: char) {
        match code {
            '0'..='9' | 'a'..='f' => {
                *self = Self {
                    color: Some(Color::Code(code)),
                    ..Self::default()
                };
            }
            'k' => self.obfuscated = true,
            'l' => self.bold = true,
            'm' => self.strikethrough = true,
            'n' => self.underlined = true,
            'o' => self.italic = true,
            _ => *self = Self::default(),
        }
    }
}

/// A stretch of one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Run<'a> {
    Text { text: String, style: Style },
    Image(&'a Segment),
}

enum Token<'a> {
    Char(char),
    Code(char),
    Hex(u8, u8, u8),
    Open(u8, u8, u8),
    Close(u8, u8, u8),
    Image(&'a Segment),
}

fn hex6(text: &str) -> Option<(u8, u8, u8)> {
    let bytes = text.get(..6)?;
    if !bytes.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let part = |i: usize| u8::from_str_radix(&bytes[i..i + 2], 16).ok();
    Some((part(0)?, part(2)?, part(4)?))
}

const fn is_code(c: char) -> bool {
    matches!(c.to_ascii_lowercase(), '0'..='9' | 'a'..='f' | 'k'..='o' | 'r')
}

fn scan<'a>(text: &str, out: &mut Vec<Token<'a>>) {
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        if c == '&' {
            if let Some(hex) = after.strip_prefix('#')
                && let Some((r, g, b)) = hex6(hex)
            {
                out.push(Token::Hex(r, g, b));
                rest = &hex[6..];
                continue;
            }
            if let Some(code) = after.chars().next()
                && is_code(code)
            {
                out.push(Token::Code(code.to_ascii_lowercase()));
                rest = &after[code.len_utf8()..];
                continue;
            }
        } else if c == '<' {
            if let Some(hex) = after.strip_prefix('#')
                && let Some((r, g, b)) = hex6(hex)
                && hex[6..].starts_with('>')
            {
                out.push(Token::Open(r, g, b));
                rest = &hex[7..];
                continue;
            }
            if let Some(hex) = after.strip_prefix("/#")
                && let Some((r, g, b)) = hex6(hex)
                && hex[6..].starts_with('>')
            {
                out.push(Token::Close(r, g, b));
                rest = &hex[7..];
                continue;
            }
        }
        out.push(Token::Char(c));
        rest = after;
    }
}

fn lerp(from: u8, to: u8, step: usize, last: usize) -> u8 {
    let (from, to) = (from as usize, to as usize);
    let value = if to >= from {
        from + (to - from) * step / last
    } else {
        from - (from - to) * step / last
    };
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn push_char<'a>(runs: &mut Vec<Run<'a>>, c: char, style: Style) {
    if let Some(Run::Text { text, style: last }) = runs.last_mut()
        && *last == style
    {
        text.push(c);
    } else {
        runs.push(Run::Text {
            text: c.to_string(),
            style,
        });
    }
}

/// The style of every piece of `rendered`.
#[must_use]
pub fn runs(rendered: &Rendered) -> Vec<Run<'_>> {
    let mut tokens = Vec::new();
    for segment in &rendered.0 {
        match segment {
            Segment::Text(text) => scan(text, &mut tokens),
            Segment::Literal(text) => tokens.extend(text.chars().map(Token::Char)),
            image => tokens.push(Token::Image(image)),
        }
    }

    let mut out = Vec::new();
    let mut style = Style::default();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            Token::Char(c) => push_char(&mut out, c, style),
            Token::Code(code) => style.apply(code),
            Token::Hex(r, g, b) => {
                style = Style {
                    color: Some(Color::Rgb(r, g, b)),
                    ..Style::default()
                };
            }
            Token::Image(segment) => out.push(Run::Image(segment)),
            // A closing tag with no opening one is not a tag.
            Token::Close(r, g, b) => {
                for c in format!("</#{r:02X}{g:02X}{b:02X}>").chars() {
                    push_char(&mut out, c, style);
                }
            }
            Token::Open(r, g, b) => {
                index = gradient(&tokens, index, (r, g, b), &mut style, &mut out);
            }
        }
        index += 1;
    }
    out
}

/// A gradient from the token at `open`; the index of its last token. It is a scope: the style
/// before it comes back after it, and format codes inside it stay inside it. Without a closing
/// tag it keeps the opening colour to the end.
fn gradient<'a>(
    tokens: &[Token<'a>],
    open: usize,
    start: (u8, u8, u8),
    style: &mut Style,
    out: &mut Vec<Run<'a>>,
) -> usize {
    let close = tokens[open + 1..]
        .iter()
        .position(|t| matches!(t, Token::Close(..)))
        .map(|i| open + 1 + i);
    let inner = &tokens[open + 1..close.unwrap_or(tokens.len())];
    let end = close.map_or(start, |i| match tokens[i] {
        Token::Close(r, g, b) => (r, g, b),
        _ => start,
    });
    let chars = inner.iter().filter(|t| matches!(t, Token::Char(_))).count();
    let last = chars.saturating_sub(1).max(1);
    let before = *style;
    let mut step = 0;
    for token in inner {
        match *token {
            Token::Char(c) => {
                let color = Color::Rgb(
                    lerp(start.0, end.0, step, last),
                    lerp(start.1, end.1, step, last),
                    lerp(start.2, end.2, step, last),
                );
                push_char(
                    out,
                    c,
                    Style {
                        color: Some(color),
                        ..*style
                    },
                );
                step += 1;
            }
            // Only the formats count inside: the gradient is the colour.
            Token::Code(code) if !matches!(code, '0'..='9' | 'a'..='f') => style.apply(code),
            Token::Image(segment) => out.push(Run::Image(segment)),
            _ => {}
        }
    }
    *style = before;
    close.unwrap_or(tokens.len() - 1)
}

const LEGACY: [(char, (u8, u8, u8)); 16] = [
    ('0', (0, 0, 0)),
    ('1', (0, 0, 170)),
    ('2', (0, 170, 0)),
    ('3', (0, 170, 170)),
    ('4', (170, 0, 0)),
    ('5', (170, 0, 170)),
    ('6', (255, 170, 0)),
    ('7', (170, 170, 170)),
    ('8', (85, 85, 85)),
    ('9', (85, 85, 255)),
    ('a', (85, 255, 85)),
    ('b', (85, 255, 255)),
    ('c', (255, 85, 85)),
    ('d', (255, 85, 255)),
    ('e', (255, 255, 85)),
    ('f', (255, 255, 255)),
];

/// The legacy code nearest to a colour, for clients without full colours.
#[must_use]
pub fn nearest_code(r: u8, g: u8, b: u8) -> char {
    let distance = |c: (u8, u8, u8)| {
        let d = |x: u8, y: u8| {
            let v = i32::from(x) - i32::from(y);
            v * v
        };
        d(r, c.0) + d(g, c.1) + d(b, c.2)
    };
    LEGACY
        .iter()
        .min_by_key(|(_, c)| distance(*c))
        .map_or('f', |(code, _)| *code)
}

/// The text a Bedrock client shows: `§` codes, nearest colours; images are already text.
#[must_use]
pub fn to_section_codes(rendered: &Rendered) -> String {
    let mut out = String::new();
    let mut shown = Style::default();
    for run in runs(rendered) {
        let Run::Text { text, style } = run else {
            continue;
        };
        if style != shown {
            let lost = (shown.bold && !style.bold)
                || (shown.italic && !style.italic)
                || (shown.underlined && !style.underlined)
                || (shown.strikethrough && !style.strikethrough)
                || (shown.obfuscated && !style.obfuscated)
                || (shown.color.is_some() && style.color.is_none());
            let code = |color: Option<Color>| match color {
                Some(Color::Code(c)) => Some(c),
                Some(Color::Rgb(r, g, b)) => Some(nearest_code(r, g, b)),
                None => None,
            };
            let recolour = code(style.color) != code(shown.color);
            if lost {
                out.push_str("§r");
            }
            if let Some(c) = code(style.color)
                && (recolour || lost)
            {
                out.push('§');
                out.push(c);
            }
            let again = lost || recolour;
            for (on, was, c) in [
                (style.obfuscated, shown.obfuscated, 'k'),
                (style.bold, shown.bold, 'l'),
                (style.strikethrough, shown.strikethrough, 'm'),
                (style.underlined, shown.underlined, 'n'),
                (style.italic, shown.italic, 'o'),
            ] {
                if on && (!was || again) {
                    out.push('§');
                    out.push(c);
                }
            }
            shown = style;
        }
        // A pushed value's own `§` must not become a code on the client.
        out.push_str(&text.replace('§', ""));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse;
    use crate::placeholders::{Key, Provided};

    struct Alex;
    impl Source for Alex {
        fn value(&self, key: Key) -> String {
            match key {
                Key::Player => "Alex".into(),
                Key::Prefix => "VIP".into(),
                _ => String::new(),
            }
        }
    }

    fn config(icons: &str) -> Config {
        parse(&format!(
            "[settings]\nicons = {icons}\n\
             [icon vip]\nglyph = U+E100\nfont = myserver:ranks\nfallback = &6[VIP]\n\
             [icon star]\nhead = abc=\nfallback = &e*\nhead-name = star\n"
        ))
        .expect("parses")
    }

    fn t(text: &str) -> Template {
        Template::parse(text)
    }

    fn cx<'a>(config: &'a Config, group: &'a str, policy: Policy) -> Context<'a> {
        Context {
            group,
            config,
            policy,
            lang: 0,
            face: None,
        }
    }

    fn java(config: &Config) -> Policy {
        Policy::Java(config.icons_mode)
    }

    #[test]
    fn a_glyph_goes_out_only_when_the_pack_is_required() {
        let text_mode = config("text");
        let r = render(
            &t("{icon} {player}"),
            &Alex,
            cx(&text_mode, "vip", java(&text_mode)),
        );
        assert_eq!(r, Rendered(vec![Segment::Text("&6[VIP] Alex".into())]));

        let pack = config("pack-required");
        let r = render(&t("{icon} {player}"), &Alex, cx(&pack, "VIP", java(&pack)));
        assert_eq!(
            r,
            Rendered(vec![
                Segment::Glyph {
                    text: "\u{E100}".into(),
                    font: "myserver:ranks".into()
                },
                Segment::Text(" Alex".into()),
            ])
        );
    }

    #[test]
    fn heads_need_no_pack_but_bedrock_gets_text() {
        let config = config("text");
        let r = render(
            &t("{icon:star}"),
            &Alex,
            cx(&config, "default", java(&config)),
        );
        assert!(matches!(&r.0[..], [Segment::Head { name, .. }] if name == "star"));
        let bedrock = render(
            &t("{icon:star}{icon}"),
            &Alex,
            cx(&config, "vip", Policy::TextOnly),
        );
        assert_eq!(to_section_codes(&bedrock), "§e*§6[VIP]");
    }

    #[test]
    fn a_group_without_an_image_shows_nothing_for_icon() {
        let config = config("pack-required");
        let r = render(
            &t("{icon}{player}"),
            &Alex,
            cx(&config, "default", java(&config)),
        );
        assert_eq!(r, Rendered(vec![Segment::Text("Alex".into())]));
    }

    #[test]
    fn lines_are_joined_with_a_reset() {
        let config = config("text");
        let r = render_lines(
            [&t("&ea"), &t("b")],
            &Alex,
            cx(&config, "", Policy::TextOnly),
        );
        assert_eq!(r, Rendered(vec![Segment::Text("&ea&r\nb".into())]));
    }

    #[test]
    fn section_codes() {
        let r = Rendered(vec![Segment::Text("&6Gold &Zx && &Rend".into())]);
        assert_eq!(to_section_codes(&r), "§6Gold &Zx && §rend");
        let r = Rendered(vec![Segment::Text("&l&cA&6B&#FF0000C".into())]);
        assert_eq!(to_section_codes(&r), "§cA§6B§4C", "a colour ends bold");
        let r = Rendered(vec![Segment::Text("&cA&lB&cC".into())]);
        assert_eq!(to_section_codes(&r), "§cA§lB§r§cC");
    }

    fn style_of(run: &Run<'_>) -> Style {
        match run {
            Run::Text { style, .. } => *style,
            Run::Image(_) => panic!("an image"),
        }
    }

    fn text_of<'a>(run: &'a Run<'_>) -> &'a str {
        match run {
            Run::Text { text, .. } => text,
            Run::Image(_) => panic!("an image"),
        }
    }

    #[test]
    fn a_style_carries_across_a_pushed_value_and_the_value_stays_literal() {
        let r = Rendered(vec![
            Segment::Text("&aRate: ".into()),
            Segment::Literal("&c{x}".into()),
            Segment::Text("%&7!".into()),
        ]);
        let runs = runs(&r);
        assert_eq!(runs.len(), 2);
        assert_eq!(text_of(&runs[0]), "Rate: &c{x}%");
        assert_eq!(style_of(&runs[0]).color, Some(Color::Code('a')));
        assert_eq!(text_of(&runs[1]), "!");
    }

    #[test]
    fn a_pushed_value_is_never_read_as_codes_on_bedrock_either() {
        let r = Rendered(vec![Segment::Literal("&c§4x".into())]);
        assert_eq!(to_section_codes(&r), "&c4x");
    }

    #[test]
    fn hex_codes_and_formats() {
        let r = Rendered::text("&#12aBEF&lA&#000000B &ZC");
        let runs = runs(&r);
        assert_eq!(style_of(&runs[0]).color, Some(Color::Rgb(0x12, 0xab, 0xef)));
        assert!(style_of(&runs[0]).bold);
        assert_eq!(style_of(&runs[1]).color, Some(Color::Rgb(0, 0, 0)));
        assert!(!style_of(&runs[1]).bold, "a colour ends bold");
        assert_eq!(text_of(&runs[1]), "B &ZC");
        // Not six digits: not a code.
        let bad = Rendered::text("&#12G");
        let bad_runs = super::runs(&bad);
        assert_eq!(text_of(&bad_runs[0]), "&#12G");
    }

    #[test]
    fn a_gradient_runs_from_colour_to_colour_and_is_a_scope() {
        let r = Rendered::text("&a<#000000>&mab&9c</#FF0000>d");
        let runs = runs(&r);
        let colours: Vec<_> = runs.iter().map(|x| style_of(x).color).collect();
        assert_eq!(
            colours,
            [
                Some(Color::Rgb(0, 0, 0)),
                Some(Color::Rgb(127, 0, 0)),
                Some(Color::Rgb(255, 0, 0)),
                Some(Color::Code('a')),
            ]
        );
        assert!(style_of(&runs[0]).strikethrough, "formats count inside");
        assert!(!style_of(&runs[3]).strikethrough, "and end with it");
        assert_eq!(text_of(&runs[3]), "d");
    }

    #[test]
    fn a_gradient_goes_down_and_handles_short_and_broken_tags() {
        let down = Rendered::text("<#FF0000>ab</#000000>");
        let runs_down = runs(&down);
        assert_eq!(style_of(&runs_down[1]).color, Some(Color::Rgb(0, 0, 0)));
        let one = Rendered::text("<#102030>x</#FFFFFF>");
        assert_eq!(
            style_of(&runs(&one)[0]).color,
            Some(Color::Rgb(0x10, 0x20, 0x30))
        );
        let open = Rendered::text("<#102030>xy");
        let runs_open = runs(&open);
        assert_eq!(runs_open.len(), 1, "no closing tag: one colour");
        assert_eq!(text_of(&runs_open[0]), "xy");
        let lone = Rendered::text("a</#102030>b");
        assert_eq!(text_of(&runs(&lone)[0]), "a</#102030>b");
        let text = Rendered::text("<#12>x");
        assert_eq!(text_of(&runs(&text)[0]), "<#12>x");
        assert!(runs(&Rendered::text("<#102030></#102030>")).is_empty());
    }

    #[test]
    fn images_keep_their_place_inside_styled_text() {
        let glyph = Segment::Glyph {
            text: "\u{E100}".into(),
            font: "a:b".into(),
        };
        let r = Rendered(vec![
            Segment::Text("&ax".into()),
            glyph.clone(),
            Segment::Text("y".into()),
        ]);
        let runs = runs(&r);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1], Run::Image(&glyph));
        assert_eq!(style_of(&runs[2]).color, Some(Color::Code('a')));
    }

    #[test]
    fn bedrock_gets_the_nearest_colour() {
        assert_eq!(nearest_code(255, 170, 10), '6');
        assert_eq!(nearest_code(0, 0, 0), '0');
        let r = Rendered::text("<#FF5555>ab</#FF5555>");
        assert_eq!(to_section_codes(&r), "§cab");
    }

    #[test]
    fn an_own_head_goes_to_java_only_and_never_doubles_the_name() {
        struct One(Provided);
        impl Source for One {
            fn value(&self, _key: Key) -> String {
                String::new()
            }
            fn provided(&self, _name: &str) -> Option<&Provided> {
                Some(&self.0)
            }
        }
        let config = config("text");
        let face = Face {
            id: "id".into(),
            textures: Some("abc=".into()),
            signature: None,
        };
        let with_face = Context {
            face: Some(&face),
            ..cx(&config, "", java(&config))
        };
        let r = render(&t("{head}"), &Alex, with_face);
        assert!(
            matches!(&r.0[..], [Segment::Head { name, shadow: false, hat: true, .. }]
            if name == OWN_HEAD_TEXT)
        );
        let bedrock = Context {
            policy: Policy::TextOnly,
            ..with_face
        };
        assert!(render(&t("{head}"), &Alex, bedrock).is_empty());
        // A plugin can name an image for a player.
        let r = render(
            &t("{x:y}"),
            &One(Provided::Icon("vip".into())),
            cx(&config, "", java(&config)),
        );
        assert_eq!(r, Rendered(vec![Segment::Text("&6[VIP]".into())]));
    }
}
