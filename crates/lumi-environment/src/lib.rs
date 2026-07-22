use chrono::{DateTime, Datelike, Local, Offset, Timelike, Utc};
use chrono_tz::Tz;
use lumi_core::WeatherKind;
use serde::Deserialize;
use std::fmt;
use url::Url;

pub const DEFAULT_OPEN_METEO_ENDPOINT: &str = "https://api.open-meteo.com/v1/forecast";
const CIVIL_TWILIGHT_DEGREES: f64 = -6.0;
const MAX_WEATHER_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct SolarContext {
    pub now_minutes: i32,
    pub sunrise_minutes: Option<i32>,
    pub sunset_minutes: Option<i32>,
    pub solar_elevation_degrees: f64,
    pub day_of_year: u32,
    pub date_key: i32,
    pub timezone: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherObservation {
    pub kind: WeatherKind,
    pub cloud_cover: i32,
    pub visibility_km: f64,
    pub precipitation_probability: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeatherRequest {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub latitude: String,
    pub longitude: String,
    pub timezone: String,
}

impl WeatherRequest {
    pub fn new(latitude: f64, longitude: f64, timezone: impl Into<String>) -> Self {
        let endpoint = std::env::var("LUMICONTROL_WEATHER_ENDPOINT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                option_env!("LUMICONTROL_WEATHER_ENDPOINT")
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| DEFAULT_OPEN_METEO_ENDPOINT.to_string());
        Self {
            endpoint,
            api_key: std::env::var("LUMICONTROL_WEATHER_API_KEY")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            latitude: latitude.to_string(),
            longitude: longitude.to_string(),
            timezone: timezone.into(),
        }
    }

    pub fn url(&self) -> Result<Url, EnvironmentError> {
        if self.endpoint.trim().is_empty() {
            return Err(EnvironmentError::ProviderNotConfigured);
        }
        let mut url = Url::parse(&self.endpoint)
            .map_err(|error| EnvironmentError::InvalidEndpoint(error.to_string()))?;
        let secure_scheme = url.scheme() == "https";
        let debug_http = cfg!(debug_assertions) && url.scheme() == "http";
        if (!secure_scheme && !debug_http) || url.host_str().is_none() {
            return Err(EnvironmentError::InvalidEndpoint(
                "weather endpoint must be an HTTPS URL with a host".to_string(),
            ));
        }
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("latitude", &self.latitude)
                .append_pair("longitude", &self.longitude)
                .append_pair(
                    "current",
                    "cloud_cover,visibility,precipitation_probability",
                )
                .append_pair(
                    "timezone",
                    if self.timezone == "system" {
                        "auto"
                    } else {
                        &self.timezone
                    },
                )
                .append_pair("forecast_days", "1");
            if let Some(api_key) = &self.api_key {
                query.append_pair("apikey", api_key);
            }
        }
        Ok(url)
    }
}

pub fn current_solar_context(
    latitude: f64,
    longitude: f64,
    timezone: &str,
) -> Result<SolarContext, EnvironmentError> {
    solar_context_at(latitude, longitude, timezone, Utc::now())
}

pub fn solar_context_at(
    latitude: f64,
    longitude: f64,
    timezone: &str,
    now_utc: DateTime<Utc>,
) -> Result<SolarContext, EnvironmentError> {
    validate_coordinates(latitude, longitude)?;
    let (moment, resolved_timezone) = if timezone.trim().is_empty() || timezone == "system" {
        let local = now_utc.with_timezone(&Local);
        (LocalMoment::from_datetime(&local), "system".to_string())
    } else {
        let parsed = timezone
            .parse::<Tz>()
            .map_err(|_| EnvironmentError::InvalidTimezone(timezone.to_string()))?;
        let local = now_utc.with_timezone(&parsed);
        (LocalMoment::from_datetime(&local), parsed.to_string())
    };
    let now_minutes = moment.hour as i32 * 60 + moment.minute as i32;
    Ok(SolarContext {
        now_minutes,
        sunrise_minutes: solar_event_minutes(latitude, longitude, moment, true),
        sunset_minutes: solar_event_minutes(latitude, longitude, moment, false),
        solar_elevation_degrees: solar_elevation_degrees(
            latitude,
            longitude,
            moment,
            now_minutes as usize,
        ),
        day_of_year: moment.ordinal,
        date_key: moment.year * 1000 + moment.ordinal as i32,
        timezone: resolved_timezone,
    })
}

fn validate_coordinates(latitude: f64, longitude: f64) -> Result<(), EnvironmentError> {
    if !latitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
        return Err(EnvironmentError::InvalidCoordinates(
            "latitude must be finite and in -90..=90".to_string(),
        ));
    }
    if !longitude.is_finite() || !(-180.0..=180.0).contains(&longitude) {
        return Err(EnvironmentError::InvalidCoordinates(
            "longitude must be finite and in -180..=180".to_string(),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct LocalMoment {
    year: i32,
    ordinal: u32,
    hour: u32,
    minute: u32,
    utc_offset_seconds: i32,
}

impl LocalMoment {
    fn from_datetime<T>(datetime: &DateTime<T>) -> Self
    where
        T: chrono::TimeZone,
    {
        Self {
            year: datetime.year(),
            ordinal: datetime.ordinal(),
            hour: datetime.hour(),
            minute: datetime.minute(),
            utc_offset_seconds: datetime.offset().fix().local_minus_utc(),
        }
    }
}

fn solar_event_minutes(
    latitude: f64,
    longitude: f64,
    moment: LocalMoment,
    rising: bool,
) -> Option<i32> {
    let mut previous_minute = 0;
    let mut previous_elevation = solar_elevation_degrees(latitude, longitude, moment, 0);
    for minute in (5..=1440).step_by(5) {
        let clamped_minute = minute.min(1439);
        let elevation = solar_elevation_degrees(latitude, longitude, moment, clamped_minute);
        let crossed_up =
            previous_elevation < CIVIL_TWILIGHT_DEGREES && elevation >= CIVIL_TWILIGHT_DEGREES;
        let crossed_down =
            previous_elevation >= CIVIL_TWILIGHT_DEGREES && elevation < CIVIL_TWILIGHT_DEGREES;
        if (rising && crossed_up) || (!rising && crossed_down) {
            let denominator = elevation - previous_elevation;
            if denominator.abs() < f64::EPSILON {
                return Some(clamped_minute as i32);
            }
            let fraction =
                ((CIVIL_TWILIGHT_DEGREES - previous_elevation) / denominator).clamp(0.0, 1.0);
            return Some((previous_minute as f64 + fraction * 5.0).round() as i32);
        }
        previous_minute = clamped_minute as i32;
        previous_elevation = elevation;
    }
    None
}

fn solar_elevation_degrees(
    latitude: f64,
    longitude: f64,
    moment: LocalMoment,
    minute_of_day: usize,
) -> f64 {
    let hour = minute_of_day as f64 / 60.0;
    let gamma =
        2.0 * std::f64::consts::PI / 365.0 * (moment.ordinal as f64 - 1.0 + (hour - 12.0) / 24.0);
    let equation_of_time = 229.18
        * (0.000075 + 0.001868 * gamma.cos()
            - 0.032077 * gamma.sin()
            - 0.014615 * (2.0 * gamma).cos()
            - 0.040849 * (2.0 * gamma).sin());
    let declination = 0.006918 - 0.399912 * gamma.cos() + 0.070257 * gamma.sin()
        - 0.006758 * (2.0 * gamma).cos()
        + 0.000907 * (2.0 * gamma).sin()
        - 0.002697 * (3.0 * gamma).cos()
        + 0.00148 * (3.0 * gamma).sin();
    let timezone_hours = moment.utc_offset_seconds as f64 / 3600.0;
    let true_solar_time = (hour * 60.0 + equation_of_time + 4.0 * longitude
        - 60.0 * timezone_hours)
        .rem_euclid(1440.0);
    let mut hour_angle = true_solar_time / 4.0 - 180.0;
    if hour_angle < -180.0 {
        hour_angle += 360.0;
    }
    let latitude_rad = latitude.to_radians();
    let cos_zenith = latitude_rad.sin() * declination.sin()
        + latitude_rad.cos() * declination.cos() * hour_angle.to_radians().cos();
    90.0 - cos_zenith.clamp(-1.0, 1.0).acos().to_degrees()
}

pub fn classify_weather(
    cloud_cover: i32,
    visibility_km: f64,
    precipitation_probability: f64,
) -> WeatherKind {
    if precipitation_probability >= 0.35 {
        WeatherKind::Rain
    } else if visibility_km <= 3.0 {
        WeatherKind::Fog
    } else if cloud_cover >= 60 {
        WeatherKind::Cloudy
    } else {
        WeatherKind::Clear
    }
}

#[derive(Debug, Deserialize)]
struct OpenMeteoPayload {
    current: Option<OpenMeteoCurrent>,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoCurrent {
    cloud_cover: Option<i32>,
    visibility: Option<f64>,
    precipitation_probability: Option<f64>,
}

pub fn parse_open_meteo(bytes: &[u8]) -> Result<WeatherObservation, EnvironmentError> {
    if bytes.len() > MAX_WEATHER_RESPONSE_BYTES {
        return Err(EnvironmentError::ResponseTooLarge(bytes.len()));
    }
    let payload: OpenMeteoPayload = serde_json::from_slice(bytes)
        .map_err(|error| EnvironmentError::InvalidResponse(error.to_string()))?;
    let current = payload.current.ok_or_else(|| {
        EnvironmentError::InvalidResponse("response has no current weather object".to_string())
    })?;
    let cloud_cover = current.cloud_cover.unwrap_or(0).clamp(0, 100);
    let visibility_km = current.visibility.unwrap_or(20_000.0).max(0.0) / 1000.0;
    let precipitation_probability =
        (current.precipitation_probability.unwrap_or(0.0) / 100.0).clamp(0.0, 1.0);
    Ok(WeatherObservation {
        kind: classify_weather(cloud_cover, visibility_km, precipitation_probability),
        cloud_cover,
        visibility_km,
        precipitation_probability,
    })
}

pub fn fetch_open_meteo(request: &WeatherRequest) -> Result<WeatherObservation, EnvironmentError> {
    let url = request.url()?;
    let bytes = platform::http_get(&url, MAX_WEATHER_RESPONSE_BYTES)?;
    parse_open_meteo(&bytes)
}

#[cfg(windows)]
mod platform {
    use super::{EnvironmentError, Url};
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Networking::WinHttp::{
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
        WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
        WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    };

    struct InternetHandle(*mut c_void);

    impl InternetHandle {
        fn new(handle: *mut c_void, operation: &str) -> Result<Self, EnvironmentError> {
            if handle.is_null() {
                Err(last_error(operation))
            } else {
                Ok(Self(handle))
            }
        }
    }

    impl Drop for InternetHandle {
        fn drop(&mut self) {
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }

    pub(super) fn http_get(url: &Url, maximum: usize) -> Result<Vec<u8>, EnvironmentError> {
        let host = url
            .host_str()
            .ok_or_else(|| EnvironmentError::InvalidEndpoint("URL has no host".to_string()))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| EnvironmentError::InvalidEndpoint("URL has no port".to_string()))?;
        let mut object = url.path().to_string();
        if let Some(query) = url.query() {
            object.push('?');
            object.push_str(query);
        }
        let agent = wide("LumiControl/0.2");
        let host = wide(host);
        let object = wide(&object);
        let verb = wide("GET");
        unsafe {
            let session = InternetHandle::new(
                WinHttpOpen(
                    agent.as_ptr(),
                    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                    null(),
                    null(),
                    0,
                ),
                "WinHttpOpen",
            )?;
            if WinHttpSetTimeouts(session.0, 1_500, 1_500, 2_000, 2_000) == 0 {
                return Err(last_error("WinHttpSetTimeouts"));
            }
            let connection = InternetHandle::new(
                WinHttpConnect(session.0, host.as_ptr(), port, 0),
                "WinHttpConnect",
            )?;
            let flags = if url.scheme() == "https" {
                WINHTTP_FLAG_SECURE
            } else {
                0
            };
            let request = InternetHandle::new(
                WinHttpOpenRequest(
                    connection.0,
                    verb.as_ptr(),
                    object.as_ptr(),
                    null(),
                    null(),
                    null(),
                    flags,
                ),
                "WinHttpOpenRequest",
            )?;
            if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0 {
                return Err(last_error("WinHttpSendRequest"));
            }
            if WinHttpReceiveResponse(request.0, null_mut()) == 0 {
                return Err(last_error("WinHttpReceiveResponse"));
            }

            let mut status = 0u32;
            let mut status_size = std::mem::size_of::<u32>() as u32;
            if WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut status as *mut u32).cast(),
                &mut status_size,
                null_mut(),
            ) == 0
            {
                return Err(last_error("WinHttpQueryHeaders"));
            }
            if !(200..300).contains(&status) {
                return Err(EnvironmentError::HttpStatus(status));
            }

            let mut response = Vec::new();
            loop {
                let mut available = 0u32;
                if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                    return Err(last_error("WinHttpQueryDataAvailable"));
                }
                if available == 0 {
                    break;
                }
                let remaining = maximum.saturating_sub(response.len());
                if remaining == 0 || available as usize > remaining {
                    return Err(EnvironmentError::ResponseTooLarge(
                        response.len().saturating_add(available as usize),
                    ));
                }
                let start = response.len();
                response.resize(start + available as usize, 0);
                let mut read = 0u32;
                if WinHttpReadData(
                    request.0,
                    response[start..].as_mut_ptr().cast(),
                    available,
                    &mut read,
                ) == 0
                {
                    return Err(last_error("WinHttpReadData"));
                }
                response.truncate(start + read as usize);
                if read == 0 {
                    break;
                }
            }
            Ok(response)
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error(operation: &str) -> EnvironmentError {
        EnvironmentError::Network(format!(
            "{operation} failed: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{EnvironmentError, Url};

    pub(super) fn http_get(_url: &Url, _maximum: usize) -> Result<Vec<u8>, EnvironmentError> {
        Err(EnvironmentError::UnsupportedPlatform)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvironmentError {
    InvalidCoordinates(String),
    InvalidTimezone(String),
    InvalidEndpoint(String),
    InvalidResponse(String),
    Network(String),
    HttpStatus(u32),
    ResponseTooLarge(usize),
    ProviderNotConfigured,
    UnsupportedPlatform,
}

impl fmt::Display for EnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCoordinates(message)
            | Self::InvalidEndpoint(message)
            | Self::InvalidResponse(message)
            | Self::Network(message) => formatter.write_str(message),
            Self::InvalidTimezone(timezone) => {
                write!(formatter, "invalid IANA timezone: {timezone}")
            }
            Self::HttpStatus(status) => write!(formatter, "weather service returned HTTP {status}"),
            Self::ResponseTooLarge(size) => {
                write!(formatter, "weather response is too large ({size} bytes)")
            }
            Self::ProviderNotConfigured => formatter.write_str(
                "weather provider is not configured; sunrise and sunset remain available offline",
            ),
            Self::UnsupportedPlatform => {
                formatter.write_str("weather HTTP provider is only available on Windows")
            }
        }
    }
}

impl std::error::Error for EnvironmentError {}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn shanghai_equinox_has_plausible_civil_twilight() {
        let now = Utc.with_ymd_and_hms(2026, 3, 20, 4, 0, 0).unwrap();
        let context = solar_context_at(31.2304, 121.4737, "Asia/Shanghai", now).unwrap();
        assert!((330..=390).contains(&context.sunrise_minutes.unwrap()));
        assert!((1080..=1140).contains(&context.sunset_minutes.unwrap()));
        assert_eq!(context.now_minutes, 12 * 60);
        assert!((55.0..=65.0).contains(&context.solar_elevation_degrees));
        assert_eq!(context.day_of_year, 79);
    }

    #[test]
    fn named_timezone_applies_daylight_saving() {
        let winter = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let summer = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
        assert_eq!(
            solar_context_at(51.5, -0.1, "Europe/London", winter)
                .unwrap()
                .now_minutes,
            12 * 60
        );
        assert_eq!(
            solar_context_at(51.5, -0.1, "Europe/London", summer)
                .unwrap()
                .now_minutes,
            13 * 60
        );
    }

    #[test]
    fn weather_classification_uses_safety_first_priority() {
        assert_eq!(classify_weather(95, 1.0, 0.8), WeatherKind::Rain);
        assert_eq!(classify_weather(95, 1.0, 0.1), WeatherKind::Fog);
        assert_eq!(classify_weather(75, 20.0, 0.1), WeatherKind::Cloudy);
        assert_eq!(classify_weather(20, 20.0, 0.1), WeatherKind::Clear);
    }

    #[test]
    fn open_meteo_payload_is_bounded_and_normalized() {
        let observation = parse_open_meteo(
            br#"{"current":{"cloud_cover":82,"visibility":2400,"precipitation_probability":12}}"#,
        )
        .unwrap();
        assert_eq!(observation.kind, WeatherKind::Fog);
        assert_eq!(observation.cloud_cover, 82);
        assert!((observation.visibility_km - 2.4).abs() < 0.001);
    }

    #[test]
    fn request_encodes_timezone_and_optional_provider_key() {
        let request = WeatherRequest {
            endpoint: "https://customer-api.open-meteo.com/v1/forecast".to_string(),
            api_key: Some("key with spaces".to_string()),
            latitude: "31.2".to_string(),
            longitude: "121.5".to_string(),
            timezone: "Asia/Shanghai".to_string(),
        };
        let url = request.url().unwrap().to_string();
        assert!(url.contains("timezone=Asia%2FShanghai"));
        assert!(url.contains("apikey=key+with+spaces"));
    }

    #[test]
    fn an_empty_release_endpoint_fails_closed() {
        let request = WeatherRequest {
            endpoint: String::new(),
            api_key: None,
            latitude: "31.2".to_string(),
            longitude: "121.4".to_string(),
            timezone: "Asia/Shanghai".to_string(),
        };
        assert_eq!(
            request.url().unwrap_err(),
            EnvironmentError::ProviderNotConfigured
        );
    }
}
