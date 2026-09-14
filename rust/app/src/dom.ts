/** 设置页用的极简 DOM 构造辅助（无框架，避免任何外部依赖）。 */

type Attrs = Record<string, string | number | boolean | undefined>;

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  children: (Node | string)[] = [],
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === false) continue;
    if (key === "class") node.className = String(value);
    else if (key === "text") node.textContent = String(value);
    else if (value === true) node.setAttribute(key, "");
    else node.setAttribute(key, String(value));
  }
  for (const child of children) {
    node.append(child);
  }
  return node;
}

/** 分组卡片：标题 + 可选说明 + 若干行。 */
export function card(title: string, rows: HTMLElement[], desc?: string): HTMLElement {
  const children: (Node | string)[] = [h("h2", { text: title })];
  if (desc) children.push(h("p", { class: "card-desc", text: desc }));
  children.push(...rows);
  return h("div", { class: "card" }, children);
}

/** 带左侧标签的一行（标签右对齐，贴 macOS 设置面板惯例）。 */
export function row(label: string, ...body: HTMLElement[]): HTMLElement {
  return h("div", { class: "row" }, [
    h("label", { class: "row-label", text: label }),
    h("div", { class: "row-body" }, body),
  ]);
}

/** 整行（无左标签），用于开关与多行编辑区。 */
export function fullRow(...body: HTMLElement[]): HTMLElement {
  return h("div", { class: "row" }, body);
}

/** 纵向排列的一行（标签在上、内容在下）。 */
export function columnRow(label: string, ...body: HTMLElement[]): HTMLElement {
  return h("div", { class: "row column" }, [
    h("label", { class: "hint", text: label }),
    ...body,
  ]);
}

export function select(options: { value: string; label: string }[], current: string): HTMLSelectElement {
  const node = h("select");
  for (const opt of options) {
    node.append(h("option", { value: opt.value, text: opt.label }));
  }
  node.value = options.some((o) => o.value === current) ? current : (options[0]?.value ?? "");
  return node;
}

export function checkbox(label: string, checked: boolean): HTMLLabelElement {
  const input = h("input", { type: "checkbox" });
  input.checked = checked;
  return h("label", { class: "check" }, [input, label]) as HTMLLabelElement;
}

/** 从 checkbox() 产出的 label 中取回 input。 */
export function checkboxInput(label: HTMLElement): HTMLInputElement {
  return label.querySelector("input") as HTMLInputElement;
}

export function radioGroup(
  name: string,
  options: { value: string; label: string }[],
  current: string,
): HTMLElement {
  const group = h("div", { class: "radio-group" });
  for (const opt of options) {
    const input = h("input", { type: "radio", name, value: opt.value });
    input.checked = opt.value === current;
    group.append(h("label", { class: "check" }, [input, opt.label]));
  }
  return group;
}

export function radioValue(group: HTMLElement, fallback: string): string {
  const checked = group.querySelector<HTMLInputElement>("input:checked");
  return checked ? checked.value : fallback;
}

export function textInput(value: string, placeholder = "", type = "text"): HTMLInputElement {
  const node = h("input", { type, placeholder });
  node.value = value;
  return node;
}

export function button(label: string, cls: string, onClick: () => void): HTMLButtonElement {
  const node = h("button", { type: "button", class: cls, text: label });
  node.addEventListener("click", onClick);
  return node;
}
