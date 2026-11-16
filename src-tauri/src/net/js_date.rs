//! JS `Date.parse`·`new Date(ms).toISOString()` 중 API 응답 해석이 쓰는 부분.
//!
//! `Date.parse` 는 V8 의 관대한 해석(공백 구분·월 이름·로컬 시간…)이라 전부 옮기지 않는다. 받는 것은 ISO 8601 꼴뿐이다:
//! `YYYY`·`YYYY-MM`·`YYYY-MM-DD`, 거기에 `THH:MM[:SS[.fff…]]` 와 `Z`·`±HH:MM`·`±HHMM`(시간대가 있어야 한다 — 없으면 TS 는 로컬 시간이라 값이 머신마다 다르다).
//! 받는 입력은 V8 과 같은 값(날짜 일은 1..=31 이고 달력을 넘으면 다음 달로 밀린다: `2023-02-29` → 3월 1일)이고, 못 받는 입력은 `None`(TS 가 값을 주는 입력만 이쪽이 더 엄격하다).
use super::json::Json;

const DAY_MS: f64 = 86_400_000.0;
/// ECMAScript TimeClip 상한(±8.64e15 ms)
const TIME_MAX: f64 = 8.64e15;

/// 1970-01-01 기준 일수(윤년 달력, 월 1..=12). 일(day)이 달 길이를 넘어도 그대로 더한다(MakeDay 와 같다)
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn digits(b: &[u8], i: &mut usize, n: usize) -> Option<i64> {
    let s = b.get(*i..*i + n)?;
    if !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    *i += n;
    Some(s.iter().fold(0i64, |a, c| a * 10 + i64::from(c - b'0')))
}

/// `Date.parse(s)`(ms). 해석할 수 없거나 시간대 없는 날짜·시각이면 `None`
pub fn parse_date_ms(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    // 확장 연도 `±YYYYYY`(`-000000` 은 규격이 금지하고 V8 은 로컬 시간의 옛 해석으로 넘어간다)
    let year = match b.first() {
        Some(sign @ (b'+' | b'-')) => {
            i = 1;
            let y = digits(b, &mut i, 6)?;
            if *sign == b'-' {
                if y == 0 {
                    return None;
                }
                -y
            } else {
                y
            }
        }
        _ => digits(b, &mut i, 4)?,
    };
    let (mut month, mut day) = (1, 1);
    if b.get(i) == Some(&b'-') {
        i += 1;
        month = digits(b, &mut i, 2)?;
        if b.get(i) == Some(&b'-') {
            i += 1;
            day = digits(b, &mut i, 2)?;
        }
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (mut h, mut mi, mut sec, mut ms) = (0, 0, 0, 0);
    let mut has_time = false;
    if b.get(i) == Some(&b'T') {
        i += 1;
        has_time = true;
        h = digits(b, &mut i, 2)?;
        if b.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        mi = digits(b, &mut i, 2)?;
        if b.get(i) == Some(&b':') {
            i += 1;
            sec = digits(b, &mut i, 2)?;
            if b.get(i) == Some(&b'.') {
                i += 1;
                let start = i;
                while b.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                if i == start {
                    return None;
                }
                let mut frac = b[start..i].iter().take(3).fold(0i64, |a, c| a * 10 + i64::from(c - b'0'));
                for _ in (i - start)..3 {
                    frac *= 10;
                }
                ms = frac;
            }
        }
        if h > 24 || mi > 59 || sec > 59 || (h == 24 && (mi != 0 || sec != 0 || ms != 0)) {
            return None;
        }
    }
    let mut offset_min = 0i64;
    match b.get(i) {
        Some(b'Z') => i += 1,
        // 오프셋은 시각이 있을 때만(`2026-01-01+09:00` 은 V8 이 NaN 으로 본다)
        Some(sign @ (b'+' | b'-')) if has_time => {
            let neg = *sign == b'-';
            i += 1;
            let oh = digits(b, &mut i, 2)?;
            if b.get(i) == Some(&b':') {
                i += 1;
            }
            let om = digits(b, &mut i, 2)?;
            if oh > 23 || om > 59 {
                return None;
            }
            offset_min = (oh * 60 + om) * if neg { -1 } else { 1 };
        }
        // 시간대가 없으면 시각이 있는 입력은 TS 가 로컬 시간으로 푼다
        _ if has_time => return None,
        _ => {}
    }
    if i != b.len() {
        return None;
    }
    let days = days_from_civil(year, month, 1) + (day - 1);
    let t = days as f64 * DAY_MS + (h * 3_600_000 + mi * 60_000 + sec * 1000 + ms) as f64 - offset_min as f64 * 60_000.0;
    (t.abs() <= TIME_MAX).then_some(t)
}

/// `toEpoch(iso)`: 문자열이 아니거나 비었거나 해석할 수 없으면 0, 아니면 초(내림)
pub fn to_epoch(v: Option<&Json>) -> f64 {
    match v {
        Some(Json::Str(s)) if !s.is_empty() => parse_date_ms(s).map_or(0.0, |t| (t / 1000.0).floor()),
        _ => 0.0,
    }
}

/// `new Date(ms).toISOString()`. 유효하지 않은 시각(NaN·범위 밖)이면 `None`(TS 는 RangeError)
pub fn iso_string(ms: f64) -> Option<String> {
    if !ms.is_finite() || ms.abs() > TIME_MAX {
        return None;
    }
    let t = ms.trunc();
    let days = (t / DAY_MS).floor() as i64;
    let rem = (t - days as f64 * DAY_MS) as i64;
    let (y, m, d) = civil_from_days(days);
    let year = if (0..=9999).contains(&y) { format!("{y:04}") } else { format!("{}{:06}", if y < 0 { '-' } else { '+' }, y.abs()) };
    Some(format!("{year}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 기본_형식() {
        assert_eq!(parse_date_ms("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_date_ms("2024-01-01T00:00:00+0900"), Some(1_704_034_800_000.0));
        assert_eq!(parse_date_ms("2023-02-29"), Some(1_677_628_800_000.0));
        assert_eq!(parse_date_ms("1969-12-31T23:59:59.500Z"), Some(-500.0));
        assert_eq!(parse_date_ms("2024-01-01T00:00:00"), None);
        assert_eq!(parse_date_ms("2024-01-01T24:00:01Z"), None);
    }

    #[test]
    fn iso_왕복() {
        assert_eq!(iso_string(0.0).as_deref(), Some("1970-01-01T00:00:00.000Z"));
        assert_eq!(iso_string(-500.0).as_deref(), Some("1969-12-31T23:59:59.500Z"));
        assert_eq!(iso_string(8.64e15).as_deref(), Some("+275760-09-13T00:00:00.000Z"));
        assert_eq!(iso_string(8.64e15 + 1.0), None);
    }
}
