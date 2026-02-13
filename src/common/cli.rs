use std::path::PathBuf;
use std::process::Command;
use crate::paths::config_dir;

/// Check if a process is running and return its PID
pub fn get_pid(pidfile: &str) -> Option<i32> {
    std::fs::read_to_string(pidfile).ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .filter(|&pid| unsafe { libc::kill(pid, 0) } == 0)
}

/// Show config directory contents
pub fn cmd_config(app_name: &str) {
    let dir = config_dir(app_name);
    if dir.exists() {
        println!("{}", dir.display());
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                println!("  {}", e.file_name().to_string_lossy());
            }
        }
    } else {
        println!("Config directory does not exist: {}", dir.display());
        println!("Run '{} --generate-config' to create it.", app_name);
    }
}

/// Generate default config files
pub fn cmd_generate_config(app_name: &str, default_css: &str, default_config: &str) {
    let dir = config_dir(app_name);
    std::fs::create_dir_all(&dir).expect("failed to create config dir");
    for (name, content) in [("style.css", default_css), ("config", default_config)] {
        let p = dir.join(name);
        if p.exists() {
            println!("{} already exists at {}", name, p.display());
        } else {
            let _ = std::fs::write(&p, content);
            println!("Created {}", p.display());
        }
    }
    println!("Config directory: {}", dir.display());
}

/// Reload daemon (kill existing + spawn new)
pub fn cmd_reload(app_name: &str, pidfile: &str) {
    let exe = std::env::current_exe().expect("cannot find self");
    if let Some(pid) = get_pid(pidfile) {
        unsafe { libc::kill(pid, libc::SIGTERM) };
        for _ in 0..20 {
            if unsafe { libc::kill(pid, 0) } != 0 { break; }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = std::fs::remove_file(pidfile);
    }
    let _ = Command::new(&exe)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    println!("{} reloaded", app_name);
}

/// Write PID file
pub fn write_pid(pidfile: &str) {
    let _ = std::fs::write(pidfile, std::process::id().to_string());
}

/// Remove PID file
pub fn remove_pid(pidfile: &str) {
    let _ = std::fs::remove_file(pidfile);
}

/// Get pidfile path for an app
pub fn pidfile_path(app_name: &str) -> String {
    format!("/tmp/{}-{}.pid", app_name, unsafe { libc::getuid() })
}

