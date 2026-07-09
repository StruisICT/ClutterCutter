// Squarified treemap layout (Bruls, Huizing & van Wijk 2000). Pure geometry —
// no Win32 — so the GUI's treemap view can unit-test its math headlessly.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rectf {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rectf {
    pub fn area(&self) -> f64 {
        self.w * self.h
    }
}

/// Lays out `sizes` inside `bounds` as a squarified treemap. Returns one rect
/// per input, in input order; zero/negative sizes get a zero-area rect. The
/// rects tile `bounds` — areas proportional to size, no overlap, no gaps
/// beyond float error. Input order doesn't matter for correctness; aspect
/// ratios are best when sizes are roughly descending.
pub fn layout(sizes: &[i64], bounds: Rectf) -> Vec<Rectf> {
    let mut out = vec![Rectf::default(); sizes.len()];
    let total: f64 = sizes.iter().filter(|&&s| s > 0).map(|&s| s as f64).sum();
    if total <= 0.0 || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return out;
    }

    // Squarify over positive sizes in descending order; map results back to
    // the caller's order via the index vector.
    let mut order: Vec<usize> = (0..sizes.len()).filter(|&i| sizes[i] > 0).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(sizes[i]));

    let scale = bounds.area() / total;
    let areas: Vec<f64> = order.iter().map(|&i| sizes[i] as f64 * scale).collect();

    let mut rem = bounds;
    let mut start = 0;
    while start < areas.len() {
        // Grow the row while doing so improves the worst aspect ratio.
        let side = rem.w.min(rem.h);
        let mut end = start + 1;
        let mut best = worst_ratio(&areas[start..end], side);
        while end < areas.len() {
            let cand = worst_ratio(&areas[start..end + 1], side);
            if cand > best {
                break;
            }
            best = cand;
            end += 1;
        }
        lay_row(&areas[start..end], &order[start..end], &mut rem, &mut out);
        start = end;
    }
    out
}

// Worst (largest) aspect ratio in a row of `areas` laid along a side of
// length `side`. Ratios are >= 1.0; lower is squarer.
fn worst_ratio(areas: &[f64], side: f64) -> f64 {
    let sum: f64 = areas.iter().sum();
    if sum <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let (mut min, mut max) = (f64::INFINITY, 0.0f64);
    for &a in areas {
        min = min.min(a);
        max = max.max(a);
    }
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min))
}

// Lays a finished row along the shorter side of `rem`, writing each item's
// rect to `out` at its original index, then shrinks `rem` by the row.
fn lay_row(areas: &[f64], idxs: &[usize], rem: &mut Rectf, out: &mut [Rectf]) {
    let sum: f64 = areas.iter().sum();
    if sum <= 0.0 {
        return;
    }
    if rem.w >= rem.h {
        // Vertical strip on the left edge; items stack top-to-bottom.
        let t = sum / rem.h.max(f64::MIN_POSITIVE);
        let mut y = rem.y;
        for (&a, &oi) in areas.iter().zip(idxs) {
            let ih = a / t;
            out[oi] = Rectf {
                x: rem.x,
                y,
                w: t,
                h: ih,
            };
            y += ih;
        }
        rem.x += t;
        rem.w = (rem.w - t).max(0.0);
    } else {
        // Horizontal strip along the top edge; items run left-to-right.
        let t = sum / rem.w.max(f64::MIN_POSITIVE);
        let mut x = rem.x;
        for (&a, &oi) in areas.iter().zip(idxs) {
            let iw = a / t;
            out[oi] = Rectf {
                x,
                y: rem.y,
                w: iw,
                h: t,
            };
            x += iw;
        }
        rem.y += t;
        rem.h = (rem.h - t).max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-6;

    fn bounds() -> Rectf {
        Rectf {
            x: 10.0,
            y: 20.0,
            w: 600.0,
            h: 400.0,
        }
    }

    fn intersection_area(a: &Rectf, b: &Rectf) -> f64 {
        let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
        let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert!(layout(&[], bounds()).is_empty());
    }

    #[test]
    fn all_nonpositive_sizes_give_zero_rects() {
        let rects = layout(&[0, -5, 0], bounds());
        assert_eq!(rects, vec![Rectf::default(); 3]);
    }

    #[test]
    fn areas_are_proportional_to_sizes() {
        let sizes = [6_000, 3_000, 2_000, 1_000];
        let total: i64 = sizes.iter().sum();
        let rects = layout(&sizes, bounds());
        for (size, rect) in sizes.iter().zip(&rects) {
            let expected = bounds().area() * (*size as f64) / total as f64;
            assert!(
                (rect.area() - expected).abs() < EPS * bounds().area(),
                "size {size}: area {} != expected {expected}",
                rect.area()
            );
        }
    }

    #[test]
    fn output_matches_input_order_even_when_unsorted() {
        let sizes = [1_000, 9_000, 4_000];
        let rects = layout(&sizes, bounds());
        assert!(rects[1].area() > rects[2].area());
        assert!(rects[2].area() > rects[0].area());
    }

    #[test]
    fn zero_sizes_mixed_with_real_ones_get_zero_rects() {
        let sizes = [5_000, 0, 3_000, 0];
        let rects = layout(&sizes, bounds());
        assert_eq!(rects[1], Rectf::default());
        assert_eq!(rects[3], Rectf::default());
        assert!(rects[0].area() > 0.0 && rects[2].area() > 0.0);
    }

    #[test]
    fn rects_stay_inside_bounds() {
        let sizes: Vec<i64> = (1..=25).map(|i| i * 997).collect();
        let b = bounds();
        for r in layout(&sizes, b) {
            assert!(r.x >= b.x - EPS && r.y >= b.y - EPS);
            assert!(r.x + r.w <= b.x + b.w + EPS);
            assert!(r.y + r.h <= b.y + b.h + EPS);
        }
    }

    #[test]
    fn rects_tile_without_overlap() {
        let sizes: Vec<i64> = (1..=12).map(|i| i * i * 100).collect();
        let rects = layout(&sizes, bounds());
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                let overlap = intersection_area(&rects[i], &rects[j]);
                assert!(
                    overlap < EPS * bounds().area(),
                    "rects {i} and {j} overlap by {overlap}"
                );
            }
        }
    }

    #[test]
    fn rects_cover_the_full_bounds_area() {
        let sizes = [7, 5, 3, 2, 1, 1];
        let covered: f64 = layout(&sizes, bounds()).iter().map(Rectf::area).sum();
        assert!((covered - bounds().area()).abs() < EPS * bounds().area());
    }

    #[test]
    fn four_equal_sizes_in_a_square_make_quadrants() {
        let b = Rectf {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        for r in layout(&[10, 10, 10, 10], b) {
            assert!((r.w - 50.0).abs() < EPS && (r.h - 50.0).abs() < EPS);
        }
    }
}
