//! Auto-pairing for the code editor: typing an opener inserts the matching closer (cursor between),
//! a closer typed in front of an auto-inserted one steps over it, Backspace between an empty pair
//! deletes both, and typing an opener with a selection wraps it. Quotes additionally guard against
//! auto-closing mid-word or duplicating.
//!
//! The *mechanism* lives here generically; the *policy* (which pairs apply) is per-language via
//! [`pairs_for_language`] — a sensible default for code, with overrides (e.g. JSON has no `()`).

use gpui::{Context, Window};

use crate::input::InputState;

/// An auto-closing pair. `open == close` marks a symmetric pair (a quote), which gets extra guards.
#[derive(Debug, Clone, Copy)]
pub(super) struct BracketPair {
    pub open: char,
    pub close: char,
}

const fn pair(open: char, close: char) -> BracketPair {
    BracketPair { open, close }
}

/// The default code pair set: brackets + double-quote strings.
const DEFAULT_PAIRS: &[BracketPair] = &[
    pair('{', '}'),
    pair('[', ']'),
    pair('(', ')'),
    pair('"', '"'),
];

/// JSON: objects, arrays, and strings — no parentheses (and no single quotes).
const JSON_PAIRS: &[BracketPair] = &[pair('{', '}'), pair('[', ']'), pair('"', '"')];

/// The auto-pair rules for a language. The default covers code generally; languages refine it.
pub(super) fn pairs_for_language(language: &str) -> &'static [BracketPair] {
    match language {
        "json" => JSON_PAIRS,
        _ => DEFAULT_PAIRS,
    }
}

fn is_word(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

impl InputState {
    /// Try to handle a single typed `ch` as an auto-pair action. Returns `true` if handled (the
    /// caller should not run the normal insert). Only fires for real typed input in a code editor
    /// (the caller guards on `!silent_replace_text` and no IME composition).
    pub(super) fn try_auto_pair(
        &mut self,
        range_utf16: Option<&std::ops::Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.mode.is_code_editor() {
            return false;
        }
        let mut chars = new_text.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return false; // not exactly one char (paste / IME / control)
        };

        let pairs = pairs_for_language(self.mode.language().unwrap_or(""));
        let range = range_utf16
            .map(|r| self.range_from_utf16(r))
            .unwrap_or_else(|| self.selected_range.into());

        // Wrap a non-empty selection: `sel` → `<open>sel<close>`, keeping `sel` selected.
        if range.start != range.end {
            let Some(p) = pairs.iter().find(|p| p.open == ch) else {
                return false;
            };
            let selected = self.text.slice(range.clone()).to_string();
            let wrapped = format!("{}{}{}", p.open, selected, p.close);
            self.replace_text_in_range_silent(Some(self.range_to_utf16(&range)), &wrapped, window, cx);
            let inner_start = range.start + 1;
            let inner_end = inner_start + selected.chars().count();
            self.selected_range = (inner_start..inner_end).into();
            cx.notify();
            return true;
        }

        let at = range.start;
        let next = self.char_after_offset(at);
        let prev = self.char_before_offset(at);

        // Step over a just-typed closer that already sits at the cursor (e.g. typing `}` at `{|}`).
        if pairs.iter().any(|p| p.close == ch && p.open != p.close) && next == Some(ch) {
            self.move_cursor_to(at + 1, cx);
            return true;
        }

        // Quotes (symmetric): step over, guard against mid-word, else auto-close.
        if let Some(p) = pairs.iter().find(|p| p.open == ch && p.open == p.close) {
            if next == Some(ch) {
                self.move_cursor_to(at + 1, cx);
                return true;
            }
            if is_word(prev) || is_word(next) {
                return false; // apostrophe / closing an existing quote — insert a single char
            }
            self.insert_pair(at, *p, window, cx);
            return true;
        }

        // Bracket opener: insert the pair, cursor between.
        if let Some(p) = pairs.iter().find(|p| p.open == ch && p.open != p.close) {
            self.insert_pair(at, *p, window, cx);
            return true;
        }

        false
    }

    fn insert_pair(&mut self, at: usize, p: BracketPair, window: &mut Window, cx: &mut Context<Self>) {
        let text = format!("{}{}", p.open, p.close);
        self.replace_text_in_range_silent(Some(self.range_to_utf16(&(at..at))), &text, window, cx);
        self.move_cursor_to(at + 1, cx);
    }

    fn move_cursor_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = (offset..offset).into();
        cx.notify();
    }

    /// On Backspace with an empty selection between an empty pair (`{|}`, `"|"`), delete both.
    /// Returns `true` if handled.
    pub(super) fn try_delete_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.mode.is_code_editor() || !self.selected_range.is_empty() {
            return false;
        }
        let at = self.cursor();
        let (Some(prev), Some(next)) = (self.char_before_offset(at), self.char_after_offset(at))
        else {
            return false;
        };
        let pairs = pairs_for_language(self.mode.language().unwrap_or(""));
        if pairs.iter().any(|p| p.open == prev && p.close == next) {
            let range = (at - 1)..(at + 1);
            self.replace_text_in_range_silent(Some(self.range_to_utf16(&range)), "", window, cx);
            return true;
        }
        false
    }
}
