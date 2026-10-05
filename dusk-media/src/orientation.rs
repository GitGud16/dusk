//! How a file says its picture is turned: a video's display matrix (docs/ARCHITECTURE.md,
//! "Fit and rotation").

use dusk_core::Orientation;

/// What it takes to show upright a video whose display matrix is `matrix` (nine 16.16 and
/// 2.30 fixed-point numbers, as FFmpeg stores it). The choices are FFmpeg's own autorotation
/// (fftools, `get_rotation` and the transpose and flip filters it inserts), so Dusk shows a
/// file as `ffmpeg` and players built on it do. A turn that is not a multiple of 90 degrees is
/// left upright.
pub(crate) fn from_display_matrix(matrix: &[i32; 9]) -> Orientation {
    let fixed = |value: i32| f64::from(value) / 65536.0;
    let scale = (
        fixed(matrix[0]).hypot(fixed(matrix[3])),
        fixed(matrix[1]).hypot(fixed(matrix[4])),
    );
    if scale.0 == 0.0 || scale.1 == 0.0 {
        return Orientation::UPRIGHT;
    }
    // av_display_rotation_get gives the counterclockwise angle; get_rotation turns it into
    // the clockwise one, rounded to whole degrees and brought into 0..360.
    let counterclockwise = -(fixed(matrix[1]) / scale.1)
        .atan2(fixed(matrix[0]) / scale.0)
        .to_degrees();
    let mut theta = -counterclockwise.round();
    theta -= 360.0 * (theta / 360.0 + 0.9 / 360.0).floor();
    let near = |angle: f64| (theta - angle).abs() < 1.0;
    let mirror = Orientation::new(0, true);
    let upside_down_mirror = Orientation::new(2, true);
    if near(90.0) {
        // "cclock_flip" transposes; "clock" turns a quarter clockwise.
        if matrix[3] > 0 {
            Orientation::new(3, true)
        } else {
            Orientation::new(1, false)
        }
    } else if near(180.0) {
        let mut orientation = Orientation::UPRIGHT;
        if matrix[0] < 0 {
            orientation = orientation.then(mirror);
        }
        if matrix[4] < 0 {
            orientation = orientation.then(upside_down_mirror);
        }
        orientation
    } else if near(270.0) {
        // "clock_flip" turns a quarter clockwise and flips top to bottom; "cclock" turns a
        // quarter counterclockwise.
        if matrix[3] < 0 {
            Orientation::new(1, true)
        } else {
            Orientation::new(3, false)
        }
    } else if (near(0.0) || near(360.0)) && matrix[4] < 0 {
        upside_down_mirror
    } else {
        Orientation::UPRIGHT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FFmpeg's `av_display_rotation_set`: a matrix turning the picture clockwise by
    /// `degrees` to display it.
    fn turned(degrees: f64) -> [i32; 9] {
        let radians = (-degrees).to_radians();
        let fixed = |value: f64| (value * 65536.0).round() as i32;
        let (c, s) = (radians.cos(), radians.sin());
        [fixed(c), fixed(-s), 0, fixed(s), fixed(c), 0, 0, 0, 1 << 30]
    }

    /// FFmpeg's `av_display_matrix_flip`.
    fn flipped(mut matrix: [i32; 9], horizontal: bool, vertical: bool) -> [i32; 9] {
        for (index, value) in matrix.iter_mut().enumerate() {
            let sign = match index % 3 {
                0 if horizontal => -1,
                1 if vertical => -1,
                _ => 1,
            };
            *value *= sign;
        }
        matrix
    }

    #[test]
    fn a_plain_matrix_leaves_the_picture_upright() {
        assert_eq!(from_display_matrix(&turned(0.0)), Orientation::UPRIGHT);
    }

    #[test]
    fn turns_follow_the_matrix() {
        // A portrait phone video: stored landscape, shown a quarter turn clockwise.
        assert_eq!(
            from_display_matrix(&turned(90.0)),
            Orientation::new(1, false)
        );
        assert_eq!(
            from_display_matrix(&turned(180.0)),
            Orientation::new(2, false)
        );
        assert_eq!(
            from_display_matrix(&turned(270.0)),
            Orientation::new(3, false)
        );
        assert_eq!(
            from_display_matrix(&turned(-90.0)),
            Orientation::new(3, false)
        );
    }

    #[test]
    fn mirrors_follow_the_matrix() {
        let upright = turned(0.0);
        assert_eq!(
            from_display_matrix(&flipped(upright, true, false)),
            Orientation::new(0, true)
        );
        assert_eq!(
            from_display_matrix(&flipped(upright, false, true)),
            Orientation::new(2, true)
        );
        // FFmpeg transposes such a picture: rows become columns.
        let transposed = from_display_matrix(&flipped(turned(90.0), true, false));
        assert_eq!(transposed.map((0.25, 0.75)), (0.75, 0.25));
    }

    #[test]
    fn an_odd_angle_or_an_empty_matrix_is_left_upright() {
        assert_eq!(from_display_matrix(&turned(45.0)), Orientation::UPRIGHT);
        assert_eq!(from_display_matrix(&[0; 9]), Orientation::UPRIGHT);
    }
}
