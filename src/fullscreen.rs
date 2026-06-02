use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{Print, Stylize},
    terminal::{
        self, disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use std::io::{self, Write};
use std::panic;
use std::sync::Once;

static PANIC_HOOK: Once = Once::new();

#[derive(Debug, Clone)]
pub struct FullscreenSnapshot {
    pub version: String,
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub permissions: String,
    pub mode: String,
    pub workspace: String,
    pub branch: String,
    pub repo_state: String,
    pub session: String,
    pub context_files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FullscreenInput {
    Submit(String),
    Exit,
}

#[derive(Debug, Clone)]
struct TranscriptEntry {
    role: &'static str,
    content: String,
}

pub struct FullscreenUi {
    entries: Vec<TranscriptEntry>,
    input: InputBuffer,
    status: String,
    scroll: usize,
    active: bool,
}

impl FullscreenUi {
    pub fn enter() -> Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(Self {
            entries: Vec::new(),
            input: InputBuffer::default(),
            status: "idle".to_string(),
            scroll: 0,
            active: true,
        })
    }

    pub fn push_user(&mut self, value: impl Into<String>) {
        self.push("user", value);
    }

    pub fn push_assistant(&mut self, value: impl Into<String>) {
        self.push("assistant", value);
    }

    pub fn push_system(&mut self, value: impl Into<String>) {
        self.push("system", value);
    }

    pub fn clear_entries(&mut self) {
        self.entries.clear();
        self.scroll = 0;
    }

    pub fn set_status(&mut self, value: impl Into<String>) {
        self.status = value.into();
    }

    pub fn draw(&mut self, snapshot: &FullscreenSnapshot) -> Result<()> {
        let (width, height) = terminal::size().unwrap_or((100, 30));
        let mut stdout = io::stdout();
        queue!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;
        self.draw_header(&mut stdout, snapshot, width)?;
        self.draw_transcript(&mut stdout, width, height)?;
        self.draw_input(&mut stdout, snapshot, width, height)?;
        stdout.flush()?;
        Ok(())
    }

    pub fn read_input(&mut self, snapshot: &FullscreenSnapshot) -> Result<FullscreenInput> {
        self.input.clear();
        loop {
            self.draw(snapshot)?;
            if let Event::Key(key) = event::read()? {
                if let Some(result) = self.handle_key(key) {
                    return Ok(result);
                }
            }
        }
    }

    fn push(&mut self, role: &'static str, value: impl Into<String>) {
        let content = value.into();
        if !content.trim().is_empty() {
            self.entries.push(TranscriptEntry { role, content });
            self.scroll = 0;
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<FullscreenInput> {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(FullscreenInput::Exit)
            }
            KeyCode::Char(ch) => {
                self.input.insert(ch);
                None
            }
            KeyCode::Backspace => {
                self.input.backspace();
                None
            }
            KeyCode::Delete => {
                self.input.delete();
                None
            }
            KeyCode::Left => {
                self.input.move_left();
                None
            }
            KeyCode::Right => {
                self.input.move_right();
                None
            }
            KeyCode::Home => {
                self.input.move_home();
                None
            }
            KeyCode::End => {
                self.input.move_end();
                None
            }
            KeyCode::Esc => {
                if self.input.is_empty() {
                    Some(FullscreenInput::Exit)
                } else {
                    self.input.clear();
                    None
                }
            }
            KeyCode::Enter => {
                let value = self.input.value().trim().to_string();
                if value.is_empty() {
                    None
                } else {
                    Some(FullscreenInput::Submit(value))
                }
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_add(8);
                None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(8);
                None
            }
            KeyCode::Up => {
                self.scroll = self.scroll.saturating_add(1);
                None
            }
            KeyCode::Down => {
                self.scroll = self.scroll.saturating_sub(1);
                None
            }
            _ => None,
        }
    }

    fn draw_header<W: Write>(
        &self,
        stdout: &mut W,
        snapshot: &FullscreenSnapshot,
        width: u16,
    ) -> Result<()> {
        let title = format!(
            " ClaudeCodeX {}  {}:{} ",
            snapshot.version, snapshot.provider, snapshot.model
        );
        let meta = format!(
            " effort {}  mode {}  permissions {} ",
            snapshot.effort, snapshot.mode, snapshot.permissions
        );
        queue!(
            stdout,
            Print(fill_line(&title, width).bold()),
            Print("\r\n"),
            Print(fill_line(&meta, width).dark_grey()),
            Print("\r\n"),
            Print(
                fill_line(
                    &format!(
                        " {}  {} instruction file{}  session {} ",
                        snapshot.workspace,
                        snapshot.context_files,
                        if snapshot.context_files == 1 { "" } else { "s" },
                        snapshot.session
                    ),
                    width
                )
                .dark_grey()
            ),
            Print("\r\n"),
            Print("─".repeat(width as usize).dark_grey())
        )?;
        Ok(())
    }

    fn draw_transcript<W: Write>(&self, stdout: &mut W, width: u16, height: u16) -> Result<()> {
        let top = 4u16;
        let input_height = 4u16;
        let available = height.saturating_sub(top + input_height).max(1);
        let lines = self.transcript_lines(width.saturating_sub(4).max(20) as usize);
        let visible_count = available as usize;
        let end = lines.len().saturating_sub(self.scroll);
        let start = end.saturating_sub(visible_count);
        for (row, line) in lines[start..end].iter().enumerate() {
            queue!(
                stdout,
                MoveTo(0, top + row as u16),
                Print(truncate_to_width(line, width as usize))
            )?;
        }
        Ok(())
    }

    fn draw_input<W: Write>(
        &self,
        stdout: &mut W,
        snapshot: &FullscreenSnapshot,
        width: u16,
        height: u16,
    ) -> Result<()> {
        let y = height.saturating_sub(4);
        let inner_width = width.saturating_sub(2).max(20) as usize;
        let label = format!(" ccx · {} ", snapshot.mode);
        let top = format!(
            "╭{}{}",
            label,
            "─".repeat(inner_width.saturating_sub(label.chars().count()))
        );
        let input_value = self.input.value();
        let input_line = format!("│ {}", truncate_to_width(&input_value, inner_width - 2));
        let bottom = format!("╰{}", "─".repeat(inner_width));
        let footer = format!(
            " {} | {} | {} | {} ",
            self.status, snapshot.branch, snapshot.repo_state, snapshot.session
        );
        queue!(
            stdout,
            MoveTo(0, y),
            Print(fill_line(&top, width).dark_grey()),
            MoveTo(0, y + 1),
            Print(fill_line(&input_line, width).cyan()),
            MoveTo(0, y + 2),
            Print(fill_line(&bottom, width).dark_grey()),
            MoveTo(0, y + 3),
            Print(fill_line(&footer, width).dark_grey()),
            MoveTo(cursor_x(&self.input, width), y + 1)
        )?;
        Ok(())
    }

    fn transcript_lines(&self, width: usize) -> Vec<String> {
        if self.entries.is_empty() {
            return vec![
                "system  Ready. Type /help for commands, /exit to quit, Ctrl+C to leave."
                    .to_string(),
            ];
        }

        let mut lines = Vec::new();
        for entry in &self.entries {
            let prefix = format!("{:<9}", entry.role);
            let wrapped = wrap_text(&entry.content, width.saturating_sub(prefix.len()).max(16));
            for (index, line) in wrapped.iter().enumerate() {
                if index == 0 {
                    lines.push(format!("{prefix}{line}"));
                } else {
                    lines.push(format!("{:<9}{line}", ""));
                }
            }
            lines.push(String::new());
        }
        lines
    }
}

impl Drop for FullscreenUi {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
            self.active = false;
        }
    }
}

#[derive(Debug, Default, Clone)]
struct InputBuffer {
    chars: Vec<char>,
    cursor: usize,
}

impl InputBuffer {
    fn value(&self) -> String {
        self.chars.iter().collect()
    }

    fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    fn insert(&mut self, ch: char) {
        self.chars.insert(self.cursor, ch);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.chars.len();
    }
}

fn cursor_x(input: &InputBuffer, width: u16) -> u16 {
    let max_x = width.saturating_sub(1);
    (2 + input.cursor as u16).min(max_x)
}

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
            previous(info);
        }));
    });
}

fn fill_line(value: &str, width: u16) -> String {
    let width = width as usize;
    let truncated = truncate_to_width(value, width);
    let len = truncated.chars().count();
    if len >= width {
        truncated
    } else {
        format!("{truncated}{}", " ".repeat(width - len))
    }
}

fn truncate_to_width(value: &str, width: usize) -> String {
    value.chars().take(width).collect()
}

fn wrap_text(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for raw_line in value.lines() {
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            let needs_space = !current.is_empty();
            let next_len =
                current.chars().count() + word.chars().count() + usize::from(needs_space);
            if next_len > width && !current.is_empty() {
                lines.push(current);
                current = String::new();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        if current.is_empty() {
            lines.push(String::new());
        } else {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_buffer_edits_at_cursor() {
        let mut input = InputBuffer::default();
        input.insert('a');
        input.insert('c');
        input.move_left();
        input.insert('b');
        assert_eq!(input.value(), "abc");
        input.backspace();
        assert_eq!(input.value(), "ac");
    }

    #[test]
    fn wrap_text_preserves_short_lines() {
        assert_eq!(wrap_text("hello world", 20), vec!["hello world"]);
    }

    #[test]
    fn wrap_text_splits_words_across_lines() {
        assert_eq!(wrap_text("hello world", 7), vec!["hello", "world"]);
    }
}
