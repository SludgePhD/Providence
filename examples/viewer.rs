use std::io;

use macroquad::{models::Vertex, prelude::*, texture::Texture2D};
use providence::{data::Eye, net::Subscriber};

const SCALE: f32 = 80.0;

#[macroquad::main("Providence Viewer")]
async fn main() -> io::Result<()> {
    let mut sub = Subscriber::autoconnect_blocking()?;

    let mut msg = sub.block()?;

    loop {
        if let Some(next) = sub.next()? {
            msg = next;
        }

        let width = screen_width();
        let height = screen_height();

        let [x, y] = msg.head_position;
        let [x, y] = [x * width, y * height];

        clear_background(BLACK);
        render_eye(&msg.left_eye, SCALE, Vec3::new(x - SCALE * 1.5, y, 0.0));
        render_eye(&msg.right_eye, SCALE, Vec3::new(x + SCALE * 1.5, y, 0.0));
        next_frame().await;
    }
}

fn render_eye(eye: &Eye, scale: f32, offset: Vec3) {
    let texture = Texture2D::from_rgba8(
        eye.texture.width as _,
        eye.texture.height as _,
        &eye.texture.data,
    );
    let mesh = Mesh {
        vertices: eye
            .mesh
            .vertices
            .iter()
            .map(|vert| Vertex {
                position: Vec3::new(vert.position[0], vert.position[1], 0.0) * scale + offset,
                uv: Vec2::new(vert.uv[0], vert.uv[1]),
                color: WHITE,
            })
            .collect(),
        indices: eye.mesh.indices.clone(),
        texture: Some(texture),
    };

    draw_mesh(&mesh);
}
