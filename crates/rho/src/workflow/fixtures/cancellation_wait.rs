use std::{fs, io::Write as _, net::Shutdown, os::unix::net::UnixStream, path::Path};

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let socket = arguments.next().expect("ready socket path");
    let pid_file = arguments.next().expect("pid file path");
    assert!(arguments.next().is_none(), "unexpected argument");
    fs::write(Path::new(&pid_file), format!("{}\n", std::process::id())).unwrap();
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.write_all(b"x").unwrap();
    stream.shutdown(Shutdown::Both).unwrap();
    loop {
        std::thread::park();
    }
}
