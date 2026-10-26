// Linear's timestamps share one format (`iso`), so the code compares them as text; GitHub's
// differ ("…:00Z"), so anything compared across the two is read to a number (`instant`).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn since_epoch() -> Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()
}

pub fn now_ms() -> i64 {
    since_epoch().as_millis() as i64
}

pub fn now_secs() -> u64 {
    since_epoch().as_secs()
}

pub fn now_iso() -> String {
    iso(now_ms())
}

// e.g. 2026-09-26T01:02:03.456Z
pub fn iso(ms: i64) -> String {
    let (days, ms_of_day) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let (hour, minute, second, millis) = (ms_of_day / 3_600_000, ms_of_day / 60_000 % 60, ms_of_day / 1000 % 60, ms_of_day % 1000);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

// This machine's wall clock, e.g. 14:51:37, for a person reading `listen`'s terminal: the
// system's own zone, through its C library, as no crate here has it; UTC where it can't be read.
pub fn local_clock() -> String {
    let secs = local::seconds_of_day(now_secs()).unwrap_or(now_secs() % 86_400);
    format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

#[cfg(windows)]
mod local {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }

    pub fn seconds_of_day(_now: u64) -> Option<u64> {
        let mut time = SystemTime::default();
        // SAFETY: GetLocalTime only fills in the struct it is handed.
        unsafe { GetLocalTime(&mut time) };
        Some(u64::from(time.hour) * 3600 + u64::from(time.minute) * 60 + u64::from(time.second))
    }
}

#[cfg(unix)]
mod local {
    use std::ffi::c_int;

    // `struct tm`'s fields as every libc starts it, then room for the `tm_gmtoff` and
    // `tm_zone` that some add.
    #[repr(C)]
    #[derive(Default)]
    struct Tm {
        sec: c_int,
        min: c_int,
        hour: c_int,
        rest: [c_int; 6],
        extra: [usize; 4],
    }

    unsafe extern "C" {
        fn localtime_r(time: *const i64, tm: *mut Tm) -> *mut Tm;
    }

    pub fn seconds_of_day(now: u64) -> Option<u64> {
        let (time, mut tm) = (i64::try_from(now).ok()?, Tm::default());
        // SAFETY: localtime_r reads the time and writes only the struct it is handed, which
        // is at least as large as the platform's.
        let filled = unsafe { localtime_r(&time, &mut tm) };
        (!filled.is_null()).then(|| u64::try_from(tm.hour * 3600 + tm.min * 60 + tm.sec).ok()).flatten()
    }
}

#[cfg(not(any(windows, unix)))]
mod local {
    pub fn seconds_of_day(_now: u64) -> Option<u64> {
        None
    }
}

// None for a time without a zone, which is ambiguous, or one out of range.
pub fn instant(text: &str) -> Option<i64> {
    let (date, time) = text.split_once('T')?;
    let mut ymd = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (ymd.next()??, ymd.next()??, ymd.next()??);
    let at = time.find(['Z', 'z', '+', '-'])?;
    let (clock, zone) = time.split_at(at);
    // A clock's second may be a leap second.
    let fields = |text: &str, n: usize| {
        let parts: Vec<i64> = text.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        let (h, m, s) = match parts[..] {
            [h, m] if n == 2 => (h, m, 0),
            [h, m, s] if n == 3 => (h, m, s),
            _ => return None,
        };
        ((0..24).contains(&h) && (0..60).contains(&m) && (0..=60).contains(&s)).then_some((h, m, s))
    };
    let offset_ms = match zone {
        "Z" | "z" => 0,
        _ => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (h, m, _) = fields(&zone[1..], 2)?;
            sign * (h * 60 + m) * 60_000
        }
    };
    let (clock, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let (h, m, s) = fields(clock, 3)?;
    if !fraction.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let millis = if fraction.is_empty() { 0 } else { format!("{fraction:0<3}").get(..3)?.parse::<i64>().ok()? };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + m) * 60_000 + s * 1000 + millis - offset_ms)
}

// The minute `minute` of an hour on a fixed day, as a timestamp and as milliseconds.
#[cfg(test)]
pub fn t(minute: u32) -> String {
    format!("2026-09-26T01:{minute:02}:00Z")
}

#[cfg(test)]
pub fn ms(minute: u32) -> i64 {
    instant(&t(minute)).unwrap()
}

#[cfg(test)]
mod tests;
