//! `trek-update.exe`; see the library for what it does. No console window: Trek starts it as it
//! quits, and what it did goes to `update.log`.

#![windows_subsystem = "windows"]

fn main() {
    let log = trek_update::log_file();
    let code = match trek_update::parse(std::env::args_os().skip(1)) {
        Ok(plan) => trek_update::run(&plan, &mut |line| trek_update::append_log(&log, line)),
        Err(e) => {
            trek_update::append_log(&log, &format!("trek-update: {e}"));
            2
        }
    };
    std::process::exit(code);
}
