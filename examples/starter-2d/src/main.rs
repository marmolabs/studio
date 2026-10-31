use bevy::prelude::*;
use starter_2d::{
    build_app,
    capture::{CAPTURE_SEED, CapturePlugin},
};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

/// Parse `--capture <dir>` from the command line.
fn capture_dir(args: &[String]) -> Result<Option<PathBuf>, String> {
    match args.iter().position(|a| a == "--capture") {
        None => Ok(None),
        Some(i) => args
            .get(i + 1)
            .map(|d| Some(PathBuf::from(d)))
            .ok_or_else(|| "--capture needs a directory argument".to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let capture = match capture_dir(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\nusage: starter-2d [--capture <dir>]");
            return ExitCode::from(2);
        }
    };

    let exit = match capture {
        Some(dir) => {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                eprintln!("error: cannot create {}: {e}", dir.display());
                return ExitCode::from(2);
            }
            let mut app = build_app(CAPTURE_SEED);
            app.add_plugins(CapturePlugin { dir });
            app.run()
        }
        None => {
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(1, |d| d.as_nanos() as u64);
            build_app(seed).run()
        }
    };
    match exit {
        AppExit::Success => ExitCode::SUCCESS,
        AppExit::Error(code) => ExitCode::from(code.get()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_capture_flag() {
        assert_eq!(capture_dir(&args(&[])), Ok(None));
        assert_eq!(
            capture_dir(&args(&["--capture", "out"])),
            Ok(Some(PathBuf::from("out")))
        );
        assert!(capture_dir(&args(&["--capture"])).is_err());
    }
}
