use std::mem;

const CRC32_SIZE: usize = mem::size_of::<u32>();
const LEN_SIZE: usize = mem::size_of::<u32>();
const RECORD_TYPE_SIZE: usize = mem::size_of::<u8>();

const HEADER_SIZE: usize = CRC32_SIZE + LEN_SIZE + LEN_SIZE + RECORD_TYPE_SIZE;

#[repr(u8)]
pub enum RecordType {
    Put = 0,
    Delete = 1,
}

pub struct WalRecord<'a> {
    pub record_type: RecordType,
    pub key: &'a [u8],
    pub value: &'a [u8],
}

pub fn encoded_len(key: &[u8], value: &[u8]) -> usize {
    debug_assert!(key.len() < u32::MAX as usize, "Key is too large for WAL");
    debug_assert!(
        value.len() < u32::MAX as usize,
        "Value is too large for WAL"
    );

    HEADER_SIZE + key.len() + value.len()
}
