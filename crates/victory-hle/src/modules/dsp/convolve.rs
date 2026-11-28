//! The DSP's convolution, computed on the CPU.
//!
//! The title hands over each impulse response already in the coprocessor's
//! layout. Read from the responses the retail title passes to `XDspActivate`
//! (every block below checked against its neighbours): blocks of 2048 floats, each
//! the unscaled 2048-point FFT of a 2048-sample window of the response that
//! starts one block (1024 samples) before that block's own 1024 samples
//! (window k covers samples (k-1)*1024 .. (k+1)*1024, zeros before 0). The
//! bins are interleaved re/im, 0 to 1023 in natural order; bin 0's imaginary
//! slot is 0 (the Nyquist bin is not kept). A last block that starts with
//! the word 0xFFFFACE1 holds the hardware's schedule and is not response data.
//!
//! With windows like that, an input block zero-padded to 2048 samples and
//! multiplied with window k gives, in the second half of the inverse FFT,
//! that block's part of the output k blocks later: each output block is the
//! second half of IFFT(sum over k of X[t-k] * H[k]) / 2048. How the hardware
//! scales its result is not known; `dsp::GAIN` makes up for it.

const N: usize = 2048;
const B: usize = 1024;
const TRAILER: u32 = 0xFFFF_ACE1;

/// A complex radix-2 FFT of 2048 points.
struct Fft {
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<u16>,
}

fn fft() -> &'static Fft {
    static FFT: std::sync::OnceLock<Fft> = std::sync::OnceLock::new();
    FFT.get_or_init(|| {
        let bits = N.trailing_zeros();
        let rev = (0..N).map(|i| (i.reverse_bits() >> (usize::BITS - bits)) as u16).collect();
        let (cos, sin) = (0..N / 2)
            .map(|k| {
                let a = -2.0 * std::f64::consts::PI * k as f64 / N as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .unzip();
        Fft { cos, sin, rev }
    })
}

impl Fft {
    /// In place; `inverse` flips the sign of the exponent and does not scale.
    fn run(&self, re: &mut [f32; N], im: &mut [f32; N], inverse: bool) {
        for i in 0..N {
            let j = self.rev[i] as usize;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let sign = if inverse { -1.0 } else { 1.0 };
        let mut len = 2;
        while len <= N {
            let half = len / 2;
            let step = N / len;
            for start in (0..N).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], sign * self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let (br, bi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                    re[b] = re[a] - br;
                    im[b] = im[a] - bi;
                    re[a] += br;
                    im[a] += bi;
                }
            }
            len *= 2;
        }
    }
}

/// One mono convolution stream's state.
pub(super) struct Convolver {
    /// The response's windows: `parts` blocks of 1024 bins, split re/im.
    h_re: Vec<f32>,
    h_im: Vec<f32>,
    parts: usize,
    /// The last `parts` input spectra, a ring; `head` is the newest.
    x_re: Vec<f32>,
    x_im: Vec<f32>,
    head: usize,
    /// Input blocks in a row that were silent: once the ring holds only
    /// silence, the output is silence without any arithmetic.
    quiet: usize,
}

impl Convolver {
    /// `None` when the response is not in the layout described above.
    pub(super) fn new(ir: &[f32]) -> Option<Self> {
        if ir.is_empty() || ir.len() % N != 0 {
            return None;
        }
        let mut blocks = ir.len() / N;
        if ir[(blocks - 1) * N].to_bits() == TRAILER {
            blocks -= 1;
        }
        // trailing windows of zeros add nothing
        while blocks > 0 && ir[(blocks - 1) * N..blocks * N].iter().all(|&v| v == 0.0) {
            blocks -= 1;
        }
        let parts = blocks.max(1);
        let mut h_re = vec![0f32; parts * B];
        let mut h_im = vec![0f32; parts * B];
        for k in 0..blocks {
            let block = &ir[k * N..(k + 1) * N];
            for bin in 0..B {
                let (r, i) = (block[2 * bin], block[2 * bin + 1]);
                // a non-finite value is not response data; leave it out
                h_re[k * B + bin] = if r.is_finite() { r } else { 0.0 };
                h_im[k * B + bin] = if i.is_finite() { i } else { 0.0 };
            }
        }
        Some(Self {
            h_re,
            h_im,
            parts,
            x_re: vec![0f32; parts * B],
            x_im: vec![0f32; parts * B],
            head: 0,
            quiet: usize::MAX,
        })
    }

    /// One block: 1024 input samples in, 1024 output samples out, times `gain`.
    pub(super) fn process(&mut self, input: &[f32], output: &mut [f32], gain: f32) {
        let silent = input.iter().all(|&v| v == 0.0 || !v.is_finite());
        self.quiet = if silent { self.quiet.saturating_add(1) } else { 0 };
        self.head = (self.head + 1) % self.parts;
        let slot = self.head * B;
        if silent {
            self.x_re[slot..slot + B].fill(0.0);
            self.x_im[slot..slot + B].fill(0.0);
            if self.quiet >= self.parts {
                output.fill(0.0);
                return;
            }
        } else {
            let mut re = [0f32; N];
            let mut im = [0f32; N];
            for (r, &v) in re.iter_mut().zip(input) {
                *r = if v.is_finite() { v } else { 0.0 };
            }
            fft().run(&mut re, &mut im, false);
            self.x_re[slot..slot + B].copy_from_slice(&re[..B]);
            self.x_im[slot..slot + B].copy_from_slice(&im[..B]);
        }
        // sum over k of X[t-k] * H[k]
        let mut yr = [0f32; N];
        let mut yi = [0f32; N];
        for k in 0..self.parts {
            let x = ((self.head + self.parts - k) % self.parts) * B;
            let h = k * B;
            let (xr, xi) = (&self.x_re[x..x + B], &self.x_im[x..x + B]);
            let (hr, hi) = (&self.h_re[h..h + B], &self.h_im[h..h + B]);
            for bin in 0..B {
                yr[bin] += xr[bin] * hr[bin] - xi[bin] * hi[bin];
                yi[bin] += xr[bin] * hi[bin] + xi[bin] * hr[bin];
            }
        }
        // the spectrum of a real signal: the upper half mirrors the lower (Nyquist bin stays 0)
        for bin in 1..B {
            yr[N - bin] = yr[bin];
            yi[N - bin] = -yi[bin];
        }
        yi[0] = 0.0;
        fft().run(&mut yr, &mut yi, true);
        let scale = gain / N as f32;
        for (o, &v) in output.iter_mut().zip(&yr[B..]) {
            *o = v * scale;
        }
    }
}

/// Builds a response in the hardware layout from time-domain samples, as
/// the title's responses were found to be laid out.
#[cfg(test)]
pub(super) fn layout(ir: &[f32]) -> Vec<f32> {
    // one window more than the response has blocks: its first half holds the last block
    let blocks = ir.len().div_ceil(B) + 1;
    let mut out = vec![0f32; (blocks + 1) * N];
    for k in 0..blocks {
        let mut re = [0f32; N];
        let mut im = [0f32; N];
        for (i, r) in re.iter_mut().enumerate() {
            let t = (k * B + i) as isize - B as isize;
            if t >= 0 && (t as usize) < ir.len() {
                *r = ir[t as usize];
            }
        }
        fft().run(&mut re, &mut im, false);
        for bin in 0..B {
            out[k * N + 2 * bin] = re[bin];
            out[k * N + 2 * bin + 1] = if bin == 0 { 0.0 } else { im[bin] };
        }
    }
    out[blocks * N] = f32::from_bits(TRAILER);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_a_direct_convolution_across_blocks() {
        let ir: Vec<f32> = (0..2500).map(|i| ((i * 37 % 101) as f32 - 50.0) / 400.0 * (-(i as f32) / 900.0).exp()).collect();
        let mut c = Convolver::new(&layout(&ir)).unwrap();
        let input: Vec<f32> = (0..B * 5).map(|i| ((i * 13 % 29) as f32 - 14.0) / 20.0).collect();
        let mut got = vec![0f32; B * 5];
        for (i, o) in input.chunks(B).zip(got.chunks_mut(B)) {
            c.process(i, o, 1.0);
        }
        for n in 0..B * 5 {
            let want: f32 = (0..=n).filter(|&m| n - m < ir.len()).map(|m| input[m] * ir[n - m]).sum();
            assert!((got[n] - want).abs() < 2e-3, "sample {n}: {} vs {want}", got[n]);
        }
    }

    #[test]
    fn a_silent_ring_gives_silence() {
        let mut c = Convolver::new(&layout(&[1.0, 0.5])).unwrap();
        let mut out = vec![1f32; B];
        for _ in 0..4 {
            c.process(&[0.0; B], &mut out, 1.0);
        }
        assert!(out.iter().all(|&v| v == 0.0));
    }
}
