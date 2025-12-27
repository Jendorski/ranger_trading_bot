use anyhow::Result;
use futures_util::StreamExt;
use log::info;
use std::fs;
use std::io::{Read, SeekFrom, Write};
use tokio::fs::File;

use std::path::Path;
use std::time::Duration;
use tokio::time;
//Ichimoku is used for BTC on the weekly timeframe
///Download the one-minute BTCUSD from the dataset from : https://www.kaggle.com/api/v1/datasets/download/mczielinski/bitcoin-historical-data,
/// resolve it into a weekly timeframe, and calculate the ichimoku
pub async fn ichimoku_loop() -> Result<()> {
    let loop_interval_seconds = 604800;

    let mut interval = time::interval(Duration::from_secs(loop_interval_seconds));

    let url = "https://www.kaggle.com/api/v1/datasets/download/mczielinski/bitcoin-historical-data";

    loop {
        interval.tick().await;
        if let Err(e) = download_large_file(url, "data/btcusd_1-min_data.zip").await {
            eprintln!("CRITICAL ERROR in ichimoku_loop: {:?}", e);
            eprintln!("Retrying in {} seconds...", loop_interval_seconds);
        }
    }
}

async fn download_large_file(url: &str, path: &str) -> Result<()> {
    println!("Downloading {}...", url);

    let mut stream = reqwest::get(url).await?.bytes_stream();

    let mut temp = tempfile::NamedTempFile::new()?;
    while let Some(item) = stream.next().await {
        temp.write_all(item?.as_ref())?;
    }

    temp.persist(path)?;
    println!("Downloaded {}", url);

    Ok(())
}
