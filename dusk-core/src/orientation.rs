//! The eight ways a picture can be turned and mirrored (docs/ARCHITECTURE.md, "Fit and
//! rotation"): how a file stores its picture relative to upright, and what a clip's rotation
//! and flips do to it.

use crate::model::Rotation;

/// One of the eight ways a rectangle maps onto itself: mirrored left to right when
/// `mirrored`, then turned clockwise by a number of quarter turns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Orientation {
    turns: u8,
    mirrored: bool,
}

impl Orientation {
    /// As stored: neither turned nor mirrored.
    pub const UPRIGHT: Orientation = Orientation {
        turns: 0,
        mirrored: false,
    };

    /// Mirrored left to right when `mirrored`, then turned clockwise `turns` quarter turns.
    pub fn new(turns: u8, mirrored: bool) -> Orientation {
        Orientation {
            turns: turns % 4,
            mirrored,
        }
    }

    /// Clockwise quarter turns, 0 to 3, after the mirroring.
    pub fn turns(self) -> u8 {
        self.turns
    }

    /// Whether the picture is mirrored left to right before turning.
    pub fn mirrored(self) -> bool {
        self.mirrored
    }

    /// What it takes to show a picture upright whose EXIF orientation tag is `tag` (1 to 8);
    /// any other value is taken as upright.
    pub fn from_exif(tag: u16) -> Orientation {
        let (turns, mirrored) = match tag {
            2 => (0, true),
            3 => (2, false),
            4 => (2, true),
            5 => (3, true),
            6 => (1, false),
            7 => (1, true),
            8 => (3, false),
            _ => (0, false),
        };
        Orientation::new(turns, mirrored)
    }

    /// What a clip's edits do: turn it by `rotate`, then mirror it left to right with
    /// `flip_h` and top to bottom with `flip_v`.
    pub fn from_edits(rotate: Rotation, flip_h: bool, flip_v: bool) -> Orientation {
        let turns = match rotate {
            Rotation::None => 0,
            Rotation::Quarter => 1,
            Rotation::Half => 2,
            Rotation::ThreeQuarters => 3,
        };
        let mut edits = Orientation::new(turns, false);
        if flip_h {
            edits = edits.then(Orientation::new(0, true));
        }
        if flip_v {
            // Top to bottom is left to right turned half way round.
            edits = edits.then(Orientation::new(2, true));
        }
        edits
    }

    /// This orientation, followed by `next`.
    pub fn then(self, next: Orientation) -> Orientation {
        // A mirror reverses the direction of the turns before it: M R = R⁻¹ M.
        let carried = if next.mirrored {
            4 - self.turns
        } else {
            self.turns
        };
        Orientation::new(next.turns + carried, self.mirrored != next.mirrored)
    }

    /// The orientation that undoes this one.
    pub fn inverse(self) -> Orientation {
        if self.mirrored {
            // (R^t M)⁻¹ = M R⁻ᵗ = R^t M: a mirrored orientation undoes itself.
            self
        } else {
            Orientation::new(4 - self.turns, false)
        }
    }

    /// Whether width and height trade places.
    pub fn swaps_sides(self) -> bool {
        self.turns % 2 == 1
    }

    /// The size, (width, height), of a `size` picture oriented this way.
    pub fn apply_to_size(self, size: (u32, u32)) -> (u32, u32) {
        if self.swaps_sides() {
            (size.1, size.0)
        } else {
            size
        }
    }

    /// Where `point`, as fractions of a picture's width and height from its top-left corner,
    /// lands in the picture oriented this way.
    pub fn map(self, point: (f64, f64)) -> (f64, f64) {
        let (mut x, mut y) = point;
        if self.mirrored {
            x = 1.0 - x;
        }
        for _ in 0..self.turns {
            // A clockwise quarter turn: the top edge becomes the right edge.
            (x, y) = (1.0 - y, x);
        }
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Orientation> {
        (0..8).map(|n| Orientation::new(n % 4, n >= 4)).collect()
    }

    const CORNERS: [(f64, f64); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];

    #[test]
    fn a_quarter_turn_moves_the_top_left_corner_to_the_top_right() {
        let quarter = Orientation::new(1, false);
        assert_eq!(quarter.map((0.0, 0.0)), (1.0, 0.0));
        assert_eq!(quarter.map((1.0, 0.0)), (1.0, 1.0));
        assert_eq!(quarter.map((0.25, 0.0)), (1.0, 0.25));
        assert_eq!(quarter.apply_to_size((4032, 3024)), (3024, 4032));
        assert!(quarter.swaps_sides());
    }

    #[test]
    fn mirroring_swaps_left_and_right() {
        let mirror = Orientation::new(0, true);
        assert_eq!(mirror.map((0.0, 0.0)), (1.0, 0.0));
        assert_eq!(mirror.map((0.25, 1.0)), (0.75, 1.0));
        assert_eq!(mirror.apply_to_size((1920, 1080)), (1920, 1080));
        assert!(!mirror.swaps_sides());
    }

    #[test]
    fn turns_wrap_at_four() {
        assert_eq!(Orientation::new(5, false), Orientation::new(1, false));
        assert_eq!(Orientation::new(4, true).turns(), 0);
    }

    #[test]
    fn exif_tags_name_the_eight_orientations() {
        let tags: Vec<(u8, bool)> = (1..=8)
            .map(|tag| {
                let orientation = Orientation::from_exif(tag);
                (orientation.turns(), orientation.mirrored())
            })
            .collect();
        assert_eq!(
            tags,
            [
                (0, false),
                (0, true),
                (2, false),
                (2, true),
                (3, true),
                (1, false),
                (1, true),
                (3, false)
            ]
        );
        assert_eq!(Orientation::from_exif(0), Orientation::UPRIGHT);
        assert_eq!(Orientation::from_exif(9), Orientation::UPRIGHT);
        // Tag 4 is a top-to-bottom mirror: the top-left corner goes to the bottom-left.
        assert_eq!(Orientation::from_exif(4).map((0.0, 0.0)), (0.0, 1.0));
    }

    #[test]
    fn following_one_orientation_with_another_maps_points_through_both() {
        for first in all() {
            for second in all() {
                let both = first.then(second);
                for point in CORNERS.iter().copied().chain([(0.25, 0.5)]) {
                    let through = second.map(first.map(point));
                    let direct = both.map(point);
                    assert!(
                        (through.0 - direct.0).abs() < 1e-12
                            && (through.1 - direct.1).abs() < 1e-12,
                        "{first:?} then {second:?} at {point:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_orientation_has_an_inverse() {
        for orientation in all() {
            assert_eq!(
                orientation.then(orientation.inverse()),
                Orientation::UPRIGHT
            );
            assert_eq!(
                orientation.inverse().then(orientation),
                Orientation::UPRIGHT
            );
        }
    }

    #[test]
    fn edits_turn_first_then_flip() {
        assert_eq!(
            Orientation::from_edits(Rotation::None, false, false),
            Orientation::UPRIGHT
        );
        // A top-to-bottom flip is a mirror and a half turn.
        assert_eq!(
            Orientation::from_edits(Rotation::None, false, true),
            Orientation::new(2, true)
        );
        // Turned a quarter, then mirrored left to right: the top-left corner, at the top
        // right after the turn, is mirrored back to the top left.
        let edits = Orientation::from_edits(Rotation::Quarter, true, false);
        assert_eq!(edits.map((0.0, 0.0)), (0.0, 0.0));
        assert_eq!(edits.map((1.0, 0.0)), (0.0, 1.0));
        assert_eq!(
            Orientation::from_edits(Rotation::Half, true, true),
            Orientation::UPRIGHT
        );
    }
}
