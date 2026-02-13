mod app;
mod config;
mod entries;
mod ui;

use gtk4::prelude::*;
use gtk4::Application;
use std::process::Command;

use app::{activate, setup_signals};
use common::cli::{
    cmd_config, cmd_generate_config, cmd_reload, get_pid, pidfile_path, remove_pid, write_pid,
};
use config::{default_config, default_css, APP_NAME};

fn print_usage() {
    eprintln!("{} - clipboard manager\n", APP_NAME);
    eprintln!("Usage:");
    eprintln!("  {}                      Start daemon", APP_NAME);
    eprintln!("  {} toggle               Toggle window", APP_NAME);
    eprintln!("  {} --theme <name>       Preview theme", APP_NAME);
    eprintln!("  {} show-themes          List themes", APP_NAME);
    eprintln!("  {} --config             Show config dir", APP_NAME);
    eprintln!("  {} --generate-config    Create defaults", APP_NAME);
    eprintln!("  {} --reload             Restart daemon", APP_NAME);
    eprintln!("  {} --help               Show help", APP_NAME);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pidfile = pidfile_path(APP_NAME);

    if args.len() > 1 {
        match args[1].as_str() {
            "--help" | "-h" => {
                print_usage();
                return;
            }
            "--config" => {
                cmd_config(APP_NAME);
                return;
            }
            "--generate-config" => {
                cmd_generate_config(APP_NAME, default_css(), default_config());
                return;
            }
            "--reload" => {
                cmd_reload(APP_NAME, &pidfile);
                return;
            }
            "toggle" | "open" => {
                if let Some(pid) = get_pid(&pidfile) {
                    unsafe { libc::kill(pid, libc::SIGUSR1) };
                } else {
                    eprintln!("Daemon not running");
                }
                return;
            }
            "close" => {
                if let Some(pid) = get_pid(&pidfile) {
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                }
                return;
            }
            "show-themes" | "--themes" => {
                println!("Available themes:");
                for (name, _) in common::paths::builtin_themes() {
                    println!("  {}", name);
                }
                return;
            }
            "-T" | "--theme" => {
                if args.len() < 3 {
                    eprintln!("Usage: {} --theme <name>", APP_NAME);
                    return;
                }
                let theme = &args[2];
                if common::paths::get_theme_css(theme).is_none() {
                    eprintln!("Unknown theme: {}", theme);
                    return;
                }
                if let Some(pid) = get_pid(&pidfile) {
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let _ = std::fs::remove_file(&pidfile);
                }
                let exe = std::env::current_exe().expect("cannot find self");
                let _ = Command::new(&exe)
                    .env("GUI_THEME_OVERRIDE", theme)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                println!("Started with theme: {}", theme);
                return;
            }
            other => {
                eprintln!("Unknown option: {}", other);
                print_usage();
                std::process::exit(1);
            }
        }
    }

    if let Some(pid) = get_pid(&pidfile) {
        unsafe { libc::kill(pid, libc::SIGUSR1) };
        return;
    }

    write_pid(&pidfile);

    let app = Application::builder()
        .application_id("com.vib1240n.cliphist-gui")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(|app| {
        activate(app);
        setup_signals(app);
    });

    app.run_with_args::<String>(&[]);
    remove_pid(&pidfile);
}
