import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

type ChipEntry = {
  name: string;
  path: string;
  flashSize: number;
  pageSize: number;
  pids: number[];
};

type InspectInfo = {
  chipName: string;
  configPath: string;
  format: string;
  entry: number;
  flashBase: number;
  flashSize: number;
  pageSize: number;
  pages: number;
  totalBytes: number;
  segments: { start: number; end: number; bytes: number }[];
};

type BurnSummary = {
  chipName: string;
  pid: number;
  pages: number;
  bytes: number;
  elapsedMs: number;
  verified: boolean;
};

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const els = {
  firmware: $<HTMLInputElement>("firmware"),
  base: $<HTMLInputElement>("base"),
  chip: $<HTMLSelectElement>("chip"),
  chipInfo: $<HTMLDivElement>("chipInfo"),
  port: $<HTMLSelectElement>("port"),
  baud: $<HTMLInputElement>("baud"),
  erase: $<HTMLSelectElement>("erase"),
  verify: $<HTMLInputElement>("verify"),
  inspect: $<HTMLButtonElement>("inspect"),
  burn: $<HTMLButtonElement>("burn"),
  bar: $<HTMLDivElement>("bar"),
  status: $<HTMLDivElement>("status"),
  log: $<HTMLPreElement>("log"),
};

let chips: ChipEntry[] = [];

const hex = (n: number) => "0x" + n.toString(16).toUpperCase().padStart(8, "0");
const fmtBytes = (n: number) =>
  n >= 1024 ? `${(n / 1024).toFixed(1)} KiB` : `${n} B`;

function log(line: string) {
  els.log.textContent += line + "\n";
  els.log.scrollTop = els.log.scrollHeight;
}

function setStatus(text: string) {
  els.status.textContent = text;
}

function busy(on: boolean) {
  els.burn.disabled = on;
  els.inspect.disabled = on;
}

async function refreshPorts() {
  const ports = await invoke<string[]>("list_ports");
  els.port.innerHTML = "";
  if (!ports.length) {
    els.port.innerHTML = `<option value="">未发现串口</option>`;
    return;
  }
  for (const p of ports) {
    const o = document.createElement("option");
    o.value = p;
    o.textContent = p;
    els.port.appendChild(o);
  }
}

async function refreshChips() {
  chips = await invoke<ChipEntry[]>("list_chips");
  els.chip.innerHTML = "";
  if (!chips.length) {
    els.chip.innerHTML = `<option value="">未找到 chips/*.toml</option>`;
    updateChipInfo();
    return;
  }
  for (const c of chips) {
    const o = document.createElement("option");
    o.value = c.path;
    o.textContent = `${c.name}  (${fmtBytes(c.flashSize)} / ${fmtBytes(c.pageSize)} 页)`;
    els.chip.appendChild(o);
  }
  updateChipInfo();
}

function updateChipInfo() {
  const c = chips.find((x) => x.path === els.chip.value);
  if (!c) {
    els.chipInfo.textContent = "";
    return;
  }
  const pids = c.pids.map((p) => "0x" + p.toString(16).toUpperCase().padStart(4, "0")).join(", ");
  els.chipInfo.textContent = `PID ${pids || "不校验"} · ${fmtBytes(c.flashSize)} Flash · ${fmtBytes(c.pageSize)}/页`;
}

async function pickFirmware() {
  const file = await open({
    multiple: false,
    filters: [{ name: "固件", extensions: ["elf", "bin"] }],
  });
  if (typeof file === "string") {
    els.firmware.value = file;
    if (file.toLowerCase().endsWith(".elf")) els.base.value = "";
  }
}

function parseBase(): number | undefined {
  const raw = els.base.value.trim();
  if (!raw) return undefined;
  const n = Number(raw);
  return Number.isFinite(n) ? n : undefined;
}

async function doInspect() {
  if (!els.firmware.value) {
    setStatus("请先选择固件");
    return;
  }
  try {
    setStatus("解析中…");
    const info = await invoke<InspectInfo>("inspect", {
      firmware: els.firmware.value,
      config: els.chip.value,
      base: parseBase(),
    });
    els.log.textContent = "";
    log(`芯片    ${info.chipName}`);
    log(`格式    ${info.format}   入口 ${hex(info.entry)}`);
    log(`Flash   ${fmtBytes(info.flashSize)} @ ${hex(info.flashBase)}   页大小 ${fmtBytes(info.pageSize)}`);
    for (const s of info.segments) {
      log(`数据段  ${hex(s.start)} .. ${hex(s.end)}   ${fmtBytes(s.bytes)}`);
    }
    log(`擦除    ${info.pages} 页`);
    setStatus(`就绪 · 共 ${fmtBytes(info.totalBytes)}，覆盖 ${info.pages} 页`);
  } catch (e) {
    setStatus("解析失败");
    log(String(e));
  }
}

async function doBurn() {
  if (!els.firmware.value) {
    setStatus("请先选择固件");
    return;
  }
  if (!els.port.value) {
    setStatus("请先选择串口");
    return;
  }
  busy(true);
  els.bar.style.width = "0%";
  els.log.textContent = "";
  log("将 BOOT0 拨到 1 并复位，正在握手…");
  try {
    setStatus("握手中…");
    const summary = await invoke<BurnSummary>("burn", {
      req: {
        firmware: els.firmware.value,
        port: els.port.value,
        config: els.chip.value,
        baud: els.baud.value ? Number(els.baud.value) : undefined,
        erase: els.erase.value || undefined,
        verify: els.verify.checked,
        base: parseBase(),
      },
    });
    els.bar.style.width = "100%";
    setStatus("烧录完成");
    log("");
    log(`完成    ${summary.chipName}  PID 0x${summary.pid.toString(16).toUpperCase().padStart(4, "0")}`);
    log(`擦除    ${summary.pages} 页`);
    log(`写入    ${fmtBytes(summary.bytes)}${summary.verified ? "（已校验）" : ""}`);
    log(`耗时    ${(summary.elapsedMs / 1000).toFixed(2)} s`);
    log("请将 BOOT0 拨回 0，按一下复位键运行程序。");
  } catch (e) {
    setStatus("烧录失败");
    log("");
    log(String(e));
  } finally {
    busy(false);
  }
}

await listen<{ stage: string; done: number; total: number }>("burn:progress", (ev) => {
  const p = ev.payload;
  const pct = p.total ? Math.round((p.done / p.total) * 100) : 100;
  els.bar.style.width = `${pct}%`;
  setStatus(`${p.stage} ${pct}%`);
});

document.getElementById("pickFirmware")!.addEventListener("click", pickFirmware);
document.getElementById("refreshPorts")!.addEventListener("click", refreshPorts);
document.getElementById("refreshChips")!.addEventListener("click", refreshChips);
els.chip.addEventListener("change", updateChipInfo);
els.inspect.addEventListener("click", doInspect);
els.burn.addEventListener("click", doBurn);

await refreshPorts();
await refreshChips();
