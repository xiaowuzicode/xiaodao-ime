#!/usr/bin/env python3
"""生成菜单栏状态图标（resources/menubar/*.png，40×40，菜单栏按 20pt 显示）。

设计：单色「声浪」glyph，无底盘，走 macOS 模板图标（黑色+透明，系统自动
适配深浅色菜单栏）；录音态用固定红色（非模板），是唯一的彩色状态。
改动 glyph 后重新执行本脚本并提交生成的 PNG 即可（需 pip install pillow）。
"""
import os

from PIL import Image, ImageDraw

SIZE = 40
BLACK = (0, 0, 0, 255)
RECORD_RED = (229, 72, 77, 255)     # 与 HUD/Windows 托盘的录音红一致
PAUSED_BLACK = (0, 0, 0, 90)        # 模板图标靠 alpha 表达"变淡"

OUT_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "resources", "menubar",
)


def _canvas():
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    return img, ImageDraw.Draw(img)


def draw_wave(color) -> Image.Image:
    """声浪：5 根圆头竖条，中间高两侧低。"""
    img, d = _canvas()
    heights = [12, 22, 32, 22, 12]
    bar_w, gap = 4, 3
    total = len(heights) * bar_w + (len(heights) - 1) * gap
    x = (SIZE - total) // 2
    for h in heights:
        y0 = (SIZE - h) // 2
        d.rounded_rectangle([x, y0, x + bar_w - 1, y0 + h - 1],
                            radius=bar_w // 2, fill=color)
        x += bar_w + gap
    return img


def draw_dots(color) -> Image.Image:
    """转写中：三个小圆点（省略号）。"""
    img, d = _canvas()
    r = 3.5
    for cx in (10, 20, 30):
        d.ellipse([cx - r, 20 - r, cx + r, 20 + r], fill=color)
    return img


def draw_sparkle(color) -> Image.Image:
    """润色中：四角星（sparkle）。"""
    img, d = _canvas()
    cx = cy = 20
    outer, inner = 14, 4.5
    pts = []
    for i, (dx, dy) in enumerate(
            [(0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)]):
        r = outer if i % 2 == 0 else inner
        # 对角方向的单位化
        if dx and dy:
            dx *= 0.7071
            dy *= 0.7071
        pts.append((cx + dx * r, cy + dy * r))
    d.polygon(pts, fill=color)
    return img


ICONS = {
    "idle.png": draw_wave(BLACK),
    "recording.png": draw_wave(RECORD_RED),
    "transcribing.png": draw_dots(BLACK),
    "polishing.png": draw_sparkle(BLACK),
    "paused.png": draw_wave(PAUSED_BLACK),
}


def main() -> None:
    os.makedirs(OUT_DIR, exist_ok=True)
    for name, img in ICONS.items():
        path = os.path.join(OUT_DIR, name)
        img.save(path)
        print("✅", path)


if __name__ == "__main__":
    main()
