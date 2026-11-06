#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    dir: PathBuf,
    cli: PathBuf,
    powershell: PathBuf,
    calls: PathBuf,
    media_capture: PathBuf,
    closed: PathBuf,
    killed: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("deskact-vlc-test-{}-{unique}", std::process::id()));
        fs::create_dir_all(&dir).expect("create fixture directory");
        let cli = dir.join("agent-desktop");
        let powershell = dir.join("powershell");
        let calls = dir.join("calls.log");
        let media_capture = dir.join("media-path.log");
        let closed = dir.join("closed");
        let killed = dir.join("killed");
        fs::write(
            &cli,
            r##"#!/bin/sh
printf '%s\n' "$*" >> "$DESKACT_TEST_CALLS"
fixture_title() {
 name=$(basename "$(cat "$DESKACT_TEST_MEDIA_CAPTURE")")
 printf '%s - VLC media player' "$name"
}
user_window() { printf '{"id":"user-vlc","title":"My media - VLC media player","app_name":"vlc.exe","accessible":true,"minimized":false,"visible":true,"pid":1234}'; }
fixture_window() { title=$(fixture_title); printf '{"id":"vlc-window","title":"%s","app_name":"vlc.exe","accessible":true,"minimized":false,"visible":true,"pid":9876}' "$title"; }
case "$1" in
 version) echo '{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}' ;;
 list-windows)
  if [ -f "$DESKACT_TEST_CLOSED" ]; then
   if [ "$DESKACT_TEST_MODE" = existing-user ]; then printf '{"version":"2.6","ok":true,"command":"list-windows","data":[%s]}\n' "$(user_window)"; else echo '{"version":"2.6","ok":true,"command":"list-windows","data":[]}'; fi
  elif grep -q '^powershell-launch ' "$DESKACT_TEST_CALLS" && [ "$DESKACT_TEST_MODE" = ambiguous ]; then title=$(fixture_title); printf '{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"vlc-window","title":"%s","app_name":"vlc.exe","accessible":true,"minimized":false,"visible":true,"pid":9876},{"id":"vlc-window-2","title":"%s","app_name":"vlc.exe","accessible":true,"minimized":false,"visible":true,"pid":9877}]}\n' "$title" "$title"
  elif grep -q '^powershell-launch ' "$DESKACT_TEST_CALLS"; then
   if [ "$DESKACT_TEST_MODE" = existing-user ]; then printf '{"version":"2.6","ok":true,"command":"list-windows","data":[%s,%s]}\n' "$(user_window)" "$(fixture_window)"; else printf '{"version":"2.6","ok":true,"command":"list-windows","data":[%s]}\n' "$(fixture_window)"; fi
  elif [ "$DESKACT_TEST_MODE" = existing-user ]; then printf '{"version":"2.6","ok":true,"command":"list-windows","data":[%s]}\n' "$(user_window)"
  else echo '{"version":"2.6","ok":true,"command":"list-windows","data":[]}'; fi ;;
 snapshot)
  n=$(grep -c '^snapshot ' "$DESKACT_TEST_CALLS"); elapsed=00:03; [ "$n" -ge 2 ] && elapsed=00:05; [ "$n" -ge 3 ] && elapsed=00:07; [ "$DESKACT_TEST_MODE" = stationary ] && elapsed=00:03; [ "$DESKACT_TEST_MODE" = missing-time ] && elapsed=--:--;
  state='{"role":"statictext","name":"Playing"},'; [ "$DESKACT_TEST_MODE" = paused ] && state='{"role":"statictext","name":"Paused"},'; [ "$DESKACT_TEST_MODE" = missing-state ] && state='';
  printf '{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"vlc.exe","window":{"id":"vlc-window","title":"deskact-vlc-fixture-test.wav - VLC media player"},"snapshot_id":"vlc-%s","ref_count":1,"complete":true,"tree":{"role":"window","children":[%s{"role":"statictext","name":"%s","ref_id":"@vlc-%s:e1"},{"role":"statictext","name":"00:30","ref_id":"@vlc-%s:e2"},{"role":"button","name":"Close","ref_id":"@vlc-%s:e3"}]}}}\n' "$n" "$state" "$elapsed" "$n" "$n" "$n" ;;
 focus-window)
  printf '{"version":"2.6","ok":true,"command":"focus-window","data":{"focused":{"id":"%s","is_focused":true}}}\n' "$5" ;;
 press)
  [ "$2" = "--headed" ] && [ "$3" = "--app" ] && [ "$4" = "vlc.exe" ] && [ "$5" = "--window-id" ] && [ "$7" = "alt+f4" ] || { echo 'unexpected VLC close shortcut arguments' >&2; exit 2; }
  touch "$DESKACT_TEST_CLOSED"; echo '{"version":"2.6","ok":true,"command":"press","data":{"disposition":{"delivery":"delivered_unverified","retry":"unsafe"}}}' ;;
 *) echo "unexpected command: $1" >&2; exit 2 ;;
esac
"##,
        )
        .expect("write fake native CLI");
        make_executable(&cli);
        fs::write(
            &powershell,
            r##"#!/bin/sh
case "$*" in
 *Stop-Process*)
  case "$*" in *9876*"$DESKACT_VLC_MEDIA"*) ;; *) echo 'VLC cleanup did not target the launched PID and fixture' >&2; exit 2 ;; esac
  touch "$DESKACT_TEST_KILLED" ;;
 *Win32_Process*) printf '%s\n' 'C:\Program Files\VideoLAN\VLC\vlc.exe' ;;
 *Start-Process*)
  [ -f "$DESKACT_VLC_MEDIA" ] || { echo 'VLC fixture is missing' >&2; exit 2; }
  [ "$(dd if="$DESKACT_VLC_MEDIA" bs=4 count=1 2>/dev/null)" = RIFF ] || { echo 'VLC fixture is not a WAV file' >&2; exit 2; }
  case "$*" in *"$DESKACT_VLC_CONFIG"*) ;; *) echo 'VLC config path was not isolated under the workspace' >&2; exit 2 ;; esac
  case "$*" in *"$DESKACT_VLC_MEDIA"*) ;; *) echo 'VLC was not passed the generated fixture path' >&2; exit 2 ;; esac
  case "$*" in *'--config="'*) ;; *) echo 'VLC was not passed its isolated config option' >&2; exit 2 ;; esac
  printf 'powershell-launch %s\n' "$*" >> "$DESKACT_TEST_CALLS"
  printf '%s\n' "$DESKACT_VLC_MEDIA" > "$DESKACT_TEST_MEDIA_CAPTURE"
  echo 'DESKACT_VLC_PID=9876'
  ;;
 *) echo 'unexpected PowerShell command' >&2; exit 2 ;;
esac
"##,
        )
        .expect("write fake PowerShell");
        make_executable(&powershell);
        Self {
            dir,
            cli,
            powershell,
            calls,
            media_capture,
            closed,
            killed,
        }
    }

    fn run(&self) -> std::process::Output {
        self.run_with_mode("normal")
    }

    fn run_with_mode(&self, mode: &str) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .arg("vlc-acceptance")
            .env("DESKACT_AGENT_DESKTOP", &self.cli)
            .env("DESKACT_POWERSHELL", &self.powershell)
            .env(
                "DESKACT_VLC_EXECUTABLE",
                r"C:\Program Files\VideoLAN\VLC\vlc.exe",
            )
            .env("DESKACT_TEST_CALLS", &self.calls)
            .env("DESKACT_TEST_MEDIA_CAPTURE", &self.media_capture)
            .env("DESKACT_TEST_CLOSED", &self.closed)
            .env("DESKACT_TEST_KILLED", &self.killed)
            .env("DESKACT_TEST_MODE", mode)
            .env_remove("WSL_DISTRO_NAME")
            .output()
            .expect("run developer harness")
    }

    fn calls(&self) -> String {
        fs::read_to_string(&self.calls).unwrap_or_default()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn make_executable(path: &std::path::Path) {
    let mut permissions = fs::metadata(path)
        .expect("read executable metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fixture executable");
}

/// Refuse to claim playback when fresh accessible position does not advance.
#[test]
fn vlc_acceptance_reports_stationary_playback_as_unverified() {
    let fixture = Fixture::new();
    let output = fixture.run_with_mode("stationary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("did not advance"), "{stderr}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(
        fixture.closed.exists(),
        "test VLC window was not closed after verification failed"
    );
}

/// Fail closed when VLC's playback clock cannot be read through accessibility.
#[test]
fn vlc_acceptance_reports_missing_accessible_playback_time_as_unverified() {
    let fixture = Fixture::new();
    let output = fixture.run_with_mode("missing-time");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("did not expose one current and total playback time pair"),
        "{stderr}"
    );
    assert!(
        fixture.closed.exists(),
        "test VLC window was not closed after verification failed"
    );
}

/// Do not report success when fresh accessibility evidence says VLC is paused.
#[test]
fn vlc_acceptance_reports_paused_playback_as_unverified() {
    let fixture = Fixture::new();
    let output = fixture.run_with_mode("paused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("not in the playing state"), "{stderr}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(fixture.closed.exists(), "test VLC window was not closed");
}

/// Fail closed when VLC exposes no accessible playback-state indicator.
#[test]
fn vlc_acceptance_reports_missing_playback_state_as_unverified() {
    let fixture = Fixture::new();
    let output = fixture.run_with_mode("missing-state");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("did not expose an accessible playback state"),
        "{stderr}"
    );
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(fixture.closed.exists(), "test VLC window was not closed");
}

/// Reject multiple new VLC windows instead of guessing which one owns the test fixture.
#[test]
fn vlc_acceptance_refuses_ambiguous_new_fixture_windows() {
    let fixture = Fixture::new();
    let output = fixture.run_with_mode("ambiguous");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let calls = fixture.calls();
    assert!(!output.status.success());
    assert!(stderr.contains("target is ambiguous"), "{stderr}");
    assert!(
        !calls.contains("snapshot ") && !calls.contains("press "),
        "ambiguous VLC windows were acted on: {calls}"
    );
    assert!(
        fixture.killed.exists(),
        "ambiguous owned VLC process was not cleaned up"
    );
}

/// Leave an existing user VLC window untouched while verifying the newly launched fixture instance.
#[test]
fn vlc_acceptance_does_not_select_or_close_an_existing_user_window() {
    let fixture = Fixture::new();
    let output = fixture.run_with_mode("existing-user");
    let calls = fixture.calls();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr: {stderr}; calls: {calls}");
    assert!(
        calls.contains("snapshot --app vlc.exe --window-id vlc-window -i"),
        "fixture VLC window was not selected: {calls}"
    );
    assert!(
        !calls.contains("--window-id user-vlc"),
        "an existing user VLC window was targeted: {calls}"
    );
    assert!(fixture.closed.exists(), "test VLC window was not closed");
}

/// Play only a generated WAV fixture, confirm advancing UIA position, and close the test VLC window.
#[test]
fn vlc_acceptance_verifies_playback_of_its_generated_fixture() {
    let fixture = Fixture::new();
    let output = fixture.run();
    let calls = fixture.calls();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}\ncalls: {calls}"
    );
    assert!(stdout.contains("verified advancing playback"), "{stdout}");
    assert!(
        calls.contains("snapshot --app vlc.exe --window-id vlc-window -i"),
        "{calls}"
    );
    assert!(
        calls.contains("focus-window --app vlc.exe --window-id vlc-window"),
        "test-owned VLC window was not focused for cleanup: {calls}"
    );
    assert!(
        calls.contains("press --headed --app vlc.exe --window-id vlc-window alt+f4"),
        "test-owned VLC window was not closed with the locale-independent shortcut: {calls}"
    );
    assert!(
        calls.contains("--no-one-instance"),
        "VLC was not launched in its own instance: {calls}"
    );
    assert!(
        calls.contains("--no-metadata-network-access"),
        "metadata network access was not disabled: {calls}"
    );
    assert!(
        calls.contains("--no-qt-recentplay"),
        "recent-media tracking was not disabled: {calls}"
    );
    assert!(
        fixture.closed.exists(),
        "the test VLC window was not closed"
    );
    assert!(
        fixture.killed.exists(),
        "owned VLC process cleanup was skipped"
    );

    let media_path =
        fs::read_to_string(&fixture.media_capture).expect("capture launched fixture path");
    let media_path = media_path.trim();
    assert!(
        !std::path::Path::new(media_path).exists(),
        "generated media fixture was not cleaned up: {media_path}"
    );
}
