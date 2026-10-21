//! Dates for the file lists, in the phone's local time.

use std::time::{SystemTime, UNIX_EPOCH};

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// (year, month 1-12, day, hour, minute) of `t` in local time.
#[cfg(unix)]
fn local_parts(t: SystemTime) -> (i32, u32, u32, u32, u32) {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // SAFETY: localtime_r only writes into the zeroed tm we hand it.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let tt = secs as libc::time_t;
        libc::localtime_r(&tt, &mut tm);
        (tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32)
    }
}

#[cfg(not(unix))]
fn local_parts(t: SystemTime) -> (i32, u32, u32, u32, u32) {
    // UTC on the desktop preview; civil-from-days (Howard Hinnant)
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as i32, m as u32, d as u32, (rem / 3600) as u32, (rem % 3600 / 60) as u32)
}

/// "2026-10-21 14:05"
pub fn full(t: SystemTime) -> String {
    let (y, m, d, hh, mm) = local_parts(t);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
}

/// "21 Oct 14:05" this year, "21 Oct 2025" before that.
pub fn short(t: SystemTime) -> String {
    let (y, m, d, hh, mm) = local_parts(t);
    let this_year = local_parts(SystemTime::now()).0;
    let month = MONTHS[(m as usize).saturating_sub(1).min(11)];
    if y == this_year { format!("{d} {month} {hh:02}:{mm:02}") } else { format!("{d} {month} {y}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn dates_are_formatted() {
        let t = UNIX_EPOCH + Duration::from_secs(86_400 * 365);
        assert!(full(t).starts_with("1971-"), "{}", full(t));
        assert!(short(t).ends_with("1971"), "{}", short(t));
        let now = short(SystemTime::now());
        assert!(now.contains(':'), "this year shows the time: {now}");
    }
}
