//! Tonal adjustments and straightening baked into pixels. The same formulas
//! run in the image shader for the live preview (`shaders/image.hlsl`,
//! `adjust`), so what is applied is what was seen.

/// Exposure (EV stops), contrast, saturation and warmth, each roughly in
/// −1…1 (exposure −2…2). All zero: no change.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Adjust {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub warmth: f32,
}

impl Adjust {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    /// For the shader: the four values in one vector.
    pub fn to_array(self) -> [f32; 4] {
        [self.exposure, self.contrast, self.saturation, self.warmth]
    }

    /// One linear-light colour (straight alpha).
    pub fn apply_linear(&self, c: [f32; 3]) -> [f32; 3] {
        let gain = 2f32.powf(self.exposure);
        let mut c = c.map(|v| v * gain);
        // Warmth: a gentle white-balance shift.
        c[0] *= 1.0 + 0.12 * self.warmth;
        c[2] *= 1.0 - 0.12 * self.warmth;
        // Contrast around middle grey, in a perceptual (gamma) space.
        if self.contrast != 0.0 {
            let k = 1.0 + self.contrast;
            c = c.map(|v| {
                let p = v.max(0.0).powf(1.0 / 2.2);
                (((p - 0.46) * k + 0.46).max(0.0)).powf(2.2)
            });
        }
        if self.saturation != 0.0 {
            let lum = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            let k = 1.0 + self.saturation;
            c = c.map(|v| (lum + (v - lum) * k).max(0.0));
        }
        c.map(|v| v.clamp(0.0, 1.0))
    }

    /// Straight-alpha sRGB RGBA8 pixels, in place.
    pub fn apply_srgb8(&self, px: &mut [u8]) {
        if self.is_identity() {
            return;
        }
        let lut: Vec<f32> = (0..256).map(|i| to_linear(i as f32 / 255.0)).collect();
        for p in px.as_chunks_mut::<4>().0 {
            let c = self.apply_linear([
                lut[usize::from(p[0])],
                lut[usize::from(p[1])],
                lut[usize::from(p[2])],
            ]);
            for (d, v) in p.iter_mut().zip(c) {
                *d = (to_srgb(v) * 255.0 + 0.5) as u8;
            }
        }
    }
}

fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Scale of the largest rectangle with the image's proportions that fits
/// inside the image turned by `deg` degrees.
pub fn inscribed_scale(w: f64, h: f64, deg: f64) -> f64 {
    let (s, c) = deg.to_radians().abs().sin_cos();
    (w / (w * c + h * s)).min(h / (w * s + h * c))
}

/// Turns RGBA8 pixels by `deg` degrees clockwise (as the view shows) and
/// keeps the largest upright rectangle without empty corners, sampling
/// bilinearly. Returns the pixels and their size.
pub fn straighten(px: &[u8], w: usize, h: usize, deg: f64) -> (Vec<u8>, usize, usize) {
    if deg == 0.0 {
        return (px.to_vec(), w, h);
    }
    let k = inscribed_scale(w as f64, h as f64, deg);
    let (ow, oh) = (
        ((w as f64 * k).floor() as usize).max(1),
        ((h as f64 * k).floor() as usize).max(1),
    );
    let (s, c) = deg.to_radians().sin_cos();
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let (ocx, ocy) = (ow as f64 / 2.0, oh as f64 / 2.0);
    let mut out = vec![0u8; ow * oh * 4];
    let at = |x: usize, y: usize| &px[(y * w + x) * 4..][..4];
    for y in 0..oh {
        for x in 0..ow {
            // Output offset from the centre, turned back into the source.
            let (dx, dy) = (x as f64 + 0.5 - ocx, y as f64 + 0.5 - ocy);
            let sx = cx + dx * c + dy * s - 0.5;
            let sy = cy - dx * s + dy * c - 0.5;
            let (x0, y0) = (sx.floor().max(0.0) as usize, sy.floor().max(0.0) as usize);
            let (x0, y0) = (x0.min(w - 1), y0.min(h - 1));
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (
                (sx - x0 as f64).clamp(0.0, 1.0),
                (sy - y0 as f64).clamp(0.0, 1.0),
            );
            let d = &mut out[(y * ow + x) * 4..][..4];
            for (i, v) in d.iter_mut().enumerate() {
                let top = f64::from(at(x0, y0)[i]) * (1.0 - fx) + f64::from(at(x1, y0)[i]) * fx;
                let bot = f64::from(at(x0, y1)[i]) * (1.0 - fx) + f64::from(at(x1, y1)[i]) * fx;
                *v = (top * (1.0 - fy) + bot * fy).round() as u8;
            }
        }
    }
    (out, ow, oh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_directions() {
        let mut px = vec![10, 128, 250, 255];
        Adjust::default().apply_srgb8(&mut px);
        assert_eq!(px, [10, 128, 250, 255]);
        let grey = [0.2, 0.2, 0.2];
        let brighter = Adjust {
            exposure: 1.0,
            ..Default::default()
        }
        .apply_linear(grey);
        assert!((brighter[0] - 0.4).abs() < 1e-5);
        let warm = Adjust {
            warmth: 1.0,
            ..Default::default()
        }
        .apply_linear(grey);
        assert!(warm[0] > grey[0] && warm[2] < grey[2]);
        let grey_stays = Adjust {
            saturation: -1.0,
            ..Default::default()
        }
        .apply_linear([0.5, 0.1, 0.1]);
        assert!((grey_stays[0] - grey_stays[1]).abs() < 1e-6);
    }

    #[test]
    fn straighten_crops_inside_and_turns_clockwise() {
        // A 41×41 image with one red pixel right of the centre.
        let (w, h) = (41, 41);
        let mut px = vec![255u8; w * h * 4];
        let red = (30, 20);
        px[(red.1 * w + red.0) * 4..][..4].copy_from_slice(&[255, 0, 0, 255]);
        let (out, ow, oh) = straighten(&px, w, h, 90.0);
        assert_eq!((ow, oh), (w, h));
        // Clockwise: right of the centre goes below it.
        let (x, y) = (20, 30);
        assert_eq!(&out[(y * ow + x) * 4..][..4], &[255, 0, 0, 255]);
        // A small tilt shrinks the frame, and no corner is empty.
        let k = inscribed_scale(400.0, 300.0, 5.0);
        assert!(k < 1.0 && k > 0.85);
    }
}
