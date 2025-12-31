use anyhow::Result;
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tokio::time;

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use crate::exchange::bitget::Candle;
use crate::helper::Helper;
use crate::helper::{LAST_25_WEEKLY_ICHIMOKU_SPANS, WEEKLY_CANDLES, WEEKLY_ICHIMOKU};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenkanKijunCross {
    Bullish,
    Bearish,
}

// pub enum CrossStrength {
//     StrongBullish,
//     WeakBullish,
//     StrongBearish,
//     WeakBearish,
// }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KumoCross {
    Bullish,
    Bearish,
}

#[derive(Debug, Serialize)]
pub struct Ichimoku {
    pub conversion_line: Vec<Option<f64>>, // Tenkan-sen
    pub base_line: Vec<Option<f64>>,       // Kijun-sen
    pub leading_span_a: Vec<Option<f64>>,  // Senkou A
    pub leading_span_b: Vec<Option<f64>>,  // Senkou B
    pub lagging_span: Vec<Option<f64>>,    // Chikou
}

//Ichimoku is used for BTC on the weekly timeframe
///Download the one-minute BTCUSD from the dataset from : https://www.kaggle.com/api/v1/datasets/download/mczielinski/bitcoin-historical-data,
/// resolve it into a weekly timeframe, and calculate the ichimoku
pub async fn ichimoku_loop(redis_conn: MultiplexedConnection) -> Result<()> {
    let loop_interval_seconds = 604800;

    let mut interval = time::interval(Duration::from_secs(loop_interval_seconds));

    let url = "https://www.kaggle.com/api/v1/datasets/download/mczielinski/bitcoin-historical-data";

    loop {
        interval.tick().await;

        //let url = url.to_string();
        let result = tokio::task::spawn_blocking(move || {
            download_large_file(url, "data/btcusd_1-min_data.zip")
        })
        .await;

        match result {
            Ok(Err(e)) => {
                eprintln!("CRITICAL ERROR in ichimoku_loop: {:?}", e);
                eprintln!("Retrying in {} seconds...", loop_interval_seconds);
            }
            Err(e) => {
                eprintln!("Task Join Error: {:?}", e);
            }
            _ => {}
        }

        let _extract_weekly = tokio::task::spawn_blocking(move || {
            Helper::extract_into_weekly_candle(
                "data/btcusd_1-min_data.csv",
                "data/btcusd_weekly_data.csv",
            )
        })
        .await;

        let ichimoku_conn = redis_conn.clone();
        let _process_weekly_ichimoku =
            tokio::task::spawn(async move { process_weekly_ichimoku(ichimoku_conn).await }).await;
    }
}

fn download_large_file(url: &str, path: &str) -> Result<()> {
    println!("Downloading {}...", url);

    let mut response = reqwest::blocking::get(url)?;
    let mut temp = tempfile::NamedTempFile::new()?;
    io::copy(&mut response, &mut temp)?;

    temp.persist(path)?;
    println!("Downloaded {}", url);

    println!("Extracting {}...", path);
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let parent_dir = Path::new(path).parent().unwrap_or(Path::new("."));

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = match file.enclosed_name() {
            Some(path) => parent_dir.join(path),
            None => continue,
        };

        if file.name().ends_with('/') {
            fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    fs::create_dir_all(p)?;
                }
            }
            let mut outfile = fs::File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;
        }
    }
    println!("Extracted {}", path);

    Ok(())
}

fn donchian_midpoint(candles: &[Candle], index: usize, length: usize) -> Option<f64> {
    if index + 1 < length {
        return None;
    }

    let start = index + 1 - length;
    let slice = &candles[start..=index];

    let lowest = slice.iter().map(|c| c.low).fold(f64::INFINITY, f64::min);
    let highest = slice
        .iter()
        .map(|c| c.high)
        .fold(f64::NEG_INFINITY, f64::max);

    Some((lowest + highest) / 2.0)
}

fn ichimoku_processor(
    candles: &[Candle],
    conversion_periods: usize, // 9
    base_periods: usize,       // 26
    span_b_periods: usize,     // 52
    displacement: usize,       // 26
) -> Ichimoku {
    let len = candles.len();

    let mut conversion = vec![None; len];
    let mut base = vec![None; len];
    let mut span_a = vec![None; len + displacement];
    let mut span_b = vec![None; len + displacement];
    let mut lagging = vec![None; len];

    for i in 0..len {
        conversion[i] = donchian_midpoint(candles, i, conversion_periods);
        base[i] = donchian_midpoint(candles, i, base_periods);

        // Lagging span (close shifted backward)
        if i >= displacement {
            lagging[i - displacement] = Some(candles[i].close);
        }

        // Leading spans (shifted forward)
        if let (Some(conv), Some(base)) = (conversion[i], base[i]) {
            span_a[i + displacement] = Some((conv + base) / 2.0);
        }

        if let Some(b) = donchian_midpoint(candles, i, span_b_periods) {
            span_b[i + displacement] = Some(b);
        }
    }

    let bounds = kumo_bounds(&span_a, &span_b);
    //println!("bounds: {:?}", bounds);

    //println!("span_a: {:?}", span_a);
    //println!("span_b: {:?}", span_b);

    Ichimoku {
        conversion_line: conversion,
        base_line: base,
        leading_span_a: span_a,
        leading_span_b: span_b,
        lagging_span: lagging,
    }
}

fn tenkan_kijun_cross(
    tenkan: &[Option<f64>],
    kijun: &[Option<f64>],
) -> Vec<Option<TenkanKijunCross>> {
    let len = tenkan.len().min(kijun.len());
    let mut signals = vec![None; len];

    for i in 1..len {
        let (t_prev, k_prev) = (tenkan[i - 1], kijun[i - 1]);
        let (t_now, k_now) = (tenkan[i], kijun[i]);

        if let (Some(tp), Some(kp), Some(tn), Some(kn)) = (t_prev, k_prev, t_now, k_now) {
            // Bullish cross
            if tp <= kp && tn > kn {
                signals[i] = Some(TenkanKijunCross::Bullish);
            }

            // Bearish cross
            if tp >= kp && tn < kn {
                signals[i] = Some(TenkanKijunCross::Bearish);
            }
        }
    }

    signals
}

fn kumo_bounds(
    span_a: &[Option<f64>],
    span_b: &[Option<f64>],
) -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let len = span_a.len().min(span_b.len());

    let mut upper = vec![None; len];
    let mut lower = vec![None; len];

    for i in 0..len {
        match (span_a[i], span_b[i]) {
            (Some(a), Some(b)) => {
                upper[i] = Some(a.max(b));
                lower[i] = Some(a.min(b));
            }
            _ => {}
        }
    }

    (upper, lower)
}

fn get_last_25_spans(
    span_a: &[Option<f64>],
    span_b: &[Option<f64>],
) -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let len_a = span_a.len();
    let len_b = span_b.len();

    let start_a = if len_a > 25 { len_a - 25 } else { 0 };
    let start_b = if len_b > 25 { len_b - 25 } else { 0 };

    (span_a[start_a..].to_vec(), span_b[start_b..].to_vec())
}

async fn process_weekly_ichimoku(mut redis_conn: MultiplexedConnection) -> Result<()> {
    let weekly_candles = Helper::read_candles_from_csv("data/btcusd_weekly_data.csv").unwrap();
    let serde_weekly_candles = serde_json::to_string(&weekly_candles).unwrap();
    let _: () = redis_conn.set(WEEKLY_CANDLES, serde_weekly_candles).await?;

    let weekly_ichimoku = ichimoku_processor(&weekly_candles, 9, 26, 52, 26);
    let serde_weekly_ichimoku = serde_json::to_string(&weekly_ichimoku).unwrap();
    let _: () = redis_conn
        .set(WEEKLY_ICHIMOKU, serde_weekly_ichimoku)
        .await?;

    let (last_25_span_a, last_25_span_b) = get_last_25_spans(
        &weekly_ichimoku.leading_span_a,
        &weekly_ichimoku.leading_span_b,
    );

    let mut l_25 = HashMap::new();
    l_25.insert("span_a", last_25_span_a);
    l_25.insert("span_b", last_25_span_b);

    let serde_last_25_spans = serde_json::to_string(&l_25).unwrap();
    let _: () = redis_conn
        .set(LAST_25_WEEKLY_ICHIMOKU_SPANS, serde_last_25_spans)
        .await?;

    Ok(())
}

pub fn kumo_cross(span_a: &[Option<f64>], span_b: &[Option<f64>]) -> Vec<Option<KumoCross>> {
    let len = span_a.len().min(span_b.len());
    let mut signals = vec![None; len];

    for i in 1..len {
        let (a_prev, b_prev) = (span_a[i - 1], span_b[i - 1]);
        let (a_now, b_now) = (span_a[i], span_b[i]);

        if let (Some(ap), Some(bp), Some(an), Some(bn)) = (a_prev, b_prev, a_now, b_now) {
            // Bullish Kumo flip
            if ap <= bp && an > bn {
                signals[i] = Some(KumoCross::Bullish);
            }

            // Bearish Kumo flip
            if ap >= bp && an < bn {
                signals[i] = Some(KumoCross::Bearish);
            }
        }
    }

    signals
}

pub fn kumo_cross_from_bounds(
    upper: &[Option<f64>],
    lower: &[Option<f64>],
    span_a: &[Option<f64>],
) -> Vec<Option<KumoCross>> {
    let len = upper.len().min(span_a.len());
    let mut signals = vec![None; len];

    for i in 1..len {
        if let (Some(a_prev), Some(u_prev), Some(_l_prev), Some(a_now), Some(u_now), Some(_l_now)) = (
            span_a[i - 1],
            upper[i - 1],
            lower[i - 1],
            span_a[i],
            upper[i],
            lower[i],
        ) {
            let prev_bull = (u_prev - a_prev).abs() < 1e-9;
            let now_bull = (u_now - a_now).abs() < 1e-9;

            if !prev_bull && now_bull {
                signals[i] = Some(KumoCross::Bullish);
            }

            if prev_bull && !now_bull {
                signals[i] = Some(KumoCross::Bearish);
            }
        }
    }

    signals
}
