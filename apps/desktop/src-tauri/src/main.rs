// release 版在 Windows 不顯示額外的主控台視窗
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    hammer_desktop_lib::run();
}
