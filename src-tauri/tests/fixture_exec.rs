//! Fixture scripts must be executable the moment their writer returns, even under concurrent spawns.
#![cfg(unix)]

mod common;

use std::{fs, thread};

#[test]
fn freshly_written_fixtures_never_report_text_file_busy_under_concurrent_spawns() {
    let dir = std::env::temp_dir().join(format!("deskact-fixture-exec-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create directory");
    let workers: Vec<_> = (0..8)
        .map(|worker| {
            let dir = dir.clone();
            thread::spawn(move || {
                for round in 0..40 {
                    let script = dir.join(format!("fixture-{worker}-{round}"));
                    common::write_executable(&script, "#!/bin/sh\nexit 0\n");
                    common::spawn_once(&script)
                        .unwrap_or_else(|error| panic!("spawn {}: {error}", script.display()));
                }
            })
        })
        .collect();
    for worker in workers {
        worker
            .join()
            .expect("worker finished without a busy fixture");
    }
    let _ = fs::remove_dir_all(&dir);
}
