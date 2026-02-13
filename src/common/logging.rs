use std::io::Write;
use std::path::PathBuf;

pub const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024;

pub fn log_dir(app_name: &str) -> PathBuf {
    std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or("/tmp".into())).join(".local/state")
        })
        .join(app_name)
}

pub fn log_path(app_name: &str) -> PathBuf {
    log_dir(app_name).join(format!("{}.log", app_name))
}

pub fn log(app_name: &str, msg: &str) {
    let dir = log_dir(app_name);
    let _ = std::fs::create_dir_all(&dir);
    let path = log_path(app_name);
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > MAX_LOG_SIZE {
            let _ = std::fs::rename(&path, dir.join(format!("{}.log.1", app_name)));
        }
    }
    let timestamp = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut buf = [0u8; 64];
        let len = unsafe {
            let t = now as libc::time_t;
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            libc::strftime(
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                c"%Y-%m-%d %H:%M:%S".as_ptr(),
                &tm,
            )
        };
        String::from_utf8_lossy(&buf[..len]).to_string()
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[{}] {}", timestamp, msg);
    }
}
