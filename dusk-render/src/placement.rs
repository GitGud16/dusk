//! Where a picture goes in the frame and where each output pixel reads it from
//! (docs/ARCHITECTURE.md, "Fit and rotation"): the stored picture is turned upright as its
//! file says, cropped, turned and mirrored as its clip says, then fitted with bars or made to
//! fill the frame.

use dusk_core::{Fit, Orientation, Rect};

/// How a picture is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// How the stored picture is turned and mirrored to show it upright.
    pub orientation: Orientation,
    /// The part of the upright picture shown, in its pixels; `None` for all of it.
    pub crop: Option<Rect>,
    /// What the clip's rotation and flips do after the crop.
    pub edits: Orientation,
    /// Bars where the shapes differ, or cover the frame and crop the rest.
    pub fit: Fit,
}

impl Default for Placement {
    fn default() -> Placement {
        Placement {
            orientation: Orientation::UPRIGHT,
            crop: None,
            edits: Orientation::UPRIGHT,
            fit: Fit::Fit,
        }
    }
}

/// Where the pixels of a frame's rectangle read the stored picture from. Output pixel `u`,
/// `v` of the rectangle (its center at `u + 0.5`) reads source position
/// `origin + (u + 0.5) × step` along each axis; with `transposed`, output x follows the
/// source's y axis and output y its x axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mapping {
    /// The rectangle of the frame the picture covers.
    pub rect: FrameRect,
    pub transposed: bool,
    /// Along output x: source position of the rectangle's left edge, and per output pixel.
    pub x_origin: f32,
    pub x_step: f32,
    /// Along output y.
    pub y_origin: f32,
    pub y_step: f32,
    /// The part of the source shown, in source pixels: `x` from `window.0` to `window.1`,
    /// `y` from `window.2` to `window.3`, ends excluded.
    pub window: (u32, u32, u32, u32),
}

/// A rectangle of the frame, in frame pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The mapping for a stored picture of `size` shown with `placement` in a frame of `frame`.
pub(crate) fn mapping(size: (u32, u32), placement: &Placement, frame: (u32, u32)) -> Mapping {
    let upright = placement.orientation.apply_to_size(size);
    let crop = placement
        .crop
        .map_or(whole(upright), |crop| clamp_to(crop, upright));
    let shown = placement.edits.apply_to_size((crop.width, crop.height));
    // The part of the shown picture in view, as fractions of it: all of it, or when filling,
    // the middle part with the frame's shape.
    let (rect, view) = match placement.fit {
        Fit::Fit => (fit(shown, frame), (0.0, 0.0, 1.0, 1.0)),
        Fit::Fill => (
            FrameRect {
                x: 0,
                y: 0,
                width: frame.0,
                height: frame.1,
            },
            fill_view(shown, frame),
        ),
    };
    let (width, height) = (f64::from(size.0), f64::from(size.1));
    let upright = (f64::from(upright.0), f64::from(upright.1));
    let to_stored = |point: (f64, f64)| {
        let stored = placement.orientation.inverse().map(point);
        (stored.0 * width, stored.1 * height)
    };
    // Where a point of the rectangle, in its pixels, reads the stored picture, in its pixels.
    let to_source = |(u, v): (f64, f64)| {
        let in_view = (
            view.0 + u / f64::from(rect.width) * view.2,
            view.1 + v / f64::from(rect.height) * view.3,
        );
        let in_crop = placement.edits.inverse().map(in_view);
        to_stored((
            (f64::from(crop.x) + in_crop.0 * f64::from(crop.width)) / upright.0,
            (f64::from(crop.y) + in_crop.1 * f64::from(crop.height)) / upright.1,
        ))
    };
    // Every step is a turn, a mirror or a scale, so three points give the whole mapping.
    let origin = to_source((0.0, 0.0));
    let across = to_source((1.0, 0.0));
    let down = to_source((0.0, 1.0));
    let across = (across.0 - origin.0, across.1 - origin.1);
    let down = (down.0 - origin.0, down.1 - origin.1);
    let transposed = across.0.abs() < across.1.abs();
    let (x_origin, x_step, y_origin, y_step) = if transposed {
        (origin.1, across.1, origin.0, down.0)
    } else {
        (origin.0, across.0, origin.1, down.1)
    };
    let corners = [
        (crop.x, crop.y),
        (crop.x + crop.width, crop.y + crop.height),
    ]
    .map(|(x, y)| {
        let (x, y) = to_stored((f64::from(x) / upright.0, f64::from(y) / upright.1));
        // Turns and mirrors take whole pixels to whole pixels.
        (x.round() as u32, y.round() as u32)
    });
    let (xs, ys) = ([corners[0].0, corners[1].0], [corners[0].1, corners[1].1]);
    Mapping {
        rect,
        transposed,
        x_origin: x_origin as f32,
        x_step: x_step as f32,
        y_origin: y_origin as f32,
        y_step: y_step as f32,
        window: (
            xs[0].min(xs[1]),
            xs[0].max(xs[1]),
            ys[0].min(ys[1]),
            ys[0].max(ys[1]),
        ),
    }
}

/// All of a picture of `size`.
fn whole(size: (u32, u32)) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    }
}

/// `crop`, kept inside a picture of `size` and at least a pixel each way.
fn clamp_to(crop: Rect, size: (u32, u32)) -> Rect {
    let x = crop.x.min(size.0.saturating_sub(1));
    let y = crop.y.min(size.1.saturating_sub(1));
    Rect {
        x,
        y,
        width: crop.width.clamp(1, size.0 - x),
        height: crop.height.clamp(1, size.1 - y),
    }
}

/// The middle part of a picture of size `shown` that has the shape of `frame`, as fractions
/// of the picture: left, top, width and height.
fn fill_view(shown: (u32, u32), frame: (u32, u32)) -> (f64, f64, f64, f64) {
    let (shown_width, shown_height) = (f64::from(shown.0), f64::from(shown.1));
    let (frame_width, frame_height) = (f64::from(frame.0), f64::from(frame.1));
    if shown_width * frame_height > frame_width * shown_height {
        // Wider than the frame: the sides go.
        let part = frame_width * shown_height / (frame_height * shown_width);
        ((1.0 - part) / 2.0, 0.0, part, 1.0)
    } else {
        let part = frame_height * shown_width / (frame_width * shown_height);
        (0.0, (1.0 - part) / 2.0, 1.0, part)
    }
}

/// The size of a picture of size `picture` fitted into a frame of size `frame`: aspect ratio
/// kept, at least one pixel each way. The preview draws the sequence at this size.
pub fn fit_size(picture: (u32, u32), frame: (u32, u32)) -> (u32, u32) {
    let rect = fit(picture, frame);
    (rect.width, rect.height)
}

/// Where a picture of size `picture` lands in a frame of size `frame`: aspect ratio kept,
/// centered, at least one pixel each way.
pub(crate) fn fit(picture: (u32, u32), frame: (u32, u32)) -> FrameRect {
    let (picture_width, picture_height) = (u64::from(picture.0), u64::from(picture.1));
    let (frame_width, frame_height) = (u64::from(frame.0), u64::from(frame.1));
    let (width, height) = if picture_width * frame_height >= frame_width * picture_height {
        let height = (picture_height * frame_width + picture_width / 2) / picture_width;
        (frame_width, height.clamp(1, frame_height))
    } else {
        let width = (picture_width * frame_height + picture_height / 2) / picture_height;
        (width.clamp(1, frame_width), frame_height)
    };
    // Every value is at most the frame's size, which is a u32.
    FrameRect {
        x: ((frame_width - width) / 2) as u32,
        y: ((frame_height - height) / 2) as u32,
        width: width as u32,
        height: height as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u32, y: u32, width: u32, height: u32) -> FrameRect {
        FrameRect {
            x,
            y,
            width,
            height,
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_picture_with_the_frames_shape_fills_it() {
        assert_eq!(fit((1920, 1080), (1280, 720)), rect(0, 0, 1280, 720));
    }

    #[test]
    fn a_wider_picture_gets_bars_above_and_below() {
        // 2.39:1 in 16:9.
        assert_eq!(fit((1920, 804), (1920, 1080)), rect(0, 138, 1920, 804));
    }

    #[test]
    fn a_taller_picture_gets_bars_left_and_right() {
        assert_eq!(fit((1080, 1920), (1920, 1080)), rect(656, 0, 608, 1080));
    }

    #[test]
    fn a_tiny_picture_still_covers_a_pixel() {
        assert_eq!(fit((10_000, 1), (100, 100)).height, 1);
    }

    #[test]
    fn a_plain_picture_maps_straight_through() {
        let map = mapping((1920, 1080), &Placement::default(), (1280, 720));
        assert_eq!(map.rect, rect(0, 0, 1280, 720));
        assert!(!map.transposed);
        assert!(near(map.x_origin, 0.0) && near(map.x_step, 1.5));
        assert!(near(map.y_origin, 0.0) && near(map.y_step, 1.5));
        assert_eq!(map.window, (0, 1920, 0, 1080));
    }

    #[test]
    fn a_phone_clip_turned_upright_reads_its_columns() {
        // Stored landscape, shown portrait with bars: a quarter turn clockwise.
        let placement = Placement {
            orientation: Orientation::new(1, false),
            ..Placement::default()
        };
        let map = mapping((1920, 1080), &placement, (1920, 1080));
        assert_eq!(map.rect, rect(656, 0, 608, 1080));
        assert!(map.transposed);
        // Turned clockwise, the stored bottom edge is on the left: output x runs up the
        // source's rows, output y along its columns.
        assert!(near(map.x_origin, 1080.0) && near(map.x_step, -1080.0 / 608.0));
        assert!(near(map.y_origin, 0.0) && near(map.y_step, 1920.0 / 1080.0));
        assert_eq!(map.window, (0, 1920, 0, 1080));
    }

    #[test]
    fn a_mirrored_clip_reads_right_to_left() {
        let placement = Placement {
            edits: Orientation::new(0, true),
            ..Placement::default()
        };
        let map = mapping((640, 360), &placement, (640, 360));
        assert!(!map.transposed);
        assert!(near(map.x_origin, 640.0) && near(map.x_step, -1.0));
        assert!(near(map.y_origin, 0.0) && near(map.y_step, 1.0));
    }

    #[test]
    fn a_crop_reads_only_its_part() {
        let placement = Placement {
            crop: Some(Rect {
                x: 100,
                y: 50,
                width: 640,
                height: 360,
            }),
            ..Placement::default()
        };
        let map = mapping((1920, 1080), &placement, (1280, 720));
        assert_eq!(map.rect, rect(0, 0, 1280, 720));
        assert!(near(map.x_origin, 100.0) && near(map.x_step, 0.5));
        assert!(near(map.y_origin, 50.0) && near(map.y_step, 0.5));
        assert_eq!(map.window, (100, 740, 50, 410));
    }

    #[test]
    fn a_crop_is_in_upright_pixels() {
        // The crop's top-left corner of the upright portrait picture is, stored, at the
        // bottom-left: x 0, y 1080 - 300.
        let placement = Placement {
            orientation: Orientation::new(1, false),
            crop: Some(Rect {
                x: 0,
                y: 0,
                width: 300,
                height: 200,
            }),
            ..Placement::default()
        };
        let map = mapping((1920, 1080), &placement, (300, 200));
        assert!(map.transposed);
        assert_eq!(map.window, (0, 200, 780, 1080));
    }

    #[test]
    fn filling_crops_the_overflow_evenly() {
        // A 4:3 picture filling a 16:9 frame loses an eighth at the top and the bottom.
        let placement = Placement {
            fit: Fit::Fill,
            ..Placement::default()
        };
        let map = mapping((1440, 1080), &placement, (1920, 1080));
        assert_eq!(map.rect, rect(0, 0, 1920, 1080));
        assert!(near(map.x_origin, 0.0) && near(map.x_step, 0.75));
        assert!(near(map.y_origin, 135.0) && near(map.y_step, 0.75));
        // A wider picture loses its sides.
        let map = mapping((1920, 800), &placement, (1080, 1080));
        assert!(near(map.x_origin, 560.0) && near(map.x_step, 800.0 / 1080.0));
    }
}
