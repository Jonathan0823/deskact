//! Shared test fixtures.

#![allow(dead_code)]

use std::{
    fs, io, os::unix::fs::PermissionsExt, path::Path, process::Command, thread, time::Duration,
};

/// Write an executable fixture script that can be spawned immediately.
///
/// Tests run on many threads and spawn child processes while other threads are still writing
/// fixtures. A child forked in that window briefly inherits the writer's descriptor, so exec-ing the
/// new file fails with ETXTBSY ("Text file busy", os error 26) until the child has exec'd. After
/// writing, this probes the file and retries on ETXTBSY, so callers never see that race.
pub fn write_executable(path: &Path, contents: impl AsRef<str>) {
    fs::write(path, contents.as_ref()).expect("write fixture executable");
    make_executable(path);
}

/// Mark an already written fixture executable and wait until it can be spawned.
pub fn make_executable(path: &Path) {
    let mut permissions = fs::metadata(path)
        .expect("read fixture metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fixture executable");
    wait_until_executable(path);
}

fn wait_until_executable(path: &Path) {
    for _ in 0..200 {
        match Command::new(path)
            .arg("--deskact-fixture-probe")
            .env("DESKACT_TEST_CALLS", "/dev/null")
            .env("DESKACT_TEST_INVOCATIONS", "/dev/null")
            .stdin(std::process::Stdio::null())
            .output()
        {
            Err(error) if error.raw_os_error() == Some(26) => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("fixture {} cannot be executed: {error}", path.display()),
            Ok(_) => return,
        }
    }
    panic!("fixture {} stayed busy", path.display());
}

/// Spawn `path` once, reporting ETXTBSY as an error so tests can assert it never happens.
pub fn spawn_once(path: &Path) -> io::Result<()> {
    Command::new(path).arg("run").output().map(|_| ())
}
