use crate::config::{BootMode, Config};
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
    if clean.is_empty() { "item".into() } else { clean }
}

pub fn sanitize_ipxe_text(text: &str) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '$' { '_' } else { c })
        .collect();
    clean.trim().to_string()
}

#[derive(Clone, Serialize)]
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

async fn run_cmd(cmd: &[&str]) -> Option<String> {
    match Command::new(cmd[0]).args(&cmd[1..]).output().await {
        Ok(out) => {
            if out.status.success() {
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                eprintln!(
                    "[CMD ERR] {:?} failed: {}",
                    cmd,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                None
            }
        }
        Err(e) => {
            eprintln!("[CMD ERR] {:?} failed to spawn: {}", cmd, e);
            None
        }
    }
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

    async fn alloc_ids(&self) -> (u32, u16) {
        let mut tids = self.used_tids.lock().await;
        let mut tid = self.config.tgt_base_tid + 1;
        while tids.contains(&tid) {
            tid += 1;
        }
        tids.insert(tid);
        let mut ports = self.used_ports.lock().await;
        let mut port = self.config.nbd_base_port + 1;
        while ports.contains(&port) {
            port += 1;
        }
        ports.insert(port);
        (tid, port)
    }

    async fn export_iscsi(&self, tid: u32, iqn: &str, path: &str) {
        log(&format!(
            "Exporting iSCSI Target: TID={} IQN={} Backing={}",
            tid, iqn, path
        ));
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

    async fn export_nbd(&self, port: u16, path: &str) -> Option<Child> {
        log(&format!("Exporting NBD: Port={} Backing={}", port, path));
        let child = Command::new("qemu-nbd")
            .args(["-t", "-f", "raw", "-p", &port.to_string(), "-b", "0.0.0.0", path])
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
            "tgtadm", "--lld", "iscsi", "--op", "delete", "--mode", "logicalunit",
            "--tid", &t, "--lun", "1",
        ])
        .await;
        run_cmd(&[
            "tgtadm", "--lld", "iscsi", "--op", "delete", "--mode", "target",
            "--tid", &t,
        ])
        .await;
    }

    async fn unexport_one(&self, target: &mut Target) {
        if self.config.boot_mode == BootMode::Iscsi {
            self.unexport_iscsi(target.info.tid).await;
        } else if let Some(mut child) = target.child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
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
                if name.ends_with(".vtoy") || name.ends_with(".img") {
                    files.push(name);
                }
            }
        }
        files.sort();

        let mut active_ids: HashSet<String> = HashSet::new();
        for (idx, fname) in files.iter().enumerate() {
            let stem = std::path::Path::new(fname)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut vtoy_id = sanitize_ipxe_key(&stem);
            if vtoy_id.is_empty() || vtoy_id == "item" {
                vtoy_id = format!("disk_{}", idx + 1);
            }
            active_ids.insert(vtoy_id.clone());
            let full_path = format!("{}/{}", vtoy_dir.trim_end_matches('/'), fname);

            let mut map = self.targets.lock().await;
            if !map.contains_key(&vtoy_id) {
                let (tid, nbd_port) = self.alloc_ids().await;
                let iqn = format!("{}-{}", self.config.iqn_prefix, vtoy_id);
                let has_parts = crate::image::has_partition_table(&full_path).await;
                let mut info = TargetInfo {
                    id: vtoy_id.clone(),
                    file: fname.clone(),
                    path: full_path.clone(),
                    tid,
                    iqn: sanitize_ipxe_text(&iqn),
                    nbd_port,
                    proc_pid: None,
                    alive: None,
                    has_partitions: has_parts,
                    root_dev: "/dev/nbd0".into(),
                };
                if self.config.boot_mode == BootMode::Iscsi {
                    let iqn2 = info.iqn.clone();
                    let p = info.path.clone();
                    self.export_iscsi(tid, &iqn2, &p).await;
                } else {
                    let p = info.path.clone();
                    if let Some(child) = self.export_nbd(nbd_port, &p).await {
                        info.proc_pid = child.id();
                        info.alive = Some(true);
                        map.insert(
                            vtoy_id.clone(),
                            Target { info, child: Some(child) },
                        );
                        continue;
                    }
                }
                map.insert(vtoy_id.clone(), Target { info, child: None });
            } else if self.config.boot_mode == BootMode::Nbd {
                let t = map.get_mut(&vtoy_id).unwrap();
                let need_respawn = match t.child.as_mut() {
                    Some(c) => c.try_wait().ok().flatten().is_some(),
                    None => true,
                };
                if need_respawn {
                    let port = t.info.nbd_port;
                    log(&format!(
                        "qemu-nbd died, respawning on port {}",
                        port
                    ));
                    let path = t.info.path.clone();
                    if let Some(child) = self.export_nbd(port, &path).await {
                        t.child = Some(child);
                        t.info.proc_pid = t.child.as_ref().and_then(|c| c.id());
                        t.info.alive = Some(true);
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
            if self.config.boot_mode == BootMode::Nbd {
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

    /// Build (menu_block, handler_block) with byte-format parity with Python v2.
    /// Python: new = orig[:pos] + menu_block + orig[pos:] + handler_block
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
                    format!(
                        "sanboot iscsi:${{next-server}}::::{} || goto boot_failed",
                        iqn
                    ),
                    "boot".to_string(),
                ]
            } else {
                vec![
                    format!(":{}", item_key),
                    format!(
                        "echo Booting {} via NBD root ({})...",
                        label, info.root_dev
                    ),
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
        handlers.push(
            ":boot_failed\necho Boot failed! Press any key to return to menu.\nprompt\ngoto start\n"
                .to_string(),
        );
        let menu_block = format!("{}\n", menu_items.join("\n"));
        let handler_block = format!("\n\n{}\n", handlers.join("\n\n"));
        (menu_block, handler_block)
    }
}
