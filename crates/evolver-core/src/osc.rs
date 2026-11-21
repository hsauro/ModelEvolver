//! Phase- and amplitude-invariant "sustained oscillation" score.
//!
//! A port of the reference Python implementation
//! (`analysis/objective_prototype.py`, `score_signal` -> `score_combined`) that
//! the Pascal `uOscillationObjective.pas` was derived from. The Pascal version
//! zero-pads the spectrum to a power of two, which spreads each spectral peak
//! across more bins than its harmonic windows cover; a pure sine then scores a
//! harmonic fraction of ~0.90 instead of ~1.0. This port follows the Python
//! reference exactly (see objectpascal_bugs.md, B19).
//!
//! `score_oscillation` scores one species' trajectory and returns F in [0, 1]
//! (higher = cleaner sustained oscillator):
//!
//! F = rho * harmonic * sustained * amplitude
//!
//! - rho: autocorrelation-peak periodicity (waveform-invariant);
//! - harmonic: fraction of spectral power on the fundamental and its
//!   harmonics, with the fundamental anchored at the ACF period (a spiky
//!   waveform's 2nd harmonic is often taller than the fundamental);
//! - sustained: early/late amplitude ratio (rejects damped or growing signals);
//! - amplitude: relative fluctuation against a floor (rejects flat signals).

use std::cell::RefCell;
use std::f64::consts::PI;

use rustfft::{FftPlanner, num_complex::Complex};

const TRANSIENT_FRACTION: f64 = 0.4;
const REL_AMP_FLOOR: f64 = 0.02;
const MAX_HARMONIC: usize = 10;

thread_local! {
    // FFT plans are cached per thread (and per worker in the browser).
    static PLANNER: RefCell<FftPlanner<f64>> = RefCell::new(FftPlanner::new());
}

fn fft_forward(buf: &mut [Complex<f64>]) {
    PLANNER.with(|p| p.borrow_mut().plan_fft_forward(buf.len()).process(buf));
}

fn fft_inverse(buf: &mut [Complex<f64>]) {
    PLANNER.with(|p| p.borrow_mut().plan_fft_inverse(buf.len()).process(buf));
}

fn linear_detrend(x: &mut [f64]) {
    let n = x.len() as f64;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let i = i as f64;
        sx += i;
        sy += v;
        sxx += i * i;
        sxy += i * v;
    }
    let d = n * sxx - sx * sx;
    if d == 0.0 {
        return;
    }
    let a = (n * sxy - sx * sy) / d;
    let b = (sy - a * sx) / n;
    for (i, v) in x.iter_mut().enumerate() {
        *v -= a * i as f64 + b;
    }
}

fn mean_std(x: &[f64]) -> (f64, f64) {
    let n = x.len() as f64;
    let mean = x.iter().sum::<f64>() / n;
    let var = x.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n;
    (mean, var.sqrt())
}

/// Height and lag of the first autocorrelation peak past the initial descent
/// (Wiener-Khinchin with zero padding to 2n). Lag `None` if there is no peak.
fn acf_periodicity(x: &[f64]) -> (f64, Option<usize>) {
    let n = x.len();
    let (mean, std) = mean_std(x);
    if std < 1e-12 {
        return (0.0, None);
    }
    let mut buf = vec![Complex::new(0.0, 0.0); 2 * n];
    for (b, &v) in buf.iter_mut().zip(x) {
        b.re = v - mean;
    }
    fft_forward(&mut buf);
    for b in buf.iter_mut() {
        *b = Complex::new(b.norm_sqr(), 0.0);
    }
    fft_inverse(&mut buf);
    let acf0 = buf[0].re;
    if acf0 <= 0.0 {
        return (0.0, None);
    }
    let acf = |i: usize| buf[i].re / acf0;
    let start = (0..n).find(|&i| acf(i) < 0.0).unwrap_or(1);
    if start >= n - 1 {
        return (0.0, None);
    }
    let mut lag = start;
    for i in start + 1..n {
        if acf(i) > acf(lag) {
            lag = i;
        }
    }
    (acf(lag).max(0.0), Some(lag))
}

/// Fraction of (non-DC) spectral power on the fundamental and its harmonics.
fn harmonic_fraction(x: &[f64], dt: f64, lag: Option<usize>) -> f64 {
    let n = x.len();
    let (mean, std) = mean_std(x);
    if std < 1e-12 {
        return 0.0;
    }
    let mut buf: Vec<Complex<f64>> = x
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let hann = 0.5 - 0.5 * (2.0 * PI * i as f64 / (n - 1) as f64).cos();
            Complex::new((v - mean) * hann, 0.0)
        })
        .collect();
    fft_forward(&mut buf);
    let np = n / 2 + 1; // rfft length
    let mut p: Vec<f64> = buf[..np].iter().map(|c| c.norm_sqr()).collect();
    p[0] = 0.0;
    if np > 1 {
        p[1] = 0.0;
    }
    let total: f64 = p.iter().sum();
    if total <= 0.0 {
        return 0.0;
    }
    let argmax = |lo: usize, hi: usize| {
        // first index of the maximum in p[lo..hi]
        let mut best = lo;
        for i in lo + 1..hi {
            if p[i] > p[best] {
                best = i;
            }
        }
        best
    };
    let f0b = match lag {
        Some(lag) if lag > 0 => {
            let df = 1.0 / (n as f64 * dt);
            let f0 = (1.0 / (lag as f64 * dt)) / df;
            let w0 = ((0.15 * f0).round() as i64).max(2);
            let lo = (f0 as i64 - w0).max(1);
            let hi = (f0 as i64 + w0 + 1).min(np as i64);
            if hi <= lo {
                return 0.0;
            }
            argmax(lo as usize, hi as usize)
        }
        _ => argmax(0, np),
    };
    if f0b < 1 {
        return 0.0;
    }
    let mut harm = 0.0;
    for k in 1..=MAX_HARMONIC {
        let c = f0b * k;
        if c >= np {
            break;
        }
        let w = ((0.03 * (k * f0b) as f64).round() as usize).max(1);
        harm += p[c.saturating_sub(w)..(c + w + 1).min(np)].iter().sum::<f64>();
    }
    (harm / total).min(1.0)
}

fn sustained_factor(x: &[f64]) -> f64 {
    let h = x.len() / 2;
    let (_, se) = mean_std(&x[..h]);
    let (_, sl) = mean_std(&x[h..]);
    let mx = se.max(sl);
    if mx < 1e-12 { 0.0 } else { se.min(sl) / mx }
}

fn amplitude_factor(x: &[f64]) -> f64 {
    let (mean, std) = mean_std(x);
    let denom = if mean.abs() > 1e-9 { mean.abs() } else { 1.0 };
    (std / denom / REL_AMP_FLOOR).clamp(0.0, 1.0)
}

/// The four factors of the score: `[rho, harmonic, sustained, amplitude]`.
pub fn score_components(y: &[f64], dt: f64) -> [f64; 4] {
    if y.len() < 16 || y.iter().any(|v| !v.is_finite()) {
        return [0.0; 4];
    }
    let i0 = (TRANSIENT_FRACTION * y.len() as f64) as usize;
    let xr = &y[i0..];
    let mut xd = xr.to_vec();
    linear_detrend(&mut xd);
    let (rho, lag) = acf_periodicity(&xd);
    let harm = harmonic_fraction(&xd, dt, lag);
    [rho, harm, sustained_factor(xr), amplitude_factor(xr)]
}

/// Score one trajectory sampled at spacing `dt`. Returns F in [0, 1].
pub fn score_oscillation(y: &[f64], dt: f64) -> f64 {
    score_components(y, dt).iter().product()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(f: impl Fn(f64) -> f64) -> (Vec<f64>, f64) {
        let n = 2048;
        let dt = 150.0 / (n - 1) as f64;
        ((0..n).map(|i| f(i as f64 * dt)).collect(), dt)
    }

    // Golden values from analysis/objective_prototype.py (score_combined),
    // t = linspace(0, 150, 2048).
    fn check(f: impl Fn(f64) -> f64, expected: f64) {
        let (y, dt) = signal(f);
        let got = score_oscillation(&y, dt);
        assert!((got - expected).abs() < 1e-5, "got {got}, Python {expected}; {:?}", score_components(&y, dt));
    }

    #[test]
    fn sine() {
        check(|t| 5.0 + 2.0 * (2.0 * PI * t / 10.0).sin(), 0.886245);
    }

    #[test]
    fn spiky() {
        check(|t| 1.0 + (6.0 * (2.0 * PI * t / 12.0).sin()).exp(), 0.729133);
    }

    #[test]
    fn damped() {
        check(|t| 5.0 + 2.0 * (-t / 15.0).exp() * (2.0 * PI * t / 10.0).sin(), 0.001793);
    }

    #[test]
    fn ramp_and_flat() {
        check(|t| 1.0 + 0.1 * t, 0.0);
        check(|_| 3.0, 0.0);
    }
}
