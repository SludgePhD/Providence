use std::io::{self, BufRead, Write};
use std::sync::OnceLock;

use async_std::io::{Read as AsyncRead, Write as AsyncWrite};
use async_std::prelude::*;
use serde::{Deserialize, Serialize};

use crate::fingerprint::serde_fingerprint;

static FINGERPRINT: OnceLock<u64> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingMessage {
    /// XY position of the center of the face (in range 0..1).
    ///
    /// X points right, Y points down.
    pub head_position: [f32; 2],
    /// Head rotation as a quaternion.
    ///
    /// The 4 floats are `x`, `y`, `z`, `w` in `q = w * x*i * y*j * z*k`.
    pub head_rotation: [f32; 4],
    pub left_eye: Eye,
    pub right_eye: Eye,
}

impl TrackingMessage {
    pub fn read<R: BufRead>(mut read: R) -> io::Result<Self> {
        let mut fingerprint = [0; 8];
        read.read_exact(&mut fingerprint)?;
        let fingerprint = u64::from_le_bytes(fingerprint);

        if Self::fingerprint() != fingerprint {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "message fingerprint mismatch",
            ));
        }

        let mut size = [0; 4];
        read.read_exact(&mut size)?;
        let size = u32::from_le_bytes(size);

        let val = bincode::deserialize_from(&mut read.take(size.into())).map_err(convert_error)?;
        Ok(val)
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(&Self::fingerprint().to_le_bytes())?;

        let size = bincode::serialized_size(self).map_err(convert_error)?;
        writer.write_all(&u32::try_from(size).unwrap().to_le_bytes())?;

        bincode::serialize_into(&mut writer, self).map_err(convert_error)?;

        Ok(())
    }

    pub async fn async_read<R: AsyncRead + Unpin>(mut read: R) -> io::Result<Self> {
        let mut fingerprint = [0; 8];
        read.read_exact(&mut fingerprint).await?;
        let fingerprint = u64::from_le_bytes(fingerprint);

        if Self::fingerprint() != fingerprint {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "message fingerprint mismatch",
            ));
        }

        let mut size = [0; 4];
        read.read_exact(&mut size).await?;
        let size = u32::from_le_bytes(size);

        let mut buf = vec![0; size as usize];
        read.read_exact(&mut buf).await?;
        let val = bincode::deserialize_from(&*buf).map_err(convert_error)?;

        Ok(val)
    }

    pub async fn async_write<W: AsyncWrite + Unpin>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(&Self::fingerprint().to_le_bytes()).await?;

        let size = bincode::serialized_size(self).map_err(convert_error)?;
        writer
            .write_all(&u32::try_from(size).unwrap().to_le_bytes())
            .await?;

        let buf = bincode::serialize(self).map_err(convert_error)?;
        writer.write_all(&buf).await?;
        Ok(())
    }

    fn fingerprint() -> u64 {
        *FINGERPRINT.get_or_init(|| serde_fingerprint::<Self>())
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
    // FIXME: ideally these two would only be present if the iris is actually visible
    pub iris_center: [f32; 2],
    pub iris_radius: f32,
}

/// A 2D triangle mesh in counter-clockwise winding order.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // RGBA
}
