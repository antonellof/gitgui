//! Terminal session management: raw mode, alternate screen, mode toggles,
//! and restoration on every exit path (drop, panic, signals).

pub mod input;
pub mod kitty;
pub mod probe;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Typed errors for this module.
#[derive(Debug)]
pub enum TermError {
    NotATty,
    Io(io::Error),
}

impl std::fmt::Display for TermError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TermError::NotATty => write!(f, "stdin and stdout must be a terminal"),
            TermError::Io(e) => write!(f, "terminal io error: {e}"),
        }
    }
}

impl std::error::Error for TermError {}

impl From<io::Error> for TermError {
    fn from(e: io::Error) -> Self {
        TermError::Io(e)
    }
}

/// Sequences sent when entering the session (PROTOCOLS section 1).
pub const ENTER_SEQ: &[u8] =
    b"\x1b[?1049h\x1b[?25l\x1b[?1004h\x1b[?1003h\x1b[?1006h\x1b[?1016h\x1b[?2004h\x1b[>15u";

/// Sequences sent when leaving, reverse order, preceded by the delete-all
/// graphics command so nothing leaks into the main screen.
pub const LEAVE_SEQ: &[u8] = b"\x1b_Ga=d,d=A,q=2\x1b\\\x1b[<u\x1b[?2004l\x1b[?1016l\x1b[?1006l\x1b[?1003l\x1b[?1004l\x1b[?25h\x1b[?1049l";

static ORIGINAL: Mutex<Option<libc::termios>> = Mutex::new(None);
static IN_SESSION: AtomicBool = AtomicBool::new(false);

static GOT_SIGWINCH: AtomicBool = AtomicBool::new(false);
static GOT_QUIT_SIGNAL: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigwinch(_: libc::c_int) {
    GOT_SIGWINCH.store(true, Ordering::SeqCst);
}

extern "C" fn on_quit_signal(_: libc::c_int) {
    GOT_QUIT_SIGNAL.store(true, Ordering::SeqCst);
}

/// True once after each SIGWINCH.
pub fn take_sigwinch() -> bool {
    GOT_SIGWINCH.swap(false, Ordering::SeqCst)
}

/// True once SIGINT or SIGTERM arrived.
pub fn quit_requested() -> bool {
    GOT_QUIT_SIGNAL.load(Ordering::SeqCst)
}

pub fn is_tty() -> bool {
    // SAFETY: isatty on constant descriptors.
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 && libc::isatty(libc::STDOUT_FILENO) == 1 }
}

/// Write all bytes to stdout with `write(2)`, no buffering, no locking.
pub fn write_all(mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        // SAFETY: buf is a valid slice.
        let n = unsafe { libc::write(libc::STDOUT_FILENO, buf.as_ptr() as *const _, buf.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        buf = &buf[n as usize..];
    }
    Ok(())
}

/// Wait up to `timeout` for stdin bytes, then read once. Returns 0 on
/// timeout and `UnexpectedEof` when the terminal went away (read returned 0
/// after poll reported the descriptor ready, or POLLHUP).
pub fn read_timeout(buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
    let mut pfd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
    // SAFETY: pfd is a valid pollfd array of length 1.
    let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
    if rc < 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::Interrupted {
            return Ok(0);
        }
        return Err(e);
    }
    if rc == 0 {
        return Ok(0);
    }
    // SAFETY: buf is a valid writable slice.
    let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut _, buf.len()) };
    if n < 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::Interrupted {
            return Ok(0);
        }
        return Err(e);
    }
    if n == 0 {
        // Ready but nothing to read: end of file, the pty is gone.
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stdin closed"));
    }
    Ok(n as usize)
}

fn tcgetattr() -> io::Result<libc::termios> {
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: t is a valid out pointer.
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(t)
}

fn tcsetattr(t: &libc::termios) -> io::Result<()> {
    // SAFETY: t is a valid termios.
    if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Switch stdin to raw mode as specified in PROTOCOLS section 1. Remembers
/// the original settings for [`restore_terminal`].
pub fn enter_raw_mode() -> Result<(), TermError> {
    if !is_tty() {
        return Err(TermError::NotATty);
    }
    let orig = tcgetattr()?;
    let mut raw = orig;
    raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
    raw.c_iflag &= !(libc::IXON | libc::ICRNL | libc::BRKINT | libc::INPCK | libc::ISTRIP);
    raw.c_oflag &= !libc::OPOST;
    raw.c_cc[libc::VMIN] = 0;
    raw.c_cc[libc::VTIME] = 0;
    tcsetattr(&raw)?;
    let mut guard = ORIGINAL.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        *guard = Some(orig);
    }
    Ok(())
}

/// Restore the original termios settings if raw mode was entered.
pub fn leave_raw_mode() {
    let orig = ORIGINAL.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(t) = orig {
        let _ = tcsetattr(&t);
    }
}

/// Restore everything: leave the alternate screen and modes if a session is
/// active, then restore termios. Idempotent, safe to call from a panic hook.
pub fn restore_terminal() {
    if IN_SESSION.swap(false, Ordering::SeqCst) {
        let _ = write_all(LEAVE_SEQ);
    }
    leave_raw_mode();
}

/// Install the panic hook and signal handlers. Call once at startup.
pub fn install_handlers() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
    // SAFETY: handlers only touch atomics.
    unsafe {
        libc::signal(
            libc::SIGWINCH,
            on_sigwinch as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            on_quit_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            on_quit_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGHUP,
            on_quit_signal as *const () as libc::sighandler_t,
        );
    }
}

/// A full interactive session: raw mode plus alternate screen and input
/// modes. Dropping it restores the terminal.
pub struct Session {
    _private: (),
}

impl Session {
    pub fn enter() -> Result<Self, TermError> {
        enter_raw_mode()?;
        write_all(ENTER_SEQ)?;
        IN_SESSION.store(true, Ordering::SeqCst);
        Ok(Session { _private: () })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Raw mode only, without the alternate screen. Used by `--probe`.
pub struct RawGuard {
    _private: (),
}

impl RawGuard {
    pub fn enter() -> Result<Self, TermError> {
        enter_raw_mode()?;
        Ok(RawGuard { _private: () })
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        leave_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_sequence_bytes() {
        assert_eq!(
            ENTER_SEQ,
            b"\x1b[?1049h\x1b[?25l\x1b[?1004h\x1b[?1003h\x1b[?1006h\x1b[?1016h\x1b[?2004h\x1b[>15u"
        );
    }

    #[test]
    fn leave_sequence_is_reverse_of_enter_with_delete_all() {
        assert!(LEAVE_SEQ.starts_with(b"\x1b_Ga=d,d=A,q=2\x1b\\"));
        let s = std::str::from_utf8(LEAVE_SEQ).unwrap();
        let order = [
            "\x1b[<u", "?2004l", "?1016l", "?1006l", "?1003l", "?1004l", "?25h", "?1049l",
        ];
        let mut last = 0;
        for o in order {
            let pos = s[last..].find(o).unwrap_or_else(|| panic!("missing {o:?}")) + last;
            assert!(pos >= last);
            last = pos;
        }
    }

    #[test]
    fn restore_without_session_is_noop() {
        restore_terminal();
        assert!(!IN_SESSION.load(Ordering::SeqCst));
    }
}
