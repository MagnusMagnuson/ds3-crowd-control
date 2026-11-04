use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn init() {
    let Ok(dir) = app_dir() else {
        return;
    };
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("ds3mod.log");
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(path) {
        *LOG.lock().expect("log mutex") = Some(file);
    }
}

pub fn app_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var("APPDATA").map_err(|_| "APPDATA is not set".to_string())?;
    Ok(PathBuf::from(appdata).join("Ds3CrowdControl"))
}

pub fn log(message: impl AsRef<str>) {
    let line = format!(
        "{} {}\n",
        chrono_stamp(),
        message.as_ref().replace(['\r', '\n'], " ")
    );
    if let Ok(mut slot) = LOG.lock() {
        if let Some(file) = slot.as_mut() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }
}

fn chrono_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}
