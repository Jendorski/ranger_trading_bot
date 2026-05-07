#![allow(dead_code)]

use anyhow::Result;
use chrono::{DateTime, Datelike, TimeZone, Utc};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufWriter;

use crate::data::kaggle::CSV_PATH;
use crate::types::{Candle, RawMinuteRow, Timeframe};

/// Every timeframe that must have a seed file on disk before the bot starts.
///
/// Order is descending by bar width — widest timeframes first so that if the
/// aggregation run is interrupted, the most macro-relevant seeds are written
/// before the shorter ones.
pub const SEED_MANIFEST: &[(Timeframe, &str)] = &[
    (Timeframe::W2,  "data/candles_2w.json"),
    (Timeframe::W1,  "data/candles_1w.json"),
    (Timeframe::D3,  "data/candles_3d.json"),
    (Timeframe::D1,  "data/candles_1d.json"),
    (Timeframe::H4,  "data/candles_4h.json"),
    (Timeframe::H1,  "data/candles_1h.json"),
    (Timeframe::M15, "data/candles_15m.json"),
];

/// Returns `true` only when the seed file at `path` exists, is readable, and
/// deserialises as a non-empty `Vec<Candle>`. A file that is present but empty
/// or corrupt is treated as missing so it will be regenerated.
pub fn seed_file_exists(path: &str) -> bool {
    match read_candles_json(path) {
        Ok(candles) => !candles.is_empty(),
        Err(_) => false,
    }
}

/// Reads the Kaggle 1-minute CSV once and writes a seed file for every entry
/// in [`SEED_MANIFEST`] that does not already pass [`seed_file_exists`].
///
/// This is a blocking function — call it inside `tokio::task::spawn_blocking`.
///
/// Each timeframe is attempted independently: a failure writing one seed does
/// not prevent the others from being written.  The function returns `Err` only
/// if the CSV read itself fails (nothing useful can be done without the source
/// data).
pub fn generate_missing_seeds() -> Result<()> {
    std::fs::create_dir_all("data")?;

    let missing: Vec<(Timeframe, &str)> = SEED_MANIFEST
        .iter()
        .filter(|(_, path)| !seed_file_exists(path))
        .copied()
        .collect();

    if missing.is_empty() {
        println!("[aggregator] All seed files present — nothing to generate");
        return Ok(());
    }

    println!(
        "[aggregator] {} seed file(s) missing — reading {CSV_PATH}",
        missing.len()
    );

    let candles_1m = read_1min_csv(CSV_PATH)?;

    for (tf, path) in &missing {
        let aggregated = aggregate(&candles_1m, *tf);
        match write_candles_json(&aggregated, *tf) {
            Ok(()) => println!("[aggregator] ✓ {path}"),
            Err(e) => eprintln!("[aggregator] ✗ {path}: {e}"),
        }
    }

    Ok(())
}

type BucketEntry = (f64, f64, f64, f64, f64, DateTime<Utc>);

/// Read pre-aggregated candles from a JSON file written by [`write_candles_json`].
pub fn read_candles_json(path: &str) -> Result<Vec<Candle>> {
    let data = std::fs::read_to_string(path)?;
    let candles: Vec<Candle> = serde_json::from_str(&data)?;
    Ok(candles)
}

/// Read the Kaggle 1-minute CSV and return a vec of valid Candle, sorted ascending.
pub fn read_1min_csv(path: &str) -> Result<Vec<Candle>> {
    println!("[aggregator] Reading 1-min candles from {path}");
    let file = File::open(path)?;
    let mut rdr = csv::Reader::from_reader(file);
    let mut candles = Vec::new();

    for result in rdr.deserialize::<RawMinuteRow>() {
        let row = match result {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Skip rows with NaN values (common in the Kaggle dataset at start of history)
        let (open, high, low, close, volume) =
            match (row.open, row.high, row.low, row.close, row.volume_btc) {
                (Some(o), Some(h), Some(l), Some(c), Some(v)) => (o, h, l, c, v),
                _ => continue,
            };

        let timestamp = Utc.timestamp_opt(row.timestamp as i64, 0).single();
        let timestamp = match timestamp {
            Some(t) => t,
            None => continue,
        };

        candles.push(Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
        });
    }

    candles.sort_by_key(|c| c.timestamp);
    println!(
        "[aggregator] Loaded {} 1-min candles from {}",
        candles.len(),
        path
    );
    Ok(candles)
}

/// Aggregate 1-min candles into the target timeframe.
/// For W2 and Monthly, uses calendar-aware bucketing.
pub fn aggregate(candles_1m: &[Candle], tf: Timeframe) -> Vec<Candle> {
    if candles_1m.is_empty() {
        return Vec::new();
    }

    // BTreeMap<bucket_key, (open, high, low, close, volume, ts)>
    let mut buckets: BTreeMap<i64, BucketEntry> = BTreeMap::new();

    for c in candles_1m {
        let key = bucket_key(c.timestamp, tf);

        buckets
            .entry(key)
            .and_modify(|b| {
                // b = (open, high, low, close, volume, ts)
                b.1 = b.1.max(c.high);
                b.2 = b.2.min(c.low);
                b.3 = c.close; // last close wins
                b.4 += c.volume;
            })
            .or_insert((c.open, c.high, c.low, c.close, c.volume, c.timestamp));
    }

    let result: Vec<Candle> = buckets
        .into_values()
        .map(|(open, high, low, close, volume, timestamp)| Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
        })
        .collect();

    let n_in = candles_1m.len();
    let n_out = result.len();
    let label = tf.label();
    println!("[aggregator] Aggregated {n_in} candles → {n_out} {label} candles");

    result
}

/// Serialize aggregated candles to `data/candles_{label}.json`, overwriting any prior file.
pub fn write_candles_json(candles: &[Candle], tf: Timeframe) -> Result<()> {
    let label = tf.label();
    let path = format!("data/candles_{label}.json");
    let file = File::create(&path)?;
    let writer = BufWriter::new(file);
    serde_json::to_writer(writer, candles)?;
    let n = candles.len();
    println!("[aggregator] Wrote {n} candles → {path}");
    Ok(())
}

/// Return a stable integer key that groups 1-min candles into the correct bucket.
fn bucket_key(ts: DateTime<Utc>, tf: Timeframe) -> i64 {
    match tf {
        Timeframe::Monthly => {
            // Group by calendar year-month
            (ts.year() as i64) * 100 + (ts.month() as i64)
        }
        Timeframe::W2 => {
            // Epoch seconds // (14 days in seconds)
            ts.timestamp() / (14 * 24 * 3600)
        }
        _ => {
            // All other TFs: floor to fixed-width seconds bucket
            ts.timestamp() / tf.seconds()
        }
    }
}
