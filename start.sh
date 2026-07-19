#!/usr/bin/env bash
# 激活项目 venv 并启动小岛AI输入法（菜单栏常驻）。
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"

if [[ ! -x "$DIR/.venv/bin/python" ]]; then
  echo "未找到 venv，请先执行：python3 -m venv .venv && .venv/bin/pip install -r requirements.txt" >&2
  exit 1
fi

exec "$DIR/.venv/bin/python" "$DIR/app.py"
