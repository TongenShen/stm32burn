//! Tauri 后端：把 stm32burn 的库直接暴露给前端。
//!
//! 不做任何命令行文本解析——所有数据都是结构化的结构体/事件。

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use stm32burn::burn::{Burner, Stage, StageError};
use stm32burn::config::{Config, EraseMode};
use stm32burn::elf::{Format, Firmware};
use stm32burn::transport::{self, SerialTransport};

// ---------- 前端可见的数据结构 ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChipEntry {
    name: String,
    path: String,
    flash_size: u32,
    page_size: u32,
    pids: Vec<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SegmentInfo {
    start: u32,
    end: u32,
    bytes: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InspectInfo {
    chip_name: String,
    config_path: String,
    format: String,
    entry: u32,
    flash_base: u32,
    flash_size: u32,
    page_size: u32,
    pages: usize,
    total_bytes: usize,
    segments: Vec<SegmentInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BurnRequest {
    firmware: String,
    port: String,
    config: String,
    baud: Option<u32>,
    erase: Option<String>,
    verify: bool,
    base: Option<u32>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    stage: String,
    done: usize,
    total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BurnSummary {
    chip_name: String,
    pid: u32,
    pages: usize,
    bytes: usize,
    elapsed_ms: u64,
    verified: bool,
}

// ---------- 命令 ----------

#[tauri::command]
fn list_ports() -> Vec<String> {
    transport::list_ports().unwrap_or_default()
}

/// 扫描几处约定位置下的 chips/*.toml，供前端下拉选择。
#[tauri::command]
fn list_chips(app: AppHandle) -> Vec<ChipEntry> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    // 安装版：配置随安装包放在资源目录
    if let Ok(res) = app.path().resource_dir() {
        dirs.push(res.join("chips"));
    }
    // 便携版：配置放在 exe 旁边
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("chips"));
        }
    }
    // 开发模式：从仓库根或 gui 目录启动
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("chips"));
        dirs.push(cwd.join("../chips"));
    }

    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Ok(cfg) = Config::load(&path) else {
                continue;
            };
            if out.iter().any(|c: &ChipEntry| c.path == path.to_string_lossy()) {
                continue;
            }
            out.push(ChipEntry {
                name: cfg.chip.name.clone(),
                path: path.to_string_lossy().to_string(),
                flash_size: cfg.chip.flash_size,
                page_size: cfg.chip.page_size,
                pids: cfg.chip.pids.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[tauri::command]
fn inspect(firmware: String, config: String, base: Option<u32>) -> Result<InspectInfo, String> {
    let cfg = Config::load(Path::new(&config)).map_err(|e| e.to_string())?;
    let mut fw = Firmware::load(Path::new(&firmware), base).map_err(|e| e.to_string())?;
    fw.retain_flash(cfg.chip.flash_base, cfg.chip.flash_size)
        .map_err(|e| e.to_string())?;

    let (start, end) = fw.span();
    let pages = cfg.chip.pages_covering(start, end).len();

    Ok(InspectInfo {
        chip_name: cfg.chip.name.clone(),
        config_path: config,
        format: match fw.format {
            Format::Elf => "ELF".to_owned(),
            Format::Bin => "BIN".to_owned(),
        },
        entry: fw.entry,
        flash_base: cfg.chip.flash_base,
        flash_size: cfg.chip.flash_size,
        page_size: cfg.chip.page_size,
        pages,
        total_bytes: fw.byte_len(),
        segments: fw
            .images
            .iter()
            .map(|img| SegmentInfo {
                start: img.addr,
                end: img.addr + img.data.len() as u32,
                bytes: img.data.len(),
            })
            .collect(),
    })
}

#[tauri::command]
async fn burn(app: AppHandle, req: BurnRequest) -> Result<BurnSummary, String> {
    tauri::async_runtime::spawn_blocking(move || burn_blocking(&app, req))
        .await
        .map_err(|e| format!("烧录任务异常: {e}"))?
}

fn burn_blocking(app: &AppHandle, req: BurnRequest) -> Result<BurnSummary, String> {
    let mut cfg = Config::load(Path::new(&req.config)).map_err(|e| e.to_string())?;
    if let Some(baud) = req.baud {
        cfg.uart.baud = baud;
    }

    let mode = match req.erase.as_deref() {
        Some("page") => EraseMode::Page,
        Some("mass") => EraseMode::Mass,
        Some("extended") => EraseMode::Extended,
        _ => cfg.chip.erase_mode,
    };

    let mut fw = Firmware::load(Path::new(&req.firmware), req.base).map_err(|e| e.to_string())?;
    fw.retain_flash(cfg.chip.flash_base, cfg.chip.flash_size)
        .map_err(|e| e.to_string())?;

    let serial = SerialTransport::open(&req.port, &cfg.uart).map_err(|e| e.to_string())?;
    let mut burner = Burner::new(serial, false);
    let started = Instant::now();

    burner.sync(20).map_err(|e| e.to_string())?;
    let pid = burner.get_id().map_err(|e| e.to_string())?;

    if !cfg.chip.pids.is_empty() && !cfg.chip.pids.contains(&(pid as u32)) {
        let expect = cfg
            .chip
            .pids
            .iter()
            .map(|p| format!("0x{p:04X}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "芯片不匹配：读到 PID 0x{pid:04X}，但 {} 期望 {expect}",
            cfg.chip.name
        ));
    }

    // 只在百分比变化时抛事件，避免每 256 字节刷新一次 UI。
    let mut last_pct = usize::MAX;
    let mut progress = |stage: Stage, done: usize, total: usize| {
        let pct = (done * 100).checked_div(total).unwrap_or(100);
        if pct != last_pct {
            last_pct = pct;
            let _ = app.emit(
                "burn:progress",
                ProgressEvent {
                    stage: stage.label().to_string(),
                    done,
                    total,
                },
            );
        }
    };

    burner
        .program(&cfg.chip, &fw, mode, req.verify, &mut progress)
        .map_err(|se: StageError| format!("{}阶段失败: {}", se.stage.label(), se.err))?;

    let (start, end) = fw.span();
    let pages = cfg.chip.pages_covering(start, end).len();

    Ok(BurnSummary {
        chip_name: cfg.chip.name.clone(),
        pid: pid as u32,
        pages,
        bytes: fw.byte_len(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        verified: req.verify,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            list_ports,
            list_chips,
            inspect,
            burn
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
