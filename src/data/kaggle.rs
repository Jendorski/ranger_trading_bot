#![allow(dead_code)]

use anyhow::Result;
use std::fs;
use std::io;
use std::path::Path;

pub const KAGGLE_URL: &str =
    "https://www.kaggle.com/api/v1/datasets/download/mczielinski/bitcoin-historical-data";

pub const ZIP_PATH: &str = "data/btcusd_1-min_data.zip";
pub const CSV_PATH: &str = "data/btcusd_1-min_data.csv";

/// Download the Kaggle 1-minute BTC dataset zip and extract the CSV.
/// Pattern copied directly from btc_trading_bot/src/trackers/ichimoku/mod.rs.
/// Blocking — must be called inside `tokio::task::spawn_blocking`.
pub fn download_and_extract() -> Result<()> {
    fs::create_dir_all("data")?;

    println!("[kaggle] Downloading {KAGGLE_URL} ...");
    let mut response = reqwest::blocking::get(KAGGLE_URL)?;
    let mut temp = tempfile::NamedTempFile::new()?;
    io::copy(&mut response, &mut temp)?;
    temp.persist(ZIP_PATH)?;
    println!("[kaggle] Download complete → {ZIP_PATH}");

    println!("[kaggle] Extracting {ZIP_PATH} ...");
    let file = fs::File::open(ZIP_PATH)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let parent_dir = Path::new(ZIP_PATH).parent().unwrap_or(Path::new("."));

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let outpath = match entry.enclosed_name() {
            Some(p) => parent_dir.join(p),
            None => continue,
        };

        if entry.name().ends_with('/') {
            fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    fs::create_dir_all(p)?;
                }
            }
            let mut outfile = fs::File::create(&outpath)?;
            io::copy(&mut entry, &mut outfile)?;
        }
    }

    println!("[kaggle] Extraction complete → {CSV_PATH}");
    Ok(())
}
