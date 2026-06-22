// 防止 Windows release 弹出控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    xwjd_export_app_lib::run()
}
