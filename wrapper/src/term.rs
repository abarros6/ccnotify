//! Raw-mode handling for the real terminal so keystrokes pass through
//! to the pty untouched, plus window-size queries for resize relay.

#[cfg(unix)]
pub struct RawGuard {
    orig: libc::termios,
}

#[cfg(unix)]
impl RawGuard {
    /// Put the controlling terminal into raw mode. Returns None when
    /// stdin is not a tty (e.g. piped input), in which case no mode
    /// change is needed.
    pub fn enable() -> Option<Self> {
        unsafe {
            if libc::isatty(libc::STDIN_FILENO) == 0 {
                return None;
            }
            let mut orig: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut orig) != 0 {
                return None;
            }
            let mut raw = orig;
            libc::cfmakeraw(&mut raw);
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) != 0 {
                return None;
            }
            Some(Self { orig })
        }
    }

    /// Restore the original terminal modes (also runs on Drop; exposed
    /// so we can restore before process::exit skips destructors).
    pub fn restore(&self) {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.orig);
        }
    }
}

#[cfg(unix)]
impl Drop for RawGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Current terminal size as (rows, cols), with an 80x24 fallback.
#[cfg(unix)]
pub fn size() -> (u16, u16) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_row > 0 {
            return (ws.ws_row, ws.ws_col);
        }
    }
    (24, 80)
}

#[cfg(not(unix))]
pub struct RawGuard;

#[cfg(not(unix))]
impl RawGuard {
    pub fn enable() -> Option<Self> {
        None
    }
    pub fn restore(&self) {}
}

#[cfg(not(unix))]
pub fn size() -> (u16, u16) {
    (24, 80)
}
