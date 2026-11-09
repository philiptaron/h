//! The spinner drawn on the terminal while completion waits for GitHub.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

const UNICODE_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const ASCII_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// A spinner and a message drawn just after the cursor, and wiped when it is done.
///
/// The cursor is saved and restored around each frame, and line wrapping is off while one is
/// drawn, so however near the edge of the terminal the cursor is, nothing scrolls or moves: the
/// shell, which knows nothing of the spinner, finds its line as it left it. That holds only when
/// nothing follows the cursor on the line, since wiping clears to the end of it; the completion
/// functions ask for a spinner only then.
pub struct Spinner<W: Write> {
    out: W,
    message: String,
    frames: &'static [&'static str],
    frame: usize,
    drawn: bool,
}

impl<W: Write> Spinner<W> {
    /// A spinner showing `message` on `out`, drawn with braille dots when `unicode` and with
    /// `|/-\` otherwise. Nothing is drawn until the first [`Spinner::tick`].
    pub fn new(out: W, message: &str, unicode: bool) -> Spinner<W> {
        let frames = if unicode { UNICODE_FRAMES } else { ASCII_FRAMES };
        let message = if unicode { message.to_string() } else { message.replace('…', "...") };
        Spinner { out, message, frames, frame: 0, drawn: false }
    }

    /// Draw the next frame. The terminal going away is no reason to stop waiting, so failures to
    /// write are let be.
    pub fn tick(&mut self) {
        let frame = self.frames[self.frame % self.frames.len()];
        self.frame += 1;
        let drawn = write!(self.out, "\x1b7\x1b[?7l {frame} {}\x1b[?7h\x1b8", self.message);
        self.drawn |= drawn.is_ok();
        let _ = self.out.flush();
    }

    /// Wipe whatever was drawn, clearing from the cursor to the end of the line.
    pub fn wipe(&mut self) {
        if std::mem::take(&mut self.drawn) {
            let _ = self.out.write_all(b"\x1b[K");
            let _ = self.out.flush();
        }
    }
}

impl<W: Write> Drop for Spinner<W> {
    fn drop(&mut self) {
        self.wipe();
    }
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn note_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Catch Ctrl-C from now on instead of dying of it, so that whoever is waiting with a spinner up
/// can wipe it before stopping, rather than leave it on the screen; see [`is_interrupted`]. The
/// shell, waiting for what completion prints, goes on to its own handling of Ctrl-C only once
/// that is done, so the wipe never lands on its next line.
pub fn catch_interrupts() {
    let handler = note_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: the handler only stores to an atomic, which is safe to do in a signal handler.
    unsafe { libc::signal(libc::SIGINT, handler) };
}

/// Whether Ctrl-C was pressed since [`catch_interrupts`].
pub fn is_interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Whether the locale says the terminal speaks UTF-8, as the first of `LC_ALL`, `LC_CTYPE` and
/// `LANG` that is set does.
pub fn is_utf8_locale() -> bool {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|var| std::env::var(var).ok().filter(|value| !value.is_empty()));
    locale.is_some_and(|locale| {
        let locale = locale.to_ascii_lowercase();
        locale.contains("utf-8") || locale.contains("utf8")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_frames_in_place_and_wipes_them() {
        let mut out = Vec::new();
        {
            let mut spinner = Spinner::new(&mut out, "asking GitHub…", true);
            spinner.tick();
            spinner.tick();
        }
        let want = "\x1b7\x1b[?7l ⠋ asking GitHub…\x1b[?7h\x1b8\
                    \x1b7\x1b[?7l ⠙ asking GitHub…\x1b[?7h\x1b8\
                    \x1b[K";
        assert_eq!(String::from_utf8(out).unwrap(), want);
    }

    #[test]
    fn speaks_ascii_without_utf8() {
        let mut out = Vec::new();
        Spinner::new(&mut out, "asking GitHub…", false).tick();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\x1b7\x1b[?7l | asking GitHub...\x1b[?7h\x1b8\x1b[K"
        );
    }

    #[test]
    fn wipes_nothing_when_nothing_was_drawn() {
        let mut out = Vec::new();
        drop(Spinner::new(&mut out, "x", true));
        assert!(out.is_empty());
    }
}
