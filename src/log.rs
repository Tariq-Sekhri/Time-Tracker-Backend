use chrono::Local;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

static INFO_FILE: OnceLock<Mutex<io::BufWriter<std::fs::File>>> = OnceLock::new();
static ERROR_FILE: OnceLock<Mutex<io::BufWriter<std::fs::File>>> = OnceLock::new();

pub fn init() -> io::Result<()> {
    fs::create_dir_all("logs")?;
    let info = OpenOptions::new()
        .create(true)
        .append(true)
        .open("logs/info.log")?;
    let error = OpenOptions::new()
        .create(true)
        .append(true)
        .open("logs/error.log")?;
    INFO_FILE
        .set(Mutex::new(io::BufWriter::new(info)))
        .ok();
    ERROR_FILE
        .set(Mutex::new(io::BufWriter::new(error)))
        .ok();
    Ok(())
}

fn timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

fn write_to_file(
    file: &OnceLock<Mutex<io::BufWriter<std::fs::File>>>,
    line: &str,
) {
    if let Some(mutex) = file.get() {
        if let Ok(mut writer) = mutex.lock() {
            let _ = writeln!(writer, "{line}");
            let _ = writer.flush();
        }
    }
}

pub fn write_info(message: &str, file: &str, line: u32) {
    let entry = format!(
        "[{}] [INFO] {}:{} — {}",
        timestamp(),
        file,
        line,
        message
    );
    println!("{entry}");
    write_to_file(&INFO_FILE, &entry);
}

pub fn write_error(message: &str, file: &str, line: u32) {
    let entry = format!(
        "[{}] [ERROR] {}:{} — {}",
        timestamp(),
        file,
        line,
        message
    );
    eprintln!("{entry}");
    write_to_file(&ERROR_FILE, &entry);
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {{
        $crate::log::write_info(
            &format!($($arg)*),
            file!(),
            line!(),
        );
    }};
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {{
        $crate::log::write_error(
            &format!($($arg)*),
            file!(),
            line!(),
        );
    }};
}
