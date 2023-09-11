use std::cmp;

use providence_io::data::{self, Mesh, Vertex};
use zaru::{face::landmark::mediapipe::LandmarkResultV2, image::Image, num::TotalF32, rect::Rect};

// winding order: clockwise (flipped later)
// 9 vertices along the top, the remaining 7 along the bottom
static TRIS: &[[u8; 3]] = &[
    [0, 1, 15],
    [1, 2, 15],
    [15, 2, 14],
    [2, 3, 14],
    [14, 3, 13],
    [3, 4, 13],
    [13, 4, 12],
    [4, 5, 12],
    [12, 5, 11],
    [5, 6, 11],
    [11, 6, 10],
    [6, 7, 10],
    [10, 7, 9],
    [7, 8, 9],
];

#[derive(Clone, Copy)]
pub enum Eye {
    Left,
    Right,
}

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
    /// - `face_landmarks`: the landmarks of the whole face, positioned on `img`.
    /// - `img`: the image the face landmarks were computed on.
    /// - `eye`: which [`Eye`] to extract from the landmarks.
    pub fn triangulate_eye(
        &mut self,
        face_landmarks: &LandmarkResultV2,
        img: &Image,
        eye: Eye,
    ) -> TriangulatedEye {
        let (eye_landmarks, iris_landmarks) = match eye {
            Eye::Left => (
                face_landmarks.left_eye_contour(),
                face_landmarks.left_iris(),
            ),
            Eye::Right => (
                face_landmarks.right_eye_contour(),
                face_landmarks.right_iris(),
            ),
        };

        let mut points = [[0.0; 3]; 16];
        for (out, lm) in points.iter_mut().zip(eye_landmarks) {
            out[0] = lm.x();
            out[1] = lm.y();
            out[2] = lm.z();
        }

        // Compute AABB to crop image to
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for pt in &points {
            min[0] = cmp::min(TotalF32(min[0]), TotalF32(pt[0])).0;
            min[1] = cmp::min(TotalF32(min[1]), TotalF32(pt[1])).0;
            min[2] = cmp::min(TotalF32(min[2]), TotalF32(pt[2])).0;
            max[0] = cmp::max(TotalF32(max[0]), TotalF32(pt[0])).0;
            max[1] = cmp::max(TotalF32(max[1]), TotalF32(pt[1])).0;
            max[2] = cmp::max(TotalF32(max[2]), TotalF32(pt[2])).0;
        }
        min = min.map(f32::floor);
        max = max.map(f32::ceil);

        let img = img
            .view(Rect::bounding([[min[0], min[1]], [max[0], max[1]]]).unwrap())
            .to_image();

        // Vertex positions are mapped so that all vertices fit into a rect from -0.5 to 0.5
        let ranges = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        let max_range = ranges.into_iter().max_by_key(|f| TotalF32(*f)).unwrap();

        // FIXME: no Z coordinate in the tracking data. UVs are not yet adjusted to make
        // the resulting mesh look correct.
        self.mesh.vertices.clear();
        self.mesh
            .vertices
            .extend(points.into_iter().map(|[x, y, _z]| Vertex {
                position: [
                    (x - min[0] - ranges[0] * 0.5) / max_range,
                    (y - min[1] - ranges[1] * 0.5) / max_range,
                    //(z - min[2] - ranges[2] * 0.5) / range,
                ],
                uv: [(x - min[0]) / ranges[0], (y - min[1]) / ranges[1]],
            }));

        let [iris_center, rest @ ..] = iris_landmarks.map(|lm| {
            [
                (lm.x() - min[0] - ranges[0] * 0.5) / max_range,
                (lm.y() - min[1] - ranges[1] * 0.5) / max_range,
            ]
        });

        let radii = rest.map(|[x, y]| {
            let dx = iris_center[0] - x;
            let dy = iris_center[1] - y;
            (dx * dx + dy * dy).sqrt()
        });
        let iris_radius = radii.into_iter().sum::<f32>() / 4.0;

        TriangulatedEye {
            texture: img,
            mesh: self.mesh.clone(),
            iris_center,
            iris_radius,
        }
    }
}

pub struct TriangulatedEye {
    mesh: Mesh,
    texture: Image,
    iris_center: [f32; 2],
    iris_radius: f32,
}

impl TriangulatedEye {
    /// Flips vertex positions so that the resulting mesh will appear flipped horizontally.
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
        self.iris_center = [-self.iris_center[0], self.iris_center[1]];
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
            iris_center: self.iris_center,
            iris_radius: self.iris_radius,
        }
    }
}
