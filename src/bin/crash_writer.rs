use std::{
    env,
    fs::OpenOptions,
    io::{BufWriter, Write},
};

use fold_rs::storage_engine::StorageEngine;
use rand::{distr::Alphanumeric, Rng, RngExt};

fn random_string(rng: &mut impl Rng, len: usize) -> String {
    rng.sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn main() {
    let dir = env::args().nth(1).expect("usage: crash_writer <dir>");

    let mut engine = StorageEngine::open(&dir).expect("open engine");

    let acked_path = std::path::Path::new(&dir).join("acked.log");

    let acked_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&acked_path)
        .expect("open acked.log");

    let mut acked = BufWriter::new(acked_file);

    let mut rng = rand::rng();

    loop {
        let key = random_string(&mut rng, 16);

        let value = random_string(&mut rng, 128);

        match engine.put(key.as_bytes(), value.as_bytes()) {
            Ok(()) => {
                writeln!(acked, "{}\t{}", key, value,).unwrap();

                acked.flush().expect("flush ack");

                acked.get_ref().sync_all().expect("fsync ack");
            }

            Err(err) => {
                eprintln!("put failed: {err}");
                break;
            }
        }
    }
}
