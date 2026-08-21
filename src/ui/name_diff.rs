//! Which characters of a rename actually changed.
//!
//! A rename prints two long names and the reader has to spot the difference.
//! Colouring both whole leaves that job undone, so the shared start and end are
//! trimmed, the longest run the two middles still share becomes an anchor, and
//! each side of it is walked the same way. Two edits to one name therefore stay
//! two marks with the untouched text between them left alone.
//!
//! This is an algorithm, not rendering: it returns which characters are kept.

use super::popups::VERB;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// A change line split into its verb, the name that goes, and the name that
/// arrives if there is one.
pub(super) fn split_change(line: &str) -> (String, String, Option<String>) {
    const ARROW: &str = "  ->  ";
    let verb: String = line.chars().take(VERB).collect();
    let rest: String = line.chars().skip(VERB).collect();
    match rest.split_once(ARROW) {
        Some((from, to)) => (
            verb.trim_end().to_string(),
            from.to_string(),
            Some(to.to_string()),
        ),
        None => (verb.trim_end().to_string(), rest, None),
    }
}

/// A name with the characters that changed blocked out, the rest plain.
pub(super) fn marked(text: &str, kept: &[bool], changed: Color, pan: usize) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, ch) in text.chars().enumerate().skip(pan) {
        let style = if kept.get(i).copied().unwrap_or(false) {
            Style::default()
        } else {
            Style::default()
                .bg(changed)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        };
        match spans.last_mut() {
            Some(Span {
                content,
                style: last,
            }) if *last == style => {
                content.to_mut().push(ch);
            }
            _ => spans.push(Span::styled(ch.to_string(), style)),
        }
    }
    Line::from(spans)
}

pub(super) fn chars_of(s: &str) -> Vec<char> {
    s.chars().collect()
}

/// How many characters must agree in a row before a match means anything.
pub(super) const MIN_ANCHOR: usize = 4;
/// Longest pair worth the quadratic search for an anchor.
pub(super) const ANCHOR_LIMIT: usize = 512;

/// Which characters of each name survive into the other.
///
/// Trims the shared start and end, then anchors on the longest run the two
/// middles still share and recurses either side of it, so two edits to one
/// name stay two marks with the untouched text between them left alone.
///
/// Deliberately not a longest common subsequence. An LCS matches a
/// *subsequence*, so scattered characters count: renaming `Ode To Felix
/// (Ikerya Project Remix)` to `Ode To *testing edits* Felix` let it pair the
/// `ix` of the new `Felix` with the `ix` of the old `Remix`, and `Fel` read as
/// invented text. An anchor is a *substring*: contiguous, and at least
/// `MIN_ANCHOR` long, so a two character coincidence can never become one.
///
/// Two names with nothing in common need no special case: no anchor clears the
/// threshold, so both print as one solid block, which is exactly "this became
/// that".
pub(super) fn common(old: &[char], new: &[char]) -> (Vec<bool>, Vec<bool>) {
    let mut kept_old = vec![false; old.len()];
    let mut kept_new = vec![false; new.len()];
    align(old, new, 0, 0, &mut kept_old, &mut kept_new);
    (kept_old, kept_new)
}

/// Marks what `old` and `new` share, writing into the masks at the offsets the
/// two slices sit at in the whole name.
pub(super) fn align(
    old: &[char],
    new: &[char],
    at_old: usize,
    at_new: usize,
    kept_old: &mut [bool],
    kept_new: &mut [bool],
) {
    let (n, m) = (old.len(), new.len());
    let most = n.min(m);

    let head = (0..most).take_while(|&i| old[i] == new[i]).count();
    // The tail may not eat into the head: with nothing between them there is
    // no change left to show.
    let tail = (0..most - head)
        .take_while(|&k| old[n - 1 - k] == new[m - 1 - k])
        .count();

    kept_old[at_old..at_old + head].fill(true);
    kept_new[at_new..at_new + head].fill(true);
    kept_old[at_old + n - tail..at_old + n].fill(true);
    kept_new[at_new + m - tail..at_new + m].fill(true);

    let (old_mid, new_mid) = (&old[head..n - tail], &new[head..m - tail]);
    if old_mid.is_empty() || new_mid.is_empty() {
        return;
    }

    // What is left is a change on both sides unless they still share a run
    // long enough to mean something, in which case it is really two changes
    // with untouched text between them.
    let Some((o, e, len)) = anchor(old_mid, new_mid) else {
        return;
    };

    let (o_at, n_at) = (at_old + head, at_new + head);
    align(&old_mid[..o], &new_mid[..e], o_at, n_at, kept_old, kept_new);
    kept_old[o_at + o..o_at + o + len].fill(true);
    kept_new[n_at + e..n_at + e + len].fill(true);
    align(
        &old_mid[o + len..],
        &new_mid[e + len..],
        o_at + o + len,
        n_at + e + len,
        kept_old,
        kept_new,
    );
}

/// The longest run of characters the two share, if it is long enough to be
/// worth trusting: where it starts in each, and how long it is.
pub(super) fn anchor(old: &[char], new: &[char]) -> Option<(usize, usize, usize)> {
    if old.len() > ANCHOR_LIMIT || new.len() > ANCHOR_LIMIT {
        return None;
    }

    let mut prev = vec![0usize; new.len() + 1];
    let mut best = (0, 0, 0);
    for (i, o) in old.iter().enumerate() {
        let mut row = vec![0usize; new.len() + 1];
        for (j, e) in new.iter().enumerate() {
            if o == e {
                row[j + 1] = prev[j] + 1;
                if row[j + 1] > best.2 {
                    best = (i + 1 - row[j + 1], j + 1 - row[j + 1], row[j + 1]);
                }
            }
        }
        prev = row;
    }

    (best.2 >= MIN_ANCHOR).then_some(best)
}

/// Colour by what the line would do:/// Colour by what the line would do:/// Colour by what the line would do: green makes something, blue moves or
/// renames it, red takes it away.
pub(super) fn change_style(line: &str) -> Style {
    let colour = match line.split_whitespace().next() {
        Some("save" | "copy") => Color::Green,
        Some("rename" | "move") => Color::Blue,
        Some("delete" | "DELETE") => Color::Red,
        _ => Color::Reset,
    };
    Style::default().fg(colour)
}
