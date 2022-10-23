use std::{
    hash::Hash,
    io::{self, BufRead, Write},
};

use async_std::io::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingMessage {
    /// In range 0..1
    pub head_position: [f32; 2],
    /// Head rotation as a quaternion.
    ///
    /// The 4 floats are `x`, `y`, `z`, `r` in `q = r * x*i * y*j * z*k`.
    pub head_rotation: [f32; 4],
    pub left_eye: Eye,
    pub right_eye: Eye,
}

impl TrackingMessage {
    pub fn read<R: BufRead>(mut read: R) -> io::Result<Self> {
        let mut size = [0; 4];
        read.read_exact(&mut size)?;
        let size = u32::from_le_bytes(size);
        let val = bincode::deserialize_from(&mut read.take(size.into())).map_err(convert_error)?;
        Ok(val)
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        let size = bincode::serialized_size(self).map_err(convert_error)?;
        writer.write_all(&u32::try_from(size).unwrap().to_le_bytes())?;
        bincode::serialize_into(&mut writer, self).map_err(convert_error)?;
        Ok(())
    }

    pub async fn async_read<R: async_std::io::Read + Unpin>(mut read: R) -> io::Result<Self> {
        let mut size = [0; 4];
        read.read_exact(&mut size).await?;
        let size = u32::from_le_bytes(size);
        let mut buf = vec![0; size as usize];
        read.read_exact(&mut buf).await?;
        let val = bincode::deserialize_from(&*buf).map_err(convert_error)?;
        Ok(val)
    }

    pub async fn async_write<W: async_std::io::Write + Unpin>(
        &self,
        mut writer: W,
    ) -> io::Result<()> {
        let size = bincode::serialized_size(self).map_err(convert_error)?;
        writer
            .write_all(&u32::try_from(size).unwrap().to_le_bytes())
            .await?;
        let buf = bincode::serialize(self).map_err(convert_error)?;
        writer.write_all(&buf).await?;
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
