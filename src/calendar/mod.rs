use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

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

impl CalendarEvent {
    pub const REDIS_KEY: &'static str = "trading_bot:calendar_events";

    pub fn load_events<P: AsRef<Path>>(path: P) -> anyhow::Result<Vec<Self>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let events: Vec<Self> = serde_json::from_reader(reader)?;
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
        let events: Vec<Self> = raw_jsons
            .into_iter()
            .map(|j| serde_json::from_str(&j).unwrap())
            .collect();
        Ok(events)
    }

    pub async fn fetch_events<P: AsRef<Path>>(
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

    pub fn filter_events(
        events: &[Self],
        country: Option<&str>,
        importance: Option<&str>,
    ) -> Vec<Self> {
        events
            .iter()
            .filter(|e| {
                let match_country =
                    country.map_or(true, |c| e.zone.to_lowercase() == c.to_lowercase());
                let match_importance = importance.map_or(true, |i| {
                    e.importance
                        .as_deref()
                        .map_or(false, |imp| imp.to_lowercase() == i.to_lowercase())
                });
                match_country && match_importance
            })
            .cloned()
            .collect()
    }
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

        let events = CalendarEvent::load_events(temp_file.path())?;

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "537908");
        assert_eq!(events[0].importance.as_deref(), Some("high"));
        assert_eq!(events[1].time, "All Day");
        assert!(events[1].currency.is_none());

        Ok(())
    }

    #[test]
    fn test_filter_events() {
        let events = vec![
            CalendarEvent {
                id: "1".to_string(),
                date: "01/01/2026".to_string(),
                time: "10:00".to_string(),
                zone: "united states".to_string(),
                currency: Some("USD".to_string()),
                importance: Some("high".to_string()),
                event: "Event 1".to_string(),
                actual: None,
                forecast: None,
                previous: None,
            },
            CalendarEvent {
                id: "2".to_string(),
                date: "01/01/2026".to_string(),
                time: "10:00".to_string(),
                zone: "euro zone".to_string(),
                currency: Some("EUR".to_string()),
                importance: Some("medium".to_string()),
                event: "Event 2".to_string(),
                actual: None,
                forecast: None,
                previous: None,
            },
        ];

        let high_impact = CalendarEvent::filter_events(&events, None, Some("high"));
        assert_eq!(high_impact.len(), 1);
        assert_eq!(high_impact[0].id, "1");

        let us_events = CalendarEvent::filter_events(&events, Some("United States"), None);
        assert_eq!(us_events.len(), 1);
        assert_eq!(us_events[0].id, "1");

        let euro_medium = CalendarEvent::filter_events(&events, Some("euro zone"), Some("medium"));
        assert_eq!(euro_medium.len(), 1);
        assert_eq!(euro_medium[0].id, "2");
    }
}
