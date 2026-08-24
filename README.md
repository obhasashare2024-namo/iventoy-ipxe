# iventoy-vtoy-bridge-rs

iVentoy fork bridge — Dynamic iPXE Menu Injection & Storage Backend (Rust port of `bridge.py`).

## 背景

本项目是 iVentoy fork 版 `bridge.py`（286 行）的 Rust 全移植版，经双头农奴研究（rp001 Gemini Pro DR + rq001 Qwen 深思）一致结论采用 Full Rewrite 路线，并已在 Artix 本机跑通。

## 架构

- tokio 异步核心（rt-multi-thread / process / net / signal）
- axum 0.8 HTTP 服务
- reqwest 上游回调
- 单一静态 binary，零 Python 运行时依赖

## 核心功能（与 bridge.py 对等）

1. 动态 iPXE 菜单注入
2. qemu-nbd 存储后端导出
3. 分区表侦测
4. iptables 防火墙联动
5. 配置载入

## 构建

```bash
cargo build --release
# 产物: target/release/iventoy-vtoy-bridge
```

## 部署

单 binary 直接部署到 runit/OpenRC 等非 systemd 发行版：

```bash
./target/release/iventoy-vtoy-bridge --help
```
