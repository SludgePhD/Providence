use std::{cmp, hash::BuildHasherDefault};

use crate::data::{self, Eye, Mesh, Vertex};
use indexmap::IndexSet;
use rustc_hash::FxHasher;
use spade::{DelaunayTriangulation, HasPosition, Point2, Triangulation};
use zaru::{
    face::eye::EyeLandmarks,
    image::{Image, Rect},
    num::TotalF32,
};

// FIXME: drop the `spade` crate and just define a fixed tessellation by hand

#[derive(Clone, Copy)]
struct Pos([f32; 2]);

impl HasPosition for Pos {
    type Scalar = f32;

    fn position(&self) -> Point2<Self::Scalar> {
        Point2::new(self.0[0], self.0[1])
    }
}

type Triang = DelaunayTriangulation<Pos>;

#[derive(Default)]
pub struct Triangulator {
    points: Vec<Pos>,
    vertex_set: IndexSet<Vertex, BuildHasherDefault<FxHasher>>,
}

impl Triangulator {
    pub fn triangulate_eye(&mut self, eye: &EyeLandmarks, img: &Image) -> zaru::Result<Eye> {
        self.vertex_set.clear();
        self.points.clear();
        self.points
            .extend(eye.eye_contour().take(16).map(|[x, y, _]| Pos([x, y])));

        // Compute AABB to crop image to
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for pt in &self.points {
            min[0] = cmp::min(TotalF32(min[0]), TotalF32(pt.0[0])).0;
            min[1] = cmp::min(TotalF32(min[1]), TotalF32(pt.0[1])).0;
            max[0] = cmp::max(TotalF32(max[0]), TotalF32(pt.0[0])).0;
            max[1] = cmp::max(TotalF32(max[1]), TotalF32(pt.0[1])).0;
        }

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
        for pt in &mut self.points {
            pt.0[0] = pt.0[0] - min[0];
            pt.0[1] = pt.0[1] - min[1];
        }
        let max = [max[0] - min[0], max[1] - min[1]];
        let mkvertex = |pos: Point2<_>| Vertex {
            position: [
                (pos.x - max[0] * 0.5) / range,
                (pos.y - max[1] * 0.5) / range,
            ],
            uv: [pos.x / max[0], pos.y / max[1]],
        };

        let triang = Triang::bulk_load(self.points.clone())?;
        let mut mesh = Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
        };
        for vertex in triang.vertices() {
            let vertex = mkvertex(vertex.position());
            self.vertex_set.insert(vertex);
            mesh.vertices.push(vertex);
        }
        for face in triang.inner_faces() {
            // Annoyingly, spade doesn't allow access to a vertex' index in the list, so we have to
            // use a whole damn `IndexSet` to do that.
            for vertex in face.vertices() {
                let vertex = mkvertex(vertex.position());
                let index = self.vertex_set.get_index_of(&vertex).unwrap();
                mesh.indices.push(index.try_into().unwrap());
            }
        }

        Ok(Eye {
            texture: data::Image {
                width: img.width(),
                height: img.height(),
                data: img.data().to_vec(),
            },
            mesh,
        })
    }
}
