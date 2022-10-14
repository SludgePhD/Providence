use std::{
    hash::Hash,
    io::{self, BufRead, Write},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingMessage {
    pub left_eye: Eye,
    pub right_eye: Eye,
}

impl TrackingMessage {
    pub fn read<R: BufRead>(mut read: R) -> io::Result<Self> {
        let val = bincode::deserialize_from(&mut read).map_err(convert_error)?;
        Ok(val)
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        bincode::serialize_into(&mut writer, self).map_err(convert_error)?;
        Ok(())
    }
}

fn convert_error(e: bincode::Error) -> io::Error {
    match *e {
        bincode::ErrorKind::Io(io) => io,
        kind => io::Error::new(io::ErrorKind::InvalidData, kind),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eye {
    pub texture: Image,
    pub mesh: Mesh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u16>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Vertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
}

impl Hash for Vertex {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.position[0].to_bits().hash(state);
        self.position[1].to_bits().hash(state);
    }
}

impl PartialEq for Vertex {
    fn eq(&self, other: &Self) -> bool {
        self.position[0]
            .total_cmp(&other.position[0])
            .then(self.position[1].total_cmp(&other.position[1]))
            .is_eq()
    }
}

impl Eq for Vertex {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // RGBA
}
