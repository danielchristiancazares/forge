//! Virtual terminal backend for TUI snapshot testing.
//!
//! Wraps a `vt100::Parser` to simulate a real terminal, allowing us to
//! capture rendered TUI output for snapshot comparisons.

use std::fmt;
use std::io::{self, Write};

use ratatui::backend::Backend;
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// A test backend that uses vt100 to simulate a real terminal.
///
/// This captures rendered output and interprets ANSI escape sequences,
/// maintaining screen state for snapshot testing.
pub struct VT100Backend {
    parser: vt100::Parser,
    width: u16,
    height: u16,
}

impl VT100Backend {
    /// Creates a new VT100Backend with the specified dimensions.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            parser: vt100::Parser::new(height, width, 0),
            width,
            height,
        }
    }

    /// Returns a reference to the underlying vt100 parser.
    #[allow(dead_code)]
    pub fn vt100(&self) -> &vt100::Parser {
        &self.parser
    }

    /// Returns the screen contents as a string.
    pub fn contents(&self) -> String {
        self.parser.screen().contents()
    }
}

impl Write for VT100Backend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.parser.process(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl fmt::Display for VT100Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.contents())
    }
}

impl Backend for VT100Backend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        use crossterm::{Command, cursor, style};
        use std::fmt::Write as FmtWrite;

        let mut buf = String::new();
        let mut last_pos: Option<(u16, u16)> = None;
        let mut last_style: Option<ratatui::style::Style> = None;

        for (x, y, cell) in content {
            // Move cursor if needed
            if last_pos != Some((x, y)) {
                let _ = cursor::MoveTo(x, y).write_ansi(&mut buf);
            }

            // Apply style if changed
            let cell_style = cell.style();
            if last_style != Some(cell_style) {
                let _ = style::SetAttribute(style::Attribute::Reset).write_ansi(&mut buf);

                if let Some(fg) = to_crossterm_color(cell_style.fg) {
                    let _ = style::SetForegroundColor(fg).write_ansi(&mut buf);
                }
                if let Some(bg) = to_crossterm_color(cell_style.bg) {
                    let _ = style::SetBackgroundColor(bg).write_ansi(&mut buf);
                }

                last_style = Some(cell_style);
            }

            // Write cell content
            let _ = write!(buf, "{}", cell.symbol());
            last_pos = Some((x + 1, y));
        }

        self.parser.process(buf.as_bytes());
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        let pos = self.parser.screen().cursor_position();
        Ok(Position::new(pos.1, pos.0))
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        use crossterm::{Command, cursor};
        let pos = position.into();
        let mut buf = String::new();
        let _ = cursor::MoveTo(pos.x, pos.y).write_ansi(&mut buf);
        self.parser.process(buf.as_bytes());
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        use crossterm::{Command, terminal};
        let mut buf = String::new();
        let _ = terminal::Clear(terminal::ClearType::All).write_ansi(&mut buf);
        self.parser.process(buf.as_bytes());
        Ok(())
    }

    fn clear_region(&mut self, _clear_type: ratatui::backend::ClearType) -> io::Result<()> {
        // For snapshot testing, we can just clear the whole screen
        self.clear()
    }

    fn size(&self) -> io::Result<Size> {
        Ok(Size::new(self.width, self.height))
    }

    fn window_size(&mut self) -> io::Result<ratatui::backend::WindowSize> {
        Ok(ratatui::backend::WindowSize {
            columns_rows: Size::new(self.width, self.height),
            pixels: Size::new(self.width * 8, self.height * 16), // Approximate pixel size
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn to_crossterm_color(color: Option<ratatui::style::Color>) -> Option<crossterm::style::Color> {
    use crossterm::style::Color as CColor;
    use ratatui::style::Color as RColor;

    match color? {
        RColor::Reset => None,
        RColor::Black => Some(CColor::Black),
        RColor::Red => Some(CColor::DarkRed),
        RColor::Green => Some(CColor::DarkGreen),
        RColor::Yellow => Some(CColor::DarkYellow),
        RColor::Blue => Some(CColor::DarkBlue),
        RColor::Magenta => Some(CColor::DarkMagenta),
        RColor::Cyan => Some(CColor::DarkCyan),
        RColor::Gray => Some(CColor::Grey),
        RColor::DarkGray => Some(CColor::DarkGrey),
        RColor::LightRed => Some(CColor::Red),
        RColor::LightGreen => Some(CColor::Green),
        RColor::LightYellow => Some(CColor::Yellow),
        RColor::LightBlue => Some(CColor::Blue),
        RColor::LightMagenta => Some(CColor::Magenta),
        RColor::LightCyan => Some(CColor::Cyan),
        RColor::White => Some(CColor::White),
        RColor::Rgb(r, g, b) => Some(CColor::Rgb { r, g, b }),
        RColor::Indexed(i) => Some(CColor::AnsiValue(i)),
    }
}
