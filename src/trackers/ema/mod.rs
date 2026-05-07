/// Standard exponential moving average.
///
/// Seeds from the first bar's value (TradingView `ta.ema` convention) and
/// applies k = 2 / (period + 1) smoothing on every subsequent bar.
#[derive(Debug, Clone)]
pub struct Ema {
    k: f64,
    value: Option<f64>,
}

impl Ema {
    pub fn new(period: usize) -> Self {
        Self {
            k: 2.0 / (period as f64 + 1.0),
            value: None,
        }
    }

    /// Feed the next price. Returns the updated EMA value.
    pub fn update(&mut self, price: f64) -> f64 {
        let v = match self.value {
            None => price,
            Some(prev) => prev + self.k * (price - prev),
        };
        self.value = Some(v);
        v
    }
    // pub fn update(&mut self, price: f64) -> f64 {
    //     match self.value {
    //         None => {
    //             self.value = Some(price);
    //             price
    //         }
    //         Some(prev_ema) => {
    //             let ema = (price * self.k) + (prev_ema * (1.0 - self.k));

    //             self.value = Some(ema);
    //             ema
    //         }
    //     }
    // }

    pub fn current(&self) -> Option<f64> {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_from_first_bar() {
        let mut ema = Ema::new(3);
        assert_eq!(ema.current(), None);
        let v = ema.update(100.0);
        assert_eq!(v, 100.0);
        assert_eq!(ema.current(), Some(100.0));
    }

    #[test]
    fn converges_toward_new_price() {
        let mut ema = Ema::new(3); // k = 0.5
        ema.update(100.0);
        let v = ema.update(200.0); // 100 + 0.5*(200-100) = 150
        assert!((v - 150.0).abs() < 1e-9);
    }

    #[test]
    fn flat_series_stays_flat() {
        let mut ema = Ema::new(10);
        for _ in 0..50 {
            ema.update(50.0);
        }
        assert!((ema.current().unwrap() - 50.0).abs() < 1e-9);
    }
}
