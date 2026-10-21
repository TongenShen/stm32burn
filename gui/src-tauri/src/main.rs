// 发布版不弹控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

/// 双击启动时没有控制台，panic 信息会直接丢失，表现就是「点了没反应」。
/// 这里把崩溃原因同时写到临时目录和 exe 旁边，方便排查。
fn install_panic_logger() {
    std::panic::set_hook(Box::new(|info| {
        let text = format!("{info}\n");
        let mut targets: Vec<PathBuf> = vec![std::env::temp_dir().join("stm32burn-gui-crash.log")];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                targets.push(dir.join("stm32burn-gui-crash.log"));
            }
        }
        for path in targets {
            let _ = std::fs::write(&path, &text);
        }
    }));
}

fn main() {
    install_panic_logger();
    stm32burn_gui_lib::run()
}
