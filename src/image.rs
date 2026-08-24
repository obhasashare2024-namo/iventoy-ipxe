use std::path::Path;

/// Checks if the disk image has a partition table (MBR or GPT).
/// Returns true if partitioned (/dev/nbd0p1), false for raw whole-disk filesystem (/dev/nbd0).
pub async fn has_partition_table(image_path: &str) -> bool {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 520];
    let f = tokio::fs::File::open(Path::new(image_path)).await;
    let mut f = match f {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[WARN] Failed to read partition table for {}: {}", image_path, e);
            return false;
        }
    };
    let n = match f.read(&mut buf).await {
        Ok(n) => n,
        Err(e) => {
            eprintln!("[WARN] Failed to read partition table for {}: {}", image_path, e);
            return false;
        }
    };
    if n >= 512 && buf[510] == 0x55 && buf[511] == 0xAA {
        for i in 0..4 {
            let base = 446 + i * 16;
            let boot_flag = buf[base];
            let part_type = buf[base + 4];
            if (boot_flag == 0x00 || boot_flag == 0x80) && part_type != 0x00 {
                return true;
            }
        }
    }
    if n >= 520 && &buf[512..520] == b"EFI PART" {
        return true;
    }
    false
}
