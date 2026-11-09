//! 天气：Open-Meteo 免费接口（无需账号）+ 本机缓存 + 后台刷新线程。
//!
//! 页面只读 `Weather` 快照；网络失败不会清空已有数据，缓存写入数据目录，
//! 这样重启后先显示上次结果，再在后台更新。

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use chrono::{Datelike, Local, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};

pub const DEFAULT_CITY: &str = "北京";
pub const DEFAULT_REFRESH_MINUTES: u32 = 30;
const MIN_REFRESH_MINUTES: u32 = 5;
const MAX_REFRESH_MINUTES: u32 = 180;

/// WMO 天气代码归类，决定图标与配色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sun,
    Partly,
    Cloud,
    Fog,
    Rain,
    Snow,
    Thunder,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hour {
    /// `现在`、`14时` 或 `明天 5时`。
    pub label: String,
    pub temperature: f32,
    pub code: u16,
    pub precip_chance: Option<u8>,
    #[serde(default = "default_true")]
    pub is_day: bool,
}

/// 多日预报的一行。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Day {
    /// `今天`、`明天` 或 `周三`。
    pub label: String,
    pub code: u16,
    pub high: Option<f32>,
    pub low: Option<f32>,
    pub precip_chance: Option<u8>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    pub city: String,
    pub latitude: f64,
    pub longitude: f64,
    pub temperature: f32,
    pub apparent: Option<f32>,
    pub humidity: Option<u8>,
    pub wind_kmh: Option<f32>,
    pub code: u16,
    pub is_day: bool,
    pub high: Option<f32>,
    pub low: Option<f32>,
    pub precip_chance: Option<u8>,
    /// 本地时间戳（秒）。
    pub updated_at: i64,
    pub hourly: Vec<Hour>,
    #[serde(default)]
    pub days: Vec<Day>,
    #[serde(default)]
    pub sunrise: Option<String>,
    #[serde(default)]
    pub sunset: Option<String>,
}


impl Weather {
    pub fn kind(&self) -> Kind {
        kind(self.code)
    }

    pub fn label(&self) -> &'static str {
        label(self.code)
    }
}

pub fn kind(code: u16) -> Kind {
    match code {
        0 | 1 => Kind::Sun,
        2 => Kind::Partly,
        3 => Kind::Cloud,
        45 | 48 => Kind::Fog,
        51..=57 | 61..=67 | 80..=82 => Kind::Rain,
        71..=77 | 85 | 86 => Kind::Snow,
        95..=99 => Kind::Thunder,
        _ => Kind::Cloud,
    }
}

pub fn label(code: u16) -> &'static str {
    match code {
        0 => "晴",
        1 => "少云",
        2 => "多云",
        3 => "阴",
        45 | 48 => "雾",
        51 => "小毛毛雨",
        53 => "毛毛雨",
        55 => "大毛毛雨",
        56 | 57 => "冻毛毛雨",
        61 => "小雨",
        63 => "中雨",
        65 => "大雨",
        66 | 67 => "冻雨",
        71 => "小雪",
        73 => "中雪",
        75 => "大雪",
        77 => "米雪",
        80 => "阵雨",
        81 => "强阵雨",
        82 => "暴雨",
        85 => "阵雪",
        86 => "强阵雪",
        95 => "雷阵雨",
        96 | 99 => "雷暴冰雹",
        _ => "多云",
    }
}

/// 城市地理编码结果：(纬度, 经度, 城市显示名)。
pub fn parse_geocode(json: &str) -> Result<(f64, f64, String), String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|_| "城市搜索结果无法解析")?;
    let results = value["results"]
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or("没有找到这个城市")?;
    let first = &results[0];
    let latitude = first["latitude"].as_f64().ok_or("城市坐标无效")?;
    let longitude = first["longitude"].as_f64().ok_or("城市坐标无效")?;
    let name = first["name"].as_str().unwrap_or("").to_string();
    Ok((latitude, longitude, name))
}

fn f32_of(value: &serde_json::Value) -> Option<f32> {
    value.as_f64().map(|number| number as f32)
}

fn u8_percent(value: &serde_json::Value) -> Option<u8> {
    value.as_u64().map(|number| number.min(100) as u8)
}

/// 解析预报；`now` 用来截取从当前小时开始的逐小时预报。
pub fn parse_forecast(
    json: &str,
    city: &str,
    latitude: f64,
    longitude: f64,
    now: chrono::DateTime<Local>,
) -> Result<Weather, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|_| "天气数据无法解析")?;
    let current = &value["current"];
    let temperature = f32_of(&current["temperature_2m"]).ok_or("天气数据缺少当前温度")?;
    let code = current["weather_code"].as_u64().unwrap_or(0) as u16;
    let is_day = current["is_day"].as_u64().unwrap_or(1) != 0;
    let apparent = f32_of(&current["apparent_temperature"]);
    let humidity = u8_percent(&current["relative_humidity_2m"]);
    let wind_kmh = f32_of(&current["wind_speed_10m"]);
    let daily = &value["daily"];
    let high = f32_of(&daily["temperature_2m_max"][0]);
    let low = f32_of(&daily["temperature_2m_min"][0]);
    let precip_chance = u8_percent(&daily["precipitation_probability_max"][0]);
    let sunrise = daily["sunrise"][0]
        .as_str()
        .and_then(|stamp| stamp.split_once('T').map(|(_, time)| time.get(0..5).unwrap_or(time).to_string()));
    let sunset = daily["sunset"][0]
        .as_str()
        .and_then(|stamp| stamp.split_once('T').map(|(_, time)| time.get(0..5).unwrap_or(time).to_string()));
    let mut days = Vec::new();
    if let (Some(times), Some(codes)) = (daily["time"].as_array(), daily["weather_code"].as_array()) {
        let highs = daily["temperature_2m_max"].as_array();
        let lows = daily["temperature_2m_min"].as_array();
        let precip = daily["precipitation_probability_max"].as_array();
        let weekdays = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
        for (index, time) in times.iter().take(3).enumerate() {
            let Some(date) = time
                .as_str()
                .and_then(|text| chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
            else {
                continue;
            };
            let label = match (date - now.date_naive()).num_days() {
                0 => "今天".to_string(),
                1 => "明天".to_string(),
                _ => weekdays[date.weekday().num_days_from_sunday() as usize].to_string(),
            };
            days.push(Day {
                label,
                code: codes.get(index).and_then(|value| value.as_u64()).unwrap_or(0) as u16,
                high: highs.and_then(|items| items.get(index)).and_then(f32_of),
                low: lows.and_then(|items| items.get(index)).and_then(f32_of),
                precip_chance: precip.and_then(|items| items.get(index)).and_then(u8_percent),
            });
        }
    }

    let mut hourly = Vec::new();
    if let (Some(times), Some(temps), Some(codes)) = (
        value["hourly"]["time"].as_array(),
        value["hourly"]["temperature_2m"].as_array(),
        value["hourly"]["weather_code"].as_array(),
    ) {
        let precip = value["hourly"]["precipitation_probability"].as_array();
        let is_day_values = value["hourly"]["is_day"].as_array();
        let target = now
            .with_minute(0)
            .and_then(|time| time.with_second(0))
            .and_then(|time| time.with_nanosecond(0))
            .map(|time| time.naive_local());
        let mut start = 0usize;
        if let Some(target) = target {
            if let Some(index) = times.iter().position(|time| {
                time.as_str()
                    .and_then(|text| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M").ok())
                    .is_some_and(|stamp| stamp >= target)
            }) {
                start = index;
            }
        }
        let mut crossed_day = false;
        for offset in 0..times.len().saturating_sub(start).min(7) {
            let index = start + offset;
            let Some(stamp) = times[index]
                .as_str()
                .and_then(|text| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M").ok())
            else {
                continue;
            };
            let Some(temperature) = temps.get(index).and_then(|value| value.as_f64()).map(|value| value as f32)
            else {
                continue;
            };
            let code = codes.get(index).and_then(|value| value.as_u64()).unwrap_or(0) as u16;
            let precip_chance = precip
                .and_then(|items| items.get(index))
                .and_then(|value| value.as_u64())
                .map(|value| value.min(100) as u8);
            let label = if offset == 0 {
                "现在".to_string()
            } else if stamp.date() != now.date_naive() {
                if crossed_day {
                    format!("{}时", stamp.hour())
                } else {
                    crossed_day = true;
                    format!("明天 {}", stamp.hour())
                }
            } else {
                format!("{}时", stamp.hour())
            };
            let is_day = is_day_values
                .and_then(|items| items.get(index))
                .and_then(|value| value.as_u64())
                .map_or(true, |value| value != 0);
            hourly.push(Hour { label, temperature, code, precip_chance, is_day });
        }
    }

    Ok(Weather {
        city: city.to_string(),
        latitude,
        longitude,
        temperature,
        apparent,
        humidity,
        wind_kmh,
        code,
        is_day,
        high,
        low,
        precip_chance,
        updated_at: now.timestamp(),
        hourly,
        days,
        sunrise,
        sunset,
    })
}

// ---------------------------------------------------------------- 服务

pub enum Cmd {
    Configure { enabled: bool, city: String, refresh_minutes: u32 },
    Refresh,
}

#[derive(Clone, Debug)]
pub enum Event {
    Loading,
    Loaded(Weather),
    Failed(String),
}

pub struct Service {
    commands: Sender<Cmd>,
    events: Receiver<Event>,
}

impl Service {
    pub fn start() -> Self {
        let (commands, command_rx) = channel();
        let (event_tx, events) = channel();
        std::thread::Builder::new()
            .name("daozai-weather".into())
            .spawn(move || supervisor(command_rx, event_tx))
            .ok();
        Self { commands, events }
    }

    pub fn configure(&self, enabled: bool, city: String, refresh_minutes: u32) {
        let _ = self.commands.send(Cmd::Configure {
            enabled,
            city,
            refresh_minutes: refresh_minutes.clamp(MIN_REFRESH_MINUTES, MAX_REFRESH_MINUTES),
        });
    }

    pub fn refresh(&self) {
        let _ = self.commands.send(Cmd::Refresh);
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

fn cache_path() -> std::path::PathBuf {
    crate::model::config_dir().join("weather.json")
}

fn load_cache() -> Option<Weather> {
    let text = std::fs::read_to_string(cache_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_cache(weather: &Weather) {
    if let Ok(text) = serde_json::to_string(weather) {
        let _ = crate::model::atomic_write(&cache_path(), text.as_bytes());
    }
}

fn supervisor(commands: Receiver<Cmd>, events: Sender<Event>) {
    let mut enabled = false;
    let mut city = DEFAULT_CITY.to_string();
    let mut refresh_minutes = DEFAULT_REFRESH_MINUTES;
    let cache = load_cache();
    let mut last_attempt = Local::now().timestamp();
    if let Some(weather) = cache.clone() {
        let _ = events.send(Event::Loaded(weather));
    }
    let client = ureq::AgentBuilder::new().timeout(Duration::from_secs(12)).build();
    loop {
        let interval = refresh_minutes.max(MIN_REFRESH_MINUTES) as i64 * 60;
        let due = enabled && Local::now().timestamp() - last_attempt >= interval;
        let wait = if enabled {
            if due {
                Duration::from_millis(200)
            } else {
                Duration::from_secs((interval - (Local::now().timestamp() - last_attempt)).clamp(1, 60) as u64)
            }
        } else {
            Duration::from_millis(500)
        };
        match commands.recv_timeout(wait) {
            Ok(Cmd::Configure { enabled: next_enabled, city: next_city, refresh_minutes: next_minutes }) => {
                let city_changed = next_city.trim() != city;
                let turning_on = next_enabled && !enabled;
                enabled = next_enabled;
                city = if next_city.trim().is_empty() { DEFAULT_CITY.to_string() } else { next_city.trim().to_string() };
                refresh_minutes = next_minutes.clamp(MIN_REFRESH_MINUTES, MAX_REFRESH_MINUTES);
                if enabled {
                    let cache_is_fresh = cache.as_ref().is_some_and(|weather| {
                        weather.city == city && (Local::now().timestamp() - weather.updated_at) < refresh_minutes as i64 * 60
                    });
                    if city_changed || turning_on || !cache_is_fresh {
                        fetch(&client, &city, cache.as_ref(), &events, &mut last_attempt);
                    }
                }
            }
            Ok(Cmd::Refresh) => {
                if enabled {
                    fetch(&client, &city, cache.as_ref(), &events, &mut last_attempt);
                } else {
                    let _ = events.send(Event::Failed("天气未开启，可在设置中启用".into()));
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if enabled && Local::now().timestamp() - last_attempt >= interval {
                    fetch(&client, &city, cache.as_ref(), &events, &mut last_attempt);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn http_get(client: &ureq::Agent, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .call()
        .map_err(|_| "天气服务暂时不可用，请检查网络后重试".to_string())?;
    response
        .into_string()
        .map_err(|_| "读取天气数据失败，请稍后重试".to_string())
}

fn fetch(client: &ureq::Agent, city: &str, cache: Option<&Weather>, events: &Sender<Event>, last_attempt: &mut i64) {
    let _ = events.send(Event::Loading);
    let coords = match cache.filter(|weather| weather.city == city) {
        Some(weather) => (weather.latitude, weather.longitude),
        None => match geocode(client, city) {
            Ok((latitude, longitude, _name)) => (latitude, longitude),
            Err(error) => {
                *last_attempt = Local::now().timestamp();
                let _ = events.send(Event::Failed(error));
                return;
            }
        },
    };
    let (latitude, longitude) = coords;
    let body = match http_get(client, &forecast_url(latitude, longitude)) {
        Ok(body) => body,
        Err(error) => {
            *last_attempt = Local::now().timestamp();
            let _ = events.send(Event::Failed(error));
            return;
        }
    };
    *last_attempt = Local::now().timestamp();
    match parse_forecast(&body, city, latitude, longitude, Local::now()) {
        Ok(weather) => {
            save_cache(&weather);
            let _ = events.send(Event::Loaded(weather));
        }
        Err(error) => {
            let _ = events.send(Event::Failed(error));
        }
    }
}

fn forecast_url(latitude: f64, longitude: f64) -> String {
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={latitude:.4}&longitude={longitude:.4}&current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,is_day,wind_speed_10m&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max,sunrise,sunset&hourly=temperature_2m,weather_code,precipitation_probability,is_day&forecast_days=3&timezone=auto&wind_speed_unit=kmh"
    )
}

fn geocode(client: &ureq::Agent, city: &str) -> Result<(f64, f64, String), String> {
    let query: String = url::form_urlencoded::byte_serialize(city.trim().as_bytes()).collect();
    let url = format!("https://geocoding-api.open-meteo.com/v1/search?name={query}&count=1&language=zh&format=json");
    let body = http_get(client, &url).map_err(|_| "无法连接城市搜索服务，请检查网络".to_string())?;
    parse_geocode(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const GEOCODE: &str = r#"{"results":[{"id":1816670,"name":"北京","latitude":39.9075,"longitude":116.39723,"country":"中国"}]}"#;
    const FORECAST: &str = r#"{"current":{"time":"2026-11-10T04:00","temperature_2m":0.4,"apparent_temperature":-2.6,"relative_humidity_2m":79,"weather_code":0,"is_day":0,"wind_speed_10m":3.0},"hourly":{"time":["2026-11-10T03:00","2026-11-10T04:00","2026-11-10T05:00","2026-11-10T06:00"],"temperature_2m":[-1.0,0.4,0.3,-1.0],"weather_code":[1,0,0,3],"precipitation_probability":[0,0,10,null],"is_day":[0,0,0,1]},"daily":{"time":["2026-11-10","2026-11-11","2026-11-12"],"weather_code":[0,61,3],"temperature_2m_max":[11.6,9.2,8.0],"temperature_2m_min":[0.1,1.4,0.0],"precipitation_probability_max":[15,70,20],"sunrise":["2026-11-10T06:52","2026-11-11T06:53","2026-11-12T06:54"],"sunset":["2026-11-10T16:58","2026-11-11T16:57","2026-11-12T16:56"]}}"#;

    fn now() -> chrono::DateTime<Local> {
        let utc = chrono::Utc.with_ymd_and_hms(2026, 11, 9, 20, 20, 0).unwrap();
        utc.with_timezone(&Local)
    }

    fn parse() -> Weather {
        parse_forecast(FORECAST, "北京", 39.9, 116.4, now()).expect("样例天气应能解析")
    }

    #[test]
    fn parses_geocode_and_forecast() {
        let (lat, lon, name) = parse_geocode(GEOCODE).unwrap();
        assert!((lat - 39.9075).abs() < 1e-6 && (lon - 116.39723).abs() < 1e-6);
        assert_eq!(name, "北京");
        assert!(parse_geocode(r#"{"results":[]}"#).is_err());
        assert!(parse_geocode("not json").is_err());
        let weather = parse();
        assert_eq!(weather.city, "北京");
        assert!((weather.temperature - 0.4).abs() < 1e-6);
        assert!((weather.high.unwrap() - 11.6).abs() < 1e-6);
        assert!((weather.low.unwrap() - 0.1).abs() < 1e-6);
        assert_eq!(weather.humidity, Some(79));
        assert_eq!(weather.precip_chance, Some(15));
        assert_eq!(weather.kind(), Kind::Sun);
        // 本地时区下目标小时是 04:00，从 04:00 起截取。
        assert_eq!(weather.hourly.first().map(|hour| hour.label.as_str()), Some("现在"));
        assert_eq!(weather.hourly.len(), 3);
        assert_eq!(weather.hourly[2].label, "6时");
        assert_eq!(weather.hourly[2].precip_chance, None);
        // 逐小时昼夜标记与多日预报。
        assert!(!weather.hourly[0].is_day);
        assert!(weather.hourly[2].is_day);
        assert_eq!(weather.days.len(), 3);
        assert_eq!(weather.days[0].label, "今天");
        assert_eq!(weather.days[1].label, "明天");
        assert_eq!(weather.days[1].code, 61);
        assert!((weather.days[1].high.unwrap() - 9.2).abs() < 1e-6);
        assert_eq!(weather.days[1].precip_chance, Some(70));
        assert_eq!(weather.sunrise.as_deref(), Some("06:52"));
        assert_eq!(weather.sunset.as_deref(), Some("16:58"));
    }

    #[test]
    fn weather_codes_map_to_kinds_and_labels() {
        for code in [0u16, 1] {
            assert_eq!(kind(code), Kind::Sun);
        }
        assert_eq!(kind(2), Kind::Partly);
        assert_eq!(kind(3), Kind::Cloud);
        assert_eq!(kind(45), Kind::Fog);
        assert_eq!(kind(61), Kind::Rain);
        assert_eq!(kind(71), Kind::Snow);
        assert_eq!(kind(95), Kind::Thunder);
        assert_eq!(label(0), "晴");
        assert_eq!(label(95), "雷阵雨");
        assert_eq!(label(999), "多云");
    }

    #[test]
    fn missing_optional_fields_do_not_panic() {
        let minimal = r#"{"current":{"temperature_2m":5.0}}"#;
        let weather = parse_forecast(minimal, "测试", 0.0, 0.0, now()).unwrap();
        assert_eq!(weather.temperature, 5.0);
        assert!(weather.high.is_none() && weather.hourly.is_empty());
        assert!(parse_forecast("{}", "测试", 0.0, 0.0, now()).is_err());
    }

    /// 真实网络冒烟：默认跳过，设置 `DAOZAI_NETWORK_TESTS=1` 时运行。
    #[test]
    fn live_open_meteo_round_trip() {
        if std::env::var("DAOZAI_NETWORK_TESTS").is_err() {
            return;
        }
        let client = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
        let (latitude, longitude, name) = geocode(&client, "北京").expect("城市搜索应成功");
        let body = http_get(&client, &forecast_url(latitude, longitude)).expect("预报请求应成功");
        let weather = parse_forecast(&body, "北京", latitude, longitude, Local::now()).expect("预报应能解析");
        assert_eq!(weather.city, "北京");
        assert!(!name.is_empty());
        assert!(weather.temperature > -60.0 && weather.temperature < 60.0);
        assert!(!weather.hourly.is_empty(), "逐小时预报不应为空");
    }

    #[test]
    fn cache_round_trip_keeps_weather() {
        let weather = parse();
        let text = serde_json::to_string(&weather).unwrap();
        let back: Weather = serde_json::from_str(&text).unwrap();
        assert_eq!(back, weather);
        assert!(serde_json::from_str::<Weather>("{}").is_err());
    }
}
