pub mod aggregator;
pub mod kaggle;

use chrono::Utc;

/// Seed files are considered stale when the most recent candle in
/// `candles_1d.json` is older than this many days. On the next startup the
/// seeds and the source CSV are deleted and fully regenerated from a fresh
/// Kaggle download.
const STALE_AFTER_DAYS: i64 = 7;

/// Ensures all seed files listed in [`aggregator::SEED_MANIFEST`] exist,
/// are valid, and are fresh before the bot's background tracker loops start.
///
/// Decision tree:
/// 1. All seeds present **and** last candle ≤ 7 days old → return immediately.
/// 2. Seeds present but stale → delete seeds + CSV, fall through to regenerate.
/// 3. Some seeds missing, Kaggle CSV already on disk → generate in
///    `spawn_blocking` (385 MB read — must not block the async runtime).
/// 4. Some seeds missing, CSV absent → attempt download first, then generate.
/// 5. Download fails → log a warning and return; tracker loops fall back to
///    live-only Bitget candles automatically.
pub async fn ensure_seeds_ready() {
    let all_present = aggregator::SEED_MANIFEST
        .iter()
        .all(|(_, path)| aggregator::seed_file_exists(path));

    if all_present {
        let stale = tokio::task::spawn_blocking(seeds_are_stale)
            .await
            .unwrap_or(true);

        if !stale {
            log::info!("[seeds] All seed files present and fresh — skipping aggregation");
            return;
        }

        log::info!(
            "[seeds] Seed files are stale (>{STALE_AFTER_DAYS} days old) — purging for refresh"
        );
        purge_seed_files();
    }

    // Ensure the CSV exists, downloading it if necessary.
    if !std::path::Path::new(kaggle::CSV_PATH).exists() {
        log::info!("[seeds] Kaggle CSV not found — downloading from Kaggle...");
        match tokio::task::spawn_blocking(kaggle::download_and_extract).await {
            Ok(Ok(())) => log::info!("[seeds] Download complete → {}", kaggle::CSV_PATH),
            Ok(Err(e)) => {
                log::warn!("[seeds] Download failed: {e} — tracker loops will use live-only data");
                return;
            }
            Err(e) => {
                log::warn!("[seeds] spawn_blocking error during download: {e}");
                return;
            }
        }
    }

    log::info!("[seeds] Generating missing seed files from {}", kaggle::CSV_PATH);
    match tokio::task::spawn_blocking(aggregator::generate_missing_seeds).await {
        Ok(Ok(())) => log::info!("[seeds] Seed generation complete"),
        Ok(Err(e)) => log::error!("[seeds] Seed generation failed: {e}"),
        Err(e) => log::error!("[seeds] spawn_blocking error during generation: {e}"),
    }
}

/// Returns `true` if the newest candle in `candles_1d.json` is older than
/// [`STALE_AFTER_DAYS`]. Returns `true` (treat as stale) on any read error
/// so a corrupt or unreadable seed always triggers a fresh regeneration.
///
/// Uses the 1D seed as the staleness reference because it has the finest
/// granularity and the most recent timestamp among all seed files.
fn seeds_are_stale() -> bool {
    match aggregator::read_candles_json("data/candles_1d.json") {
        Ok(candles) => match candles.last() {
            Some(c) => {
                let age_days = (Utc::now() - c.timestamp).num_days();
                log::info!("[seeds] Last 1D candle is {age_days} days old");
                age_days > STALE_AFTER_DAYS
            }
            None => true,
        },
        Err(_) => true,
    }
}

/// Deletes all seed JSON files and the source CSV so the next startup
/// downloads a fresh dataset and regenerates everything from scratch.
fn purge_seed_files() {
    for (_, path) in aggregator::SEED_MANIFEST {
        match std::fs::remove_file(path) {
            Ok(()) => log::info!("[seeds] Deleted {path}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("[seeds] Could not delete {path}: {e}"),
        }
    }
    match std::fs::remove_file(kaggle::CSV_PATH) {
        Ok(()) => log::info!("[seeds] Deleted {}", kaggle::CSV_PATH),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("[seeds] Could not delete {}: {e}", kaggle::CSV_PATH),
    }
}
