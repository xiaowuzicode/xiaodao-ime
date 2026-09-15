/**
 * HUD 悬浮窗前端：监听后端 `hud` 事件，渲染「前缀 + 计时 + 声浪 + 识别文本」两行。
 *
 * 与 Python 版 `xiaodao_ime/hud.py` 行为对齐：
 * - 主行识别文本只留尾部 24 字，超出前面加「…」；
 * - 状态阶段（转写/润色）主行显示状态文案，副行显示已转写全文尾部 40 字；
 * - 声浪为最近 10 个电平的滚动柱状图，录音中为红色。
 */
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

/** 与 src-tauri/src/hud.rs 的 `HudEvent` 一一对应（serde tag = "kind"）。 */
type HudEvent =
  | { kind: "begin"; channel: "dictate" | "rewrite"; placeholder: string; hint: string }
  | { kind: "level"; level: number }
  | { kind: "partial"; elapsed: number; text: string }
  | { kind: "status"; status: string; detail: string }
  | { kind: "hide" };

const MAX_TAIL = 24; // 主行识别文本尾部最多字符数（对齐 Python _MAX_TAIL）
const MAX_DETAIL = 40; // 状态副行尾部最多字符数（对齐 Python _MAX_DETAIL）
const WAVE_SLOTS = 10; // 声浪采样槽位数

const el = {
  prefix: document.getElementById("hud-prefix") as HTMLElement,
  timer: document.getElementById("hud-timer") as HTMLElement,
  wave: document.getElementById("hud-wave") as HTMLCanvasElement,
  text: document.getElementById("hud-text") as HTMLElement,
  hint: document.getElementById("hud-hint") as HTMLElement,
};

const state = {
  visible: false,
  recording: false,
  prefix: "",
  elapsed: 0,
  partial: "",
  placeholder: "",
  hint: "",
  levels: new Array<number>(WAVE_SLOTS).fill(0),
};

/** 取字符串尾部 limit 个字符，截断时前置省略号（与 Python `_tail` 一致）。 */
function tail(text: string, limit: number): string {
  if (!text) return "";
  const chars = Array.from(text);
  return chars.length <= limit ? text : "…" + chars.slice(-limit).join("");
}

function drawWave(): void {
  const ctx = el.wave.getContext("2d");
  if (!ctx) return;
  const dpr = window.devicePixelRatio || 1;
  const cssW = el.wave.clientWidth || 60;
  const cssH = el.wave.clientHeight || 18;
  if (el.wave.width !== Math.round(cssW * dpr)) {
    el.wave.width = Math.round(cssW * dpr);
    el.wave.height = Math.round(cssH * dpr);
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, cssW, cssH);
  ctx.fillStyle = state.recording ? "#ff4d4f" : "rgba(255,255,255,0.75)";
  const gap = 2;
  const barW = Math.max(2, (cssW - gap * (WAVE_SLOTS - 1)) / WAVE_SLOTS);
  for (let i = 0; i < WAVE_SLOTS; i += 1) {
    const level = Math.min(1, Math.max(0, state.levels[i] ?? 0));
    const h = Math.max(2, level * cssH);
    const x = i * (barW + gap);
    ctx.fillRect(x, (cssH - h) / 2, barW, h);
  }
}

function render(): void {
  if (!state.visible) return; // 窗口隐藏时不渲染，避免白耗 CPU
  el.prefix.hidden = !state.prefix;
  el.prefix.textContent = state.prefix;
  el.timer.textContent = `${state.elapsed}s`;
  el.hint.textContent = state.hint;
  drawWave();
}

function apply(event: HudEvent): void {
  switch (event.kind) {
    case "begin":
      state.visible = true;
      state.recording = true;
      state.prefix = event.channel === "rewrite" ? "改写" : "";
      state.elapsed = 0;
      state.partial = "";
      state.placeholder = event.placeholder || "聆听中…";
      state.hint = event.hint;
      state.levels = new Array<number>(WAVE_SLOTS).fill(0);
      el.timer.hidden = false;
      el.wave.hidden = false;
      el.text.textContent = state.placeholder;
      break;
    case "level":
      if (!state.recording) return;
      state.levels = [...state.levels.slice(1), event.level];
      break;
    case "partial":
      if (!state.recording) return;
      state.elapsed = Math.floor(event.elapsed);
      if (event.text) state.partial = event.text;
      el.text.textContent = tail(state.partial, MAX_TAIL) || state.placeholder;
      break;
    case "status":
      state.visible = true;
      state.recording = false;
      state.prefix = "";
      state.hint = tail(event.detail, MAX_DETAIL);
      el.timer.hidden = true;
      el.wave.hidden = true;
      el.text.textContent = event.status;
      break;
    case "hide":
      state.visible = false;
      state.recording = false;
      return;
  }
  render();
}

void listen<HudEvent>("hud", (e) => apply(e.payload));

// 调试：index.html?demo=1 时让后端播一串假事件，便于单独看 HUD 动效
if (new URLSearchParams(window.location.search).get("demo") === "1") {
  void invoke("hud_demo");
}
