use providence_io::data::{self, Mesh, Vertex};
use zaru::{
    face::landmark::mediapipe::LandmarkResultV2,
    image::Image,
    iter::zip_exact,
    linalg::{Quat, Vec3f},
    num::TotalF32,
    rect::Rect,
};

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
    /// - `head_rotation_inv`: inverse of the head rotation.
    pub fn triangulate_eye(
        &mut self,
        face_landmarks: &LandmarkResultV2,
        img: &Image,
        eye: Eye,
        head_rotation_inv: Quat<f32>,
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

        let points = eye_landmarks.map(|lm| lm.position());

        // Compute AABB to crop image to
        let mut min = Vec3f::splat(f32::MAX);
        let mut max = Vec3f::splat(f32::MIN);
        for pt in points {
            min = min.min(pt);
            max = max.max(pt);
        }
        min = min.map(f32::floor);
        max = max.map(f32::ceil);

        let img = img
            .view(Rect::bounding([min.truncate(), max.truncate()]).unwrap())
            .to_image();

        // Vertex positions are mapped so that all vertices fit into a rect from -0.5 to 0.5
        let range = max - min;
        let max_range = *range
            .as_array()
            .iter()
            .max_by_key(|f| TotalF32(**f))
            .unwrap(); // TODO: add max_elem or something

        let positions = points.into_iter().map(|p| {
            let p = (p - min - range * 0.5) / max_range;
            head_rotation_inv * p
        });
        let uvs = points.iter().map(|&p| ((p - min) / range).truncate());

        self.mesh.vertices.clear();
        self.mesh
            .vertices
            .extend(zip_exact(positions, uvs).map(|(position, uv)| Vertex {
                position: position.into_array(),
                uv: uv.into_array(),
            }));

        let [iris_center, rest @ ..] = iris_landmarks.map(|lm| {
            let p = (lm.position() - min - range * 0.5) / max_range;

            head_rotation_inv * p
        });

        let radii = rest.map(|p| (iris_center - p).length());
        let iris_radius = radii.into_iter().sum::<f32>() / 4.0;

        TriangulatedEye {
            texture: img,
            mesh: self.mesh.clone(),
            iris_center: [iris_center.x, iris_center.y, iris_center.z],
            iris_radius,
        }
    }
}

pub struct TriangulatedEye {
    mesh: Mesh,
    texture: Image,
    iris_center: [f32; 3],
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
                position: [-vert.position[0], vert.position[1], vert.position[2]],
                uv: vert.uv,
            })
            .collect();
        self.iris_center = [
            -self.iris_center[0],
            self.iris_center[1],
            self.iris_center[2],
        ];
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
