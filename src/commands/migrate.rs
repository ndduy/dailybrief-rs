//! `dailybrief migrate`: apply pending migrations to `<data_dir>/brief.db` and print what was applied.

use std::io::Write;

use crate::config::{Env, load_config};
use crate::db::Db;

use super::{CommandError, db_path};

/// Opens the database (which applies pending migrations) and writes one line per migration
/// applied by this call; nothing when the ledger was already current.
pub fn run(env: &Env, out: &mut impl Write) -> Result<(), CommandError> {
    let config = load_config(env)?;
    let (_db, applied) = Db::open_reporting(&db_path(&config))?;
    for id in applied {
        writeln!(out, "applied {id}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn env_for(dir: &Path) -> Env {
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(
            dir.join("config/config.toml"),
            "[service]\ntimezone = \"UTC\"\n",
        )
        .unwrap();
        let cfg = dir.join("config/config.toml");
        let data = dir.join("data");
        Env::from_lookup(|n| match n {
            "DAILYBRIEF_CONFIG" => Some(cfg.to_string_lossy().into_owned()),
            "DAILYBRIEF_DATA_DIR" => Some(data.to_string_lossy().into_owned()),
            _ => None,
        })
        .unwrap()
    }

    #[test]
    fn first_run_reports_every_migration_and_second_run_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_for(dir.path());
        let mut out = Vec::new();
        run(&env, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "applied 0001_init\napplied 0002_feedback\n"
        );
        let mut out = Vec::new();
        run(&env, &mut out).unwrap();
        assert!(out.is_empty());
        assert!(dir.path().join("data/brief.db").exists());
    }
}
