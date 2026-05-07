/// Wilder's smoothed RSI — shared computation core.
///
/// Extracted to eliminate the identical implementation duplicated across
/// `RsiDivEngine` and `RsiRegimeTracker`. Neither module needs to know
/// about the other; both embed this struct directly.
///
/// Matches Pine Script's `ta.rsi`: SMA seed for the first `len` changes,
/// then Wilder's exponential smoothing for every bar after.
#[derive(Debug, Clone)]
pub(crate) struct RsiCore {
    len: usize,
    prev_close: Option<f64>,
    avg_gain: f64,
    avg_loss: f64,
    rsi_ready: bool,
    init_gains: Vec<f64>,
    init_losses: Vec<f64>,
}

impl RsiCore {
    pub(crate) fn new(len: usize) -> Self {
        Self {
            len,
            prev_close: None,
            avg_gain: 0.0,
            avg_loss: 0.0,
            rsi_ready: false,
            init_gains: Vec::with_capacity(len),
            init_losses: Vec::with_capacity(len),
        }
    }

    /// Feed the next close price. Returns `Some(rsi)` once the warm-up
    /// period (`len` bars) is complete, `None` during warm-up.
    pub(crate) fn update(&mut self, close: f64) -> Option<f64> {
        let result = if let Some(prev) = self.prev_close {
            let change = close - prev;
            let gain = change.max(0.0);
            let loss = (-change).max(0.0);

            if !self.rsi_ready {
                self.init_gains.push(gain);
                self.init_losses.push(loss);

                if self.init_gains.len() == self.len {
                    self.avg_gain = self.init_gains.iter().sum::<f64>() / self.len as f64;
                    self.avg_loss = self.init_losses.iter().sum::<f64>() / self.len as f64;
                    self.rsi_ready = true;
                    Some(self.compute())
                } else {
                    None
                }
            } else {
                let len_f = self.len as f64;
                self.avg_gain = (self.avg_gain * (len_f - 1.0) + gain) / len_f;
                self.avg_loss = (self.avg_loss * (len_f - 1.0) + loss) / len_f;
                Some(self.compute())
            }
        } else {
            None
        };

        self.prev_close = Some(close);
        result
    }

    /// Current RSI value. Returns `None` if the warm-up period is not yet complete.
    pub(crate) fn current(&self) -> Option<f64> {
        self.rsi_ready.then(|| self.compute())
    }

    #[inline]
    fn compute(&self) -> f64 {
        if self.avg_loss == 0.0 {
            return 100.0;
        }
        100.0 - 100.0 / (1.0 + self.avg_gain / self.avg_loss)
    }
}
