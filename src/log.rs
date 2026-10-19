use std::fmt::Display;
use std::io::IsTerminal;
use std::sync::OnceLock;

fn color() -> bool {
    static COLOR: OnceLock<bool> = OnceLock::new();
    *COLOR.get_or_init(|| std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none())
}

fn line(code: &str, msg: impl Display) {
    if color() {
        eprintln!("\x1b[{code}m[rwatch]\x1b[0m {msg}");
    } else {
        eprintln!("[rwatch] {msg}");
    }
}

pub fn info(msg: impl Display) {
    line("32", msg);
}

pub fn warn(msg: impl Display) {
    line("33", msg);
}

pub fn error(msg: impl Display) {
    line("31", msg);
}

pub fn clear_screen() {
    eprint!("\x1b[2J\x1b[3J\x1b[H");
}
