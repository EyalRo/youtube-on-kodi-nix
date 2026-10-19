use std::path::PathBuf;
use std::process::ExitCode;

use kodiimport::{Mapping, import};
use store::Store;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "usage: kodi-import <MyVideos.db> [--map KODI_PREFIX=LOCAL_PREFIX]...\n\
    example: kodi-import MyVideos131.db --map nfs://192.168.0.100/volume1/media/=/media/\n\
    The appliance database is taken from MEDIAD_DB (default /persist/mediad/mediad.sqlite).\n\
    Run the scan (start mediad once) first: only files already in the library are matched.";

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_env("KODI_IMPORT_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let mut args = std::env::args().skip(1);
    let Some(kodi_db) = args.next().filter(|a| !a.starts_with("--")) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut maps = Vec::new();
    while let Some(flag) = args.next() {
        let parsed = (flag == "--map")
            .then(|| args.next().and_then(|v| Mapping::parse(&v)))
            .flatten();
        match parsed {
            Some(m) => maps.push(m),
            None => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let db = PathBuf::from(
        std::env::var("MEDIAD_DB").unwrap_or_else(|_| "/persist/mediad/mediad.sqlite".into()),
    );
    let mut store = match Store::open(&db) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(db = %db.display(), error = %e, "cannot open the appliance database");
            return ExitCode::from(1);
        }
    };
    match import(&PathBuf::from(kodi_db), &mut store, &maps) {
        Ok(report) => {
            for p in &report.unmatched_sample {
                tracing::warn!(path = %p, "no matching file in the library (sample)");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!(error = %e, "import failed");
            ExitCode::from(1)
        }
    }
}
