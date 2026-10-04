use crate::config::{BootMode, Config};
use crate::image::{detect_disk_format, has_partition_table, is_windows_image, DiskFormat};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub fn sanitize_ipxe_key(key: &str) -> String {
    let clean: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        "item".into()
    } else {
        clean
    }
}

pub fn sanitize_ipxe_text(text: &str) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '$' { '_' } else { c })
        .collect();
    clean.trim().to_string()
}

#[derive(Clone, Serialize, Debug)]
pub struct TargetInfo {
    pub id: String,
    pub file: String,
    pub path: String,
    pub tid: u32,
    pub iqn: String,
    pub nbd_port: u16,
    #[serde(skip)]
    pub proc_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alive: Option<bool>,
    pub has_partitions: bool,
    pub root_dev: String,
    pub format: String,
    pub is_windows: bool,
}

pub struct StorageManager {
    pub config: Config,
    targets: Mutex<HashMap<String, Target>>,
    used_tids: Mutex<HashSet<u32>>,
    used_ports: Mutex<HashSet<u16>>,
}

struct Target {
    info: TargetInfo,
    child: Option<Child>,
}

fn log(msg: &str) {
    println!("[storage] {}", msg);
}

async fn run_cmd(args: &[&str]) {
    let _ = Command::new(args[0]).args(&args[1..]).output().await;
}

impl StorageManager {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            targets: Mutex::new(HashMap::new()),
            used_tids: Mutex::new(HashSet::new()),
            used_ports: Mutex::new(HashSet::new()),
        }
    }

    pub async fn alloc_tid(&self) -> u32 {
        let mut used = self.used_tids.lock().await;
        let mut tid = self.config.tgt_base_tid;
        while used.contains(&tid) {
            tid += 1;
        }
        used.insert(tid);
        tid
    }

    pub async fn alloc_port(&self) -> u16 {
        let mut used = self.used_ports.lock().await;
        let mut p = self.config.nbd_base_port;
        while used.contains(&p) {
            p += 1;
        }
        used.insert(p);
        p
    }

    async fn export_iscsi(&self, tid: u32, iqn: &str, path: &str) {
        log(&format!("Exporting iSCSI: TID={} IQN={} Backing={}", tid, iqn, path));
        run_cmd(&[
            "tgtadm", "--lld", "iscsi", "--op", "new", "--mode", "target",
            "--tid", &tid.to_string(), "-T", iqn,
        ])
        .await;
        run_cmd(&[
            "tgtadm", "--lld", "iscsi", "--op", "new", "--mode", "logicalunit",
            "--tid", &tid.to_string(), "--lun", "1", "-b", path,
        ])
        .await;
        run_cmd(&[
            "tgtadm", "--lld", "iscsi", "--op", "bind", "--mode", "target",
            "--tid", &tid.to_string(), "-I", "ALL",
        ])
        .await;
    }

    async fn export_nbd(&self, port: u16, path: &str, format: DiskFormat) -> Option<Child> {
        let fmt_arg = format.as_qemu_arg();
        log(&format!("Exporting NBD: Port={} Format={} Backing={}", port, fmt_arg, path));

        // High performance sparse + zero copy options
        let child = Command::new("qemu-nbd")
            .args([
                "-t",
                "-f", fmt_arg,
                "-p", &port.to_string(),
                "-b", "0.0.0.0",
                "--detect-zeroes=on",
                "--discard=unmap",
                "--aio=io_uring",
                "--cache=none",
                path,
            ])
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn();

        match child {
            Ok(mut c) => {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                match c.try_wait() {
                    Ok(Some(status)) => {
                        eprintln!(
                            "[ERR] qemu-nbd for {} exited immediately (code {:?})",
                            path,
                            status.code()
                        );
                        None
                    }
                    _ => Some(c),
                }
            }
            Err(e) => {
                eprintln!("[ERR] failed to spawn qemu-nbd for {}: {}", path, e);
                None
            }
        }
    }

    async fn unexport_iscsi(&self, tid: u32) {
        log(&format!("Unexporting iSCSI TID={}", tid));
        let t = tid.to_string();
        run_cmd(&[
            "tgtadm", "--lld", "iscsi", "--op", "unbind", "--mode", "target",
            "--tid", &t, "-I", "ALL",
        ])
        .await;
        run_cmd(&[
            "tgtadm", "--lld", "iscsi", "--op", "delete", "--mode", "target",
            "--tid", &t,
        ])
        .await;
    }

    async fn unexport_one(&self, t: &mut Target) {
        if self.config.boot_mode == BootMode::Iscsi || t.info.is_windows {
            self.unexport_iscsi(t.info.tid).await;
        }
        if let Some(mut c) = t.child.take() {
            let _ = c.kill().await;
        }
    }

    pub async fn shutdown_all(&self) {
        let mut map = self.targets.lock().await;
        for (_, t) in map.iter_mut() {
            self.unexport_one(t).await;
        }
        map.clear();
    }

    /// Re-scan VTOY_DIR; exports new images, respawns dead qemu-nbd, unexports vanished ones.
    pub async fn scan_and_export(&self) {
        let vtoy_dir = self.config.vtoy_dir.clone();
        if !tokio::fs::metadata(&vtoy_dir).await.is_ok() {
            if let Err(e) = tokio::fs::create_dir_all(&vtoy_dir).await {
                eprintln!("[ERR] Could not create {}: {}", vtoy_dir, e);
                return;
            }
        }
        let mut files: Vec<String> = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&vtoy_dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let lower = name.to_ascii_lowercase();
                // Filter all supported disk & ISO formats
                if lower.ends_with(".vtoy")
                    || lower.ends_with(".img")
                    || lower.ends_with(".raw")
                    || lower.ends_with(".qcow2")
                    || lower.ends_with(".vhd")
                    || lower.ends_with(".vhdx")
                    || lower.ends_with(".vmdk")
                    || lower.ends_with(".iso")
                {
                    files.push(name);
                }
            }
        }
        files.sort();

        let mut active_ids: HashSet<String> = HashSet::new();

        for filename in files {
            let vtoy_id = filename.clone();
            active_ids.insert(vtoy_id.clone());

            let mut map = self.targets.lock().await;
            if !map.contains_key(&vtoy_id) {
                let fullpath = format!("{}/{}", vtoy_dir, filename);
                let tid = self.alloc_tid().await;
                let port = self.alloc_port().await;
                let iqn = format!("{}:{}", self.config.iqn_prefix, sanitize_ipxe_key(&vtoy_id));
                let partitioned = has_partition_table(&fullpath).await;
                let root_dev = if partitioned {
                    "/dev/nbd0p2".to_string()
                } else {
                    "/dev/nbd0".to_string()
                };
                let fmt = detect_disk_format(&fullpath).await;
                let is_win = is_windows_image(&filename);

                let info = TargetInfo {
                    id: vtoy_id.clone(),
                    file: filename.clone(),
                    path: fullpath.clone(),
                    tid,
                    iqn: iqn.clone(),
                    nbd_port: port,
                    proc_pid: None,
                    alive: None,
                    has_partitions: partitioned,
                    root_dev,
                    format: fmt.as_qemu_arg().to_string(),
                    is_windows: is_win,
                };

                // Export storage backend
                let mut child = None;
                if is_win || self.config.boot_mode == BootMode::Iscsi {
                    self.export_iscsi(tid, &iqn, &fullpath).await;
                }
                // Also spawn NBD for vtoyboot or linux NBD mode
                if !is_win || self.config.boot_mode == BootMode::Nbd {
                    child = self.export_nbd(port, &fullpath, fmt).await;
                }

                map.insert(vtoy_id.clone(), Target { info, child });
            } else {
                let t = map.get_mut(&vtoy_id).unwrap();
                if self.config.boot_mode == BootMode::Nbd || (!t.info.is_windows && t.child.is_some()) {
                    let need_respawn = match t.child.as_mut() {
                        Some(c) => c.try_wait().ok().flatten().is_some(),
                        None => true,
                    };
                    if need_respawn {
                        let port = t.info.nbd_port;
                        log(&format!("qemu-nbd died, respawning on port {}", port));
                        let path = t.info.path.clone();
                        let fmt = detect_disk_format(&path).await;
                        if let Some(child) = self.export_nbd(port, &path, fmt).await {
                            t.child = Some(child);
                            t.info.proc_pid = t.child.as_ref().and_then(|c| c.id());
                            t.info.alive = Some(true);
                        }
                    }
                }
            }
        }

        let mut map = self.targets.lock().await;
        let stale: Vec<String> = map
            .keys()
            .filter(|k| !active_ids.contains(*k))
            .cloned()
            .collect();
        for k in stale {
            if let Some(mut t) = map.remove(&k) {
                self.unexport_one(&mut t).await;
                self.used_tids.lock().await.remove(&t.info.tid);
                self.used_ports.lock().await.remove(&t.info.nbd_port);
            }
        }
    }

    pub async fn health_targets(&self) -> Vec<TargetInfo> {
        let mut map = self.targets.lock().await;
        let mut out: Vec<TargetInfo> = Vec::with_capacity(map.len());
        for (_, t) in map.iter_mut() {
            let mut info = t.info.clone();
            if self.config.boot_mode == BootMode::Nbd || t.child.is_some() {
                info.alive = Some(match t.child.as_mut() {
                    Some(c) => c.try_wait().ok().flatten().is_none(),
                    None => false,
                });
            }
            out.push(info);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Builds 100% standalone, independent iPXE menu (No upstream iVentoy dependency)
    pub async fn build_standalone_ipxe_menu(&self) -> String {
        let snapshot = self.health_targets().await;
        // Apply win_cfg defaults
        let win_cfg = &self.config.json_cfg.windows_boot;
        let default_win_suffix = if win_cfg.default_mode == "vtoyboot" { "win_vtoy_" } else { "win_iscsi_" };
        let mut script = String::new();

        script.push_str("#!ipxe\n\n");
        script.push_str("set menu-timeout 15000\n");
        script.push_str("set submenu-timeout 15000\n");
        let default_item = snapshot.iter().find(|i| i.is_windows)
            .map(|i| format!("{}{}", default_win_suffix, sanitize_ipxe_key(&i.id)))
            .or_else(|| snapshot.first().map(|i| format!("vdisk_{}", sanitize_ipxe_key(&i.id))))
            .unwrap_or_else(|| "shell".into());
        script.push_str(&format!("isset ${{menu-default}} || set menu-default {}\n\n", default_item));

        script.push_str(":start\n");
        script.push_str("menu iVentoy-Rust Standalone PXE Network Boot\n\n");

        let mut win_items = Vec::new();
        let mut vdisk_items = Vec::new();
        let mut iso_items = Vec::new();
        let mut handlers = Vec::new();

        for info in &snapshot {
            let safe_id = sanitize_ipxe_key(&info.id);
            let label = sanitize_ipxe_text(&info.file);

            if info.is_windows {
                // Windows Dual Boot Modes
                let iscsi_key = format!("win_iscsi_{}", safe_id);
                let vtoy_key = format!("win_vtoy_{}", safe_id);

                win_items.push(format!("item {} [Win] {} (iSCSI Native Sanboot)", iscsi_key, label));
                win_items.push(format!("item {} [Win] {} (Ventoy vtoyboot Hook)", vtoy_key, label));

                // Handler: iSCSI
                let iqn = sanitize_ipxe_text(&info.iqn);
                handlers.push(format!(
                    ":{}\necho Booting Windows via iSCSI Target ({}) ...\nset root-path iscsi:${{next-server}}::::{}\nsanboot iscsi:${{next-server}}::::{} || goto boot_failed\n",
                    iscsi_key, iqn, iqn, iqn
                ));

                // Handler: vtoyboot
                handlers.push(format!(
                    ":{}\necho Booting Windows via Ventoy NBD Hook (Port {}) ...\nkernel {} initrd=initramfs.img ip=dhcp nbdroot=${{next-server}} nbdport={} root={} rw quiet\ninitrd {}\nboot || goto boot_failed\n",
                    vtoy_key, info.nbd_port, self.config.vtoy_kernel_url, info.nbd_port, info.root_dev, self.config.vtoy_initrd_url
                ));
            } else if info.file.to_ascii_lowercase().ends_with(".iso") {
                let item_key = format!("iso_{}", safe_id);
                iso_items.push(format!("item {} [ISO] {}", item_key, label));
                handlers.push(format!(
                    ":{}\necho Booting ISO via HTTP sanboot ...\nsanboot http://${{next-server}}:{}/iso/{} || goto boot_failed\n",
                    item_key, self.config.bridge_port, info.file
                ));
            } else {
                let item_key = format!("vdisk_{}", safe_id);
                vdisk_items.push(format!("item {} [{}] {}", item_key, info.format.to_uppercase(), label));

                handlers.push(format!(
                    ":{}\necho Booting Virtual Disk via NBD (Port {}) ...\nkernel {} initrd=initramfs.img ip=dhcp nbdroot=${{next-server}} nbdport={} root={} rw quiet\ninitrd {}\nboot || goto boot_failed\n",
                    item_key, info.nbd_port, self.config.vtoy_kernel_url, info.nbd_port, info.root_dev, self.config.vtoy_initrd_url
                ));
            }
        }

        if !win_items.is_empty() {
            script.push_str("item --gap -- --------------- Windows Native Systems ---------------\n");
            for it in win_items {
                script.push_str(&format!("{}\n", it));
            }
        }

        if !vdisk_items.is_empty() {
            script.push_str("\nitem --gap -- ----------------- Virtual Disks -----------------\n");
            for it in vdisk_items {
                script.push_str(&format!("{}\n", it));
            }
        }

        if !iso_items.is_empty() {
            script.push_str("\nitem --gap -- ---------------- ISO Installations ---------------\n");
            for it in iso_items {
                script.push_str(&format!("{}\n", it));
            }
        }

        script.push_str("\nitem --gap -- ---------------- Operations & Tools ----------------\n");
        script.push_str("item shell  Drop into iPXE Shell\n");
        script.push_str("item reboot Reboot Computer\n");
        script.push_str("item exit   Exit to Local BIOS/UEFI\n\n");

        script.push_str("choose --timeout ${menu-timeout} --default ${menu-default} selected || goto shell\n");
        script.push_str("goto ${selected}\n\n");

        script.push_str(":shell\nshell\n\n:reboot\nreboot\n\n:exit\nexit\n\n");
        script.push_str(":boot_failed\necho Boot failed! Dropping to shell in 5 seconds...\nsleep 5\ngoto shell\n\n");

        for h in handlers {
            script.push_str(&h);
            script.push_str("\n");
        }

        script
    }

    /// Legacy menu generator for backward compatibility
    #[allow(dead_code)]
    pub async fn build_vtoy_menu(&self) -> (String, String) {
        let snapshot = self.health_targets().await;
        let mut menu_items: Vec<String> = Vec::new();
        let mut handlers: Vec<String> = Vec::new();
        menu_items.push("\nitem --gap -- ------------- VTOY Images -------------".to_string());
        for info in &snapshot {
            let safe_id = sanitize_ipxe_key(&info.id);
            let label = sanitize_ipxe_text(&info.file);
            let item_key = format!("vtoy_{}", safe_id);
            menu_items.push(format!("item {} [VTOY] {}", item_key, label));
            let h = if self.config.boot_mode == BootMode::Iscsi {
                let iqn = sanitize_ipxe_text(&info.iqn);
                vec![
                    format!(":{}", item_key),
                    format!("echo Booting {} via iSCSI SANBOOT...", label),
                    format!("set root-path iscsi:${{next-server}}::::{}", iqn),
                    format!("sanboot iscsi:${{next-server}}::::{} || goto boot_failed", iqn),
                    "boot".to_string(),
                ]
            } else {
                vec![
                    format!(":{}", item_key),
                    format!("echo Booting {} via NBD root ({})...", label, info.root_dev),
                    format!(
                        "kernel {} initrd=initramfs.img ip=dhcp nbdroot=${{next-server}} nbdport={} root={} rw quiet",
                        self.config.vtoy_kernel_url, info.nbd_port, info.root_dev
                    ),
                    format!("initrd {}", self.config.vtoy_initrd_url),
                    "boot".to_string(),
                ]
            };
            handlers.push(h.join("\n"));
        }
        handlers.push(":boot_failed\necho Boot failed! Dropping to shell...\nshell\n".to_string());
        (menu_items.join("\n"), handlers.join("\n"))
    }
}
