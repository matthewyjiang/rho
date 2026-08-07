use std::fmt::{self, Debug};

use crate::tui::markdown::txm::ParseError;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub struct Style(u32);

impl Debug for Style {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut fmt = f.debug_tuple("Style");

        if self.is_bold() {
            fmt.field(&"bold");
        }
        if self.is_italic() {
            fmt.field(&"italic");
        }
        if self.is_underline() {
            fmt.field(&"underline");
        }
        if self.is_dim() {
            fmt.field(&"dim");
        }
        if self.fg_color() != 0 {
            fmt.field(&self.fg_color());
        }
        if self.bg_color() != 0 {
            fmt.field(&self.bg_color());
        }

        fmt.finish()
    }
}

const BOLD: u32 = 1 << 0;
const ITALIC: u32 = 1 << 1;
const UNDERLINE: u32 = 1 << 2;
const DIM: u32 = 1 << 3;

const FG_MASK: u32 = 0x000FF0;
const BG_MASK: u32 = 0x0FF000;

const LOCK_BOLD: u32 = 1 << 20;
const LOCK_ITALIC: u32 = 1 << 21;
const LOCK_UNDERLINE: u32 = 1 << 22;
const LOCK_DIM: u32 = 1 << 23;
const LOCK_FG: u32 = 1 << 24;
const LOCK_BG: u32 = 1 << 25;

const ATTR_MASK: u32 = BOLD | ITALIC | UNDERLINE | DIM;

impl Style {
    pub const fn new() -> Self {
        Style(0)
    }

    pub const fn bold(mut self) -> Self {
        self.0 |= BOLD | LOCK_BOLD;
        self
    }

    pub const fn italic(mut self) -> Self {
        self.0 |= ITALIC | LOCK_ITALIC;
        self
    }

    pub const fn underline(mut self) -> Self {
        self.0 |= UNDERLINE | LOCK_UNDERLINE;
        self
    }

    pub const fn dim(mut self) -> Self {
        self.0 |= DIM | LOCK_DIM;
        self
    }

    pub const fn un_bold(mut self) -> Self {
        self.0 = (self.0 & !BOLD) | LOCK_BOLD;
        self
    }

    pub const fn un_italic(mut self) -> Self {
        self.0 = (self.0 & !ITALIC) | LOCK_ITALIC;
        self
    }

    pub const fn un_underline(mut self) -> Self {
        self.0 = (self.0 & !UNDERLINE) | LOCK_UNDERLINE;
        self
    }

    pub const fn un_dim(mut self) -> Self {
        self.0 = (self.0 & !DIM) | LOCK_DIM;
        self
    }

    pub const fn fg(mut self, color: Color) -> Self {
        self.0 = (self.0 & !FG_MASK) | ((color.0 as u32) << 4) | LOCK_FG;
        self
    }

    pub const fn bg(mut self, color: Color) -> Self {
        self.0 = (self.0 & !BG_MASK) | ((color.0 as u32) << 12) | LOCK_BG;
        self
    }

    pub const fn is_bold(self) -> bool {
        self.0 & BOLD != 0
    }

    pub const fn is_italic(self) -> bool {
        self.0 & ITALIC != 0
    }

    pub const fn is_underline(self) -> bool {
        self.0 & UNDERLINE != 0
    }

    pub const fn is_dim(self) -> bool {
        self.0 & DIM != 0
    }

    pub const fn fg_color(self) -> u8 {
        ((self.0 & FG_MASK) >> 4) as u8
    }

    pub const fn bg_color(self) -> u8 {
        ((self.0 & BG_MASK) >> 12) as u8
    }

    pub const fn merge(self, other: Style) -> Style {
        let combined_lock = self.0 | other.0;

        let bold = if other.0 & LOCK_BOLD != 0 {
            other.0 & BOLD
        } else if self.0 & LOCK_BOLD != 0 {
            self.0 & BOLD
        } else {
            (self.0 | other.0) & BOLD
        };

        let italic = if other.0 & LOCK_ITALIC != 0 {
            other.0 & ITALIC
        } else if self.0 & LOCK_ITALIC != 0 {
            self.0 & ITALIC
        } else {
            (self.0 | other.0) & ITALIC
        };

        let underline = if other.0 & LOCK_UNDERLINE != 0 {
            other.0 & UNDERLINE
        } else if self.0 & LOCK_UNDERLINE != 0 {
            self.0 & UNDERLINE
        } else {
            (self.0 | other.0) & UNDERLINE
        };

        let dim = if other.0 & LOCK_DIM != 0 {
            other.0 & DIM
        } else if self.0 & LOCK_DIM != 0 {
            self.0 & DIM
        } else {
            (self.0 | other.0) & DIM
        };

        let fg = if other.0 & LOCK_FG != 0 {
            other.0 & FG_MASK
        } else if self.0 & LOCK_FG != 0 {
            self.0 & FG_MASK
        } else {
            let other_fg = other.0 & FG_MASK;
            if other_fg != 0 {
                other_fg
            } else {
                self.0 & FG_MASK
            }
        };

        let bg = if other.0 & LOCK_BG != 0 {
            other.0 & BG_MASK
        } else if self.0 & LOCK_BG != 0 {
            self.0 & BG_MASK
        } else {
            let other_bg = other.0 & BG_MASK;
            if other_bg != 0 {
                other_bg
            } else {
                self.0 & BG_MASK
            }
        };

        let lock_part = combined_lock
            & (LOCK_BOLD | LOCK_ITALIC | LOCK_UNDERLINE | LOCK_DIM | LOCK_FG | LOCK_BG);
        Style(lock_part | bold | italic | underline | dim | fg | bg)
    }

    pub const fn is_empty(self) -> bool {
        self.0 & (ATTR_MASK | FG_MASK | BG_MASK) == 0
    }

    pub fn write_ansi_prefix(&self, f: &mut impl fmt::Write) -> fmt::Result {
        if self.is_empty() {
            return Ok(());
        }

        write!(f, "\x1b[")?;
        let mut first = true;

        if self.is_bold() {
            write!(f, "1")?;
            first = false;
        }
        if self.is_italic() {
            write!(f, "{}3", if first { "" } else { ";" })?;
            first = false;
        }
        if self.is_underline() {
            write!(f, "{}4", if first { "" } else { ";" })?;
            first = false;
        }
        if self.is_dim() {
            write!(f, "{}2", if first { "" } else { ";" })?;
            first = false;
        }

        let fg = self.fg_color();
        let bg = self.bg_color();

        if fg != 0 {
            write!(f, "{}38;5;{}", if first { "" } else { ";" }, fg)?;
            first = false;
        }

        if bg != 0 {
            write!(f, "{}48;5;{}", if first { "" } else { ";" }, bg)?;
        }

        write!(f, "m")?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(u8);

impl Color {
    pub const NONE: Color = Color(0);
    pub const BLACK: Color = Color(1);
    pub const RED: Color = Color(2);
    pub const GREEN: Color = Color(3);
    pub const YELLOW: Color = Color(4);
    pub const BLUE: Color = Color(5);
    pub const MAGENTA: Color = Color(6);
    pub const CYAN: Color = Color(7);
    pub const WHITE: Color = Color(8);
    pub const BRIGHT_BLACK: Color = Color(9);
    pub const BRIGHT_RED: Color = Color(10);
    pub const BRIGHT_GREEN: Color = Color(11);
    pub const BRIGHT_YELLOW: Color = Color(12);
    pub const BRIGHT_BLUE: Color = Color(13);
    pub const BRIGHT_MAGENTA: Color = Color(14);
    pub const BRIGHT_CYAN: Color = Color(15);
    pub const BRIGHT_WHITE: Color = Color(16);

    pub const fn new(n: u8) -> Self {
        Color(n)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

pub fn parse_color(s: &str) -> Result<Color, ParseError> {
    Ok(match s {
        "black" => Color::BLACK,
        "red" => Color::RED,
        "green" => Color::GREEN,
        "yellow" => Color::YELLOW,
        "blue" => Color::BLUE,
        "magenta" => Color::MAGENTA,
        "cyan" => Color::CYAN,
        "white" => Color::WHITE,
        "brightblack" | "gray" | "grey" => Color::BRIGHT_BLACK,
        "brightred" => Color::BRIGHT_RED,
        "brightgreen" => Color::BRIGHT_GREEN,
        "brightyellow" => Color::BRIGHT_YELLOW,
        "brightblue" => Color::BRIGHT_BLUE,
        "brightmagenta" => Color::BRIGHT_MAGENTA,
        "brightcyan" => Color::BRIGHT_CYAN,
        "brightwhite" => Color::BRIGHT_WHITE,

        _ => {
            return Err(ParseError::InvalidColor {
                name: s.to_string(),
            });
        }
    })
}
