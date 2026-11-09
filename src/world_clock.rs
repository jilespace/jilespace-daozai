//! 命令面板里的小型世界时钟：中文/英文城市名 → 时区 → 当地时间。
//!
//! 不联网，直接用 `chrono-tz` 的时区数据库；查询只匹配城市名与常见别名，
//! 避免“时间”“城市”这类泛词把整张表都刷出来。

use chrono::Local;

#[derive(Debug)]
pub struct City {
    pub name: &'static str,
    pub english: &'static str,
    pub tz: &'static str,
}

const CITIES: &[City] = &[
    City { name: "北京", english: "beijing", tz: "Asia/Shanghai" },
    City { name: "上海", english: "shanghai", tz: "Asia/Shanghai" },
    City { name: "香港", english: "hong kong", tz: "Asia/Hong_Kong" },
    City { name: "台北", english: "taipei", tz: "Asia/Taipei" },
    City { name: "东京", english: "tokyo", tz: "Asia/Tokyo" },
    City { name: "首尔", english: "seoul", tz: "Asia/Seoul" },
    City { name: "新加坡", english: "singapore", tz: "Asia/Singapore" },
    City { name: "曼谷", english: "bangkok", tz: "Asia/Bangkok" },
    City { name: "悉尼", english: "sydney", tz: "Australia/Sydney" },
    City { name: "奥克兰", english: "auckland", tz: "Pacific/Auckland" },
    City { name: "伦敦", english: "london", tz: "Europe/London" },
    City { name: "巴黎", english: "paris", tz: "Europe/Paris" },
    City { name: "柏林", english: "berlin", tz: "Europe/Berlin" },
    City { name: "莫斯科", english: "moscow", tz: "Europe/Moscow" },
    City { name: "迪拜", english: "dubai", tz: "Asia/Dubai" },
    City { name: "孟买", english: "mumbai", tz: "Asia/Kolkata" },
    City { name: "纽约", english: "new york", tz: "America/New_York" },
    City { name: "洛杉矶", english: "los angeles", tz: "America/Los_Angeles" },
    City { name: "旧金山", english: "san francisco", tz: "America/Los_Angeles" },
    City { name: "多伦多", english: "toronto", tz: "America/Toronto" },
    City { name: "圣保罗", english: "sao paulo", tz: "America/Sao_Paulo" },
];

/// 匹配城市名；最多返回两个，按表内顺序。
pub fn matching(query: &str) -> Vec<&'static City> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    CITIES
        .iter()
        .filter(|city| city.name.contains(&needle) || city.english.contains(&needle))
        .take(2)
        .collect()
}

/// 把某一时刻转换到指定时区，返回「14:05」或「14:05 明天」。
pub fn format_at(tz_name: &str, now: chrono::DateTime<Local>) -> Option<String> {
    let tz: chrono_tz::Tz = tz_name.parse().ok()?;
    let local = now.with_timezone(&tz);
    let here = now.date_naive();
    let there = local.date_naive();
    let suffix = if there == here {
        String::new()
    } else if Some(there) == here.succ_opt() {
        " 明天".to_string()
    } else if Some(there) == here.pred_opt() {
        " 昨天".to_string()
    } else {
        format!(" {}", local.format("%m-%d"))
    };
    Some(format!("{}{}", local.format("%H:%M"), suffix))
}

/// 当前时刻的城市时间。
pub fn now_at(tz_name: &str) -> Option<String> {
    format_at(tz_name, Local::now())
}

/// 查询命中的城市时间，返回 (标题, 用于复制的文字)。
pub fn lookup(query: &str) -> Vec<(String, String)> {
    matching(query)
        .into_iter()
        .filter_map(|city| {
            let time = now_at(city.tz)?;
            let title = format!("{} {time}", city.name);
            Some((title.clone(), title))
        })
        .collect()
}

/// 把秒数格式化成 mm:ss（超过一小时用 h:mm:ss）。
pub fn clock_label(seconds: f64) -> String {
    let seconds = seconds.max(0.0).ceil() as u64;
    if seconds >= 3600 {
        format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

/// 给命令面板的说明文案补一个简短时区名，例如 `Asia/Tokyo`。
pub fn tz_label(tz_name: &str) -> &str {
    tz_name
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn instant() -> chrono::DateTime<Local> {
        // 2026-01-01 00:00 UTC：东京 09:00、纽约前一天 19:00、伦敦 00:00。
        let utc = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        utc.with_timezone(&Local)
    }

    #[test]
    fn formats_times_with_day_suffixes() {
        let now = instant();
        assert_eq!(format_at("Asia/Tokyo", now).unwrap(), "09:00");
        let london = format_at("Europe/London", now).unwrap();
        assert!(london.starts_with("00:00"), "伦敦时间：{london}");
        let new_york = format_at("America/New_York", now).unwrap();
        assert!(new_york.starts_with("19:00"), "纽约时间：{new_york}");
        assert!(new_york.contains("昨天") || new_york.contains("明天"), "跨日应带日期后缀：{new_york}");
        assert!(format_at("Not/AZone", now).is_none());
    }

    #[test]
    fn matching_uses_chinese_and_english_names() {
        assert_eq!(matching("东京").first().map(|city| city.name), Some("东京"));
        assert_eq!(matching("tokyo").first().map(|city| city.name), Some("东京"));
        assert!(matching("shenzhen").is_empty(), "不在表内的城市不匹配");
        assert_eq!(matching("  ").len(), 0);
        assert!(matching("new").iter().any(|city| city.name == "纽约"));
        assert!(matching("纽约").len() == 1);
    }

    #[test]
    fn lookup_returns_copyable_titles() {
        let entries = lookup("东京");
        assert_eq!(entries.len(), 1);
        let (title, copy) = &entries[0];
        assert!(title.starts_with("东京 "));
        assert_eq!(title, copy);
    }
}
