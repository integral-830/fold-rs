use std::mem;

use crc32fast::hash;

const CRC32_SIZE: usize = mem::size_of::<u32>();
const LEN_SIZE: usize = mem::size_of::<u32>();
const RECORD_TYPE_SIZE: usize = mem::size_of::<u8>();

pub const HEADER_SIZE: usize = CRC32_SIZE + LEN_SIZE + LEN_SIZE + RECORD_TYPE_SIZE;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    Put = 0,
    Delete = 1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalRecord<'a> {
    pub record_type: RecordType,
    pub key: &'a [u8],
    pub value: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalRecordOwned {
    pub record_type: RecordType,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

pub fn encoded_len(key: &[u8], value: &[u8]) -> usize {
    debug_assert!(key.len() < u32::MAX as usize, "Key is too large for WAL");
    debug_assert!(
        value.len() < u32::MAX as usize,
        "Value is too large for WAL"
    );

    HEADER_SIZE + key.len() + value.len()
}

pub fn serialize(record: &WalRecord<'_>, buf: &mut Vec<u8>) {
    debug_assert!(
        u32::try_from(record.key.len()).is_ok(),
        "key too large for WAL format"
    );

    debug_assert!(
        u32::try_from(record.value.len()).is_ok(),
        "value too large for WAL format"
    );
    let offset = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&(record.key.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(record.value.len() as u32).to_le_bytes());
    buf.push(record.record_type as u8);
    buf.extend_from_slice(record.key);
    buf.extend_from_slice(record.value);

    let crc_hash = hash(&buf[offset + CRC32_SIZE..]);
    buf[offset..offset + CRC32_SIZE].copy_from_slice(&crc_hash.to_le_bytes());
}
