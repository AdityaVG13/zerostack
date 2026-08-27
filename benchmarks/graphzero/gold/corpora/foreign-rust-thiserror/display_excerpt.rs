// Foreign Rust fixture: thiserror-style Display implementation pattern.
use core::fmt::{self, Display};

pub struct ErrorCode(u16);

impl Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("E")?;
        Display::fmt(&self.0, formatter)
    }
}
