//! Parser and canonical printer for shell-style file glob patterns.
//!
//! Grammar (informal):
//!   literal   any char that is not one of  * ? [ ] { } , \  or /
//!   escape    \X                     -> literal X
//!   star      *                      -> matches any run of chars within a path component
//!   double    **                     -> must occupy a whole path component, matches across /
//!   any       ?                      -> matches exactly one char
//!   class     [abc] [a-z] [!abc]     -> character class, optionally negated
//!   alt       {a,b,c}                -> alternation, each branch is itself a sequence
//!   slash     /                      -> path component separator
//!
//! `parse` builds an AST and rejects patterns that can't mean anything
//! (unterminated brackets/braces, `**` mixed with other text in one
//! component, empty path components, backwards ranges). `pretty_print`
//! renders a parsed pattern back out in a canonical form: alternation
//! branches are sorted and deduplicated, redundant `**/**` chains are
//! collapsed, and characters are re-escaped only where required.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Literal(String),
    Star,
    DoubleStar,
    Question,
    Slash,
    Class(CharClass),
    Alt(Vec<Vec<Segment>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CharClass {
    pub negated: bool,
    pub items: Vec<ClassItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClassItem {
    Char(char),
    Range(char, char),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub position: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "column {}: {}", self.position + 1, self.message)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, PartialEq)]
pub struct Glob(Vec<Segment>);

impl Glob {
    pub fn segments(&self) -> &[Segment] {
        &self.0
    }

    /// Reports whether `path` matches this pattern.
    ///
    /// Matching is component-by-component, split on `/`. `*` and character
    /// classes never cross a `/`; `**` matches zero or more whole
    /// components. There is no special-casing of leading dots: `*` matches
    /// a component that starts with `.` just like it matches anything else.
    pub fn matches(&self, path: &str) -> bool {
        let components = split_components(&self.0);
        let path_components: Vec<&str> = path.split('/').collect();
        match_components(&components, &path_components)
    }
}

/// A pattern broken at `/` boundaries: either a literal-ish sequence that
/// must match exactly one path component, or a `**` that can stand in for
/// any number of components.
enum Component<'a> {
    One(&'a [Segment]),
    DoubleStar,
}

fn split_components(segs: &[Segment]) -> Vec<Component<'_>> {
    let mut comps = Vec::new();
    let mut start = 0;
    for (i, seg) in segs.iter().enumerate() {
        if *seg == Segment::Slash {
            comps.push(to_component(&segs[start..i]));
            start = i + 1;
        }
    }
    comps.push(to_component(&segs[start..]));
    comps
}

fn to_component(slice: &[Segment]) -> Component<'_> {
    // validate() guarantees `**` never shares a component with anything
    // else, so a single DoubleStar here always fills the whole slice.
    if let [Segment::DoubleStar] = slice {
        Component::DoubleStar
    } else {
        Component::One(slice)
    }
}

fn match_components(pat: &[Component], path: &[&str]) -> bool {
    match pat.first() {
        None => path.is_empty(),
        Some(Component::DoubleStar) => (0..=path.len())
            .any(|skip| match_components(&pat[1..], &path[skip..])),
        Some(Component::One(segs)) => match path.first() {
            Some(head) => {
                match_component(segs, head) && match_components(&pat[1..], &path[1..])
            }
            None => false,
        },
    }
}

fn match_component(segs: &[Segment], text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    match_seq(segs, &chars)
}

fn match_seq(segs: &[Segment], chars: &[char]) -> bool {
    match segs.first() {
        None => chars.is_empty(),
        Some(Segment::Literal(lit)) => {
            let lit_chars: Vec<char> = lit.chars().collect();
            chars.len() >= lit_chars.len()
                && chars[..lit_chars.len()] == lit_chars[..]
                && match_seq(&segs[1..], &chars[lit_chars.len()..])
        }
        Some(Segment::Star) => (0..=chars.len()).any(|skip| match_seq(&segs[1..], &chars[skip..])),
        Some(Segment::Question) => !chars.is_empty() && match_seq(&segs[1..], &chars[1..]),
        Some(Segment::Class(class)) => {
            !chars.is_empty() && class_matches(class, chars[0]) && match_seq(&segs[1..], &chars[1..])
        }
        Some(Segment::Alt(branches)) => branches.iter().any(|branch| {
            let mut combined = branch.clone();
            combined.extend_from_slice(&segs[1..]);
            match_seq(&combined, chars)
        }),
        // Slash and DoubleStar never appear inside a Component::One slice:
        // split_components() cuts on both of them.
        Some(Segment::Slash) | Some(Segment::DoubleStar) => unreachable!(),
    }
}

fn class_matches(class: &CharClass, c: char) -> bool {
    let hit = class.items.iter().any(|item| match item {
        ClassItem::Char(ch) => *ch == c,
        ClassItem::Range(a, b) => *a <= c && c <= *b,
    });
    hit != class.negated
}

impl fmt::Display for Glob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&render(&self.0))
    }
}

pub fn parse(pattern: &str) -> Result<Glob, ParseError> {
    let mut p = Parser::new(pattern);
    let segs = p.parse_sequence(false)?;
    if let Some(c) = p.peek() {
        return Err(p.err(format!("unexpected '{}'", c)));
    }
    validate(&segs)?;
    Ok(Glob(normalize(segs)))
}

pub fn pretty_print(pattern: &str) -> Result<String, ParseError> {
    Ok(parse(pattern)?.to_string())
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(s: &str) -> Self {
        Parser {
            chars: s.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError {
            position: self.pos,
            message: msg.into(),
        }
    }

    fn parse_sequence(&mut self, in_alt: bool) -> Result<Vec<Segment>, ParseError> {
        let mut segs = Vec::new();
        let mut literal = String::new();
        macro_rules! flush {
            () => {
                if !literal.is_empty() {
                    segs.push(Segment::Literal(std::mem::take(&mut literal)));
                }
            };
        }
        while let Some(c) = self.peek() {
            if in_alt && (c == ',' || c == '}') {
                break;
            }
            match c {
                '\\' => {
                    self.bump();
                    match self.bump() {
                        Some(esc) => literal.push(esc),
                        None => return Err(self.err("dangling escape at end of pattern")),
                    }
                }
                '*' => {
                    flush!();
                    self.bump();
                    if self.peek() == Some('*') {
                        self.bump();
                        segs.push(Segment::DoubleStar);
                    } else {
                        segs.push(Segment::Star);
                    }
                }
                '?' => {
                    flush!();
                    self.bump();
                    segs.push(Segment::Question);
                }
                '/' => {
                    flush!();
                    self.bump();
                    segs.push(Segment::Slash);
                }
                '[' => {
                    flush!();
                    segs.push(Segment::Class(self.parse_class()?));
                }
                '{' => {
                    flush!();
                    self.bump();
                    segs.push(Segment::Alt(self.parse_alt()?));
                }
                _ => {
                    self.bump();
                    literal.push(c);
                }
            }
        }
        flush!();
        Ok(segs)
    }

    fn parse_alt(&mut self) -> Result<Vec<Vec<Segment>>, ParseError> {
        let mut branches = Vec::new();
        loop {
            let branch = self.parse_sequence(true)?;
            branches.push(branch);
            match self.bump() {
                Some(',') => continue,
                Some('}') => break,
                _ => return Err(self.err("unterminated '{' - missing closing '}'")),
            }
        }
        if branches.iter().all(|b| b.is_empty()) {
            return Err(self.err("empty alternation '{}'"));
        }
        Ok(branches)
    }

    fn parse_class(&mut self) -> Result<CharClass, ParseError> {
        let start = self.pos;
        self.bump(); // consume '['
        let mut negated = false;
        if matches!(self.peek(), Some('!') | Some('^')) {
            negated = true;
            self.bump();
        }
        let mut items = Vec::new();
        let mut first = true;
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError {
                        position: start,
                        message: "unterminated '[' - missing closing ']'".into(),
                    })
                }
                Some(']') if !first => {
                    self.bump();
                    break;
                }
                Some(c) => {
                    first = false;
                    self.bump();
                    let ch = if c == '\\' {
                        match self.bump() {
                            Some(e) => e,
                            None => return Err(self.err("dangling escape in character class")),
                        }
                    } else {
                        c
                    };
                    if self.peek() == Some('-') && self.peek2().is_some() && self.peek2() != Some(']') {
                        self.bump(); // consume '-'
                        let end_c = self.bump().unwrap();
                        if end_c > ch {
                            items.push(ClassItem::Range(ch, end_c));
                        } else if end_c == ch {
                            items.push(ClassItem::Char(ch));
                        } else {
                            return Err(self.err(format!(
                                "invalid range '{}-{}': start is greater than end",
                                ch, end_c
                            )));
                        }
                    } else {
                        items.push(ClassItem::Char(ch));
                    }
                }
            }
        }
        if items.is_empty() {
            return Err(ParseError {
                position: start,
                message: "empty character class '[]'".into(),
            });
        }
        Ok(CharClass { negated, items })
    }
}

fn validate(segs: &[Segment]) -> Result<(), ParseError> {
    for (i, seg) in segs.iter().enumerate() {
        match seg {
            Segment::DoubleStar => {
                let ok_before = i == 0 || segs[i - 1] == Segment::Slash;
                let ok_after = i + 1 == segs.len() || segs[i + 1] == Segment::Slash;
                if !ok_before || !ok_after {
                    return Err(ParseError {
                        position: 0,
                        message: "'**' must occupy a whole path component".into(),
                    });
                }
            }
            Segment::Slash => {
                if i + 1 < segs.len() && segs[i + 1] == Segment::Slash {
                    return Err(ParseError {
                        position: 0,
                        message: "empty path component ('//')".into(),
                    });
                }
            }
            Segment::Alt(branches) => {
                for b in branches {
                    validate(b)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Collapses redundant `**` chains (`**/**` -> `**`) at every nesting level.
fn normalize(segs: Vec<Segment>) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        match &segs[i] {
            Segment::DoubleStar => {
                out.push(Segment::DoubleStar);
                i += 1;
                while i + 1 < segs.len() && segs[i] == Segment::Slash && segs[i + 1] == Segment::DoubleStar {
                    i += 2;
                }
            }
            Segment::Alt(branches) => {
                let normalized = branches.clone().into_iter().map(normalize).collect();
                out.push(Segment::Alt(normalized));
                i += 1;
            }
            other => {
                out.push(other.clone());
                i += 1;
            }
        }
    }
    out
}

fn needs_escape(c: char) -> bool {
    matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | ',' | '\\')
}

fn render(segs: &[Segment]) -> String {
    let mut out = String::new();
    for seg in segs {
        match seg {
            Segment::Literal(s) => {
                for c in s.chars() {
                    if needs_escape(c) {
                        out.push('\\');
                    }
                    out.push(c);
                }
            }
            Segment::Star => out.push('*'),
            Segment::DoubleStar => out.push_str("**"),
            Segment::Question => out.push('?'),
            Segment::Slash => out.push('/'),
            Segment::Class(c) => render_class(c, &mut out),
            Segment::Alt(branches) => {
                let mut rendered: Vec<String> = branches.iter().map(|b| render(b)).collect();
                rendered.sort();
                rendered.dedup();
                out.push('{');
                out.push_str(&rendered.join(","));
                out.push('}');
            }
        }
    }
    out
}

fn render_class(c: &CharClass, out: &mut String) {
    out.push('[');
    if c.negated {
        out.push('!');
    }
    for item in &c.items {
        match item {
            ClassItem::Char(ch) => {
                if *ch == ']' || *ch == '\\' {
                    out.push('\\');
                }
                out.push(*ch);
            }
            ClassItem::Range(a, b) => {
                out.push(*a);
                out.push('-');
                out.push(*b);
            }
        }
    }
    out.push(']');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_plain_pattern() {
        assert_eq!(pretty_print("src/**/*.rs").unwrap(), "src/**/*.rs");
    }

    #[test]
    fn sorts_and_dedups_alternation() {
        assert_eq!(pretty_print("{b,a,a}.txt").unwrap(), "{a,b}.txt");
    }

    #[test]
    fn collapses_redundant_double_star() {
        assert_eq!(pretty_print("a/**/**/b").unwrap(), "a/**/b");
    }

    #[test]
    fn rejects_unterminated_class() {
        assert!(pretty_print("[abc").is_err());
    }

    #[test]
    fn rejects_unterminated_alt() {
        assert!(pretty_print("{a,b").is_err());
    }

    #[test]
    fn rejects_double_star_mixed_with_text() {
        assert!(pretty_print("a**b").is_err());
    }

    #[test]
    fn rejects_backwards_range() {
        assert!(pretty_print("[z-a]").is_err());
    }

    #[test]
    fn rejects_empty_component() {
        assert!(pretty_print("a//b").is_err());
    }

    #[test]
    fn allows_leading_bracket_literal() {
        assert_eq!(pretty_print("[]]").unwrap(), "[\\]]");
    }

    fn matches(pattern: &str, path: &str) -> bool {
        parse(pattern).unwrap().matches(path)
    }

    #[test]
    fn matches_literal_path() {
        assert!(matches("src/lib.rs", "src/lib.rs"));
        assert!(!matches("src/lib.rs", "src/main.rs"));
    }

    #[test]
    fn star_matches_within_one_component() {
        assert!(matches("src/*.rs", "src/lib.rs"));
        assert!(!matches("src/*.rs", "src/sub/lib.rs"));
    }

    #[test]
    fn double_star_matches_any_depth_including_zero() {
        assert!(matches("src/**/*.rs", "src/lib.rs"));
        assert!(matches("src/**/*.rs", "src/a/b/c/lib.rs"));
        assert!(!matches("src/**/*.rs", "src/lib.txt"));
    }

    #[test]
    fn question_matches_exactly_one_char() {
        assert!(matches("a?c", "abc"));
        assert!(!matches("a?c", "ac"));
        assert!(!matches("a?c", "abbc"));
    }

    #[test]
    fn class_and_negated_class() {
        assert!(matches("[abc].txt", "b.txt"));
        assert!(!matches("[abc].txt", "d.txt"));
        assert!(matches("[!abc].txt", "d.txt"));
        assert!(!matches("[!abc].txt", "a.txt"));
    }

    #[test]
    fn alternation_matches_any_branch() {
        assert!(matches("*.{rs,toml}", "Cargo.toml"));
        assert!(matches("*.{rs,toml}", "lib.rs"));
        assert!(!matches("*.{rs,toml}", "README.md"));
    }

    #[test]
    fn star_does_not_special_case_leading_dot() {
        assert!(matches("*.rs", ".rs"));
    }
}
