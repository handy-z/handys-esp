use handy_sdk::roblox::screen::{ScreenView, Vec2, Vec3, distance, world_to_screen};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Part {
    pub pos: Vec3,
    pub size: Vec3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// One drawn object, in Roblox client-area pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marker {
    pub r#box: Rect,
    pub name: String,
    /// From your character; None without one.
    pub studs: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    /// Render size the boxes are in.
    pub size: Vec2,
    pub markers: Vec<Marker>,
}

// std::min / std::max, NaN handling included.
fn min(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}
fn max(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}

/// Box around the parts' corners on screen. None with no parts, any corner behind the camera, or farther than
/// max_studs (0 = no limit) from `from`. shortcut: corners of each part's axis-aligned box, so rotated parts get a
/// looser box; read Primitive.Rotation when that matters.
pub fn marker_for(v: &ScreenView, parts: &[Part], name: String, from: Option<Vec3>, max_studs: i32) -> Option<Marker> {
    if parts.is_empty() {
        return None;
    }
    let mut mid = Vec3::default();
    for p in parts {
        mid = Vec3 {
            x: mid.x + p.pos.x,
            y: mid.y + p.pos.y,
            z: mid.z + p.pos.z,
        };
    }
    let n = parts.len() as f32;
    mid = Vec3 {
        x: mid.x / n,
        y: mid.y / n,
        z: mid.z / n,
    };
    let mut m = Marker {
        r#box: Rect {
            left: 1e9,
            top: 1e9,
            right: -1e9,
            bottom: -1e9,
        },
        name,
        studs: None,
    };
    if let Some(from) = from {
        let d = distance(from, mid);
        m.studs = Some(d);
        if max_studs > 0 && d > max_studs as f32 {
            return None;
        }
    }
    let half = |bit: bool| if bit { 0.5f32 } else { -0.5 };
    for p in parts {
        for c in 0..8 {
            let corner = Vec3 {
                x: p.pos.x + half(c & 1 != 0) * p.size.x,
                y: p.pos.y + half(c & 2 != 0) * p.size.y,
                z: p.pos.z + half(c & 4 != 0) * p.size.z,
            };
            // a box half behind the camera would stretch across the screen
            let s = world_to_screen(v, corner)?;
            m.r#box = Rect {
                left: min(m.r#box.left, s.x),
                top: min(m.r#box.top, s.y),
                right: max(m.r#box.right, s.x),
                bottom: max(m.r#box.bottom, s.y),
            };
        }
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use handy_sdk::roblox::screen::vec3;

    // Looks down -z, x right, y up, 800x600: a point at depth d and offset (x, y) lands at ndc (x/d, y/d).
    fn forward() -> ScreenView {
        ScreenView {
            m: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., -1., 0.],
            size: Vec2 { x: 800., y: 600. },
        }
    }

    #[test]
    fn marker_box_spans_the_cube() {
        let cube = Part {
            pos: vec3(0., 0., -10.),
            size: vec3(2., 2., 2.),
        }; // near face at depth 9, far face at 11
        let m = marker_for(&forward(), std::slice::from_ref(&cube), "Chest".into(), None, 0);
        assert!(m.as_ref().is_some_and(|m| m.name == "Chest" && m.studs.is_none()));
        let b = m.unwrap().r#box;
        // the near face is the widest: x = +-1/9 ndc
        assert!(b.left < 400. && b.right > 400. && b.top < 300. && b.bottom > 300.);
        assert!(b.right - 400. > 400. / 9. - 0.01 && b.right - 400. < 400. / 9. + 0.01);
    }

    #[test]
    fn marker_behind_camera_or_empty_is_nullopt() {
        let behind = Part {
            pos: vec3(0., 0., 10.),
            size: vec3(2., 2., 2.),
        };
        let straddling = Part {
            pos: vec3(0., 0., -0.5),
            size: vec3(4., 4., 4.),
        };
        assert!(marker_for(&forward(), std::slice::from_ref(&behind), "x".into(), None, 0).is_none());
        assert!(marker_for(&forward(), std::slice::from_ref(&straddling), "x".into(), None, 0).is_none());
        assert!(marker_for(&forward(), &[], "x".into(), None, 0).is_none());
    }

    #[test]
    fn marker_distance_and_limit() {
        let p = Part {
            pos: vec3(0., 0., -10.),
            size: vec3(1., 1., 1.),
        };
        let one = std::slice::from_ref(&p);
        let m = marker_for(&forward(), one, "x".into(), Some(vec3(0., 0., 0.)), 0);
        assert_eq!(m.and_then(|m| m.studs), Some(10.));
        assert!(marker_for(&forward(), one, "x".into(), Some(vec3(0., 0., 0.)), 10).is_some());
        assert!(marker_for(&forward(), one, "x".into(), Some(vec3(0., 0., 0.)), 9).is_none());
    }
}
