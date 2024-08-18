use std::io;

use macroquad::{models::Vertex, prelude::*, texture::Texture2D};
use providence_io::{data::Eye, net::Subscriber};
use zaru::linalg::Quat;

const SCALE: f32 = 120.0;

#[macroquad::main("Providence Viewer")]
async fn main() -> io::Result<()> {
    let mut sub = Subscriber::autoconnect_blocking()?;
    println!("connected to tracker");

    let mut msg = sub.block()?;
    if msg.faces.is_empty() {
        println!("no faces in view; waiting");
        while msg.faces.is_empty() {
            msg = sub.block()?;
        }
    }
    println!("received first tracking message, starting output");

    loop {
        if let Some(next) = sub.next()? {
            msg = next;
        }

        clear_background(BLACK);

        let [face, ..] = &*msg.faces else {
            next_frame().await;
            continue;
        };
        let width = screen_width();
        let height = screen_height();

        let [x, y] = face.head_position;
        let [x, y] = [x * width, y * height];

        if let Some(eye) = &face.left_eye {
            render_eye(eye, SCALE, Vec3::new(x - SCALE * 1.5, y, 0.0));
        }
        if let Some(eye) = &face.right_eye {
            render_eye(eye, SCALE, Vec3::new(x + SCALE * 1.5, y, 0.0));
        }
        draw_circle(x, y, 5.0, Color::new(1.0, 1.0, 1.0, 1.0));

        let [x, y, z, w] = face.head_rotation;
        render_rotation(Quat::from_components(w, x, y, z).normalize());
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
                // FIXME: including the Z coordinate results in the eyes getting mostly culled away
                position: Vec3::new(vert.position[0], vert.position[1], 0.0) * scale + offset,
                uv: Vec2::new(vert.uv[0], vert.uv[1]),
                color: WHITE.into(),
                normal: Vec4::Z,
            })
            .collect(),
        indices: eye.mesh.indices.clone(),
        texture: Some(texture),
    };

    draw_mesh(&mesh);

    let x = eye.iris_center[0] * scale + offset.x;
    let y = eye.iris_center[1] * scale + offset.y;
    let r = eye.iris_radius * scale;
    draw_circle(x, y, r, Color::new(1.0, 0.5, 0.5, 0.15));
}

fn render_rotation(rot: Quat<f32>) {
    let [x, y, z] = rot.to_rotation_xyz();
    draw_text_centered(&format!("X={:.02}°", x.to_degrees()), 20.0);
    draw_text_centered(&format!("Y={:.02}°", y.to_degrees()), 40.0);
    draw_text_centered(&format!("Z={:.02}°", z.to_degrees()), 60.0);
}

fn draw_text_centered(text: &str, y: f32) {
    const FONT_SIZE: f32 = 30.0;
    let dim = measure_text(text, None, FONT_SIZE as u16, 1.0);
    draw_text(
        text,
        screen_width() * 0.5 - dim.width * 0.5,
        y,
        FONT_SIZE,
        GRAY,
    );
}
