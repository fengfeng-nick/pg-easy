// 避免 Windows 的 release 构建多出一个控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    pg_easy_lib::run()
}
