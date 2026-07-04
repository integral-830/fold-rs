use std::io::{self, Read, Write};

pub const SSTABLE_MAGIC: u64 = 0x53535441424C4B31;
pub const FOOTER_SIZE: usize = 32;

#[derive(Debug, Clone, Copy)]
pub struct Footer {
    pub index_offset: u64,
    pub bloom_offset: u64,
    pub version: u8,
}

impl Footer {
    pub fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(&self.index_offset.to_le_bytes())?;
        writer.write_all(&self.bloom_offset.to_le_bytes())?;
        writer.write_all(&[self.version])?;
        writer.write_all(&[0; 7])?;
        writer.write_all(&SSTABLE_MAGIC.to_le_bytes())?;
        Ok(())
    }

    pub fn read_from(mut bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() != FOOTER_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid footer data size",
            ));
        }
        let mut buf = [0u8; 8];
        bytes.read_exact(&mut buf)?;
        let index_offset = u64::from_le_bytes(buf);
        bytes.read_exact(&mut buf)?;
        let bloom_offset = u64::from_le_bytes(buf);
        let mut version = [0u8; 1];
        bytes.read_exact(&mut version)?;
        let mut padding = [0u8; 7];
        bytes.read_exact(&mut padding)?;
        bytes.read_exact(&mut buf)?;
        let magic = u64::from_le_bytes(buf);

        if magic != SSTABLE_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid Sstable magic found",
            ));
        }
        Ok(Self {
            index_offset,
            bloom_offset,
            version: version[0],
        })
    }
}
