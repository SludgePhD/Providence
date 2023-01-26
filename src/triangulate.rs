use std::cmp;

use providence_io::data::{self, Eye, Mesh, Vertex};
use zaru::{
    face::eye::EyeLandmarks,
    image::{Image, Rect},
    num::TotalF32,
};

// winding order: counter-clockwise
static TRIS: &[[u8; 3]] = &[
    [0, 1, 9],
    [1, 2, 9],
    [9, 2, 10],
    [2, 3, 10],
    [10, 3, 11],
    [3, 4, 11],
    [11, 4, 12],
    [4, 5, 12],
    [12, 5, 13],
    [5, 6, 13],
    [13, 6, 14],
    [6, 7, 14],
    [14, 7, 15],
    [7, 8, 15],
];

pub struct Triangulator {
    mesh: Mesh,
    flipped_mesh: Mesh,
}

impl Triangulator {
    pub fn new() -> Self {
        Self {
            mesh: Mesh {
                // The indices are always fixed, only the vertices change.
                indices: TRIS.iter().flat_map(|&tri| tri.map(u16::from)).collect(),
                vertices: Vec::new(),
            },
            flipped_mesh: Mesh {
                indices: TRIS
                    .iter()
                    .flat_map(|&[i, j, k]| [i, k, j].map(u16::from))
                    .collect(),
                vertices: Vec::new(),
            },
        }
    }

    /// Triangulates an eye.
    ///
    /// # Parameters
    ///
    /// - `eye`: the landmarks of the eye, positioned on `img`.
    /// - `img`: the image the landmarks were computed on.
    /// - `flip`: whether to flip all emitted triangles from being wound counter-clockwise to being
    ///   wound clockwise.
    pub fn triangulate_eye(
        &mut self,
        eye: &EyeLandmarks,
        img: &Image,
        flip: bool,
    ) -> zaru::Result<Eye> {
        let mut points = [[0.0, 0.0]; 16];
        for (out, [x, y, _]) in points.iter_mut().zip(eye.eye_contour()) {
            out[0] = x;
            out[1] = y;
        }

        // Compute AABB to crop image to
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for pt in &points {
            min[0] = cmp::min(TotalF32(min[0]), TotalF32(pt[0])).0;
            min[1] = cmp::min(TotalF32(min[1]), TotalF32(pt[1])).0;
            max[0] = cmp::max(TotalF32(max[0]), TotalF32(pt[0])).0;
            max[1] = cmp::max(TotalF32(max[1]), TotalF32(pt[1])).0;
        }
        min[0] = min[0].floor();
        min[1] = min[1].floor();
        max[0] = max[0].ceil();
        max[1] = max[1].ceil();

        let img = img
            .view(Rect::from_corners(
                (min[0] as _, min[1] as _),
                (max[0] as _, max[1] as _),
            ))
            .to_image();

        // We scale all points so that the largest axis spans from 0.0 to 1.0
        let range = [max[0] - min[0], max[1] - min[1]]
            .into_iter()
            .max_by_key(|f| TotalF32(*f))
            .unwrap();
        for pt in &mut points {
            pt[0] = pt[0] - min[0];
            pt[1] = pt[1] - min[1];
        }
        let max = [max[0] - min[0], max[1] - min[1]];

        let mesh = if flip {
            &mut self.flipped_mesh
        } else {
            &mut self.mesh
        };
        mesh.vertices.clear();
        mesh.vertices.extend(points.iter().map(|&[x, y]| Vertex {
            position: [(x - max[0] * 0.5) / range, (y - max[1] * 0.5) / range],
            uv: [x / max[0], y / max[1]],
        }));

        Ok(Eye {
            texture: data::Image {
                width: img.width(),
                height: img.height(),
                data: img.data().to_vec(),
            },
            mesh: mesh.clone(),
        })
    }
}
