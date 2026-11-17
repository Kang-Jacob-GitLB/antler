//! 한도 해제 시각의 표시(TS `client.ts` 의 `formatTime` — `toLocaleTimeString('ko-KR', {hour:'2-digit', minute:'2-digit'})`).
//! 문구는 12시간제 `오전 09:05`·`오후 03:05`(0시는 오전 12, 정오는 오후 12)이다.
//!
//! 로컬 시각은 새 크레이트 없이 Windows 의 시간대 API(`SystemTimeToTzSpecificLocalTime` — 이미 쓰는 `windows` 크레이트의 `Win32_System_Time` 기능)로 만든다.
//! 시간대 규칙(서머타임 포함)은 OS 가 그 날짜에 맞춰 적용한다. Windows 가 아니거나 변환이 안 되면 UTC `HH:MM(UTC)` 로 물러난다
//! (macOS·Linux 는 Phase 6 에서 `localtime_r` 등으로 채운다).

/// JS `Date` 가 표현하는 시각의 한계(ms) — 넘으면 `Invalid Date`
const JS_DATE_LIMIT_MS: f64 = 8.64e15;

/// epoch 초 → 표시 문구
pub fn format_epoch_sec(epoch_sec: f64) -> String {
    if !epoch_sec.is_finite() || (epoch_sec * 1000.0).abs() > JS_DATE_LIMIT_MS {
        return "Invalid Date".to_string();
    }
    let secs = epoch_sec.floor() as i64;
    match local_hour_minute(secs) {
        Some((h, m)) => format_clock(h, m),
        None => utc_clock(secs),
    }
}

/// 12시간제 문구(ko-KR). `hour` 는 0..=23
pub fn format_clock(hour: u32, minute: u32) -> String {
    let (ampm, h12) = (if hour < 12 { "오전" } else { "오후" }, if hour.is_multiple_of(12) { 12 } else { hour % 12 });
    format!("{ampm} {h12:02}:{minute:02}")
}

fn utc_clock(secs: i64) -> String {
    let day_sec = secs.rem_euclid(86_400);
    format!("{:02}:{:02}(UTC)", day_sec / 3600, day_sec % 3600 / 60)
}

/// 1970-01-01 기준 일수 → (연, 월, 일). Howard Hinnant 의 civil_from_days
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(windows)]
fn local_hour_minute(secs: i64) -> Option<(u32, u32)> {
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::System::Time::SystemTimeToTzSpecificLocalTime;

    let (days, day_sec) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, mo, d) = civil_from_days(days);
    // SYSTEMTIME 은 1601..=30827 년만 표현한다
    if !(1601..=30_827).contains(&y) {
        return None;
    }
    let utc = SYSTEMTIME {
        wYear: y as u16,
        wMonth: mo as u16,
        wDayOfWeek: 0,
        wDay: d as u16,
        wHour: (day_sec / 3600) as u16,
        wMinute: (day_sec % 3600 / 60) as u16,
        wSecond: (day_sec % 60) as u16,
        wMilliseconds: 0,
    };
    let mut local = SYSTEMTIME::default();
    // 시간대 정보를 주지 않으면(None) 현재 시간대를 쓴다
    // SAFETY: 두 SYSTEMTIME 은 이 함수의 지역 값이고 호출 동안 유효하다
    unsafe { SystemTimeToTzSpecificLocalTime(None, &utc, &mut local) }.ok()?;
    Some((u32::from(local.wHour), u32::from(local.wMinute)))
}

#[cfg(not(windows))]
fn local_hour_minute(_secs: i64) -> Option<(u32, u32)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 시각_문구는_ko_kr_12시간제다() {
        // Node: new Date(...).toLocaleTimeString('ko-KR', {hour:'2-digit', minute:'2-digit', timeZone:'UTC'})
        let table = [(0, 5, "오전 12:05"), (9, 5, "오전 09:05"), (11, 59, "오전 11:59"), (12, 5, "오후 12:05"), (15, 5, "오후 03:05"), (23, 5, "오후 11:05"), (0, 0, "오전 12:00")];
        for (h, m, want) in table {
            assert_eq!(format_clock(h, m), want, "{h}:{m}");
        }
    }

    #[test]
    fn 날짜_변환이_알려진_날과_같다() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(-135_140), (1600, 1, 1));
        assert_eq!(civil_from_days(1_900_000_000 / 86_400), (2030, 3, 17));
    }

    #[test]
    fn 범위를_벗어난_값은_invalid_date() {
        assert_eq!(format_epoch_sec(f64::NAN), "Invalid Date");
        assert_eq!(format_epoch_sec(f64::INFINITY), "Invalid Date");
        assert_eq!(format_epoch_sec(1e13), "Invalid Date");
    }

    /// 로컬 변환은 시간대마다 다르므로 값 대신 성질을 본다: 문구 모양이 맞고 UTC 와 15분 단위로 어긋난다(모든 시간대 오프셋은 15분의 배수다)
    #[test]
    fn 로컬_시각은_utc_와_15분_단위로_어긋난다() {
        for epoch in [0.0_f64, 1_900_000_000.0, 1_751_000_000.0, 1_767_225_600.5] {
            let text = format_epoch_sec(epoch);
            let secs = epoch.floor() as i64;
            let utc_min = secs.rem_euclid(86_400) / 60;
            if let Some(rest) = text.strip_prefix("오전 ").or_else(|| text.strip_prefix("오후 ")) {
                let pm = text.starts_with("오후");
                let (h, m) = rest.split_once(':').expect("hh:mm");
                let (h12, m): (i64, i64) = (h.parse().unwrap(), m.parse().unwrap());
                assert!((1..=12).contains(&h12) && (0..60).contains(&m), "{text}");
                let local_min = ((h12 % 12) + if pm { 12 } else { 0 }) * 60 + m;
                assert_eq!((local_min - utc_min).rem_euclid(15), 0, "{text} (epoch {epoch})");
            } else {
                assert_eq!(text, utc_clock(secs));
            }
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn windows_가_아니면_utc_로_물러난다() {
        assert_eq!(format_epoch_sec(1_900_000_000.0), "17:46(UTC)");
    }
}
