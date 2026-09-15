#!/usr/bin/env python3
"""统一入口：按平台分发到 macOS（rumps 菜单栏）或 Windows（pystray 托盘）。"""
import sys

if sys.platform == "darwin":
    from app import main
elif sys.platform.startswith("win"):
    from app_win import main
else:
    raise SystemExit("小岛AI输入法目前支持 macOS 与 Windows；Linux 支持在路线图中")

if __name__ == "__main__":
    main()
