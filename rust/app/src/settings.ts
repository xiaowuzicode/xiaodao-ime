/**
 * 设置窗口前端：分组对照 Python 版 `settings_window.py`，字段对照 `xiaodao_ime/settings.py`。
 *
 * 保存策略：先 `get_settings` 取全量 JSON（含本页不展示的未知键），只改动本页字段后整体写回，
 * 保证手工编辑过 settings.json 的用户不会被本页清空配置。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  button,
  card,
  checkbox,
  checkboxInput,
  columnRow,
  fullRow,
  h,
  radioGroup,
  radioValue,
  row,
  select,
  textInput,
} from "./dom";

type Json = Record<string, unknown>;
type HotkeyChoice = { id: string; label: string };

/** 与 Python `polisher.BUILTIN_STYLES` 一致的 5 个内置风格。 */
const BUILTIN_STYLES = ["润色", "书面化", "轻度纠错", "翻译成英文", "会议纪要"];
const OFF_STYLE = "关闭"; // app_styles 里表示「该 App 不润色」
const RECORD_MODES = [
  { value: "toggle", label: "单击开始 / 再击结束" },
  { value: "hold", label: "按住说话（双击锁定）" },
];
const PROVIDERS = [
  { value: "openai", label: "OpenAI 兼容（DeepSeek / Kimi / GLM…）" },
  { value: "anthropic", label: "Anthropic" },
];

const root = document.getElementById("settings") as HTMLElement;
const note = document.getElementById("settings-note") as HTMLElement;
const btnSave = document.getElementById("btn-save") as HTMLButtonElement;
const btnCancel = document.getElementById("btn-cancel") as HTMLButtonElement;

let original: Json = {};
let collect: (() => Json) | null = null;

function obj(value: unknown): Json {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Json) : {};
}

function str(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function setNote(text: string): void {
  note.textContent = text;
}

/** 键值对编辑区：每行两个输入框 + 删除，底部一个「添加」。 */
function kvEditor(
  entries: [string, string][],
  keyPlaceholder: string,
  valuePlaceholder: string,
  valueFactory?: (current: string) => HTMLElement,
): { node: HTMLElement; read: () => Record<string, string> } {
  const list = h("div", { class: "kv-list" });
  const addRow = (key: string, value: string) => {
    const keyInput = textInput(key, keyPlaceholder);
    const valueNode = valueFactory ? valueFactory(value) : textInput(value, valuePlaceholder);
    const line = h("div", { class: "kv-row" }, [keyInput, valueNode]);
    line.append(button("删除", "btn btn-mini btn-danger", () => line.remove()));
    list.append(line);
  };
  entries.forEach(([key, value]) => addRow(key, value));
  const node = h("div", { class: "kv-list" }, [
    list,
    h("div", {}, [button("添加一行", "btn btn-mini", () => addRow("", ""))]),
  ]);
  const read = () => {
    const out: Record<string, string> = {};
    list.querySelectorAll<HTMLElement>(".kv-row").forEach((line) => {
      const inputs = line.querySelectorAll<HTMLInputElement | HTMLSelectElement>("input, select");
      const key = (inputs[0]?.value ?? "").trim();
      const value = (inputs[1]?.value ?? "").trim();
      if (key) out[key] = value;
    });
    return out;
  };
  return { node, read };
}

function build(settings: Json, hotkeys: HotkeyChoice[]): void {
  const hotkeyOptions = hotkeys.map((k) => ({ value: k.id, label: k.label }));
  const polish = obj(settings.polish);
  const customStyles = obj(polish.styles);

  // ---- 热键与录音 ----
  const selDictate = select(hotkeyOptions, str(settings.hotkey, hotkeyOptions[0]?.value));
  const selRewrite = select(hotkeyOptions, str(settings.rewrite_hotkey, hotkeyOptions[1]?.value));
  const modeGroup = radioGroup("record_mode", RECORD_MODES, str(settings.record_mode, "toggle"));
  const chkPreview = checkbox(
    "实时预览悬浮窗（计时 / 声浪 / 边说边出字）",
    settings.live_preview !== false,
  );
  const chkSounds = checkbox("录音开始 / 结束提示音", settings.sounds !== false);

  // ---- AI 润色 ----
  const chkPolish = checkbox("启用：去口水词、修同音错字、规范标点", polish.enabled === true);
  const selProvider = select(PROVIDERS, str(polish.provider, "openai"));
  const fldBase = textInput(str(polish.base_url), "https://api.deepseek.com");
  const fldKey = textInput(str(polish.api_key), "sk-…（只存本机，不上传）", "password");
  const btnReveal = button("显示", "btn btn-mini", () => {
    const hidden = fldKey.type === "password";
    fldKey.type = hidden ? "text" : "password";
    btnReveal.textContent = hidden ? "隐藏" : "显示";
  });
  const fldModel = textInput(str(polish.model), "deepseek-chat");
  const fldTimeout = textInput(String(polish.timeout ?? 30), "30", "number");
  const selStyle = select(
    BUILTIN_STYLES.map((s) => ({ value: s, label: s })),
    str(polish.style, "润色"),
  );
  const styleEditor = kvEditor(
    Object.entries(customStyles).map(([k, v]) => [k, str(v)]),
    "风格名（如：程序员周报体）",
    "system 提示词",
  );
  // 自定义风格增删改后，重建风格下拉（内置 5 个 + 自定义键，同名以自定义为准）
  const refreshStyles = () => {
    const current = selStyle.value;
    const names = [...BUILTIN_STYLES];
    for (const name of Object.keys(styleEditor.read())) {
      if (!names.includes(name)) names.push(name);
    }
    selStyle.replaceChildren(...names.map((n) => h("option", { value: n, text: n })));
    selStyle.value = names.includes(current) ? current : names[0];
  };
  styleEditor.node.addEventListener("input", refreshStyles);
  styleEditor.node.addEventListener("click", () => window.setTimeout(refreshStyles, 0));
  refreshStyles();

  const polishCtrls = [selProvider, fldBase, fldKey, fldModel, fldTimeout, selStyle];
  const syncPolish = () => {
    const on = checkboxInput(chkPolish).checked;
    polishCtrls.forEach((c) => ((c as HTMLInputElement).disabled = !on));
  };
  checkboxInput(chkPolish).addEventListener("change", syncPolish);
  syncPolish();

  // ---- 热词 / 替换表 / 场景风格 ----
  const hotwords = Array.isArray(settings.hotwords) ? (settings.hotwords as unknown[]) : [];
  const areaHotwords = h("textarea", { placeholder: "每行一个热词，如：Ordo" });
  areaHotwords.value = hotwords.map((w) => str(w)).filter(Boolean).join("\n");

  const replEditor = kvEditor(
    Object.entries(obj(settings.replacements)).map(([k, v]) => [k, str(v)]),
    "转写文本（如：欧朵）",
    "替换为（如：Ordo）",
  );
  const appStyleEditor = kvEditor(
    Object.entries(obj(settings.app_styles)).map(([k, v]) => [k, str(v)]),
    "应用标识（bundle id / exe 名）",
    "",
    (current) => {
      const names = [OFF_STYLE, ...Array.from(selStyle.options).map((o) => o.value)];
      return select(
        names.map((n) => ({ value: n, label: n })),
        names.includes(current) ? current : OFF_STYLE,
      );
    },
  );

  root.replaceChildren(
    card("热键与录音", [
      row("听写热键", selDictate),
      row("改写热键", selRewrite),
      row("录音方式", modeGroup),
      fullRow(chkPreview),
      fullRow(chkSounds),
    ]),
    card(
      "AI 润色（自备大模型 Key，可选）",
      [
        fullRow(chkPolish),
        row("服务商", selProvider),
        row("Base URL", fldBase),
        row("API Key", fldKey, btnReveal),
        row("模型", fldModel),
        row("超时（秒）", fldTimeout),
        row("润色风格", selStyle),
        columnRow("自定义风格（风格名 → system 提示词，同名覆盖内置）", styleEditor.node),
      ],
      "润色失败一律回退原始转写（fail-open），不影响出字。",
    ),
    card("热词", [columnRow("每行一个，注入润色提示词辅助纠正同音错字", areaHotwords)]),
    card(
      "替换表",
      [columnRow("转写后无条件字符串替换，不依赖大模型", replEditor.node)],
    ),
    card(
      "场景风格",
      [columnRow("前台应用 → 润色风格，「关闭」表示该应用不润色", appStyleEditor.node)],
      "键为 macOS bundle id / 应用名，或 Windows 进程 exe 名（大小写不敏感）。",
    ),
  );

  collect = () => {
    const next: Json = JSON.parse(JSON.stringify(original));
    next.hotkey = selDictate.value;
    next.rewrite_hotkey = selRewrite.value;
    next.record_mode = radioValue(modeGroup, "toggle");
    next.live_preview = checkboxInput(chkPreview).checked;
    next.sounds = checkboxInput(chkSounds).checked;
    const nextPolish = obj(next.polish);
    nextPolish.enabled = checkboxInput(chkPolish).checked;
    nextPolish.provider = selProvider.value;
    nextPolish.base_url = fldBase.value.trim();
    nextPolish.api_key = fldKey.value.trim();
    nextPolish.model = fldModel.value.trim();
    nextPolish.timeout = Number(fldTimeout.value) || 30;
    nextPolish.style = selStyle.value;
    nextPolish.styles = styleEditor.read();
    next.polish = nextPolish;
    next.hotwords = areaHotwords.value
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean);
    next.replacements = replEditor.read();
    next.app_styles = appStyleEditor.read();
    return next;
  };
}

async function load(): Promise<void> {
  setNote("");
  try {
    const [settings, hotkeys] = await Promise.all([
      invoke<Json>("get_settings"),
      invoke<HotkeyChoice[]>("get_hotkey_choices"),
    ]);
    original = settings;
    build(settings, hotkeys);
  } catch (err) {
    collect = null;
    root.replaceChildren(h("div", { class: "loading", text: `读取配置失败：${String(err)}` }));
  }
}

btnSave.addEventListener("click", async () => {
  if (!collect) return;
  const next = collect();
  if (next.hotkey === next.rewrite_hotkey) {
    setNote("听写热键和改写热键不能相同。");
    return;
  }
  btnSave.disabled = true;
  try {
    await invoke("save_settings", { settings: next });
    await getCurrentWindow().hide();
  } catch (err) {
    setNote(`保存失败：${String(err)}`);
  } finally {
    btnSave.disabled = false;
  }
});

btnCancel.addEventListener("click", () => {
  void getCurrentWindow().hide();
});

// 托盘点「设置…」会重新拉起本窗口：每次显示前重读配置，避免显示陈旧值
void listen("settings:reload", () => void load());
void load();
