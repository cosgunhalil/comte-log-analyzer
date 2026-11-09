//! Unity rich text in log messages: `<color=cyan>`, `<b>`, `<i>`, `<size=20>`.
//!
//! Games often color their own `Debug.Log` output, e.g.
//! `Debug.Log("<color='cyan'>Starting new race</color>")`. Unity's console
//! renders that in color; a log file keeps the tags as text. This module
//! turns such a message into styled [`Span`]s for display and into plain text
//! for search, reports and fingerprints.
//!
//! Only real rich-text tags are recognized. Anything else that happens to
//! look like a tag, such as `List<int>`, `<>c__DisplayClass` or `<null>`,
//! stays in the text unchanged. Unclosed tags style the rest of the message,
//! as in Unity.

/// RGBA color.
pub type Rgba = [u8; 4];

/// A run of text with one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub color: Option<Rgba>,
    pub bold: bool,
    pub italic: bool,
}

/// Unity's named rich-text colors.
const NAMED_COLORS: &[(&str, Rgba)] = &[
    ("aqua", [0x00, 0xff, 0xff, 0xff]),
    ("black", [0x00, 0x00, 0x00, 0xff]),
    ("blue", [0x00, 0x00, 0xff, 0xff]),
    ("brown", [0xa5, 0x2a, 0x2a, 0xff]),
    ("cyan", [0x00, 0xff, 0xff, 0xff]),
    ("darkblue", [0x00, 0x00, 0xa0, 0xff]),
    ("fuchsia", [0xff, 0x00, 0xff, 0xff]),
    ("green", [0x00, 0x80, 0x00, 0xff]),
    ("grey", [0x80, 0x80, 0x80, 0xff]),
    ("gray", [0x80, 0x80, 0x80, 0xff]),
    ("lightblue", [0xad, 0xd8, 0xe6, 0xff]),
    ("lime", [0x00, 0xff, 0x00, 0xff]),
    ("magenta", [0xff, 0x00, 0xff, 0xff]),
    ("maroon", [0x80, 0x00, 0x00, 0xff]),
    ("navy", [0x00, 0x00, 0x80, 0xff]),
    ("olive", [0x80, 0x80, 0x00, 0xff]),
    ("orange", [0xff, 0xa5, 0x00, 0xff]),
    ("purple", [0x80, 0x00, 0x80, 0xff]),
    ("red", [0xff, 0x00, 0x00, 0xff]),
    ("silver", [0xc0, 0xc0, 0xc0, 0xff]),
    ("teal", [0x00, 0x80, 0x80, 0xff]),
    ("white", [0xff, 0xff, 0xff, 0xff]),
    ("yellow", [0xff, 0xff, 0x00, 0xff]),
];

/// Tags that change nothing we can show (size, material, …) but are still
/// rich text, so they are removed from the displayed text.
const SILENT_TAGS: &[&str] = &[
    "size",
    "material",
    "quad",
    "u",
    "s",
    "sup",
    "sub",
    "mark",
    "font",
    "align",
    "alpha",
    "cspace",
    "indent",
    "line-height",
    "line-indent",
    "link",
    "lowercase",
    "uppercase",
    "smallcaps",
    "margin",
    "mspace",
    "nobr",
    "noparse",
    "page",
    "pos",
    "rotate",
    "space",
    "sprite",
    "style",
    "voffset",
    "width",
    "gradient",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tag {
    Color(Rgba),
    Bold,
    Italic,
    Silent(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Open(Tag),
    /// `</name>`
    Close(String),
}

/// Whether `text` contains any rich-text tag (cheap check before parsing).
pub fn has_markup(text: &str) -> bool {
    // Rich text is present exactly when parsing removes something.
    text.contains('<') && text.contains('>') && plain(text) != text
}

/// Splits `text` into styled spans. Text without tags gives one plain span.
pub fn parse(text: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut stack: Vec<Tag> = Vec::new();
    let mut current = String::new();

    let style = |stack: &[Tag]| {
        let color = stack.iter().rev().find_map(|t| match t {
            Tag::Color(c) => Some(*c),
            _ => None,
        });
        let bold = stack.contains(&Tag::Bold);
        let italic = stack.contains(&Tag::Italic);
        (color, bold, italic)
    };
    let flush = |spans: &mut Vec<Span>, current: &mut String, stack: &[Tag]| {
        if current.is_empty() {
            return;
        }
        let (color, bold, italic) = style(stack);
        let text = std::mem::take(current);
        match spans.last_mut() {
            Some(last) if last.color == color && last.bold == bold && last.italic == italic => {
                last.text.push_str(&text);
            }
            _ => spans.push(Span {
                text,
                color,
                bold,
                italic,
            }),
        }
    };

    let mut rest = text;
    while let Some(start) = rest.find('<') {
        current.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find('>') else {
            current.push_str(tail);
            rest = "";
            break;
        };
        match parse_tag(&tail[1..end]) {
            Some(Token::Open(tag)) => {
                flush(&mut spans, &mut current, &stack);
                stack.push(tag);
                rest = &tail[end + 1..];
            }
            // Close the innermost open tag of that kind (Unity is forgiving
            // about mismatched nesting). A closing tag with nothing to close
            // is just text.
            Some(Token::Close(name)) if stack.iter().any(|t| tag_name(t) == name) => {
                flush(&mut spans, &mut current, &stack);
                if let Some(i) = stack.iter().rposition(|t| tag_name(t) == name) {
                    stack.remove(i);
                }
                rest = &tail[end + 1..];
            }
            Some(Token::Close(_)) | None => {
                // Not rich text: keep the '<' and continue after it.
                current.push('<');
                rest = &tail[1..];
            }
        }
    }
    current.push_str(rest);
    flush(&mut spans, &mut current, &stack);
    if spans.is_empty() {
        spans.push(Span {
            text: String::new(),
            color: None,
            bold: false,
            italic: false,
        });
    }
    spans
}

/// `text` with rich-text tags removed.
pub fn plain(text: &str) -> String {
    if !text.contains('<') {
        return text.to_owned();
    }
    parse(text).into_iter().map(|s| s.text).collect()
}

fn tag_name(tag: &Tag) -> String {
    match tag {
        Tag::Color(_) => "color".to_owned(),
        Tag::Bold => "b".to_owned(),
        Tag::Italic => "i".to_owned(),
        Tag::Silent(name) => name.clone(),
    }
}

/// The inside of `<…>`, or `None` if it is not a rich-text tag.
fn parse_tag(inner: &str) -> Option<Token> {
    let inner = inner.trim();
    if let Some(name) = inner.strip_prefix('/') {
        let name = name.trim().to_ascii_lowercase();
        return is_known(&name).then_some(Token::Close(name));
    }
    let (name, value) = match inner.split_once('=') {
        Some((name, value)) => (
            name.trim().to_ascii_lowercase(),
            Some(unquote(value.trim())),
        ),
        None => (inner.to_ascii_lowercase(), None),
    };
    match (name.as_str(), value) {
        ("b", None) => Some(Token::Open(Tag::Bold)),
        ("i", None) => Some(Token::Open(Tag::Italic)),
        ("color", Some(value)) => parse_color(value).map(|c| Token::Open(Tag::Color(c))),
        (name, _) if SILENT_TAGS.contains(&name) => Some(Token::Open(Tag::Silent(name.to_owned()))),
        _ => None,
    }
}

fn is_known(name: &str) -> bool {
    matches!(name, "b" | "i" | "color") || SILENT_TAGS.contains(&name)
}

fn unquote(value: &str) -> &str {
    let v = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(quote).and_then(|v| v.strip_suffix(quote)) {
            return inner;
        }
    }
    v
}

/// `cyan`, `#0ff`, `#00ffff`, `#00ffff80`.
pub fn parse_color(value: &str) -> Option<Rgba> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let digit = |i: usize| u8::from_str_radix(hex.get(i..=i)?, 16).ok();
        let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            3 | 4 => {
                let a = if hex.len() == 4 { digit(3)? * 17 } else { 0xff };
                Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, a])
            }
            6 => Some([pair(0)?, pair(2)?, pair(4)?, 0xff]),
            8 => Some([pair(0)?, pair(2)?, pair(4)?, pair(6)?]),
            _ => None,
        };
    }
    let lower = value.to_ascii_lowercase();
    NAMED_COLORS
        .iter()
        .find(|(name, _)| *name == lower)
        .map(|(_, rgba)| *rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CYAN: Rgba = [0, 255, 255, 255];

    fn span(text: &str, color: Option<Rgba>) -> Span {
        Span {
            text: text.to_owned(),
            color,
            bold: false,
            italic: false,
        }
    }

    #[test]
    fn quoted_named_color_like_the_editor_console() {
        let spans = parse(
            "[10:15:10.634] <color='cyan'>Starting new race | Stage 01: Race 1 (Sprint)</color>",
        );
        assert_eq!(
            spans,
            [
                span("[10:15:10.634] ", None),
                span("Starting new race | Stage 01: Race 1 (Sprint)", Some(CYAN))
            ]
        );
        assert_eq!(
            plain("[10:15:10.634] <color='cyan'>Starting new race</color>"),
            "[10:15:10.634] Starting new race"
        );
    }

    #[test]
    fn color_value_forms() {
        assert_eq!(parse_color("cyan"), Some(CYAN));
        assert_eq!(parse_color("CYAN"), Some(CYAN));
        assert_eq!(parse_color("#00ffff"), Some(CYAN));
        assert_eq!(parse_color("#0ff"), Some(CYAN));
        assert_eq!(parse_color("#00FFFF80"), Some([0, 255, 255, 0x80]));
        assert_eq!(parse_color("#0ff8"), Some([0, 255, 255, 0x88]));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("notacolor"), None);
        for quoted in [
            r#"<color="red">x</color>"#,
            "<color='red'>x</color>",
            "<color=red>x</color>",
            "<COLOR=#FF0000>x</COLOR>",
        ] {
            assert_eq!(
                parse(quoted),
                [span("x", Some([255, 0, 0, 255]))],
                "{quoted}"
            );
        }
    }

    #[test]
    fn nesting_bold_italic_and_silent_tags() {
        const RED: Option<Rgba> = Some([255, 0, 0, 255]);
        let spans = parse("<b>Boss</b> <color=red>hit <i>for</i> <size=20>99</size></color> dmg");
        let bold_boss = Span {
            bold: true,
            ..span("Boss", None)
        };
        let italic_for = Span {
            italic: true,
            ..span("for", RED)
        };
        assert_eq!(
            spans,
            [
                bold_boss,
                span(" ", None),
                span("hit ", RED),
                italic_for,
                span(" 99", RED), // <size> changes nothing we show; merged
                span(" dmg", None),
            ]
        );
        assert_eq!(
            plain("<b>Boss</b> <color=red>hit <i>for</i> <size=20>99</size></color> dmg"),
            "Boss hit for 99 dmg"
        );
    }

    #[test]
    fn things_that_only_look_like_tags_stay() {
        for text in [
            "List<int> was <null>",
            "TMPro.EditorEventCallbacks/<>c:<Start>b__0_0 ()",
            "if (a < b && c > d)",
            "<color=notacolor>x</color>",
            "unclosed <b",
        ] {
            assert_eq!(plain(text), text, "{text}");
            assert!(!has_markup(text), "{text}");
        }
        assert!(has_markup("a <color=red>b</color>"));
    }

    #[test]
    fn unclosed_and_mismatched_tags_are_forgiving() {
        assert_eq!(
            parse("<color=red>rest of line"),
            [span("rest of line", Some([255, 0, 0, 255]))]
        );
        assert_eq!(
            plain("a</color>b"),
            "a</color>b",
            "nothing to close: it is text"
        );
        assert_eq!(plain(""), "");
    }
}
