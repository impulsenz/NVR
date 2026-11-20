//! Geometry for the floor plan (plan.rs, tracks.rs): turning a point on a
//! camera's picture into a point on the plan.
//!
//! A camera looking down at a flat floor (or lawn, or driveway) sees it in
//! perspective. Four spots on that floor whose place on the plan is known fix
//! the mapping exactly (a *homography*); every other spot on the same floor
//! follows. Points are fractions of the picture / of the plan (0..1).
#![allow(clippy::needless_range_loop)] // small fixed matrices: indexed loops read like the maths

pub type Pt = (f64, f64);

/// The 3×3 matrix (row by row) that maps picture points to plan points.
pub type Homography = [f64; 9];

/// Solve the mapping from four picture points to four plan points. None when
/// the points can't fix one (three in a line, two the same …).
pub fn homography(from: [Pt; 4], to: [Pt; 4]) -> Option<Homography> {
    // Eight equations, eight unknowns (the ninth matrix entry is 1).
    let mut m = [[0f64; 9]; 8];
    for i in 0..4 {
        let ((x, y), (u, v)) = (from[i], to[i]);
        m[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        m[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for col in 0..8 {
        // The largest remaining entry in this column becomes the pivot.
        let piv = (col..8).max_by(|a, b| m[*a][col].abs().total_cmp(&m[*b][col].abs()))?;
        if m[piv][col].abs() < 1e-10 {
            return None;
        }
        m.swap(col, piv);
        for row in 0..8 {
            if row != col {
                let f = m[row][col] / m[col][col];
                for k in col..9 {
                    m[row][k] -= f * m[col][k];
                }
            }
        }
    }
    let mut h = [0f64; 9];
    for i in 0..8 {
        h[i] = m[i][8] / m[i][i];
    }
    h[8] = 1.0;
    h.iter().all(|v| v.is_finite()).then_some(h)
}

/// Where a picture point lands on the plan. None for a point the mapping
/// sends to infinity (the horizon).
pub fn apply(h: &Homography, (x, y): Pt) -> Option<Pt> {
    let w = h[6] * x + h[7] * y + h[8];
    if w.abs() < 1e-9 {
        return None;
    }
    let p = ((h[0] * x + h[1] * y + h[2]) / w, (h[3] * x + h[4] * y + h[5]) / w);
    (p.0.is_finite() && p.1.is_finite()).then_some(p)
}

/// Distance in metres between two plan points (fractions of the plan's width
/// and height), given the plan's size in pixels and its scale.
pub fn metres(a: Pt, b: Pt, plan_w_px: f64, plan_h_px: f64, px_per_m: f64) -> f64 {
    if px_per_m <= 0.0 {
        return 0.0;
    }
    let dx = (a.0 - b.0) * plan_w_px;
    let dy = (a.1 - b.1) * plan_h_px;
    (dx * dx + dy * dy).sqrt() / px_per_m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Pt, b: Pt) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn four_points_fix_the_mapping_and_other_points_follow() {
        // A square on the picture that is a wide trapezoid on the floor plan
        // (the far side is narrower in the picture, as in perspective).
        let pic = [(0.1, 0.9), (0.9, 0.9), (0.7, 0.4), (0.3, 0.4)];
        let plan = [(0.2, 0.8), (0.6, 0.8), (0.6, 0.4), (0.2, 0.4)];
        let h = homography(pic, plan).unwrap();
        for i in 0..4 {
            assert!(close(apply(&h, pic[i]).unwrap(), plan[i]), "corner {i}");
        }
        // The middle of the far edge and of the near edge: straight lines stay straight.
        let far = apply(&h, (0.5, 0.4)).unwrap();
        assert!(close(far, (0.4, 0.4)), "{far:?}");
        let near = apply(&h, (0.5, 0.9)).unwrap();
        assert!(close(near, (0.4, 0.8)), "{near:?}");
        // The perspective is real: halfway up the picture is NOT halfway up the floor.
        let mid = apply(&h, (0.5, 0.65)).unwrap();
        assert!(mid.1 > 0.6 + 1e-3 || mid.1 < 0.6 - 1e-3, "{mid:?}");
        assert!((mid.0 - 0.4).abs() < 1e-6, "on the centre line");
    }

    #[test]
    fn the_identity_and_a_pure_scale_are_found_exactly() {
        let sq = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let h = homography(sq, sq).unwrap();
        assert!(close(apply(&h, (0.3, 0.7)).unwrap(), (0.3, 0.7)));
        let half = [(0.0, 0.0), (0.5, 0.0), (0.5, 0.5), (0.0, 0.5)];
        let h = homography(sq, half).unwrap();
        assert!(close(apply(&h, (0.4, 0.8)).unwrap(), (0.2, 0.4)));
    }

    #[test]
    fn points_that_cannot_fix_a_mapping_are_refused() {
        let sq = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        // Three picture points in a line.
        assert!(homography([(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (0.0, 1.0)], sq).is_none());
        // Two the same.
        assert!(homography([(0.0, 0.0), (0.0, 0.0), (1.0, 1.0), (0.0, 1.0)], sq).is_none());
    }

    #[test]
    fn distances_use_the_plans_size_and_scale() {
        // 1000 × 500 px plan, 50 px per metre: the whole width is 20 m.
        assert!((metres((0.0, 0.0), (1.0, 0.0), 1000.0, 500.0, 50.0) - 20.0).abs() < 1e-9);
        assert!((metres((0.0, 0.0), (0.0, 1.0), 1000.0, 500.0, 50.0) - 10.0).abs() < 1e-9);
        assert!((metres((0.0, 0.0), (0.3, 0.4), 1000.0, 1000.0, 100.0) - 5.0).abs() < 1e-9);
        assert_eq!(metres((0.0, 0.0), (1.0, 1.0), 1000.0, 500.0, 0.0), 0.0);
    }
}
