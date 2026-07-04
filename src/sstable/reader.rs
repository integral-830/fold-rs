use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use bytes::Bytes;
use memmap2::Mmap;

use crate::bloom::BloomFilter;
use crate::memtable::LookupResult;
use crate::sstable::footer::FOOTER_SIZE;

use super::footer::Footer;
use super::writer::IndexEntry;

pub struct SstableReader {
    mmap: Mmap,
    index: Vec<IndexEntry>,
    bloom: BloomFilter,
    footer: Footer,
}

impl SstableReader {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        if mmap.len() < FOOTER_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Sstable is smaller than expected",
            ));
        }
        let footer = Footer::read_from(&mmap[mmap.len() - FOOTER_SIZE..])?;
        if footer.index_offset > footer.bloom_offset
            || footer.bloom_offset as usize > mmap.len() - FOOTER_SIZE
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid SSTable offsets",
            ));
        }
        let index =
            Self::read_index(&mmap[footer.index_offset as usize..footer.bloom_offset as usize])?;
        let bloom =
            BloomFilter::read_from(&mmap[footer.bloom_offset as usize..mmap.len() - FOOTER_SIZE])?;
        Ok(Self {
            mmap,
            index,
            bloom,
            footer,
        })
    }

    fn read_index(bytes: &[u8]) -> io::Result<Vec<IndexEntry>> {
        let mut bytes = bytes;

        let mut entries = Vec::new();

        while !bytes.is_empty() {
            let mut key_len_buf = [0; 4];

            bytes.read_exact(&mut key_len_buf)?;

            let key_len = u32::from_le_bytes(key_len_buf) as usize;

            let mut key = vec![0; key_len];

            bytes.read_exact(&mut key)?;

            let mut offset_buf = [0; 8];

            bytes.read_exact(&mut offset_buf)?;

            let offset = u64::from_le_bytes(offset_buf);

            entries.push(IndexEntry {
                first_key: Bytes::from(key),
                block_offset: offset,
            });
        }

        Ok(entries)
    }

    fn find_block_offset(&self, key: &[u8]) -> Option<u64> {
        if self.index.is_empty() {
            return None;
        }
        match self
            .index
            .binary_search_by(|entry| entry.first_key.as_ref().cmp(key))
        {
            Ok(ind) => Some(self.index[ind].block_offset),
            Err(0) => None,
            Err(ind) => Some(self.index[ind - 1].block_offset),
        }
    }

    fn scan_block(&self, block_offset: u64, key: &[u8]) -> io::Result<Option<LookupResult>> {
        let block_end = self
            .index
            .iter()
            .find(|entry| entry.block_offset == block_offset)
            .and_then(|entry| {
                let pos = self
                    .index
                    .iter()
                    .position(|e| e.block_offset == entry.block_offset)?;
                if pos + 1 < self.index.len() {
                    Some(self.index[pos + 1].block_offset)
                } else {
                    Some(self.footer.index_offset)
                }
            })
            .unwrap();
        let mut bytes = &self.mmap[block_offset as usize..block_end as usize];
        while !bytes.is_empty() {
            let mut buff = [0u8; 4];
            bytes.read_exact(&mut buff)?;
            let key_len = u32::from_le_bytes(buff) as usize;
            bytes.read_exact(&mut buff)?;
            let value_len = u32::from_le_bytes(buff) as usize;
            let mut record_type = [0u8; 1];
            bytes.read_exact(&mut record_type)?;
            let mut record_key = vec![0; key_len];
            bytes.read_exact(&mut record_key)?;
            match record_key.as_slice().cmp(key) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    if record_type[0] == 1 {
                        return Ok(Some(LookupResult::Tombstone));
                    }
                    let mut value = vec![0; value_len];
                    bytes.read_exact(&mut value)?;
                    return Ok(Some(LookupResult::Found(Bytes::from(value))));
                }
                std::cmp::Ordering::Greater => return Ok(None),
            }
            bytes = &bytes[value_len..];
        }
        Ok(None)
    }

    pub fn get(&self, key: &[u8]) -> io::Result<Option<LookupResult>> {
        if !self.bloom.may_contain(key) {
            return Ok(None);
        }
        let Some(block_offset) = self.find_block_offset(key) else {
            return Ok(None);
        };
        self.scan_block(block_offset, key)
    }
}
