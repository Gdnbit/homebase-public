// 防止 Windows release 模式下显示 console window
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    homebase_lib::run()
}
