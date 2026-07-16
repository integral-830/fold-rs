use std::fs::rename;
use std::io::{self, Result, Write};
use std::{
    fs::{create_dir_all, File, OpenOptions},
    path::{Path, PathBuf},
};

use bytes::{BufMut, Bytes};

use crate::bloom::{bloom_size, BloomFilter};
use crate::memtable::Entry;
use crate::sstable::footer::Footer;

const SSTABLE_VERSION: u8 = 1;
const TARGET_BLOCK_SIZE: usize = 4096;

pub struct IndexEntry {
    pub first_key: Bytes,
    pub block_offset: u64,
}

pub struct SstableWriter {
    tmp_file: File,
    tmp_path: PathBuf,
    final_path: PathBuf,
    current_block: Vec<u8>,
    index_entries: Vec<IndexEntry>,
    bloom: BloomFilter,
    bytes_written: u64,
    first_key_in_block: Option<Bytes>,
    key_count: u64,
    min_key: Option<Bytes>,
    max_key: Option<Bytes>,
}

pub struct SstableMeta {
    pub path: PathBuf,
    pub key_count: u64,
    pub min_key: Bytes,
    pub max_key: Bytes,
    pub bloom: BloomFilter,
}

impl SstableWriter {
    pub fn new(dir: impl AsRef<Path>, seq: u64, expected_keys: usize) -> Result<Self> {
        let dir = dir.as_ref();
        create_dir_all(dir)?;
        let tmp_path = dir.join(format!("{seq:08}.tmp"));
        let final_path = dir.join(format!("{seq:08}.sst"));
        let tmp_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)?;
        let (num_bits, k) = bloom_size(expected_keys, 0.01);
        Ok(Self {
            tmp_file,
            tmp_path,
            final_path,
            current_block: Vec::with_capacity(TARGET_BLOCK_SIZE),
            index_entries: Vec::new(),
            bloom: BloomFilter::new(num_bits, k),
            bytes_written: 0,
            first_key_in_block: None,
            key_count: 0,
            min_key: None,
            max_key: None,
        })
    }

    pub fn add(&mut self, key: &Bytes, entry: &Entry) -> io::Result<()> {
        if self.first_key_in_block.is_none() {
            self.first_key_in_block = Some(key.clone());
        }
        self.key_count += 1;

        if self.min_key.is_none() {
            self.min_key = Some(key.clone());
        }

        self.max_key = Some(key.clone());
        self.bloom.insert(key);
        match entry {
            Entry::Value(bytes) => {
                self.current_block.put_u32_le(key.len() as u32);
                self.current_block.put_u32_le(bytes.len() as u32);
                self.current_block.put_u8(0);
                self.current_block.extend_from_slice(key);
                self.current_block.extend_from_slice(bytes);
            }
            Entry::Tombstone => {
                self.current_block.put_u32_le(key.len() as u32);
                self.current_block.put_u32_le(0);
                self.current_block.put_u8(1);
                self.current_block.extend_from_slice(key);
            }
        }
        if self.current_block.len() >= TARGET_BLOCK_SIZE {
            self.flush_block()?;
        }
        Ok(())
    }

    fn flush_block(&mut self) -> io::Result<()> {
        if self.current_block.is_empty() {
            return Ok(());
        }
        self.tmp_file.write_all(&self.current_block)?;
        let first_key = self
            .first_key_in_block
            .take()
            .expect("Block must have 1st key.");
        self.index_entries.push(IndexEntry {
            first_key,
            block_offset: self.bytes_written,
        });
        self.bytes_written += self.current_block.len() as u64;
        self.current_block.clear();
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<SstableMeta> {
        self.flush_block()?;
        let index_offset = self.bytes_written;
        let mut index = Vec::new();
        for index_entry in &self.index_entries {
            index.put_u32_le(index_entry.first_key.len() as u32);
            index.extend_from_slice(&index_entry.first_key);
            index.put_u64_le(index_entry.block_offset);
        }
        self.tmp_file.write_all(&index)?;
        self.bytes_written += index.len() as u64;
        let bloom_offset = self.bytes_written;
        self.bloom.write_to(&mut self.tmp_file)?;
        self.bytes_written += self.bloom.serialized_size() as u64;
        let footer = Footer {
            index_offset,
            bloom_offset,
            version: SSTABLE_VERSION,
        };
        footer.write_to(&mut self.tmp_file)?;
        self.tmp_file.sync_all()?;
        drop(self.tmp_file);
        rename(&self.tmp_path, &self.final_path)?;
        Ok(SstableMeta {
            path: self.final_path,
            key_count: self.key_count,
            min_key: self.min_key.expect("SSTable must conatin at least one key"),
            max_key: self.max_key.expect("SSTable must conatin at least one key"),
            bloom: self.bloom,
        })
    }
}
