use anyhow::anyhow;
use chrono::{DateTime, Duration, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

#[derive(Debug)]
pub struct NoTradeWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    //pub reason: String,
}

// #[derive(Debug)]
// pub enum FlattenPolicy {
//     None,
//     Reduce { target_exposure: f64 }, // e.g. 0.25 = keep 25%
//     FullClose,
// }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CalendarEvent {
    pub id: String,
    pub date: String,
    pub time: String,
    pub zone: String,
    pub currency: Option<String>,
    pub importance: Option<String>,
    pub event: String,
    pub actual: Option<String>,
    pub forecast: Option<String>,
    pub previous: Option<String>,
}

#[derive(PartialEq, Debug, Deserialize, Serialize, Clone)]
pub enum ImpactLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EconomicEvent {
    pub timestamp_utc: DateTime<Utc>,
    pub country: String,
    pub event: String,
    pub impact: ImpactLevel, // Low | Medium | High
}

impl TryFrom<CalendarEvent> for EconomicEvent {
    type Error = anyhow::Error;

    fn try_from(raw: CalendarEvent) -> Result<Self, Self::Error> {
        use chrono::NaiveDateTime;

        let date_part = raw.date; // e.g., "01/01/2026"
        let time_part = if raw.time.to_lowercase() == "all day" {
            "00:00"
        } else {
            &raw.time
        };

        let dt_str = format!("{} {}", date_part, time_part);
        let naive_dt = NaiveDateTime::parse_from_str(&dt_str, "%d/%m/%Y %H:%M")
            .map_err(|e| anyhow::anyhow!("Failed to parse date/time '{}': {}", dt_str, e))?;

        let timestamp_utc = DateTime::<Utc>::from_naive_utc_and_offset(naive_dt, Utc);

        let impact = match raw.importance.as_deref() {
            Some("high") => ImpactLevel::High,
            Some("medium") => ImpactLevel::Medium,
            Some("low") => ImpactLevel::Low,
            _ => ImpactLevel::Low,
        };

        Ok(Self {
            timestamp_utc,
            country: raw.zone,
            event: raw.event,
            impact,
        })
    }
}

impl EconomicEvent {
    const REDIS_KEY: &'static str = "trading_bot:calendar_events";

    pub fn load_events<P: AsRef<Path>>(path: P) -> anyhow::Result<Vec<Self>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let raw_events: Vec<CalendarEvent> = serde_json::from_reader(reader)?;

        let mut events = Vec::new();
        for raw in raw_events {
            match Self::try_from(raw) {
                Ok(event) => events.push(event),
                Err(e) => {
                    log::warn!("Skipping event due to parsing error: {}", e);
                }
            }
        }
        Ok(events)
    }

    pub async fn save_to_redis(
        conn: &mut redis::aio::MultiplexedConnection,
        events: &[Self],
    ) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let json_strings: Vec<String> = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect();

        let _: () = conn.del(Self::REDIS_KEY).await?;
        let _: () = conn.rpush(Self::REDIS_KEY, json_strings).await?;
        Ok(())
    }

    pub async fn fetch_from_redis(
        conn: &mut redis::aio::MultiplexedConnection,
    ) -> anyhow::Result<Vec<Self>> {
        let raw_jsons: Vec<String> = conn.lrange(Self::REDIS_KEY, 0, -1).await?;
        let mut events = Vec::new();

        for j in raw_jsons {
            // Try to deserialize directly as EconomicEvent (new format)
            if let Ok(event) = serde_json::from_str::<Self>(&j) {
                events.push(event);
            } else {
                // Try legacy format (CalendarEvent) and convert
                if let Ok(raw) = serde_json::from_str::<CalendarEvent>(&j) {
                    match Self::try_from(raw) {
                        Ok(event) => events.push(event),
                        Err(e) => {
                            log::warn!("Failed to convert legacy Redis event: {}", e);
                        }
                    }
                } else {
                    log::warn!("Skipping invalid Redis event JSON: {}", j);
                }
            }
        }
        Ok(events)
    }

    async fn fetch_events<P: AsRef<Path>>(
        conn: &mut redis::aio::MultiplexedConnection,
        backup_path: P,
    ) -> anyhow::Result<Vec<Self>> {
        let events = Self::fetch_from_redis(conn).await?;
        if !events.is_empty() {
            return Ok(events);
        }

        // Fallback to file
        let events = Self::load_events(backup_path)?;
        Self::save_to_redis(conn, &events).await?;
        Ok(events)
    }

    pub async fn filter_events(
        conn: &mut redis::aio::MultiplexedConnection,
        country: &str,
        importance: ImpactLevel,
    ) -> anyhow::Result<Vec<Self>> {
        if !Path::new("data/calendar_data.json").exists() {
            return Err(anyhow!("File {} not found", "data/calendar_data.json"));
        }

        let events = Self::fetch_events(conn, "data/calendar_data.json").await?;
        let filtered_events = events
            .iter()
            .filter(|e| {
                let match_country = country.to_lowercase() == e.country.to_lowercase();

                match_country && importance == e.impact
            })
            .cloned()
            .collect();
        Ok(filtered_events)
    }

    pub fn is_trading_allowed(now: DateTime<Utc>, windows: &[NoTradeWindow]) -> bool {
        !windows.iter().any(|w| now >= w.start && now <= w.end)
    }

    fn is_critical_macro_event(event: &EconomicEvent) -> bool {
        let s = event.event.as_str();
        s.contains("Consumer Price Index (CPI)")
            || s.contains("Core CPI")
            || s.contains("Non Farm Payrolls")
            || s.contains("Fed Interest Rate Decision")
            || s.contains("FOMC")
            || s.contains("GDP Growth Rate")
    }

    /**
     * |---- PRE BUFFER ----| EVENT |---- POST BUFFER ----|
     * |==================== NO TRADING ====================|
     */
    pub fn build_no_trade_windows(
        events: &[EconomicEvent],
        pre_buffer: Duration,
        post_buffer: Duration,
    ) -> Vec<NoTradeWindow> {
        events
            .iter()
            .filter(|e| e.impact == ImpactLevel::High && EconomicEvent::is_critical_macro_event(e))
            .map(|e| NoTradeWindow {
                start: e.timestamp_utc - pre_buffer,
                end: e.timestamp_utc + post_buffer,
                //reason: e.event.clone(),
            })
            .collect()
    }

    pub fn macro_trading_allowed(now: DateTime<Utc>, windows: &[NoTradeWindow]) -> bool {
        !windows.iter().any(|w| now >= w.start && now <= w.end)
    }
}

#[derive(Debug)]
pub struct MacroGuard {
    pub windows: Vec<NoTradeWindow>,
}

impl MacroGuard {
    pub async fn new(conn: &mut redis::aio::MultiplexedConnection) -> Result<Self, anyhow::Error> {
        let country = "united states";
        let calendar_events =
            EconomicEvent::filter_events(conn, country, ImpactLevel::High).await?;

        let windows: Vec<NoTradeWindow> = EconomicEvent::build_no_trade_windows(
            &calendar_events,
            Duration::hours(12),
            Duration::hours(12),
        );
        Ok(Self { windows })
    }

    pub fn trading_allowed(now: DateTime<Utc>, windows: &[NoTradeWindow]) -> bool {
        !windows.iter().any(|w| now >= w.start && now <= w.end)
    }

    // pub fn flatten_policy_for_event(event: &str) -> FlattenPolicy {
    //     match event {
    //         "Fed Interest Rate Decision" | "FOMC Statement" | "FOMC Press Conference" => {
    //             FlattenPolicy::FullClose
    //         }

    //         "Consumer Price Index (CPI)" | "Core CPI" => FlattenPolicy::Reduce {
    //             target_exposure: 0.25,
    //         },

    //         _ => FlattenPolicy::None,
    //     }
    // }

    // pub fn flatten_decision(
    //     now: DateTime<Utc>,
    //     windows: &[NoTradeWindow],
    // ) -> Option<FlattenPolicy> {
    //     windows.iter().find_map(|w| {
    //         let minutes_to_start = (w.start - now).num_minutes();

    //         if (0..=30).contains(&minutes_to_start) {
    //             Some(Self::flatten_policy_for_event(&w.reason))
    //         } else {
    //             None
    //         }
    //     })
    // }

    pub fn allow_entry(&self, now: DateTime<Utc>) -> bool {
        Self::trading_allowed(now, &self.windows)
    }

    // pub fn flatten_policy(&self, now: DateTime<Utc>) -> Option<FlattenPolicy> {
    //     Self::flatten_decision(now, &self.windows)
    // }

    // pub fn should_flatten_position(
    //     now: DateTime<Utc>,
    //     windows: &[NoTradeWindow],
    // ) -> Option<FlattenPolicy> {
    //     windows.iter().find_map(|w| {
    //         let minutes_to_event = (w.start - now).num_minutes();

    //         if minutes_to_event <= 30 && minutes_to_event >= 0 {
    //             Some(Self::flatten_policy_for_event(&w.reason))
    //         } else {
    //             None
    //         }
    //     })
    // }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_events() -> anyhow::Result<()> {
        let json_data = r#"[
            {
                "id": "537908",
                "date": "02/01/2026",
                "time": "14:45",
                "zone": "united states",
                "currency": "USD",
                "importance": "high",
                "event": "S&P Global Manufacturing PMI  (Dec)",
                "actual": "51.8",
                "forecast": "51.8",
                "previous": "52.2"
            },
            {
                "id": "1",
                "date": "19/01/2026",
                "time": "All Day",
                "zone": "united states",
                "currency": null,
                "importance": null,
                "event": "United States - Martin Luther King, Jr. Day",
                "actual": null,
                "forecast": null,
                "previous": null
            }
        ]"#;

        let mut temp_file = NamedTempFile::new()?;
        write!(temp_file, "{}", json_data)?;

        let events = EconomicEvent::load_events(temp_file.path())?;

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].country, "united states");
        assert_eq!(events[0].impact, ImpactLevel::High);
        assert_eq!(events[0].timestamp_utc.format("%H:%M").to_string(), "14:45");

        assert_eq!(events[1].country, "united states");
        assert_eq!(events[1].impact, ImpactLevel::Low); // None importance -> Low
        assert_eq!(events[1].timestamp_utc.format("%H:%M").to_string(), "00:00");

        Ok(())
    }

    #[test]
    fn test_filter_events() -> anyhow::Result<()> {
        // Since filter_events depends on a file and Redis, we might need a more complex test
        // or just test the logic with a helper if we refactor it further.
        // For now, let's just make sure it compiles and we have basic tests for the structures.
        Ok(())
    }
}
