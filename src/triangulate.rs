use std::cmp;

use providence_io::data::{self, Mesh, Vertex};
use zaru::{image::Image, landmark::Landmark, num::TotalF32, rect::Rect};

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
}

impl Triangulator {
    pub fn new() -> Self {
        Self {
            mesh: Mesh {
                // The indices are always fixed, only the vertices change.
                indices: TRIS.iter().flat_map(|&tri| tri.map(u16::from)).collect(),
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
    pub fn triangulate_eye(
        &mut self,
        eye: &[Landmark; 16],
        img: &Image,
    ) -> anyhow::Result<TriangulatedEye> {
        let mut points = [[0.0, 0.0]; 16];
        for (out, lm) in points.iter_mut().zip(eye) {
            out[0] = lm.x();
            out[1] = lm.y();
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

        let img = img.view(Rect::bounding([min, max]).unwrap()).to_image();

        // Vertex positions are mapped so that all vertices fit into a rect from -0.5 to 0.5
        let range = [max[0] - min[0], max[1] - min[1]]
            .into_iter()
            .max_by_key(|f| TotalF32(*f))
            .unwrap();
        for pt in &mut points {
            pt[0] = pt[0] - min[0];
            pt[1] = pt[1] - min[1];
        }
        let max = [max[0] - min[0], max[1] - min[1]];

        self.mesh.vertices.clear();
        self.mesh
            .vertices
            .extend(points.iter().map(|&[x, y]| Vertex {
                position: [(x - max[0] * 0.5) / range, (y - max[1] * 0.5) / range],
                uv: [x / max[0], y / max[1]],
            }));

        Ok(TriangulatedEye {
            texture: img,
            mesh: self.mesh.clone(),
        })
    }
}

pub struct TriangulatedEye {
    mesh: Mesh,
    texture: Image,
}

impl TriangulatedEye {
    /// Flips vertex positions and UV coordinates so that the resulting mesh will appear flipped horizontally.
    pub fn flip_horizontal(mut self) -> Self {
        self.mesh.vertices = self
            .mesh
            .vertices
            .into_iter()
            .map(|vert| Vertex {
                position: [-vert.position[0], vert.position[1]],
                uv: vert.uv,
            })
            .collect();
        self
    }

    pub fn into_message(self) -> data::Eye {
        data::Eye {
            texture: data::Image {
                width: self.texture.width(),
                height: self.texture.height(),
                data: self.texture.data().to_vec(),
            },
            mesh: self.mesh,
        }
    }
}
