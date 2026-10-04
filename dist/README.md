# iventoy-ipxe (All-in-One Standalone)

**Pure Rust 100% Standalone All-in-One iVentoy & iPXE Network Boot Server**  
*Zero closed-source dependencies · Zero Python overhead · 4.4MB RSS · Real-world hardware verified*

---

## 🚀 核心概述 (Overview)

`iventoy-ipxe` 是一套完全擺脫官方閉源 iVentoy 依賴、採用 **100% 純開源 Rust (Tokio + Axum)** 打造的獨立 All-in-One PXE/iPXE 網絡啟動服務器。

傳統 iVentoy 核心二進制（`iventoy` 守護進程）為封閉源碼，且對虛擬磁碟（VHD/VHDX/RAW/QCOW2）的網絡啟動支援存在侷限。本專案將動態 iPXE 菜單生成、多格式磁碟鏡像魔數嗅探、NBD/iSCSI 塊設備自動導出、Windows 雙模式開機與零拷貝 HTTP 串流整合為**單一靜態二進制程序**（1.4 MB）。

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
|  |      - qemu-nbd (GPLv2, multi-port dynamic export: 10809, 10810...)      |  |
|  |      - tgtadm (GPL, Linux SCSI target for native Windows sanboot)       |  |
|  +-------------------------------------------------------------------------+  |
+-------------------------------------------------------------------------------+
                                      |
       +------------------------------+-------------------------------+
       | Network Boot Protocols (Open Standard)                       |
       | - iPXE Network Menu Injection (HTTP :26001/ipxe/menu)        |
       | - Kernel / Initrd / ISO Direct Streaming                     |
       | - NBD Block Streaming / iSCSI SANBOOT                        |
       +--------------------------------------------------------------+
```

---

## 🌟 關鍵特性 (Features)

| 特性維度 | 官方閉源 iVentoy | **iventoy-ipxe (All-in-One Rust)** |
| :--- | :--- | :--- |
| **開源屬性** | 閉源二進制 (`iventoy`)，無源碼 | **100% 純開源 Rust (MIT/Apache-2.0)** |
| **後端架構** | 必須依賴 16001 閉源服務代理 | **完全獨立運行，零 16001 依賴** |
| **記憶體佔用** | ~50MB+ | **~4.4 MB RSS** |
| **CPU 負載** | 待機佔用波動 | **0.0% CPU 待機負載** |
| **鏡像格式嗅探** | 僅限特定後綴 | **Magic Header 自動偵測** (VPC VHD, QED, QCOW2, RAW, VDI) |
| **Windows 雙模式** | 僅官方有限模式 | **原生支援 iSCSI Native Sanboot & Ventoy vtoyboot NBD Hook** |
| **虛擬磁碟分區** | 無法自動適應多重分區 | **自動嗅探 / 配置指定 `root_dev` (如 `/dev/nbd0p3`)** |
| **傳輸最佳化** | 傳統緩衝傳輸 | **Tokio 零拷貝非同步串流 (HEAD + Range 請求)** |

---

## 🖥️ 真機實測端到端驗證 (Physical Verification)

本專案已在 Artix Linux (29 號節點，Intel Tiger Lake / RTX 3070 平台) 進行完整的真機端到端網絡啟動驗證：

1. **引導鏡像**：8GB Btrfs 虛擬磁碟 (`anitix8g.vhd.vtoy`)
2. **啟動鏈路**：QEMU UEFI PXE → iPXE DHCP → `iventoy-ipxe` 動態注入菜單 → NBD `/dev/nbd0p3` 掛載 → runit stage 2 → `seatd` / `turnstiled` / `slimski` / Xorg 1.21
3. **驗證結果**：**100% 成功進入 antiX 26 IceWM 完整彩色圖形桌面**！

![antiX 26 Desktop Screenshot](docs/screenshots/antix26_desktop_screenshot.png)

*實機截圖規格：1280x800 分辨率，Conky 實時儀表板正常運行，記憶體僅佔用 263MB。*

---

## ⚙️ 配置範例 (Configuration)

支援 `iventoy.json` 格式（與現有生態高度相容）：

```json
{
  "windows_boot": {
    "default_mode": "vtoyboot",
    "override_per_image": {
      "win11_enterprise.vhd": "iscsi",
      "win10_ltsb.vhd": "vtoyboot"
    }
  },
  "images": [
    {
      "file": "anitix8g.vhd.vtoy",
      "root_dev": "/dev/nbd0p3",
      "format": "vhd"
    }
  ]
}
```

亦可搭配 YAML 配置 (`config.yaml`)：

```yaml
bridge_host: "0.0.0.0"
bridge_port: 26001
vtoy_kernel_url: "http://192.168.1.29:26001/vtoy/vmlinuz"
vtoy_initrd_url: "http://192.168.1.29:26001/vtoy/initramfs.img"
image_dir: "/run/media/namobuddha/2tbnvV"
boot_mode: "nbd"
```

---

## 🛠️ 編譯與運行 (Build & Run)

### 構建二進制 (Build)

```bash
cargo build --release
# 生成產物: target/release/iventoy-ipxe (1.4MB stripped)
```

### 啟動服務 (Run)

```bash
./target/release/iventoy-ipxe --config /etc/iventoy-ipxe/config.yaml
```

服務將自動：
1. 掃描 `image_dir` 中的鏡像檔案並嗅探格式。
2. 自動為虛擬磁碟啟動 `qemu-nbd` 守護進程（多端口自動分配 `10809`, `10810`...）。
3. 監聽 HTTP 26001 端口，響應 `/ipxe/menu`、`/ipxe/boot.ipxe` 以及二進制串流下載。

---

## 📜 開源許可 (License)

本專案採用 **MIT License** 授權開源。
底層儲存工具完全依賴標準開源項目 (`qemu-nbd`, `tgtadm`, `ipxe`)。
