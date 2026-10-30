//! Repro for the "connection state flickers" report: drive a
//! browse-scale workload (connect + execs + SFTP listing, sequential AND
//! concurrent) against an in-process SSH/SFTP server and count the
//! `device-status` events the FE chip reacts to. A stable session must
//! dial exactly once (one `connecting` + one `connected` event, nothing
//! after) — every extra pair is one "flicker" in the UI.
//!
//! Also documents the expected churn when the connection dies mid-life:
//! the next command re-dials exactly once (+2 events), then stays quiet.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{File, FileAttributes, Handle, Name, Status, StatusCode, Version};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use mygitui_lib::remote::hostkeys::HostKeyStore;
use mygitui_lib::remote::profiles::{self, AuthKind, DeviceAuth, DeviceProfile};
use mygitui_lib::remote::prompts;
use mygitui_lib::remote::transport::{DeviceRegistry, DEFAULT_EXEC_TIMEOUT};
use mygitui_lib::remote::workspace::RemoteWorkspace;

const DEVICE_ID: &str = "churn-test-device";
const HOME: &str = "/home/tester";

/// Deterministic xorshift RNG (rand_core 0.10 dropped `OsRng`; the server
/// key just needs to fill 32 bytes).
struct TestRng(u64);

impl rand_core::TryRng for TestRng {
    type Error = core::convert::Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok(self.try_next_u64()? as u32)
    }
    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        Ok(x)
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        for chunk in dst.chunks_mut(8) {
            let x = self.try_next_u64()?;
            let bytes = x.to_le_bytes();
            let len = chunk.len();
            chunk.copy_from_slice(&bytes[..len]);
        }
        Ok(())
    }
}
impl rand_core::TryCryptoRng for TestRng {}

/// In-process SSH server: password auth accepted, execs answer with one
/// stdout line, `sftp` serves one small directory. `kill_after` makes the
/// Nth session-channel open fail hard (connection dies) to simulate a
/// dropped socket; `refuse_at` politely REJECTS the Nth open (connection
/// stays alive — a session limit like MaxSessions).
struct FakeSsh {
    execs: Arc<AtomicUsize>,
    opens: Arc<AtomicUsize>,
    kill_after: usize,
    kill_consumed: Arc<AtomicBool>,
    refuse_at: usize,
    /// Refuse EVERY session-channel open (toggleable mid-test): a device
    /// whose session cap is exhausted / wedged.
    refuse_all: Arc<AtomicBool>,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl FakeSsh {
    fn new(kill_after: usize, refuse_at: usize) -> Self {
        Self {
            execs: Arc::new(AtomicUsize::new(0)),
            opens: Arc::new(AtomicUsize::new(0)),
            kill_after,
            kill_consumed: Arc::new(AtomicBool::new(false)),
            refuse_at,
            refuse_all: Arc::new(AtomicBool::new(false)),
            channels: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl russh::server::Server for FakeSsh {
    type Handler = FakeSsh;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
        // Channel state is per-connection; the exec/open counters are
        // SHARED so the test can observe them and so kill_after/refuse_at
        // count global opens (one drop / one refusal per test, not one per
        // re-dialed connection).
        FakeSsh {
            execs: self.execs.clone(),
            opens: self.opens.clone(),
            kill_after: self.kill_after,
            kill_consumed: self.kill_consumed.clone(),
            refuse_at: self.refuse_at,
            refuse_all: self.refuse_all.clone(),
            channels: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl russh::server::Handler for FakeSsh {
    type Error = russh::Error;

    async fn auth_password(&mut self, _: &str, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let open_no = self.opens.fetch_add(1, Ordering::SeqCst) + 1;
        if self.refuse_all.load(Ordering::SeqCst) {
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        }
        if open_no > self.kill_after && !self.kill_consumed.swap(true, Ordering::SeqCst) {
            // Hard-fail the CONNECTION exactly once (not just the channel):
            // the client must see its cached handle die; the re-dialed
            // connection stays healthy.
            return Err(russh::Error::Inconsistent);
        }
        if open_no == self.refuse_at {
            // Polite SSH refusal: connection stays alive.
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.execs.fetch_add(1, Ordering::SeqCst);
        let cmd = String::from_utf8_lossy(data).to_string();
        let out = if cmd.contains("$HOME") { HOME } else { "main" };
        let _ = session.data(channel, out);
        let _ = session.exit_status_request(channel, 0);
        let _ = session.close(channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            let _ = session.channel_failure(channel_id);
            return Ok(());
        }
        let channel = self.channels.lock().await.remove(&channel_id).unwrap();
        let _ = session.channel_success(channel_id);
        russh_sftp::server::run(channel.into_stream(), FakeSftp { readdir_calls: 0 }).await;
        Ok(())
    }
}

/// One-directory SFTP server: each `opendir` gets two entries, then EOF.
struct FakeSftp {
    readdir_calls: usize,
}

impl russh_sftp::server::Handler for FakeSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn opendir(&mut self, id: u32, _path: String) -> Result<Handle, Self::Error> {
        self.readdir_calls = 0;
        Ok(Handle {
            id,
            handle: "dir".to_string(),
        })
    }

    async fn readdir(&mut self, id: u32, _handle: String) -> Result<Name, Self::Error> {
        if self.readdir_calls > 0 {
            return Err(StatusCode::Eof);
        }
        self.readdir_calls += 1;
        let mut dir = FileAttributes::dummy();
        dir.set_dir(true);
        let mut file = FileAttributes::dummy();
        file.size = Some(5);
        Ok(Name {
            id,
            files: vec![
                File {
                    filename: "project".to_string(),
                    longname: String::new(),
                    attrs: dir,
                },
                File {
                    filename: "README.md".to_string(),
                    longname: String::new(),
                    attrs: file,
                },
            ],
        })
    }

    async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_string(),
            language_tag: "en-US".to_string(),
        })
    }
}

/// Bind an embedded server on an ephemeral port; returns
/// (port, host-key algo, host-key fingerprint, server state) so the test
/// can pre-pin the key exactly as the client sees it.
async fn spawn_server(
    kill_after: usize,
    refuse_at: usize,
) -> (
    u16,
    String,
    String,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Arc<AtomicBool>,
) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let key = russh::keys::PrivateKey::random(
        &mut TestRng(0x9E3779B97F4A7C15),
        russh::keys::Algorithm::Ed25519,
    )
    .unwrap();
    let algo = key.algorithm().as_str().to_string();
    let fingerprint = key
        .public_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();
    let config = Arc::new(russh::server::Config {
        auth_rejection_time: std::time::Duration::from_millis(1),
        auth_rejection_time_initial: Some(std::time::Duration::from_millis(1)),
        keys: vec![key],
        ..Default::default()
    });
    let mut ssh = FakeSsh::new(kill_after, refuse_at);
    let execs = ssh.execs.clone();
    let opens = ssh.opens.clone();
    let refuse_all = ssh.refuse_all.clone();
    tokio::spawn(async move {
        let server = ssh.run_on_socket(config, &listener);
        server.await.unwrap();
    });
    (port, algo, fingerprint, execs, opens, refuse_all)
}

fn test_profile(port: u16) -> DeviceProfile {
    DeviceProfile {
        id: DEVICE_ID.to_string(),
        name: "churn test".to_string(),
        host: "127.0.0.1".to_string(),
        port,
        username: "tester".to_string(),
        auth: DeviceAuth {
            kind: AuthKind::Password,
            key_path: None,
            has_passphrase: false,
        },
        poll_secs: 5,
    }
}

/// Counted `device-status` emissions (the events the FE chip reacts to).
fn status_counter() -> Arc<AtomicUsize> {
    let counter = Arc::new(AtomicUsize::new(0));
    let seen = counter.clone();
    prompts::init(move |event, _payload| {
        if event == "device-status" {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });
    counter
}

/// Route the keyring crate's one-time store selection to the in-memory mock
/// instead of the OS secret service. The platform store (Secret Service on
/// Linux) needs a desktop session that CI runners don't provide — storing a
/// secret there fails with "SS error: result not returned from SS API".
///
/// Must run BEFORE the first `keyring::Entry::new`: that first call commits
/// the process to the platform store (`LazyLock`, cached forever), so we
/// trigger it with a warm-up entry and then replace the default. Only affects
/// this test binary — each integration test is its own process.
fn install_mock_keyring() {
    let _ = keyring::Entry::new("mygitui-mock-warmup", "warmup");
    let mock = keyring_core::mock::Store::new().expect("mock keyring store");
    keyring_core::set_default_store(mock);
}

/// Shared setup: trusted key, stored password, connected session.
/// `counter` must be installed (via [`status_counter`]) BEFORE this runs —
/// the first dial's two events fire during `session_for`.
async fn connect_device(name: &str, port: u16, algo: &str, fingerprint: &str) -> DeviceRegistry {
    let dir = std::env::temp_dir().join(format!("mygitui-churn-{}-{name}", std::process::id()));
    std::fs::create_dir_all(dir.join("remote")).unwrap();
    let store = Arc::new(HostKeyStore::under_dir(&dir));
    store
        .trust(&format!("127.0.0.1:{port}"), algo, fingerprint)
        .unwrap();
    profiles::set_secret(DEVICE_ID, "password", "pw").unwrap();
    let registry = DeviceRegistry::new();
    registry
        .session_for(&test_profile(port), &store)
        .await
        .expect("initial connect");
    registry
}

/// One test (not two): the event emitter is a process-global OnceLock, so
/// both scenarios must share it — expectations below are DELTAS.
#[tokio::test]
async fn status_events_match_dials() {
    install_mock_keyring();
    let counter = status_counter();

    // ---- Phase 1: healthy browse workload dials exactly once ------------
    let (port, algo, fingerprint, execs, opens, _refuse_all) =
        spawn_server(usize::MAX, usize::MAX).await;
    let registry = connect_device("browse", port, &algo, &fingerprint).await;
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "dial = connecting + connected"
    );

    let session = registry.peek(DEVICE_ID).unwrap();

    // Sequential execs (the browse path: home probe, git probes).
    for _ in 0..5 {
        session
            .exec("printf '%s' \"$HOME\"", DEFAULT_EXEC_TIMEOUT)
            .await
            .expect("exec");
    }
    assert_eq!(execs.load(Ordering::SeqCst), 5);
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "sequential execs must not re-dial (this is the flicker)"
    );

    // The SFTP browse itself (DeviceBrowser's auto-browse on mount) — the
    // workspace opens a fresh SFTP channel per operation.
    for round in 0..3 {
        let listing = tokio::task::spawn_blocking({
            let workspace = RemoteWorkspace::new(session.clone(), HOME.to_string());
            move || workspace.list_dir("", false)
        })
        .await
        .expect("browse task")
        .expect("list_dir");
        assert_eq!(listing.entries.len(), 2, "round {round}");
    }
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "SFTP browsing must not re-dial (this is the flicker)"
    );

    // Concurrent execs (FE fires browse + scan + probes together).
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..8 {
        let session = session.clone();
        tasks.spawn(async move {
            session
                .exec(&format!("echo {i}"), DEFAULT_EXEC_TIMEOUT)
                .await
                .expect("concurrent exec")
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap().stdout_str();
    }
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "concurrent execs must not re-dial either"
    );

    // Channel reuse: 5 + 8 exec channels + 3 gitlink execs + exactly ONE
    // SFTP session for the 3 browse rounds (the device session caches it;
    // per-op channels made some servers refuse the burst).
    assert_eq!(
        execs.load(Ordering::SeqCst),
        16,
        "5+8 execs + 3 gitlink probes"
    );
    assert_eq!(
        opens.load(Ordering::SeqCst),
        17,
        "one SFTP channel for 3 rounds"
    );
    let _ = profiles::delete_secrets(DEVICE_ID);

    // ---- Phase 2: a dropped connection re-dials exactly once ------------
    // Server kills the connection on the 2nd session-channel open.
    let (port, algo, fingerprint, _execs, _opens, _refuse_all) = spawn_server(1, usize::MAX).await;
    let registry = connect_device("drop", port, &algo, &fingerprint).await;
    assert_eq!(counter.load(Ordering::SeqCst), 4, "second dial adds 2");

    let session = registry.peek(DEVICE_ID).unwrap();
    // First exec opens channel #1 — fine.
    session
        .exec("echo one", DEFAULT_EXEC_TIMEOUT)
        .await
        .expect("first exec");
    assert_eq!(counter.load(Ordering::SeqCst), 4, "no churn while healthy");

    // Connection is dead now; the next command must re-dial ONCE (2 more
    // events) and then stay quiet for all following commands.
    session
        .exec("echo two", DEFAULT_EXEC_TIMEOUT)
        .await
        .expect("exec after drop");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        6,
        "a dropped connection costs exactly one re-dial pair"
    );
    session
        .exec("echo three", DEFAULT_EXEC_TIMEOUT)
        .await
        .expect("exec after redial");
    assert_eq!(counter.load(Ordering::SeqCst), 6, "quiet after re-dial");

    // ---- Phase 3: the server REFUSES one channel (session limit) --------
    // Connection stays alive; the client must NOT re-dial (no status
    // churn) and the cached handle must survive for the next command.
    let (port, algo, fingerprint, _execs, _opens, _refuse_all) = spawn_server(usize::MAX, 2).await;
    let registry = connect_device("refuse", port, &algo, &fingerprint).await;
    assert_eq!(counter.load(Ordering::SeqCst), 8, "third dial adds 2");

    let session = registry.peek(DEVICE_ID).unwrap();
    session
        .exec("echo one", DEFAULT_EXEC_TIMEOUT)
        .await
        .expect("exec before refusal");
    assert_eq!(counter.load(Ordering::SeqCst), 8);

    let err = session
        .exec("echo refused", DEFAULT_EXEC_TIMEOUT)
        .await
        .expect_err("refused channel surfaces as an error");
    assert!(
        err.contains("refused the session channel"),
        "unexpected error: {err}"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        8,
        "a channel refusal must NOT re-dial (this is the flicker)"
    );

    // The cached handle is still alive: next command works quietly.
    session
        .exec("echo three", DEFAULT_EXEC_TIMEOUT)
        .await
        .expect("exec after refusal");
    assert_eq!(counter.load(Ordering::SeqCst), 8, "quiet after refusal");

    let _ = profiles::delete_secrets(DEVICE_ID);

    // ---- Phase 4: N concurrent sftp() callers share ONE channel ---------
    // The observed flake: a scan/panel burst fires many sftp() calls at
    // once; without single-flighting each opens its own channel and
    // servers with a session cap refuse the herd outright.
    let (port, algo, fingerprint, _execs, opens, _refuse_all) =
        spawn_server(usize::MAX, usize::MAX).await;
    let registry = connect_device("herd", port, &algo, &fingerprint).await;
    assert_eq!(counter.load(Ordering::SeqCst), 10, "fourth dial adds 2");
    assert_eq!(opens.load(Ordering::SeqCst), 0, "dial opens no channels");

    let session = registry.peek(DEVICE_ID).unwrap();
    let mut herd = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let session = session.clone();
        herd.spawn(async move { session.sftp().await });
    }
    let mut opened = 0usize;
    while let Some(result) = herd.join_next().await {
        result.unwrap().expect("concurrent sftp");
        opened += 1;
    }
    assert_eq!(opened, 8, "every caller got a session");
    assert_eq!(
        opens.load(Ordering::SeqCst),
        1,
        "single-flight: 8 concurrent callers = 1 channel open"
    );
    let _ = profiles::delete_secrets(DEVICE_ID);

    // ---- Phase 5: a device refusing ALL opens costs at most 2 opens ----
    // One sftp() winner (attempt + retry), then the cooldown makes the
    // queued callers fail fast — no per-caller opens, no hammering.
    let (port, algo, fingerprint, _execs, opens, refuse_all) =
        spawn_server(usize::MAX, usize::MAX).await;
    refuse_all.store(true, Ordering::SeqCst);
    let registry = connect_device("cooldown", port, &algo, &fingerprint).await;
    assert_eq!(counter.load(Ordering::SeqCst), 12, "fifth dial adds 2");

    let session = registry.peek(DEVICE_ID).unwrap();
    let mut herd = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let session = session.clone();
        herd.spawn(async move { session.sftp().await });
    }
    let mut refused = 0usize;
    while let Some(result) = herd.join_next().await {
        let err = match result.unwrap() {
            Ok(_) => panic!("every open must be refused"),
            Err(err) => err,
        };
        assert!(err.contains("refus"), "unexpected error: {err}");
        refused += 1;
    }
    assert_eq!(refused, 4);
    assert_eq!(
        opens.load(Ordering::SeqCst),
        2,
        "winner = attempt + retry; queued callers hit the cooldown, not the server"
    );

    // During the cooldown further calls fail fast without touching the
    // device.
    let err = match session.sftp().await {
        Ok(_) => panic!("cooldown must fail fast"),
        Err(err) => err,
    };
    assert!(err.contains("cooldown"), "unexpected error: {err}");
    assert_eq!(opens.load(Ordering::SeqCst), 2, "cooldown = zero opens");

    // invalidate() clears the cooldown (fresh epoch): with refusals lifted
    // the next call opens a channel again.
    refuse_all.store(false, Ordering::SeqCst);
    session.sftp_invalidate();
    session.sftp().await.expect("open after cooldown clears");
    assert_eq!(opens.load(Ordering::SeqCst), 3);

    let _ = profiles::delete_secrets(DEVICE_ID);
}
