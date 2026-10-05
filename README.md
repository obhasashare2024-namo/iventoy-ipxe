# iventoy-ipxe (All-in-One Standalone)

**Pure Rust 100% Standalone All-in-One iVentoy & iPXE Network Boot Server**  
*Zero closed-source dependencies · Zero Python overhead · 4.4MB RSS · Real-world hardware verified*

---

## 🚀 項目概述 (Overview)

`iventoy-ipxe`（代號：方案 A - iVentoy iPXE 輕量網絡引導模式）是一套完全擺脫官方閉源 iVentoy 依賴、採用 **100% 純開源 Rust (Tokio + Axum)** 打造的獨立 All-in-One PXE/iPXE 網絡啟動服務器。

傳統官方 iVentoy 核心二進制（`iventoy` 守護進程）為閉源 binary，必須依賴私有 16001 端口代理，且對虛擬磁碟（VHD/VHDX/RAW/QCOW2）的網絡啟動支援存在侷限。本專案將動態 iPXE 菜單生成、多格式磁碟鏡像魔數嗅探、NBD/iSCSI 塊設備自動導出、Windows 雙模式開機與零拷貝 HTTP 206 串流整合為**單一靜態二進制程序**（1.4 MB）。

---

## 🏛️ 系統架構 (System Architecture)

```
+-------------------------------------------------------------------------------+
|                        iventoy-ipxe (Pure Rust Daemon)                        |
|                                                                               |
|  +-------------------+  +--------------------+  +--------------------------+  |
|  | Magic Header Sniff|  | Dynamic iPXE Engine|  | Zero-Copy HTTP Streaming |  |
|  | (.vhd/.qcow2/.raw)|  | (Windows/Linux/ISO)|  | (tokio-util ReaderStream)|  |
|  +---------+---------+  +----------+---------+  +-------------+------------+  |
|            |                       |                          |               |
|  +---------v-----------------------v--------------------------v------------+  |
|  |                Storage Backends (100% Open Source Standard)             |  |
|  |      - qemu-nbd (GPLv2, multi-port dynamic export: 10810, 10811...)      |  |
|  |      - tgtadm (GPL, Linux SCSI target for native Windows sanboot)       |  |
|  +-------------------------------------------------------------------------+  |
+-------------------------------------------------------------------------------+
                                      |
       +------------------------------+-------------------------------+
       | Network Boot Protocols (Open Standard)                       |
       | - iPXE Network Menu Injection (HTTP :26001/boot.ipxe)        |
       | - Kernel / Initrd Zero-Copy HTTP Streaming                   |
       | - NBD Block Streaming / iSCSI SANBOOT                        |
       | - HTTP 206 Partial Content (Byte-Range support)              |
       +--------------------------------------------------------------+
```

### 引導生命週期 (Boot Lifecycle)
1. **網卡握手 (PXE/DHCP)**: 客戶端網卡 PXE ROM 廣播獲取 IP，由 DHCP 指向 `http://${next-server}:26001/boot.ipxe`。
2. **動態選單加載 (Dynamic iPXE Menu)**:
   - 伺服器即時嗅探鏡像目錄，按 MAC 地址綁定或全域預設高亮目標系統；
   - 呈現 Ventoy Cyber-Dark 終端視覺風格（深藍底黑字、高對比醒目反白、超時倒計時）。
3. **內核與引導環境加載 (HTTP Direct Streaming)**:
   - iPXE 通過 HTTP 高速拉取統一啟動內核 (`/vtoy/vmlinuz`) 與自適應網絡根文件系統 (`/vtoy/initramfs.img`)。
4. **塊設備掛載與切換 (NBD Block Hook)**:
   - 內核 `hooks/nbd` 啟動，自動獲取 DHCP，連接伺服器專屬 NBD 端口（如 10811）；
   - 自動嗅探分區表（GPT/MBR），探測真實根分區（`/dev/nbd0p2` 或 `/dev/nbd0p3`）；
   - 將目標分區掛載至 `/real_root` 並執行 `switch_root`，無縫進入原生作業系統（Systemd / runit / s6）。

---

## 🌟 核心特性 (Features)

| 特性維度 | 官方閉源 iVentoy | **iventoy-ipxe (All-in-One Rust 方案 A)** |
| :--- | :--- | :--- |
| **開源屬性** | 閉源二進制 (`iventoy`)，無源碼 | **100% 純開源 Rust (MIT/Apache-2.0)** |
| **後端架構** | 必須依賴 16001 閉源服務代理 | **完全獨立運行，零 16001 依賴** |
| **記憶體佔用** | ~50MB+ | **~4.4 MB RSS** |
| **CPU 負載** | 待機佔用波動 | **0.0% CPU 待機負載** |
| **鏡像格式嗅探** | 僅限特定後綴 | **Magic Header 自動偵測** (VPC VHD, QED, QCOW2, RAW, VDI) |
| **Windows 雙模式** | 僅官方有限模式 | **原生支援 iSCSI Native Sanboot & Ventoy vtoyboot NBD Hook** |
| **虛擬磁碟分區** | 無法自動適應多重分區 | **自動嗅探 / 配置指定 `root_dev` (如 `/dev/nbd0p2`, `/dev/nbd0p3`)** |
| **傳輸最佳化** | 傳統緩衝傳輸 | **Tokio 零拷貝非同步串流 (HEAD + Range 206 請求)** |

---

## 📊 實機硬體引導驗收記錄 (Real-World Test Matrix)

本項目已在局域網 29 號宿主機進行全鏈路實機與 KVM 虛擬化壓測驗收，所有錄屏均經 macOS QuickTime Player 嚴格自驗通過：

### 1. 142GB 巨型 CachyOS 虛擬磁盤 (`0001COSext4.vhd.vtoy`)
- **磁盤規格**：142 GB Raw 鏡像，GPT 分區表，Ext4 根分區（分區 2，Offset = 852492288）。
- **引導表現**：
  - iPXE 3.5 秒自動選中目標選單項；
  - 10811 端口 NBD 握手協商成功（`Negotiation: ..size = 144384MB`）；
  - 內核成功識別 GPT 分區表 `nbd0: p1 p2`，掛載真實根分區並成功啟動 CachyOS 原生 Systemd 與 Journald。
- **產物驗收**：`track_a_cos_142g_boot.mp4`（15fps QuickTime 兼容錄屏）。

### 2. 8GB antiX Linux 虛擬磁盤 (`anitix8g.vhd.vtoy`)
- **磁盤規格**：8 GB VPC VHD 格式，GPT 分區表，Btrfs 根分區（分區 3）。
- **引導表現**：
  - 10816 端口動態導出；
  - 順利進入 antiX 26 完整彩色圖形桌面（IceWM），Conky 實時儀表板正常運行，整機記憶體佔用僅 263MB。
- **產物驗收**：`antix26_desktop_screenshot.png`（1280x800 分辨率）。

---

## ⚖️ 資源負擔評估 (Resource Trade-off)

- **服務端負擔（Server Load）**：
  - 需為每個掛載鏡像維持 `qemu-nbd` 守護進程；
  - 保持 NBD TCP 長連接握手；
  - 總結：在中小型局域網（<50 台客戶端）下資源佔用極輕微，總伺服器內存佔用 < 50MB。
- **客戶端負擔（Client Load）**：
  - **極致輕量**：客戶端僅需標準 iPXE ROM（幾百 KB 代碼量），不需在韌體階段進行複雜的文件系統與分區表解算，適合任何老舊主機或嵌入式設備。

---

## ⚙️ 配置指南 (Configuration)

### 1. YAML 配置 (`config.yaml`)
```yaml
BRIDGE_HOST: 0.0.0.0
BRIDGE_PORT: 26001
VTOY_DIR: /run/media/namobuddha/2tbnvV
BOOT_MODE: nbd
VTOY_KERNEL_URL: "http://${next-server}:26001/vtoy/vmlinuz"
VTOY_INITRD_URL: "http://${next-server}:26001/vtoy/initramfs.img"
KERNEL_PATH: /run/media/namobuddha/2tbnvV/vmlinuz
INITRD_PATH: /run/media/namobuddha/2tbnvV/initramfs.img
```

### 2. JSON 高階與 MAC 綁定配置 (`iventoy.json`)
```json
{
  "windows_boot": {
    "default_mode": "iscsi",
    "iscsi_portal_port": 3260,
    "auto_ram_cow": true,
    "cow_size_mb": 4096
  },
  "default_image": "0001COSext4.vhd.vtoy",
  "mac_bindings": {
    "52:54:00:12:34:56": "0001COSext4.vhd.vtoy"
  }
}
```

---

## 🔌 API 介面規格 (RESTful API Specification)

| 路由 | 方法 | 說明 |
| :--- | :--- | :--- |
| `/` | `GET` | 現代化 Web Cyber-Industrial 控制台 SPA |
| `/boot.ipxe` | `GET` | 動態渲染客戶端 iPXE 啟動選單 |
| `/vtoy/vmlinuz` | `GET/HEAD` | 零拷貝靜態串流引導內核（支援 Range 206） |
| `/vtoy/initramfs.img` | `GET/HEAD` | 零拷貝靜態串流網絡 initramfs |
| `/iso/*` | `GET/HEAD` | 虛擬磁碟鏡像二進制 HTTP 206 串流 |
| `/api/status` | `GET` | 獲取伺服器性能狀態（CPU、內存 RSS、鏡像列表） |
| `/api/config/default-image` | `POST` | 熱切換全域預設引導鏡像 |
| `/api/config/mac-binding` | `POST/DELETE` | 新增/移除特定 MAC 地址的預選鏡像綁定 |

---

## 🛠️ 編譯與運行 (Build & Run)

### 依賴項
- Rust 1.75+ (Cargo)
- `qemu-nbd` (qemu-img)
- 可選：`tgtadm` (用於 Windows 原生 iSCSI Sanboot)

### 構建發布包
```bash
cargo build --release
strip target/release/iventoy-ipxe
# 產物體積僅 1.4 MB
```

### 啟動服務
```bash
./target/release/iventoy-ipxe --config config.yaml
```

---

## 📜 開源許可 (License)

本專案採用 **MIT License** 授權開源。
