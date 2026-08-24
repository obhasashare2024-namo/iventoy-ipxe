/// Format unix seconds as "YYYY-MM-DD HH:MM:SS" (UTC), no external deps.
pub fn format_datetime(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let mut y: i64 = 1970;
    let mut d = days;
    loop {
        let leap = is_leap(y);
        let len = if leap { 366 } else { 365 };
        if d < len {
            break;
        }
        d -= len;
        y += 1;
    }
    let mlens = [
        31,
        if is_leap(y) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 0usize;
    while d >= mlens[mo] {
        d -= mlens[mo];
        mo += 1;
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y,
        mo + 1,
        d + 1,
        h,
        m,
        s
    )
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
