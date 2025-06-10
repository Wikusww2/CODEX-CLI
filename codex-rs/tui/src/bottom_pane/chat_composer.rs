use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Alignment;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::BorderType;
use ratatui::widgets::Borders;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;
use tui_textarea::Input;
use tui_textarea::Key;
use tui_textarea::TextArea;

use super::chat_composer_history::ChatComposerHistory;
use super::command_popup::CommandPopup;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;

/// Minimum number of visible text rows inside the textarea.
const MIN_TEXTAREA_ROWS: usize = 1;
/// Rows consumed by the border.
const BORDER_LINES: u16 = 2;

/// Result returned when the user interacts with the text area.
pub enum InputResult {
    Submitted(String),
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::mpsc;

    #[test]
    fn double_esc_clears_input() {
        let (tx, _rx) = mpsc::channel();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(true, sender);
        for ch in "hello".chars() {
            composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(composer.textarea.lines().join(""), "hello");
        composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        // first esc should not clear
        assert_eq!(composer.textarea.lines().join(""), "hello");
        composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(composer.textarea.lines().join(""), "");
    }

    #[test]
    fn esc_other_key_resets_arm() {
        let (tx, _rx) = mpsc::channel();
        let sender = AppEventSender::new(tx);
        let mut composer = ChatComposer::new(true, sender);
        for ch in "bye".chars() {
            composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        composer.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(composer.textarea.lines().join(""), "byex");
        composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(composer.textarea.lines().join(""), "");
    }
}

pub(crate) struct ChatComposer<'a> {
    textarea: TextArea<'a>,
    command_popup: Option<CommandPopup>,
    app_event_tx: AppEventSender,
    history: ChatComposerHistory,
    escape_armed: bool,
}

impl ChatComposer<'_> {
    pub fn new(has_input_focus: bool, app_event_tx: AppEventSender) -> Self {
        let mut textarea = TextArea::default();
        textarea.set_placeholder_text("send a message");
        textarea.set_cursor_line_style(ratatui::style::Style::default());

        let mut this = Self {
            textarea,
            command_popup: None,
            app_event_tx,
            history: ChatComposerHistory::new(),
            escape_armed: false,
        };
        this.update_border(has_input_focus);
        this
    }

    /// Record the history metadata advertised by `SessionConfiguredEvent` so
    /// that the composer can navigate cross-session history.
    pub(crate) fn set_history_metadata(&mut self, log_id: u64, entry_count: usize) {
        self.history.set_metadata(log_id, entry_count);
    }

    /// Integrate an asynchronous response to an on-demand history lookup. If
    /// the entry is present and the offset matches the current cursor we
    /// immediately populate the textarea.
    pub(crate) fn on_history_entry_response(
        &mut self,
        log_id: u64,
        offset: usize,
        entry: Option<String>,
    ) -> bool {
        self.history
            .on_entry_response(log_id, offset, entry, &mut self.textarea)
    }

    pub fn set_input_focus(&mut self, has_focus: bool) {
        self.update_border(has_focus);
    }

    /// Handle a key event coming from the main UI.
    pub fn handle_key_event(&mut self, key_event: KeyEvent) -> (InputResult, bool) {
        let result = match self.command_popup {
            Some(_) => self.handle_key_event_with_popup(key_event),
            None => self.handle_key_event_without_popup(key_event),
        };

        // Update (or hide/show) popup after processing the key.
        self.sync_command_popup();

        result
    }

    /// Handle key event when the slash-command popup is visible.
    fn handle_key_event_with_popup(&mut self, key_event: KeyEvent) -> (InputResult, bool) {
        let Some(popup) = self.command_popup.as_mut() else {
            tracing::error!("handle_key_event_with_popup called without an active popup");
            return (InputResult::None, false);
        };

        match key_event.into() {
            Input { key: Key::Up, .. } => {
                popup.move_up();
                (InputResult::None, true)
            }
            Input { key: Key::Down, .. } => {
                popup.move_down();
                (InputResult::None, true)
            }
            Input { key: Key::Tab, .. } => {
                if let Some(cmd) = popup.selected_command() {
                    let first_line = self
                        .textarea
                        .lines()
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("");

                    let starts_with_cmd = first_line
                        .trim_start()
                        .starts_with(&format!("/{}", cmd.command()));

                    if !starts_with_cmd {
                        self.textarea.select_all();
                        self.textarea.cut();
                        let _ = self.textarea.insert_str(format!("/{} ", cmd.command()));
                    }
                }
                (InputResult::None, true)
            }
            Input {
                key: Key::Enter,
                shift: false,
                alt: false,
                ctrl: false,
            } => {
                if let Some(cmd) = popup.selected_command() {
                    // Send command to the app layer.
                    self.app_event_tx.send(AppEvent::DispatchCommand(*cmd));

                    // Clear textarea so no residual text remains.
                    self.textarea.select_all();
                    self.textarea.cut();

                    // Hide popup since the command has been dispatched.
                    self.command_popup = None;
                    return (InputResult::None, true);
                }
                // Fallback to default newline handling if no command selected.
                self.handle_key_event_without_popup(key_event)
            }
            input => self.handle_input_basic(input),
        }
    }

    /// Handle key event when no popup is visible.
    fn handle_key_event_without_popup(&mut self, key_event: KeyEvent) -> (InputResult, bool) {
        let input: Input = key_event.into();
        if input.key == Key::Esc && !input.ctrl && !input.alt && !input.shift {
            if self.escape_armed {
                self.textarea.select_all();
                self.textarea.cut();
                self.escape_armed = false;
            } else {
                self.escape_armed = true;
            }
            return (InputResult::None, true);
        } else {
            self.escape_armed = false;
        }
        match input {
            // -------------------------------------------------------------
            // History navigation (Up / Down) – only when the composer is not
            // empty or when the cursor is at the correct position, to avoid
            // interfering with normal cursor movement.
            // -------------------------------------------------------------
            Input { key: Key::Up, .. } => {
                if self.history.should_handle_navigation(&self.textarea) {
                    let consumed = self
                        .history
                        .navigate_up(&mut self.textarea, &self.app_event_tx);
                    if consumed {
                        return (InputResult::None, true);
                    }
                }
                self.handle_input_basic(input)
            }
            Input { key: Key::Down, .. } => {
                if self.history.should_handle_navigation(&self.textarea) {
                    let consumed = self
                        .history
                        .navigate_down(&mut self.textarea, &self.app_event_tx);
                    if consumed {
                        return (InputResult::None, true);
                    }
                }
                self.handle_input_basic(input)
            }
            Input {
                key: Key::Enter,
                shift: false,
                alt: false,
                ctrl: false,
            } => {
                let text = self.textarea.lines().join("\n");
                self.textarea.select_all();
                self.textarea.cut();

                if text.is_empty() {
                    (InputResult::None, true)
                } else {
                    self.history.record_local_submission(&text);
                    (InputResult::Submitted(text), true)
                }
            }
            Input {
                key: Key::Enter, ..
            }
            | Input {
                key: Key::Char('j'),
                ctrl: true,
                alt: false,
                shift: false,
            } => {
                self.textarea.insert_newline();
                (InputResult::None, true)
            }
            input => self.handle_input_basic(input),
        }
    }

    /// Handle generic Input events that modify the textarea content.
    fn handle_input_basic(&mut self, input: Input) -> (InputResult, bool) {
        self.textarea.input(input);
        (InputResult::None, true)
    }

    /// Synchronize `self.command_popup` with the current text in the
    /// textarea. This must be called after every modification that can change
    /// the text so the popup is shown/updated/hidden as appropriate.
    fn sync_command_popup(&mut self) {
        // Inspect only the first line to decide whether to show the popup. In
        // the common case (no leading slash) we avoid copying the entire
        // textarea contents.
        let first_line = self
            .textarea
            .lines()
            .first()
            .map(|s| s.as_str())
            .unwrap_or("");

        if first_line.starts_with('/') {
            // Create popup lazily when the user starts a slash command.
            let popup = self.command_popup.get_or_insert_with(CommandPopup::new);

            // Forward *only* the first line since `CommandPopup` only needs
            // the command token.
            popup.on_composer_text_change(first_line.to_string());
        } else if self.command_popup.is_some() {
            // Remove popup when '/' is no longer the first character.
            self.command_popup = None;
        }
    }

    pub fn calculate_required_height(&self, area: &Rect) -> u16 {
        let rows = self.textarea.lines().len().max(MIN_TEXTAREA_ROWS);
        let num_popup_rows = if let Some(popup) = &self.command_popup {
            popup.calculate_required_height(area)
        } else {
            0
        };

        rows as u16 + BORDER_LINES + num_popup_rows
    }

    fn update_border(&mut self, has_focus: bool) {
        struct BlockState {
            right_title: Line<'static>,
            border_style: Style,
        }

        let bs = if has_focus {
            BlockState {
                right_title: Line::from("Enter to send | Esc Esc to clear | Ctrl+D to quit | Ctrl+J for newline")
                    .alignment(Alignment::Right),
                border_style: Style::default(),
            }
        } else {
            BlockState {
                right_title: Line::from(""),
                border_style: Style::default().dim(),
            }
        };

        self.textarea.set_block(
            ratatui::widgets::Block::default()
                .title_bottom(bs.right_title)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(bs.border_style),
        );
    }

    pub(crate) fn is_command_popup_visible(&self) -> bool {
        self.command_popup.is_some()
    }
}

impl WidgetRef for &ChatComposer<'_> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        if let Some(popup) = &self.command_popup {
            let popup_height = popup.calculate_required_height(&area);

            // Split the provided rect so that the popup is rendered at the
            // *top* and the textarea occupies the remaining space below.
            let popup_rect = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: popup_height.min(area.height),
            };

            let textarea_rect = Rect {
                x: area.x,
                y: area.y + popup_rect.height,
                width: area.width,
                height: area.height.saturating_sub(popup_rect.height),
            };

            popup.render(popup_rect, buf);
            self.textarea.render(textarea_rect, buf);
        } else {
            self.textarea.render(area, buf);
        }
    }
}
