use crate::ansi_color::{AnsiColor, ColorSupport};
use crate::highlight::Highlighting;
use crate::input::{InputSeq, KeySeq};
use crate::row::Row;
use crate::signal::SigwinchWatcher;
use std::cmp;
use std::io::{self, Write};
use std::time::SystemTime;
use unicode_width::UnicodeWidthChar;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const HELP: &str = "\
    Ctrl-Q                        : Quit
    Ctrl-S                        : Save to file
    Ctrl-P or UP                  : Move cursor up
    Ctrl-N or DOWN                : Move cursor down
    Ctrl-F or RIGHT               : Move cursor right
    Ctrl-B or LEFT                : Move cursor left
    Ctrl-A or Alt-LEFT or HOME    : Move cursor to head of line
    Ctrl-E or Alt-RIGHT or END    : Move cursor to end of line
    Ctrl-[ or Ctrl-V or PAGE DOWN : Next page
    Ctrl-] or Alt-V or PAGE UP    : Previous page
    Alt-F or Ctrl-RIGHT           : Move cursor to next word
    Alt-B or Ctrl-LEFT            : Move cursor to previous word
    Alt-N or Ctrl-DOWN            : Move cursor to next paragraph
    Alt-P or Ctrl-UP              : Move cursor to previous paragraph
    Alt-<                         : Move cursor to top of file
    Alt->                         : Move cursor to bottom of file
    Ctrl-H or BACKSPACE           : Delete character
    Ctrl-D or DELETE              : Delete next character
    Ctrl-W                        : Delete a word
    Ctrl-U                        : Delete until head of line
    Ctrl-K                        : Delete until end of line
    Ctrl-M                        : New line
    Ctrl-G                        : Search text
    Ctrl-L                        : Refresh screen
    Ctrl-?                        : Show this help";

#[derive(PartialEq)]
enum StatusMessageKind {
    Info,
    Error,
}

struct StatusMessage {
    text: String,
    timestamp: SystemTime,
    kind: StatusMessageKind,
}

impl StatusMessage {
    fn new<S: Into<String>>(message: S, kind: StatusMessageKind) -> StatusMessage {
        StatusMessage {
            text: message.into(),
            timestamp: SystemTime::now(),
            kind,
        }
    }
}

fn get_window_size<I>(input: I) -> io::Result<(usize, usize)>
where
    I: Iterator<Item = io::Result<InputSeq>>,
{
    if let Some(s) = term_size::dimensions_stdout() {
        return Ok(s);
    }

    // By moving cursor at the bottom-right corner by 'B' and 'C' commands, get the size of
    // current screen. \x1b[9999;9999H is not available since it does not guarantee cursor
    // stops on the corner. Finaly command 'n' queries cursor position.
    let mut stdout = io::stdout();
    stdout.write(b"\x1b[9999C\x1b[9999B\x1b[6n")?;
    stdout.flush()?;

    // Wait for response from terminal discarding other sequences
    for seq in input {
        if let KeySeq::Cursor(r, c) = seq?.key {
            return Ok((c, r));
        }
    }

    Ok((0, 0)) // Give up
}

pub struct Screen {
    // X coordinate in `render` text of rows
    rx: usize,
    // Screen size
    num_cols: usize,
    num_rows: usize,
    message: StatusMessage,
    // Dirty line which requires rendering update. After this line must be updated since
    // updating line may affect highlights of succeeding lines
    dirty_start: Option<usize>,
    // Watch resize signal
    sigwinch: SigwinchWatcher,
    // Scroll position (row/col offset)
    pub rowoff: usize,
    pub coloff: usize,
    pub color_support: ColorSupport,
}

impl Screen {
    pub fn new<I>(input: I) -> io::Result<Self>
    where
        I: Iterator<Item = io::Result<InputSeq>>,
    {
        let (w, h) = get_window_size(input)?;
        Ok(Self {
            rx: 0,
            num_cols: w,
            // Screen height is 1 line less than window height due to status bar
            num_rows: h.saturating_sub(2),
            message: StatusMessage::new("Ctrl-? for help", StatusMessageKind::Info),
            dirty_start: Some(0), // Render entire screen at first paint
            sigwinch: SigwinchWatcher::new()?,
            rowoff: 0,
            coloff: 0,
            color_support: ColorSupport::from_env(),
        })
    }

    fn trim_line<'a, S: AsRef<str>>(&self, line: &'a S) -> String {
        let line = line.as_ref();
        if line.len() <= self.coloff {
            return "".to_string();
        }
        line.chars().skip(self.coloff).take(self.num_cols).collect()
    }

    fn draw_status_bar<W: Write>(
        &self,
        mut buf: W,
        num_lines: usize,
        file: &str,
        modified: bool,
        lang_name: &str,
        cy: usize,
    ) -> io::Result<()> {
        write!(buf, "\x1b[{}H", self.num_rows + 1)?;

        buf.write(AnsiColor::Invert.sequence(self.color_support))?;

        let modified = if modified { "(modified) " } else { "" };
        let left = format!("{:<20?} - {} lines {}", file, num_lines, modified);
        // TODO: Handle multi-byte chars correctly
        let left = &left[..cmp::min(left.len(), self.num_cols)];
        buf.write(left.as_bytes())?; // Left of status bar

        let rest_len = self.num_cols - left.len();
        if rest_len == 0 {
            return Ok(());
        }

        let right = format!("{} {}/{}", lang_name, cy, num_lines,);
        if right.len() > rest_len {
            for _ in 0..rest_len {
                buf.write(b" ")?;
            }
            return Ok(());
        }

        for _ in 0..rest_len - right.len() {
            buf.write(b" ")?; // Add spaces at center of status bar
        }
        buf.write(right.as_bytes())?;

        // Defualt argument of 'm' command is 0 so it resets attributes
        buf.write(AnsiColor::Reset.sequence(self.color_support))?;
        Ok(())
    }

    fn draw_message_bar<W: Write>(&self, mut buf: W) -> io::Result<()> {
        write!(buf, "\x1b[{}H", self.num_rows + 2)?;
        if let Ok(d) = SystemTime::now().duration_since(self.message.timestamp) {
            if d.as_secs() < 5 {
                // TODO: Handle multi-byte chars correctly
                let msg = &self.message.text[..cmp::min(self.message.text.len(), self.num_cols)];
                if self.message.kind == StatusMessageKind::Error {
                    buf.write(AnsiColor::RedBG.sequence(self.color_support))?;
                    buf.write(msg.as_bytes())?;
                    buf.write(AnsiColor::Reset.sequence(self.color_support))?;
                } else {
                    buf.write(msg.as_bytes())?;
                }
            }
        }
        buf.write(b"\x1b[K")?;
        Ok(())
    }

    fn draw_welcome_message<W: Write>(&self, mut buf: W) -> io::Result<()> {
        let msg_buf = format!("Kiro editor -- version {}", VERSION);
        let welcome = self.trim_line(&msg_buf);
        let padding = (self.num_cols - welcome.len()) / 2;
        if padding > 0 {
            buf.write(b"~")?;
            for _ in 0..padding - 1 {
                buf.write(b" ")?;
            }
        }
        buf.write(welcome.as_bytes())?;
        Ok(())
    }

    fn draw_rows<W: Write>(&self, mut buf: W, rows: &[Row], hl: &Highlighting) -> io::Result<()> {
        let dirty_start = if let Some(s) = self.dirty_start {
            s
        } else {
            return Ok(());
        };

        let mut prev_color = AnsiColor::Reset;
        let row_len = rows.len();

        buf.write(AnsiColor::Reset.sequence(self.color_support))?;

        for y in 0..self.num_rows {
            let file_row = y + self.rowoff;

            if file_row < dirty_start {
                continue;
            }

            // Move cursor to target line
            write!(buf, "\x1b[{}H", y + 1)?;

            if file_row >= row_len {
                if rows.is_empty() && y == self.num_rows / 3 {
                    self.draw_welcome_message(&mut buf)?;
                } else {
                    if prev_color != AnsiColor::Reset {
                        buf.write(AnsiColor::Reset.sequence(self.color_support))?;
                        prev_color = AnsiColor::Reset;
                    }
                    buf.write(b"~")?;
                }
            } else {
                let row = &rows[file_row];

                let mut col = 0;
                for (c, hl) in row.render_text().chars().zip(hl.lines[file_row].iter()) {
                    col += c.width_cjk().unwrap_or(1);
                    if col <= self.coloff {
                        continue;
                    } else if col > self.num_cols + self.coloff {
                        break;
                    }

                    let color = hl.color();
                    if color != prev_color {
                        if prev_color.is_underlined() {
                            buf.write(AnsiColor::Reset.sequence(self.color_support))?; // Stop underline
                        }
                        buf.write(color.sequence(self.color_support))?;
                        prev_color = color;
                    }

                    write!(buf, "{}", c)?;
                }
            }

            // Erases the part of the line to the right of the cursor. http://vt100.net/docs/vt100-ug/chapter3.html#EL
            buf.write(b"\x1b[K")?;
        }

        if prev_color != AnsiColor::Reset {
            buf.write(AnsiColor::Reset.sequence(self.color_support))?; // Ensure to reset color at end of screen
        }

        Ok(())
    }

    fn redraw_screen(
        &self,
        rows: &[Row],
        file_name: &str,
        modified: bool,
        lang_name: &str,
        cy: usize,
        hl: &Highlighting,
    ) -> io::Result<()> {
        let mut buf = Vec::with_capacity((self.num_rows + 2) * self.num_cols);

        // \x1b[: Escape sequence header
        // Hide cursor while updating screen. 'l' is command to set mode http://vt100.net/docs/vt100-ug/chapter3.html#SM
        buf.write(b"\x1b[?25l")?;
        // H: Command to move cursor. Here \x1b[H is the same as \x1b[1;1H
        buf.write(b"\x1b[H")?;

        self.draw_rows(&mut buf, rows, hl)?;
        self.draw_status_bar(&mut buf, rows.len(), file_name, modified, lang_name, cy)?;
        self.draw_message_bar(&mut buf)?;

        // Move cursor
        let cursor_row = cy - self.rowoff + 1;
        let cursor_col = self.rx - self.coloff + 1;
        write!(buf, "\x1b[{};{}H", cursor_row, cursor_col)?;

        // Reveal cursor again. 'h' is command to reset mode https://vt100.net/docs/vt100-ug/chapter3.html#RM
        buf.write(b"\x1b[?25h")?;

        let mut stdout = io::stdout();
        stdout.write(&buf)?;
        stdout.flush()
    }

    fn next_coloff(&self, want_stop: usize, row: &Row) -> usize {
        let mut coloff = 0;
        for c in row.render_text().chars() {
            coloff += c.width_cjk().unwrap_or(1);
            if coloff >= want_stop {
                // Screen cannot start from at the middle of double-width character
                break;
            }
        }
        coloff
    }

    fn do_scroll(&mut self, rows: &[Row], cx: usize, cy: usize) {
        let prev_rowoff = self.rowoff;
        let prev_coloff = self.coloff;

        // Calculate X coordinate to render considering tab stop
        if cy < rows.len() {
            self.rx = rows[cy].rx_from_cx(cx);
        } else {
            self.rx = 0;
        }

        // Adjust scroll position when cursor is outside screen
        if cy < self.rowoff {
            // Scroll up when cursor is above the top of window
            self.rowoff = cy;
        }
        if cy >= self.rowoff + self.num_rows {
            // Scroll down when cursor is below the bottom of screen
            self.rowoff = cy - self.num_rows + 1;
        }
        if self.rx < self.coloff {
            self.coloff = self.rx;
        }
        if self.rx >= self.coloff + self.num_cols {
            // TODO: coloff must not be in the middle of character. It must be at boundary between characters
            self.coloff = self.next_coloff(self.rx - self.num_cols + 1, &rows[cy]);
        }

        if prev_rowoff != self.rowoff || prev_coloff != self.coloff {
            // If scroll happens, all rows on screen must be updated
            // TODO: Improve rendering on scrolling up/down using scroll region commands \x1b[M/\x1b[D.
            // But scroll down region command was implemented in tmux recently and not included in
            // stable release: https://github.com/tmux/tmux/commit/45f4ff54850ff9b448070a96b33e63451f973e33
            self.set_dirty_start(self.rowoff);
        }
    }

    pub fn refresh(
        &mut self,
        rows: &[Row],
        file_name: &str,
        modified: bool,
        lang_name: &str,
        cursor: (usize, usize),
        hl: &mut Highlighting,
    ) -> io::Result<()> {
        let (cx, cy) = cursor;
        self.do_scroll(rows, cx, cy);
        hl.update(rows, self.rowoff + self.num_rows);
        self.redraw_screen(rows, file_name, modified, lang_name, cy, hl)?;
        self.dirty_start = None;
        Ok(())
    }

    pub fn clear(&self) -> io::Result<()> {
        let mut stdout = io::stdout();
        // 2: Argument of 'J' command to reset entire screen
        // J: Command to erase screen http://vt100.net/docs/vt100-ug/chapter3.html#ED
        stdout.write(b"\x1b[2J")?;
        // Set cursor position to left-top corner
        stdout.write(b"\x1b[H")?;
        stdout.flush()
    }

    pub fn draw_help(&self) -> io::Result<()> {
        let help: Vec<_> = HELP
            .split('\n')
            .skip_while(|s| !s.contains(':'))
            .map(str::trim_start)
            .collect();

        let vertical_margin = if help.len() < self.num_rows {
            (self.num_rows - help.len()) / 2
        } else {
            0
        };
        let help_max_width = help.iter().map(|l| l.len()).max().unwrap();;
        let left_margin = if help_max_width < self.num_cols {
            (self.num_cols - help_max_width) / 2
        } else {
            0
        };

        let mut buf = Vec::with_capacity(self.num_rows * self.num_cols);

        for y in 0..vertical_margin {
            write!(buf, "\x1b[{}H", y + 1)?;
            buf.write(b"\x1b[K")?;
        }

        let left_pad = " ".repeat(left_margin);
        let help_height = cmp::min(vertical_margin + help.len(), self.num_rows);
        for y in vertical_margin..help_height {
            let idx = y - vertical_margin;
            write!(buf, "\x1b[{}H", y + 1)?;
            buf.write(left_pad.as_bytes())?;

            let help = &help[idx][..cmp::min(help[idx].len(), self.num_cols)];
            buf.write(AnsiColor::Cyan.sequence(self.color_support))?;
            let mut cols = help.split(':');
            if let Some(col) = cols.next() {
                buf.write(col.as_bytes())?;
            }
            buf.write(AnsiColor::Reset.sequence(self.color_support))?;
            if let Some(col) = cols.next() {
                write!(buf, ":{}", col)?;
            }

            buf.write(b"\x1b[K")?;
        }

        for y in help_height..self.num_rows {
            write!(buf, "\x1b[{}H", y + 1)?;
            buf.write(b"\x1b[K")?;
        }

        let mut stdout = io::stdout();
        stdout.write(&buf)?;
        stdout.flush()
    }

    pub fn set_dirty_start(&mut self, start: usize) {
        if let Some(s) = self.dirty_start {
            if s < start {
                return;
            }
        }
        self.dirty_start = Some(start);
    }

    pub fn maybe_resize<I>(&mut self, input: I) -> io::Result<bool>
    where
        I: Iterator<Item = io::Result<InputSeq>>,
    {
        if !self.sigwinch.notified() {
            return Ok(false); // Did not receive signal
        }

        let (w, h) = get_window_size(input)?;
        self.num_rows = h.saturating_sub(2);
        self.num_cols = w;
        self.dirty_start = Some(0);
        Ok(true)
    }

    pub fn set_info_message<S: Into<String>>(&mut self, message: S) {
        self.message = StatusMessage::new(message, StatusMessageKind::Info);
    }

    pub fn set_error_message<S: Into<String>>(&mut self, message: S) {
        self.message = StatusMessage::new(message, StatusMessageKind::Error);
    }

    pub fn rows(&self) -> usize {
        self.num_rows
    }

    pub fn cols(&self) -> usize {
        self.num_cols
    }

    pub fn message_text(&self) -> &'_ str {
        self.message.text.as_str()
    }
}
