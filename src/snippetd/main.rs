//! launch-snippetd: system-wide snippet expansion (Alfred/Raycast style).
//!
//! Type a snippet's trigger in any app and it is replaced by the content.
//!
//! - Reads key events from /dev/input/event* keyboards (needs the `input`
//!   group). With keyd, physical keyboards are grabbed and everything arrives
//!   via keyd's virtual keyboard, which is read like any other.
//! - Blocks in poll(): 0% CPU idle, no timers, no IPC, no network.
//! - State is a fixed 64-char ring of recent keystrokes: memory is flat.
//! - Expansion: `wtype` erases the trigger and types the content through the
//!   Wayland virtual-keyboard protocol (never the clipboard, never evdev, so
//!   it can't feed back into itself or leak secrets into cliphist).
//! - Hotplug: an inotify watch on /dev/input picks up new keyboards (dock).
//! - Snippets file reloads when its mtime changes (checked at most 1/s).
//!
//! Layout: US QWERTY keycode table (triggers are expected to be ASCII).

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

const BUF_CAP: usize = 64;
const EV_KEY: u16 = 1;

// keycodes (linux/input-event-codes.h)
const KEY_ESC: u16 = 1;
const KEY_BACKSPACE: u16 = 14;
const KEY_TAB: u16 = 15;
const KEY_ENTER: u16 = 28;
const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_LEFTALT: u16 = 56;
const KEY_CAPSLOCK: u16 = 58;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_RIGHTALT: u16 = 100;
const KEY_LEFTMETA: u16 = 125;
const KEY_RIGHTMETA: u16 = 126;

#[repr(C)]
struct InputEvent {
    tv_sec: libc::time_t,
    tv_usec: libc::suseconds_t,
    kind: u16,
    code: u16,
    value: i32,
}

fn log(msg: &str) {
    eprintln!("launch-snippetd: {msg}");
}

/// US layout: keycode -> (unshifted, shifted)
fn keymap() -> HashMap<u16, (char, char)> {
    let rows: [(u16, &str, &str); 4] = [
        (2, "1234567890-=", "!@#$%^&*()_+"),
        (16, "qwertyuiop[]", "QWERTYUIOP{}"),
        (30, "asdfghjkl;'`", "ASDFGHJKL:\"~"),
        (44, "zxcvbnm,./", "ZXCVBNM<>?"),
    ];
    let mut m = HashMap::new();
    for (start, lo, hi) in rows {
        for (i, (a, b)) in lo.chars().zip(hi.chars()).enumerate() {
            m.insert(start + i as u16, (a, b));
        }
    }
    m.insert(43, ('\\', '|'));
    m.insert(57, (' ', ' '));
    m
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

/// EVIOCGBIT(EV_KEY, len): _IOC(_IOC_READ, 'E', 0x20 + EV_KEY, len)
fn eviocgbit_key(len: usize) -> libc::c_ulong {
    ((2u64 << 30) | ((len as u64) << 16) | (0x45u64 << 8) | (0x20 + EV_KEY as u64)) as libc::c_ulong
}

fn is_keyboard(fd: i32) -> bool {
    let mut bits = [0u8; 96]; // KEY_MAX 0x2ff -> 768 bits
    let r = unsafe { libc::ioctl(fd, eviocgbit_key(bits.len()), bits.as_mut_ptr()) };
    if r < 0 {
        return false;
    }
    let has = |code: usize| bits[code / 8] & (1 << (code % 8)) != 0;
    has(30) && has(44) && has(57) // A, Z, space
}

fn open_keyboards(open: &mut HashMap<String, OwnedFd>) {
    let Ok(rd) = std::fs::read_dir("/dev/input") else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("event") || open.contains_key(&name) {
            continue;
        }
        let Ok(path) = std::ffi::CString::new(format!("/dev/input/{name}")) else { continue };
        let fd = unsafe {
            libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC)
        };
        if fd < 0 {
            continue; // EACCES: not in the input group
        }
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        if is_keyboard(fd) {
            open.insert(name, owned);
        } // else: dropped -> closed
    }
}

fn inotify_dev_input() -> Option<OwnedFd> {
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let path = std::ffi::CString::new("/dev/input").ok()?;
    let wd = unsafe { libc::inotify_add_watch(fd, path.as_ptr(), libc::IN_CREATE | libc::IN_ATTRIB) };
    (wd >= 0).then_some(owned)
}

fn drain(fd: i32) {
    let mut buf = [0u8; 4096];
    while unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
}

// ---------------------------------------------------------------------------
// Snippets
// ---------------------------------------------------------------------------

struct Triggers {
    settings: snippets::Settings,
    /// (trigger, content), longest trigger first
    list: Vec<(String, String)>,
    mtime: Option<SystemTime>,
    checked: Instant,
}

impl Triggers {
    fn load() -> Self {
        let mut t = Self {
            settings: snippets::Settings::load(),
            list: Vec::new(),
            mtime: None,
            checked: Instant::now(),
        };
        t.reload();
        t
    }

    fn reload(&mut self) {
        self.settings = snippets::Settings::load();
        self.mtime = std::fs::metadata(&self.settings.file).and_then(|m| m.modified()).ok();
        let all = snippets::load(&self.settings);
        let key = all
            .iter()
            .any(|s| s.encrypted && s.trigger.is_some())
            .then(snippets::key_from_keyring)
            .flatten();
        self.list = all
            .into_iter()
            .filter_map(|s| {
                let trig = s.trigger.clone()?;
                match s.content(key.as_ref()) {
                    Ok(c) => Some((trig, c)),
                    Err(e) => {
                        log(&format!("skipping '{}': {e}", s.name));
                        None
                    }
                }
            })
            .collect();
        self.list.sort_by_key(|(t, _)| std::cmp::Reverse(t.len()));
        log(&format!("{} trigger(s) loaded", self.list.len()));
    }

    /// Reload if the file changed; stat at most once per second.
    fn refresh(&mut self) {
        if self.checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.checked = Instant::now();
        let m = std::fs::metadata(&self.settings.file).and_then(|m| m.modified()).ok();
        if m != self.mtime {
            self.reload();
        }
    }

    fn matching(&self, typed: &str) -> Option<(usize, String)> {
        self.list
            .iter()
            .find(|(t, _)| typed.ends_with(t.as_str()))
            .map(|(t, c)| (t.chars().count(), c.clone()))
    }
}

/// Erase the trigger, then type the content. Runs on its own thread so the
/// event loop never blocks on wtype.
fn expand(erase: usize, content: String) {
    std::thread::spawn(move || {
        let mut erase_cmd = Command::new("wtype");
        for _ in 0..erase {
            erase_cmd.args(["-k", "BackSpace"]);
        }
        if let Err(e) = erase_cmd.status() {
            log(&format!("wtype: {e} (install wtype)"));
            return;
        }
        if let Ok(mut c) = Command::new("wtype").arg("-").stdin(Stdio::piped()).spawn() {
            if let Some(mut si) = c.stdin.take() {
                let _ = si.write_all(content.as_bytes());
            }
            let _ = c.wait();
        }
    });
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

fn main() {
    let mut triggers = Triggers::load();
    if !triggers.settings.enabled || !triggers.settings.expand {
        log("disabled ([snippets] enabled = true and expand = true to turn on)");
        return;
    }

    let keys = keymap();
    let mut devices: HashMap<String, OwnedFd> = HashMap::new();
    open_keyboards(&mut devices);
    if devices.is_empty() {
        log("no readable keyboards: add your user to the `input` group and log in again");
    }
    let watch = inotify_dev_input();

    let mut typed = String::with_capacity(BUF_CAP * 4);
    let mut shift = false;
    let mut caps = false;
    let mut mods_down = 0u8; // ctrl/alt/meta held -> shortcuts, not text

    let mut fds: Vec<libc::pollfd> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut dirty = true; // rebuild the poll set only when devices change
    let mut ev = InputEvent { tv_sec: 0, tv_usec: 0, kind: 0, code: 0, value: 0 };
    let ev_size = std::mem::size_of::<InputEvent>();

    loop {
        if dirty {
            dirty = false;
            fds.clear();
            names.clear();
            for (name, fd) in &devices {
                fds.push(libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 });
                names.push(name.clone());
            }
            if let Some(w) = &watch {
                fds.push(libc::pollfd { fd: w.as_raw_fd(), events: libc::POLLIN, revents: 0 });
            }
        }

        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if n < 0 {
            continue; // EINTR
        }

        let mut dead: Vec<String> = Vec::new(); // empty Vec: no allocation unless a device dies
        for (i, p) in fds.iter().enumerate() {
            if p.revents == 0 {
                continue;
            }
            if i >= names.len() {
                // /dev/input changed: (re)open keyboards (udev sets perms
                // after create, hence IN_ATTRIB too)
                drain(p.fd);
                std::thread::sleep(Duration::from_millis(200));
                let before = devices.len();
                open_keyboards(&mut devices);
                dirty |= devices.len() != before;
                continue;
            }
            if p.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                dead.push(names[i].clone());
                continue;
            }
            loop {
                let r = unsafe { libc::read(p.fd, (&mut ev as *mut InputEvent).cast(), ev_size) };
                if r != ev_size as isize {
                    if r < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENODEV) {
                        dead.push(names[i].clone());
                    }
                    break;
                }
                if ev.kind != EV_KEY {
                    continue;
                }
                let (code, value) = (ev.code, ev.value);
                match code {
                    KEY_LEFTSHIFT | KEY_RIGHTSHIFT => {
                        shift = value != 0;
                        continue;
                    }
                    KEY_LEFTCTRL | KEY_RIGHTCTRL | KEY_LEFTALT | KEY_RIGHTALT | KEY_LEFTMETA
                    | KEY_RIGHTMETA => {
                        if value == 1 {
                            mods_down = mods_down.saturating_add(1);
                        } else if value == 0 {
                            mods_down = mods_down.saturating_sub(1);
                        }
                        typed.clear();
                        continue;
                    }
                    _ => {}
                }
                if value == 0 {
                    continue; // release
                }
                match code {
                    KEY_CAPSLOCK => {
                        if value == 1 {
                            caps = !caps;
                        }
                    }
                    KEY_BACKSPACE => {
                        typed.pop();
                    }
                    KEY_ESC | KEY_TAB | KEY_ENTER => typed.clear(),
                    _ if mods_down > 0 => typed.clear(),
                    _ => match keys.get(&code) {
                        Some(&(lo, hi)) => {
                            let upper = if lo.is_ascii_alphabetic() { shift ^ caps } else { shift };
                            typed.push(if upper { hi } else { lo });
                            if typed.chars().count() > BUF_CAP {
                                let cut = typed.char_indices().nth(1).map_or(0, |(i, _)| i);
                                typed.drain(..cut);
                            }
                            triggers.refresh();
                            if let Some((len, content)) = triggers.matching(&typed) {
                                typed.clear();
                                expand(len, content);
                            }
                        }
                        // arrows, function keys, mouse buttons...: cursor moved
                        None => typed.clear(),
                    },
                }
            }
        }
        for name in dead {
            devices.remove(&name); // drop closes the fd
            dirty = true;
        }
    }
}
