use std::{
    env,
    fs::File,
    io::{self, stdout, Write},
    process,
    time::Instant,
};

use providence_io::net::Subscriber;

fn main() -> io::Result<()> {
    let path = match env::args_os().skip(1).next() {
        Some(path) => path,
        None => {
            eprintln!("usage: record <dest-path>");
            process::exit(1);
        }
    };
    let mut file = File::create(path)?;

    let mut sub = Subscriber::autoconnect_blocking()?;
    let mut last = Instant::now();
    loop {
        let msg = sub.block()?;
        let now = Instant::now();
        let dur: u64 = now.duration_since(last).as_micros().try_into().unwrap();
        last = now;
        file.write_all(&dur.to_le_bytes())?;
        msg.write(&mut file)?;
        file.flush()?;
        print!(".");
        stdout().flush()?;
    }
}
