use std::{
    env,
    fs::File,
    io::{self, stdout, BufReader, Read, Seek, Write},
    process, thread,
    time::Duration,
};

use providence_io::{data::TrackingMessage, net::Publisher};

fn main() -> io::Result<()> {
    let path = match env::args_os().skip(1).next() {
        Some(path) => path,
        None => {
            eprintln!("usage: replay <dest-path>");
            process::exit(1);
        }
    };
    let mut file = BufReader::new(File::open(path)?);
    let mut publisher = Publisher::spawn()?;

    loop {
        match replay(&mut file, &mut publisher) {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {}
            Err(e) => {
                return Err(e);
            }
            Ok(()) => {}
        }

        println!();
        file.seek(io::SeekFrom::Start(0))?;
    }
}

fn replay(mut file: &mut BufReader<File>, publisher: &mut Publisher) -> io::Result<()> {
    loop {
        let mut buf = [0; 8];
        file.read_exact(&mut buf)?;
        let micros = u64::from_le_bytes(buf);
        let dur = Duration::from_micros(micros);
        let msg = TrackingMessage::read(&mut file)?;
        thread::sleep(dur);
        publisher.publish(msg);
        print!(".");
        stdout().flush()?;
    }
}
