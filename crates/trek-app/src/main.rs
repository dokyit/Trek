// A release build on Windows is a GUI program: no console window behind it. (A dev build keeps
// its console, where the log goes.)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    trek_app::run();
}
