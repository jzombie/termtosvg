pub mod anim;
pub mod asciicast;
pub mod config;
pub mod term;
pub mod cli;

pub use anim::CharacterCell;
pub use asciicast::{AsciiCastError, AsciiCastV2Event, AsciiCastV2Header, AsciiCastV2Record, AsciiCastV2Theme};
pub use term::{TimedFrame, TerminalMode};
