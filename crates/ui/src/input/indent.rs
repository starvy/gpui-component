use gpui::{
    Bounds, Context, EntityInputHandler as _, Hsla, Path, PathBuilder, Pixels, SharedString,
    TextRun, TextStyle, Window, point, px,
};
use ropey::RopeSlice;

use crate::{
    RopeExt,
    input::{
        Indent, IndentInline, InputState, LastLayout, Outdent, OutdentInline, element::TextElement,
        mode::InputMode,
    },
};

#[derive(Debug, Copy, Clone)]
pub struct TabSize {
    /// Default is 2
    pub tab_size: usize,
    /// Set true to use `\t` as tab indent, default is false
    pub hard_tabs: bool,
}

impl Default for TabSize {
    fn default() -> Self {
        Self {
            tab_size: 2,
            hard_tabs: false,
        }
    }
}

impl TabSize {
    pub(super) fn to_string(&self) -> SharedString {
        if self.hard_tabs {
            "\t".into()
        } else {
            " ".repeat(self.tab_size).into()
        }
    }

    /// Count the indent size of the line in spaces.
    pub fn indent_count(&self, line: &RopeSlice) -> usize {
        let mut count = 0;
        for ch in line.chars() {
            match ch {
                '\t' => count += self.tab_size,
                ' ' => count += 1,
                _ => break,
            }
        }

        count
    }
}

impl InputState {
    /// Insert a newline at the cursor with bracket-aware auto-indentation (code-editor mode). After
    /// an opening bracket the new line is indented one level deeper; an empty pair `{|}` (cursor
    /// between an opener and its matching closer) is split so the closer drops to its own line with
    /// the cursor on an indented middle line. Otherwise the current line's indentation is carried
    /// over (the prior behavior).
    pub(super) fn insert_newline_with_indent(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cursor = self.cursor();
        let base = self.current_line_indent();
        let unit = self.mode.tab_size().to_string();
        let before = self.char_before_offset(cursor);
        let after = self.char_after_offset(cursor);

        let is_open = matches!(before, Some('{') | Some('[') | Some('('));
        let is_pair = matches!(
            (before, after),
            (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')'))
        );

        // XML/HTML: a `>` that closes an opening tag acts like an opener, and `<a>|</a>` (cursor
        // between an opening tag and its closer) acts like an empty pair — so Enter deepens, and
        // between the tags it splits the closer onto its own line. Gated to markup so generics
        // (`Vec<T>`) and comparisons in other languages are untouched.
        let markup = matches!(self.mode.language(), Some("xml") | Some("html"));
        let tag_open = markup && before == Some('>') && self.tag_before_is_opening(cursor);
        let tag_pair = tag_open && after == Some('<') && self.closing_tag_starts_at(cursor);

        if is_pair || tag_pair {
            let deeper = format!("{base}{unit}");
            let text = format!("\n{deeper}\n{base}");
            self.replace_text_in_range_silent(None, &text, window, cx);
            // Drop the cursor onto the indented middle line.
            let mid = cursor + 1 + deeper.chars().count();
            self.selected_range = (mid..mid).into();
        } else if is_open || tag_open {
            let deeper = format!("{base}{unit}");
            self.replace_text_in_range_silent(None, &format!("\n{deeper}"), window, cx);
        } else {
            let indent = self.indent_of_next_line();
            self.replace_text_in_range_silent(None, &format!("\n{indent}"), window, cx);
        }
    }

    /// True when the `>` immediately before `cursor` closes an *opening* tag — not a closing `</…>`
    /// or a self-closing `<…/>`. Bounded backward scan to the tag's `<` (char-indexed, so it's safe
    /// across multibyte text).
    fn tag_before_is_opening(&self, cursor: usize) -> bool {
        let start = cursor.saturating_sub(512);
        let prefix: String = self.text.slice(start..cursor).chars().collect();
        let Some(inner) = prefix.strip_suffix('>') else {
            return false;
        };
        if inner.ends_with('/') {
            return false; // self-closing
        }
        match inner.rfind('<') {
            // The nearest `<` opens this tag: an opening tag unless it's `</`, and the span up to our
            // `>` must hold no other `>` (else it isn't a single well-formed tag).
            Some(i) => !inner[i..].starts_with("</") && !inner[i + 1..].contains('>'),
            None => false,
        }
    }

    /// True when a closing tag (`</`) begins exactly at `cursor`.
    fn closing_tag_starts_at(&self, cursor: usize) -> bool {
        let mut it = self.text.slice(cursor..).chars();
        it.next() == Some('<') && it.next() == Some('/')
    }

    /// The leading whitespace of the line containing the cursor.
    fn current_line_indent(&self) -> String {
        let start = self.start_of_line();
        let mut indent = String::new();
        for c in self.text.slice(start..).chars() {
            if c == '\n' || c == '\r' || !c.is_whitespace() {
                break;
            }
            indent.push(c);
        }
        indent
    }

    pub(super) fn char_before_offset(&self, offset: usize) -> Option<char> {
        if offset == 0 {
            return None;
        }
        self.text.slice(offset - 1..offset).chars().next()
    }

    pub(super) fn char_after_offset(&self, offset: usize) -> Option<char> {
        // `offset` is the cursor, which never exceeds the text length, so an empty slice → None.
        self.text.slice(offset..).chars().next()
    }
}

impl InputMode {
    #[inline]
    pub(super) fn is_indentable(&self) -> bool {
        match self {
            InputMode::PlainText { multi_line, .. } | InputMode::CodeEditor { multi_line, .. } => {
                *multi_line
            }
            _ => false,
        }
    }

    #[inline]
    pub(super) fn has_indent_guides(&self) -> bool {
        match self {
            InputMode::CodeEditor {
                indent_guides,
                multi_line,
                ..
            } => *indent_guides && *multi_line,
            _ => false,
        }
    }

    #[inline]
    pub(super) fn tab_size(&self) -> TabSize {
        match self {
            InputMode::PlainText { tab, .. } => *tab,
            InputMode::CodeEditor { tab, .. } => *tab,
            _ => TabSize::default(),
        }
    }

    /// The code editor's language name, if this is a code editor.
    #[inline]
    pub(super) fn language(&self) -> Option<&str> {
        match self {
            InputMode::CodeEditor { language, .. } => Some(language.as_ref()),
            _ => None,
        }
    }
}

impl TextElement {
    /// Measure the indent width in pixels for given column count.
    fn measure_indent_width(&self, style: &TextStyle, column: usize, window: &Window) -> Pixels {
        let font_size = style.font_size.to_pixels(window.rem_size());
        let layout = window.text_system().shape_line(
            SharedString::from(" ".repeat(column)),
            font_size,
            &[TextRun {
                len: column,
                font: style.font(),
                color: Hsla::default(),
                background_color: None,
                strikethrough: None,
                underline: None,
            }],
            None,
        );

        layout.width
    }

    pub(super) fn layout_indent_guides(
        &self,
        state: &InputState,
        bounds: &Bounds<Pixels>,
        last_layout: &LastLayout,
        text_style: &TextStyle,
        window: &mut Window,
    ) -> Option<Path<Pixels>> {
        if !state.mode.has_indent_guides() {
            return None;
        }

        let indent_width =
            self.measure_indent_width(text_style, state.mode.tab_size().tab_size, window);

        let tab_size = state.mode.tab_size();
        let line_height = last_layout.line_height;
        let mut builder = PathBuilder::stroke(px(1.));
        let mut offset_y = last_layout.visible_top;
        let mut last_indents = vec![];

        for (&buffer_line, line_layout) in last_layout
            .visible_buffer_lines
            .iter()
            .zip(last_layout.lines.iter())
        {
            let line = state.text.slice_line(buffer_line);
            let mut current_indents = vec![];
            if line.len() > 0 {
                let indent_count = tab_size.indent_count(&line);
                for offset in (0..indent_count).step_by(tab_size.tab_size) {
                    let x = if indent_count > 0 {
                        indent_width * offset as f32 / tab_size.tab_size as f32
                    } else {
                        px(0.)
                    };

                    let pos = point(x + last_layout.line_number_width, offset_y);

                    builder.move_to(pos);
                    builder.line_to(point(pos.x, pos.y + line_height));
                    current_indents.push(pos.x);
                }
            } else if last_indents.len() > 0 {
                for x in &last_indents {
                    let pos = point(*x, offset_y);
                    builder.move_to(pos);
                    builder.line_to(point(pos.x, pos.y + line_height));
                }
                current_indents = last_indents.clone();
            }

            offset_y += line_layout.wrapped_lines.len() * line_height;
            last_indents = current_indents;
        }

        builder.translate(bounds.origin);
        let path = builder.build().unwrap();
        Some(path)
    }
}

impl InputState {
    /// Set whether to show indent guides in code editor mode, default is true.
    ///
    /// Only for [`InputMode::CodeEditor`] mode.
    pub fn indent_guides(mut self, indent_guides: bool) -> Self {
        debug_assert!(self.mode.is_code_editor() && self.mode.is_multi_line());
        if let InputMode::CodeEditor {
            indent_guides: l, ..
        } = &mut self.mode
        {
            *l = indent_guides;
        }
        self
    }

    /// Set indent guides in code editor mode.
    ///
    /// Only for [`InputMode::CodeEditor`] mode.
    pub fn set_indent_guides(
        &mut self,
        indent_guides: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        debug_assert!(self.mode.is_code_editor());
        if let InputMode::CodeEditor {
            indent_guides: l, ..
        } = &mut self.mode
        {
            *l = indent_guides;
        }
        cx.notify();
    }

    /// Set the tab size for the input.
    ///
    /// Only for [`InputMode::PlainText`] and [`InputMode::CodeEditor`] mode with multi_line.
    pub fn tab_size(mut self, tab: TabSize) -> Self {
        debug_assert!(self.mode.is_multi_line() || self.mode.is_code_editor());
        match &mut self.mode {
            InputMode::PlainText { tab: t, .. } => *t = tab,
            InputMode::CodeEditor { tab: t, .. } => *t = tab,
            _ => {}
        }
        self
    }

    /// Change the indentation ([`TabSize`]: width + spaces-vs-tabs) after construction so a settings
    /// change applies live. No-op for modes without indentation. See [`Self::tab_size`].
    pub fn set_tab_size(&mut self, tab: TabSize, cx: &mut Context<Self>) {
        match &mut self.mode {
            InputMode::PlainText { tab: t, .. } => *t = tab,
            InputMode::CodeEditor { tab: t, .. } => *t = tab,
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn indent_inline(
        &mut self,
        _: &IndentInline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // When a completion / code-action menu is open, Tab accepts the selection.
        if self.handle_action_for_context_menu(Box::new(IndentInline), window, cx) {
            return;
        }
        // First, try to accept inline completion if present
        if self.accept_inline_completion(window, cx) {
            return;
        }
        // Single-line inputs don't indent; let Tab fall through to focus navigation.
        if self.mode.is_single_line() {
            cx.propagate();
            return;
        }
        self.indent(false, window, cx);
    }

    pub(super) fn indent_block(&mut self, _: &Indent, window: &mut Window, cx: &mut Context<Self>) {
        self.indent(true, window, cx);
    }

    pub(super) fn outdent_inline(
        &mut self,
        _: &OutdentInline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.outdent(false, window, cx);
    }

    pub(super) fn outdent_block(
        &mut self,
        _: &Outdent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.outdent(true, window, cx);
    }

    pub(super) fn indent(&mut self, block: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.mode.is_indentable() {
            cx.propagate();
            return;
        };

        let tab_indent = self.mode.tab_size().to_string();
        let selected_range = self.selected_range;
        let mut added_len = 0;
        let is_selected = !self.selected_range.is_empty();

        if is_selected || block {
            let start_offset = self.start_of_line_of_selection(window, cx);
            let mut offset = start_offset;

            let selected_text = self
                .text_for_range(
                    self.range_to_utf16(&(offset..selected_range.end)),
                    &mut None,
                    window,
                    cx,
                )
                .unwrap_or("".into());

            for line in selected_text.split('\n') {
                self.replace_text_in_range_silent(
                    Some(self.range_to_utf16(&(offset..offset))),
                    &tab_indent,
                    window,
                    cx,
                );
                added_len += tab_indent.len();
                // +1 for "\n", the `\r` is included in the `line`.
                offset += line.len() + tab_indent.len() + 1;
            }

            if is_selected {
                self.selected_range = (start_offset..selected_range.end + added_len).into();
            } else {
                self.selected_range =
                    (selected_range.start + added_len..selected_range.end + added_len).into();
            }
        } else {
            // Selected none
            let offset = self.selected_range.start;
            self.replace_text_in_range_silent(
                Some(self.range_to_utf16(&(offset..offset))),
                &tab_indent,
                window,
                cx,
            );
            added_len = tab_indent.len();

            self.selected_range =
                (selected_range.start + added_len..selected_range.end + added_len).into();
        }
    }

    pub(super) fn outdent(&mut self, block: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.mode.is_indentable() {
            cx.propagate();
            return;
        };

        let tab_indent = self.mode.tab_size().to_string();
        let selected_range = self.selected_range;
        let mut removed_len = 0;
        let is_selected = !self.selected_range.is_empty();

        if is_selected || block {
            let start_offset = self.start_of_line_of_selection(window, cx);
            let mut offset = start_offset;

            let selected_text = self
                .text_for_range(
                    self.range_to_utf16(&(offset..selected_range.end)),
                    &mut None,
                    window,
                    cx,
                )
                .unwrap_or("".into());

            for line in selected_text.split('\n') {
                if line.starts_with(tab_indent.as_ref()) {
                    self.replace_text_in_range_silent(
                        Some(self.range_to_utf16(&(offset..offset + tab_indent.len()))),
                        "",
                        window,
                        cx,
                    );
                    removed_len += tab_indent.len();

                    // +1 for "\n"
                    offset += line.len().saturating_sub(tab_indent.len()) + 1;
                } else {
                    offset += line.len() + 1;
                }
            }

            if is_selected {
                self.selected_range =
                    (start_offset..selected_range.end.saturating_sub(removed_len)).into();
            } else {
                self.selected_range = (selected_range.start.saturating_sub(removed_len)
                    ..selected_range.end.saturating_sub(removed_len))
                    .into();
            }
        } else {
            // Selected none
            let start_offset = self.selected_range.start;
            let offset = self.start_of_line_of_selection(window, cx);
            let offset = self.offset_from_utf16(self.offset_to_utf16(offset));
            // FIXME: To improve performance
            if self
                .text
                .slice(offset..self.text.len())
                .to_string()
                .starts_with(tab_indent.as_ref())
            {
                self.replace_text_in_range_silent(
                    Some(self.range_to_utf16(&(offset..offset + tab_indent.len()))),
                    "",
                    window,
                    cx,
                );
                removed_len = tab_indent.len();
                let new_offset = start_offset.saturating_sub(removed_len);
                self.selected_range = (new_offset..new_offset).into();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ropey::RopeSlice;

    use super::TabSize;

    #[test]
    fn test_tab_size() {
        let tab = TabSize {
            tab_size: 2,
            hard_tabs: false,
        };
        assert_eq!(tab.to_string(), "  ");
        let tab = TabSize {
            tab_size: 4,
            hard_tabs: false,
        };
        assert_eq!(tab.to_string(), "    ");

        let tab = TabSize {
            tab_size: 2,
            hard_tabs: true,
        };
        assert_eq!(tab.to_string(), "\t");
        let tab = TabSize {
            tab_size: 4,
            hard_tabs: true,
        };
        assert_eq!(tab.to_string(), "\t");
    }

    #[test]
    fn test_tab_size_indent_count() {
        let tab = TabSize {
            tab_size: 4,
            hard_tabs: false,
        };
        assert_eq!(tab.indent_count(&RopeSlice::from("abc")), 0);
        assert_eq!(tab.indent_count(&RopeSlice::from("  abc")), 2);
        assert_eq!(tab.indent_count(&RopeSlice::from("    abc")), 4);
        assert_eq!(tab.indent_count(&RopeSlice::from("\tabc")), 4);
        assert_eq!(tab.indent_count(&RopeSlice::from("  \tabc")), 6);
        assert_eq!(tab.indent_count(&RopeSlice::from(" \t abc  ")), 6);
        assert_eq!(tab.indent_count(&RopeSlice::from("abc")), 0);
    }
}
