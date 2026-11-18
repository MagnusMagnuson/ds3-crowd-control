use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Lines go to a thread of their own that writes them to the file, so the game never
/// waits for the disk (or a virus scanner looking at the file) while the mod logs.
static LINES: OnceLock<crossbeam_channel::Sender<String>> = OnceLock::new();

pub fn init() {
    let Ok(dir) = app_dir() else {
        return;
    };
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("ds3mod.log");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let (sender, receiver) = crossbeam_channel::unbounded::<String>();
    if LINES.set(sender).is_err() {
        return;
    }
    std::thread::spawn(move || {
        crate::perf::name_thread("mod: writing the log");
        for line in receiver {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    });
}

pub fn app_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var("APPDATA").map_err(|_| "APPDATA is not set".to_string())?;
    Ok(PathBuf::from(appdata).join("Ds3ViewerControl"))
}

pub fn log(message: impl AsRef<str>) {
    let line = format!(
        "{} {}\n",
        chrono_stamp(),
        message.as_ref().replace(['\r', '\n'], " ")
    );
    let start = std::time::Instant::now();
    if let Some(lines) = LINES.get() {
        let _ = lines.send(line);
    }
    crate::perf::add(&crate::perf::LOG, start.elapsed());
}

fn chrono_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}
