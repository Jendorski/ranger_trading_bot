/// Gaussian Channel indicator — port of DonovanWall's GC [DW] (Pine Script).
///
/// Algorithm: Ehlers' Multi-Pole Gaussian IIR filter applied to HL2 source
/// and filtered true range separately. Bands = midline ± multiplier × filtered_TR.
///
/// Filter coefficients:
///   beta  = (1 − cos(2π/sampling)) / (2^(1/poles) − 1)
///   alpha = −beta + √(beta² + 2·beta)
///
/// Recurrence for poles = p:
///   filt[t] = α^p·src + Σ_{k=1}^{p} C(p,k)·(1−α)^k·(−1)^(k+1)·filt[t−k]
///
/// First-bar seed: all history initialised with src (equivalent to Pine Script
/// `nz(filt[k], src)`) — provably yields filt = src via the binomial theorem.
use std::collections::VecDeque;

// ─── Internal filter ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct GaussianFilter {
    alpha: f64,
    poles: usize,
    /// Previous filter outputs: hist[0]=t−1, hist[1]=t−2, … — length == poles
    hist: VecDeque<f64>,
}

impl GaussianFilter {
    fn new(poles: usize, sampling_period: usize) -> Self {
        let beta = (1.0 - (2.0 * std::f64::consts::PI / sampling_period as f64).cos())
            / (2.0_f64.powf(1.0 / poles as f64) - 1.0);
        let alpha = -beta + (beta * beta + 2.0 * beta).sqrt();
        Self {
            alpha,
            poles,
            hist: VecDeque::with_capacity(poles),
        }
    }

    fn update(&mut self, src: f64) -> f64 {
        if self.hist.is_empty() {
            for _ in 0..self.poles {
                self.hist.push_back(src);
            }
            return src;
        }

        let a = self.alpha;
        let b = 1.0 - a;
        let binom = binomial_row(self.poles);

        let mut result = a.powi(self.poles as i32) * src;
        for (k, &coef) in binom.iter().enumerate().skip(1) {
            let sign = if k % 2 == 1 { 1.0_f64 } else { -1.0_f64 };
            result += sign * coef as f64 * b.powi(k as i32) * self.hist[k - 1];
        }

        self.hist.pop_back();
        self.hist.push_front(result);
        result
    }
}

/// Row n of Pascal's triangle: [C(n,0), C(n,1), …, C(n,n)]
fn binomial_row(n: usize) -> Vec<u64> {
    let mut row = vec![1u64; n + 1];
    for k in 1..n {
        row[k] = row[k - 1] * (n as u64 - k as u64 + 1) / k as u64;
    }
    row
}

// ─── Public channel ───────────────────────────────────────────────────────────

/// Gaussian Channel with configurable poles, sampling period, and multiplier.
///
/// Call [`GaussianChannel::update`] once per bar in chronological order.
/// Output fields (`midline`, `upper_band`, `lower_band`) are `None` until the
/// first bar is processed.
#[derive(Debug, Clone)]
pub struct GaussianChannel {
    src_filter: GaussianFilter,
    tr_filter: GaussianFilter,
    multiplier: f64,
    prev_close: Option<f64>,
    pub midline: Option<f64>,
    pub upper_band: Option<f64>,
    pub lower_band: Option<f64>,
}

impl GaussianChannel {
    /// Create a new channel.
    ///
    /// `poles` must be 1–4 (matches the indicator's supported range).
    /// `sampling_period` is in bars (144 for the weekly/biweekly macro use case).
    /// `multiplier` scales the filtered true range for the bands (1.414 = √2).
    pub fn new(poles: usize, sampling_period: usize, multiplier: f64) -> Self {
        Self {
            src_filter: GaussianFilter::new(poles, sampling_period),
            tr_filter: GaussianFilter::new(poles, sampling_period),
            multiplier,
            prev_close: None,
            midline: None,
            upper_band: None,
            lower_band: None,
        }
    }

    /// Feed the next OHLC bar. Updates `midline`, `upper_band`, `lower_band`.
    pub fn update(&mut self, high: f64, low: f64, close: f64) {
        let src = (high + low) / 2.0;
        let tr = match self.prev_close {
            Some(pc) => pc.max(high) - pc.min(low),
            None => high - low,
        };
        self.prev_close = Some(close);

        let mid = self.src_filter.update(src);
        let ftr = self.tr_filter.update(tr);

        self.midline = Some(mid);
        self.upper_band = Some(mid + self.multiplier * ftr);
        self.lower_band = Some(mid - self.multiplier * ftr);
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binomial_row_poles4() {
        assert_eq!(binomial_row(4), vec![1, 4, 6, 4, 1]);
    }

    #[test]
    fn binomial_row_poles1() {
        assert_eq!(binomial_row(1), vec![1, 1]);
    }

    #[test]
    fn gaussian_filter_seeds_at_src() {
        let mut f = GaussianFilter::new(4, 144);
        let v = f.update(1000.0);
        assert!((v - 1000.0).abs() < 1e-9, "first bar should equal src");
    }

    #[test]
    fn gaussian_filter_flat_series_converges() {
        let mut f = GaussianFilter::new(4, 144);
        let mut last = 0.0;
        for _ in 0..500 {
            last = f.update(5000.0);
        }
        assert!(
            (last - 5000.0).abs() < 0.01,
            "filter should converge to constant src, got {last}"
        );
    }

    #[test]
    fn channel_bands_straddle_midline() {
        let mut gc = GaussianChannel::new(4, 144, 1.414);
        for i in 0..200 {
            let price = 50_000.0 + (i as f64).sin() * 1000.0;
            gc.update(price * 1.005, price * 0.995, price);
        }
        let mid = gc.midline.unwrap();
        let upper = gc.upper_band.unwrap();
        let lower = gc.lower_band.unwrap();
        assert!(upper > mid, "upper band must be above midline");
        assert!(lower < mid, "lower band must be below midline");
    }
}
