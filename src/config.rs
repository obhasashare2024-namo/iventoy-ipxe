#[derive(Clone, Debug)]
pub struct Config {
    pub iventoy_real: String,
    pub bridge_host: String,
    pub bridge_port: u16,
    pub vtoy_dir: String,
    pub boot_mode: BootMode,
    pub tgt_base_tid: u32,
    pub iqn_prefix: String,
    pub nbd_base_port: u16,
    pub vtoy_kernel_url: String,
    pub vtoy_initrd_url: String,
    pub kernel_path: String,
    pub initrd_path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootMode {
    Iscsi,
    Nbd,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            iventoy_real: "127.0.0.1:16001".into(),
            bridge_host: "0.0.0.0".into(),
            bridge_port: 16000,
            vtoy_dir: "/srv/vtoy/iso".into(),
            boot_mode: BootMode::Iscsi,
            tgt_base_tid: 100,
            iqn_prefix: "iqn.2026.lan:vtoy".into(),
            nbd_base_port: 10810,
            vtoy_kernel_url: "http://${next-server}:16000/vtoy/vmlinuz".into(),
            vtoy_initrd_url: "http://${next-server}:16000/vtoy/initramfs.img".into(),
            kernel_path: "/srv/vtoy/boot/vmlinuz".into(),
            initrd_path: "/srv/vtoy/boot/initramfs.img".into(),
        }
    }
}

fn parse_value(v: &str) -> String {
    let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
    v
}

pub fn load_simple_yaml(path: &str) -> Result<Config, String> {
    let mut cfg = Config::default();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(cfg),
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            let v = parse_value(v);
            match k {
                "IVENTOY_REAL" => cfg.iventoy_real = v,
                "BRIDGE_HOST" => cfg.bridge_host = v,
                "BRIDGE_PORT" => {
                    cfg.bridge_port = v.parse().unwrap_or(cfg.bridge_port)
                }
                "VTOY_DIR" => cfg.vtoy_dir = v,
                "BOOT_MODE" => {
                    cfg.boot_mode = if v.eq_ignore_ascii_case("nbd") {
                        BootMode::Nbd
                    } else {
                        BootMode::Iscsi
                    }
                }
                "TGT_BASE_TID" => cfg.tgt_base_tid = v.parse().unwrap_or(cfg.tgt_base_tid),
                "IQN_PREFIX" => cfg.iqn_prefix = v,
                "NBD_BASE_PORT" => cfg.nbd_base_port = v.parse().unwrap_or(cfg.nbd_base_port),
                "VTOY_KERNEL_URL" => cfg.vtoy_kernel_url = v,
                "VTOY_INITRD_URL" => cfg.vtoy_initrd_url = v,
                "KERNEL_PATH" => cfg.kernel_path = v,
                "INITRD_PATH" => cfg.initrd_path = v,
                _ => {}
            }
        }
    }
    Ok(cfg)
}

