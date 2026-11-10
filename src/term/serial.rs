#![cfg(unix)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::json;
use serial2::{CharSize, FlowControl, Parity, SerialPort, Settings, StopBits};

use crate::term::buffer::{BufferRead, DEFAULT_CAPACITY, RingBuffer};
use crate::term::journal::{TermJournal, TermJournalDir};
use crate::term::{ExitStatus, TermError, check_input_size, exit, validate_dimensions};

const READ_TIMEOUT: Duration = Duration::from_millis(100);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub const ERROR_MISSING: &str = "serial_not_found";
pub const ERROR_PERMISSION: &str = "serial_permission";
pub const ERROR_BUSY: &str = "serial_busy";
pub const ERROR_INVALID: &str = "serial_invalid_settings";
pub const ERROR_NEEDS_USBIPD_ATTACH: &str = "needs_usbipd_attach";
// Kept so the host/client error-kind interner still recognizes historical
// usbipd errors from an older host. New serial opens never emit these.
pub const ERROR_USBIPD_NOT_FOUND: &str = "usbipd_not_found";
pub const ERROR_USBIPD_LIST: &str = "usbipd_list_failed";
pub const ERROR_USBIPD_BUSID: &str = "usbipd_busid_missing";
pub const ERROR_USBIPD_ATTACH: &str = "usbipd_attach_failed";

pub use crate::term::{SerialConfig, SerialFlow, SerialParity};

pub type SerialSettings = SerialConfig;
pub type SerialOptions = SerialConfig;

impl SerialConfig {
    pub fn normalized_device(&self) -> PathBuf {
        normalize_device(&self.device)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialEnv {
    pub wsl2: bool,
    pub trusted_busid: Option<String>,
}

impl Default for SerialEnv {
    fn default() -> Self {
        Self::detect()
    }
}

impl SerialEnv {
    pub fn detect() -> Self {
        Self {
            wsl2: is_wsl2(),
            trusted_busid: None,
        }
    }
}

pub fn normalize_device(device: &Path) -> PathBuf {
    let raw = device.to_string_lossy();
    let upper = raw.to_ascii_uppercase();
    if let Some(number) = upper.strip_prefix("COM")
        && !number.is_empty()
        && number.chars().all(|ch| ch.is_ascii_digit())
    {
        return PathBuf::from(format!("/dev/ttyS{number}"));
    }
    device.to_path_buf()
}

pub fn is_wsl2() -> bool {
    if let Ok(value) = std::env::var("DMUXAI_SERIAL_WSL2") {
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => return true,
            "0" | "false" | "no" => return false,
            _ => {}
        }
    }
    let Ok(version) = fs::read_to_string("/proc/version") else {
        return false;
    };
    let version = version.to_ascii_lowercase();
    version.contains("microsoft") || version.contains("wsl")
}

fn manual_hint(device: &Path) -> String {
    format!(
        "serial device {} not found. On WSL2 attach the USB device from Windows: usbipd list; usbipd attach --wsl --busid <id>. On WSL1, COMn is /dev/ttySn.",
        device.display()
    )
}

fn serial_error(code: i32, kind: &'static str, message: String) -> TermError {
    TermError {
        code,
        message,
        error_kind: Some(kind),
        data: None,
    }
}

pub fn missing_device_error(device: &Path) -> TermError {
    serial_error(exit::NOT_FOUND, ERROR_MISSING, manual_hint(device))
}

pub fn permission_error(device: &Path) -> TermError {
    serial_error(
        exit::FORBIDDEN,
        ERROR_PERMISSION,
        format!(
            "permission denied on {}. Add yourself to the dialout group: sudo usermod -aG dialout $USER, then log in again (WSL: wsl --shutdown).",
            device.display()
        ),
    )
}

pub fn busy_error(device: &Path) -> TermError {
    let message = match find_process_using(device) {
        Some((pid, comm)) => format!("device is in use by pid {pid} ({comm})"),
        None => format!("device is in use: {}", device.display()),
    };
    serial_error(exit::CONFLICT, ERROR_BUSY, message)
}

pub fn map_open_error(device: &Path, error: &io::Error) -> TermError {
    match error.raw_os_error() {
        Some(libc::ENOENT) | Some(libc::ENOTDIR) | Some(libc::ENODEV) | Some(libc::ENXIO) => {
            missing_device_error(device)
        }
        Some(libc::EACCES) | Some(libc::EPERM) => permission_error(device),
        Some(libc::EBUSY) | Some(libc::ETXTBSY) => busy_error(device),
        _ => TermError::internal(format!(
            "could not open serial device {}: {error}",
            device.display()
        )),
    }
}

pub fn is_lost_io_error(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EIO) | Some(libc::ENXIO))
}

#[cfg(target_os = "linux")]
fn find_process_using(device: &Path) -> Option<(u32, String)> {
    let target = fs::canonicalize(device).unwrap_or_else(|_| device.to_path_buf());
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(fds) = fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(link) = fs::read_link(fd.path()) else {
                continue;
            };
            let matches = link == device
                || link == target
                || fs::canonicalize(&link).is_ok_and(|path| path == target);
            if matches {
                let comm = fs::read_to_string(entry.path().join("comm"))
                    .ok()
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "unknown".to_owned());
                return Some((pid, comm));
            }
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn find_process_using(_device: &Path) -> Option<(u32, String)> {
    None
}

fn char_size(data: u8) -> CharSize {
    match data {
        5 => CharSize::Bits5,
        6 => CharSize::Bits6,
        7 => CharSize::Bits7,
        _ => CharSize::Bits8,
    }
}

fn stop_bits(stop: u8) -> StopBits {
    if stop == 2 {
        StopBits::Two
    } else {
        StopBits::One
    }
}

fn parity(parity: SerialParity) -> Parity {
    match parity {
        SerialParity::None => Parity::None,
        SerialParity::Even => Parity::Even,
        SerialParity::Odd => Parity::Odd,
    }
}

fn flow_control(flow: SerialFlow) -> FlowControl {
    match flow {
        SerialFlow::None => FlowControl::None,
        SerialFlow::RtsCts => FlowControl::RtsCts,
    }
}

fn open_port(config: &SerialConfig, device: &Path) -> io::Result<SerialPort> {
    let mut port = SerialPort::open(device, |mut settings: Settings| {
        settings.set_raw();
        settings.set_baud_rate(config.baud)?;
        settings.set_char_size(char_size(config.data));
        settings.set_stop_bits(stop_bits(config.stop));
        settings.set_parity(parity(config.parity));
        settings.set_flow_control(flow_control(config.flow));
        Ok(settings)
    })?;
    port.set_read_timeout(READ_TIMEOUT)?;
    port.set_write_timeout(WRITE_TIMEOUT)?;
    Ok(port)
}

pub fn open(
    config: &SerialConfig,
    journal: Option<Arc<Mutex<TermJournal>>>,
) -> Result<SerialBackend, TermError> {
    open_with_env(config, &SerialEnv::detect(), journal)
}

pub fn open_serial(
    config: &SerialConfig,
    journal: Option<Arc<Mutex<TermJournal>>>,
) -> Result<SerialBackend, TermError> {
    open(config, journal)
}

pub fn open_with_env(
    config: &SerialConfig,
    env: &SerialEnv,
    journal: Option<Arc<Mutex<TermJournal>>>,
) -> Result<SerialBackend, TermError> {
    config.validate()?;
    let device = config.normalized_device();
    match open_port(config, &device) {
        Ok(port) => Ok(SerialBackend::start(config, device, port, journal)),
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) && env.wsl2 => {
            let busid = resolve_busid(config.usbipd_busid.as_deref(), env.trusted_busid.as_deref());
            Err(needs_usbipd_attach_error(&device, busid.as_deref()))
        }
        Err(error) => Err(map_open_error(&device, &error)),
    }
}

pub fn resolve_busid(explicit: Option<&str>, trusted: Option<&str>) -> Option<String> {
    for value in [explicit, trusted].into_iter().flatten() {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_owned());
        }
    }
    None
}

fn needs_usbipd_attach_error(device: &Path, busid: Option<&str>) -> TermError {
    let path = device.display();
    let (message, data) = match busid {
        Some(busid) => (
            format!(
                "serial device {path} not found. On WSL2, run these commands in an elevated Windows PowerShell (usbipd-win): `usbipd bind --busid {busid}` (first time only), then `usbipd attach --wsl --busid {busid}`. dmuxai does not run usbipd."
            ),
            json!({
                "busid": busid,
                "commands": [
                    format!("usbipd bind --busid {busid}"),
                    format!("usbipd attach --wsl --busid {busid}"),
                ],
            }),
        ),
        None => (
            format!(
                "serial device {path} not found. On WSL2, run these commands in an elevated Windows PowerShell (usbipd-win): `usbipd list` to find the busid, then `usbipd bind --busid <busid>` (first time only), then `usbipd attach --wsl --busid <busid>`. dmuxai does not run usbipd."
            ),
            json!({
                "commands": [
                    "usbipd list",
                    "usbipd bind --busid <busid>",
                    "usbipd attach --wsl --busid <busid>",
                ],
            }),
        ),
    };
    TermError {
        code: exit::NOT_FOUND,
        message,
        error_kind: Some(ERROR_NEEDS_USBIPD_ATTACH),
        data: Some(data),
    }
}

pub struct SerialBackend {
    port: Option<Arc<SerialPort>>,
    device: PathBuf,
    buffer: Arc<RingBuffer>,
    last_output_ms: Arc<AtomicU64>,
    journal: Option<Arc<Mutex<TermJournal>>>,
    lost: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    reader: Option<thread::JoinHandle<()>>,
}

impl SerialBackend {
    pub fn spawn(
        config: &SerialConfig,
        journal: Option<Arc<Mutex<TermJournal>>>,
    ) -> Result<Self, TermError> {
        open(config, journal)
    }

    pub fn open(
        config: &SerialConfig,
        journal: Option<Arc<Mutex<TermJournal>>>,
    ) -> Result<Self, TermError> {
        open(config, journal)
    }

    fn start(
        _config: &SerialConfig,
        device: PathBuf,
        port: SerialPort,
        journal: Option<Arc<Mutex<TermJournal>>>,
    ) -> Self {
        let port = Arc::new(port);
        let buffer = Arc::new(RingBuffer::new(DEFAULT_CAPACITY));
        let last_output_ms = Arc::new(AtomicU64::new(now_ms()));
        let lost = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let reader_port = Arc::clone(&port);
        let reader_buffer = Arc::clone(&buffer);
        let reader_last_output = Arc::clone(&last_output_ms);
        let reader_lost = Arc::clone(&lost);
        let reader_closed = Arc::clone(&closed);
        let reader_journal = journal.clone();
        let name = device.display().to_string();
        let reader = thread::Builder::new()
            .name(format!("term-serial-{name}"))
            .spawn(move || {
                let mut chunk = vec![0u8; 8192];
                loop {
                    if reader_closed.load(Ordering::Relaxed) {
                        break;
                    }
                    match reader_port.read(&mut chunk) {
                        Ok(0) => {
                            mark_lost(&reader_buffer, &reader_lost, &reader_journal);
                            break;
                        }
                        Ok(read) => {
                            let data = &chunk[..read];
                            let time = now_ms();
                            let seq = reader_buffer.push(data);
                            reader_last_output.store(time, Ordering::Relaxed);
                            if let Some(journal) = &reader_journal
                                && let Ok(mut journal) = journal.lock()
                            {
                                let text = String::from_utf8_lossy(data);
                                let _ = journal.record_output(seq, time, "child", None, &text);
                                let _ = journal.flush_output_due(time);
                            }
                        }
                        Err(error)
                            if error.kind() == io::ErrorKind::Interrupted
                                || error.kind() == io::ErrorKind::TimedOut
                                || error.kind() == io::ErrorKind::WouldBlock => {}
                        Err(_) => {
                            mark_lost(&reader_buffer, &reader_lost, &reader_journal);
                            break;
                        }
                    }
                }
            })
            .ok();

        Self {
            port: Some(port),
            device,
            buffer,
            last_output_ms,
            journal,
            lost,
            closed,
            reader,
        }
    }

    pub fn device(&self) -> &Path {
        &self.device
    }

    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    pub fn pid(&self) -> Option<u32> {
        None
    }

    pub fn seq(&self) -> u64 {
        self.buffer.seq()
    }

    pub fn buffer(&self) -> Arc<RingBuffer> {
        Arc::clone(&self.buffer)
    }

    pub fn last_output_ms(&self) -> u64 {
        self.last_output_ms.load(Ordering::Relaxed)
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.buffer.exit_status()
    }

    pub fn read_since(&self, since: u64, raw: bool) -> BufferRead {
        self.buffer.read_since(since, raw)
    }

    pub fn read_tail(&self, lines: usize, raw: bool) -> BufferRead {
        self.buffer.read_tail(lines, raw)
    }

    pub fn send(&self, data: &[u8], secret: bool) -> Result<u64, TermError> {
        check_input_size(data)?;
        let seq = self.buffer.seq();
        let port = self
            .port
            .as_ref()
            .ok_or_else(|| TermError::internal("serial device is closed"))?;
        port.write_all(data).map_err(|error| {
            TermError::internal(format!("could not write to the serial device: {error}"))
        })?;
        if let Some(journal) = &self.journal
            && let Ok(mut journal) = journal.lock()
        {
            let text = String::from_utf8_lossy(data);
            let _ = journal.record_input(seq, now_ms(), "cli", None, &text, secret);
        }
        Ok(seq)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), TermError> {
        validate_dimensions(cols, rows)?;
        self.record_event(&format!("resize {cols}x{rows}"));
        Ok(())
    }

    pub fn close(&self, _force: bool) {
        self.closed.store(true, Ordering::Relaxed);
    }

    pub fn release(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
        self.port.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        mark_lost(&self.buffer, &self.lost, &self.journal);
    }

    pub fn journal(&self) -> Option<Arc<Mutex<TermJournal>>> {
        self.journal.clone()
    }

    pub fn journal_error(&self) -> Option<String> {
        self.journal.as_ref().and_then(|journal| {
            journal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .last_error()
                .map(str::to_owned)
        })
    }

    pub fn record_event(&self, data: &str) {
        if let Some(journal) = &self.journal
            && let Ok(mut journal) = journal.lock()
        {
            let _ = journal.record_event(
                self.buffer.seq(),
                now_ms(),
                TermJournalDir::Out,
                "cli",
                None,
                data,
            );
        }
    }
}

impl std::fmt::Debug for SerialBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialBackend")
            .field("device", &self.device)
            .field("lost", &self.is_lost())
            .finish_non_exhaustive()
    }
}

impl Drop for SerialBackend {
    fn drop(&mut self) {
        self.release();
    }
}

fn mark_lost(buffer: &RingBuffer, lost: &AtomicBool, journal: &Option<Arc<Mutex<TermJournal>>>) {
    lost.store(true, Ordering::Relaxed);
    buffer.set_exited(ExitStatus::default());
    if let Some(journal) = journal
        && let Ok(mut journal) = journal.lock()
    {
        let _ = journal.flush_output();
        let _ = journal.record_event(
            buffer.seq(),
            now_ms(),
            TermJournalDir::Out,
            "child",
            None,
            "state lost",
        );
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    const TEST_ENV: [&str; 5] = [
        "DMUXAI_USBIPD_BIN",
        "DMUXAI_USBIPD_ATTACH",
        "DMUXAI_TERM_USBIPD_ATTACH",
        "DMUXAI_NO_USBIPD_ATTACH",
        "DMUXAI_SERIAL_WSL2",
    ];

    fn with_path<R>(path: &Path, action: impl FnOnce() -> R) -> R {
        let _guard = crate::state_path::test_env_lock();
        let previous: Vec<(&str, Option<std::ffi::OsString>)> = TEST_ENV
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        for name in &TEST_ENV[1..] {
            unsafe { std::env::remove_var(*name) };
        }
        unsafe {
            std::env::set_var("DMUXAI_USBIPD_BIN", path.join("usbipd.exe"));
        }
        let result = action();
        for (name, value) in previous {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
        result
    }

    fn fake_usbipd(dir: &Path, script: &str) -> PathBuf {
        let path = dir.join("usbipd.exe");
        fs::write(&path, script).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn tc_2611_wsl1_com_device_maps_to_tty_s() {
        assert_eq!(
            normalize_device(Path::new("COM3")),
            PathBuf::from("/dev/ttyS3")
        );
        assert_eq!(
            normalize_device(Path::new("com12")),
            PathBuf::from("/dev/ttyS12")
        );
        assert_eq!(
            normalize_device(Path::new("/dev/ttyUSB0")),
            PathBuf::from("/dev/ttyUSB0")
        );
    }

    #[test]
    fn tc_2611_missing_device_error_uses_fr2611_text() {
        let path = Path::new("/dev/dmuxai-missing-serial");
        let error = map_open_error(path, &io::Error::from_raw_os_error(libc::ENOENT));
        assert_eq!(error.code, exit::NOT_FOUND);
        assert_eq!(error.error_kind, Some(ERROR_MISSING));
        assert!(error.message.contains("/dev/dmuxai-missing-serial"));
        assert!(error.message.contains("usbipd attach --wsl --busid"));
        assert!(error.message.contains("/dev/ttySn"));
    }

    #[test]
    fn tc_2611_permission_error_mentions_dialout_and_wsl_shutdown() {
        let path = Path::new("/dev/ttyUSB0");
        let error = map_open_error(path, &io::Error::from_raw_os_error(libc::EACCES));
        assert_eq!(error.code, exit::FORBIDDEN);
        assert_eq!(error.error_kind, Some(ERROR_PERMISSION));
        assert!(error.message.contains("dialout"));
        assert!(error.message.contains("wsl --shutdown"));
        assert!(!error.message.contains("chmod 666"));
    }

    #[test]
    fn tc_2611_busy_error_names_the_holding_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("serial");
        let _held = fs::File::create(&path).unwrap();
        let error = map_open_error(&path, &io::Error::from_raw_os_error(libc::EBUSY));
        assert_eq!(error.code, exit::CONFLICT);
        assert_eq!(error.error_kind, Some(ERROR_BUSY));
        assert!(error.message.starts_with("device is in use by pid "));
        assert!(
            error
                .message
                .contains(std::process::id().to_string().as_str())
        );
    }

    #[test]
    fn tc_2611_reader_recognizes_eio_and_enxio_as_lost() {
        assert!(is_lost_io_error(&io::Error::from_raw_os_error(libc::EIO)));
        assert!(is_lost_io_error(&io::Error::from_raw_os_error(libc::ENXIO)));
        assert!(!is_lost_io_error(&io::Error::from_raw_os_error(
            libc::EACCES
        )));
    }

    #[test]
    fn tc_2614_busid_precedence_is_explicit_then_trusted() {
        assert_eq!(
            resolve_busid(Some("9-9"), Some("1-2")).as_deref(),
            Some("9-9")
        );
        assert_eq!(resolve_busid(None, Some("1-2")).as_deref(), Some("1-2"));
        assert_eq!(
            resolve_busid(Some("  "), Some("1-2")).as_deref(),
            Some("1-2")
        );
        assert_eq!(resolve_busid(None, None), None);
    }

    #[test]
    fn tc_2614_wsl2_without_busid_reports_guidance_without_running_usbipd() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usbipd.log");
        fake_usbipd(
            dir.path(),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 0\n",
                log.display()
            ),
        );
        let device = dir.path().join("missing-device");
        let config = SerialConfig::new(&device, 115_200);
        let env = SerialEnv {
            wsl2: true,
            trusted_busid: None,
        };
        let error = with_path(dir.path(), || {
            open_with_env(&config, &env, None).expect_err("no explicit busid")
        });
        assert_eq!(error.code, exit::NOT_FOUND);
        assert_eq!(error.error_kind, Some(ERROR_NEEDS_USBIPD_ATTACH));
        assert!(
            error.message.contains(&device.display().to_string()),
            "hint must name the real device path: {}",
            error.message
        );
        assert!(error.message.contains("usbipd list"));
        assert!(error.message.contains("<busid>"));
        assert!(error.message.contains("first time only"));
        assert!(error.message.contains("elevated Windows PowerShell"));

        let data = error.data.expect("guidance data");
        assert!(data.get("busid").is_none());
        assert_eq!(
            data["commands"],
            serde_json::json!([
                "usbipd list",
                "usbipd bind --busid <busid>",
                "usbipd attach --wsl --busid <busid>",
            ])
        );
        assert!(
            !log.exists(),
            "usbipd must not be invoked without a known busid"
        );
    }

    #[test]
    fn tc_2614_wsl2_with_busid_reports_guidance_without_running_usbipd() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usbipd.log");
        fake_usbipd(
            dir.path(),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 0\n",
                log.display()
            ),
        );
        let device = dir.path().join("missing-device");
        let mut config = SerialConfig::new(&device, 115_200);
        config.usbipd_busid = Some("1-2".to_owned());
        let env = SerialEnv {
            wsl2: true,
            trusted_busid: None,
        };
        let error = with_path(dir.path(), || {
            open_with_env(&config, &env, None).expect_err("device is still missing")
        });
        assert_eq!(error.code, exit::NOT_FOUND);
        assert_eq!(error.error_kind, Some(ERROR_NEEDS_USBIPD_ATTACH));
        assert!(
            error.message.contains(&device.display().to_string()),
            "hint must name the real device path: {}",
            error.message
        );
        assert!(error.message.contains("usbipd bind --busid 1-2"));
        assert!(error.message.contains("usbipd attach --wsl --busid 1-2"));
        assert!(!error.message.contains("usbipd list"));
        assert!(error.message.contains("first time only"));
        assert!(error.message.contains("elevated Windows PowerShell"));

        let data = error.data.expect("guidance data");
        assert_eq!(data["busid"], "1-2");
        assert_eq!(
            data["commands"],
            serde_json::json!(["usbipd bind --busid 1-2", "usbipd attach --wsl --busid 1-2",])
        );
        assert!(
            !log.exists(),
            "usbipd must not be invoked even with a known busid"
        );
    }

    #[test]
    fn tc_2614_trusted_busid_names_the_id_in_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usbipd.log");
        fake_usbipd(
            dir.path(),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 0\n",
                log.display()
            ),
        );
        let device = dir.path().join("missing-device");
        let config = SerialConfig::new(&device, 115_200);
        let env = SerialEnv {
            wsl2: true,
            trusted_busid: Some("3-1".to_owned()),
        };
        let error = with_path(dir.path(), || {
            open_with_env(&config, &env, None).expect_err("trusted busid only names the id")
        });
        assert_eq!(error.code, exit::NOT_FOUND);
        assert_eq!(error.error_kind, Some(ERROR_NEEDS_USBIPD_ATTACH));
        assert!(error.message.contains("usbipd bind --busid 3-1"));
        assert!(error.message.contains("usbipd attach --wsl --busid 3-1"));
        assert_eq!(
            error.data.expect("guidance data")["busid"],
            serde_json::Value::String("3-1".to_owned())
        );
        assert!(!log.exists(), "a trusted busid must not run usbipd");
    }

    #[test]
    fn tc_2614_no_usbipd_attach_is_a_noop_for_wsl_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usbipd.log");
        fake_usbipd(
            dir.path(),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 0\n",
                log.display()
            ),
        );
        let device = dir.path().join("missing-device");
        let mut config = SerialConfig::new(&device, 115_200);
        config.usbipd_busid = Some("9-99".to_owned());
        config.no_usbipd_attach = true;
        let env = SerialEnv {
            wsl2: true,
            trusted_busid: None,
        };
        let error = with_path(dir.path(), || {
            open_with_env(&config, &env, None).expect_err("device missing")
        });
        assert_eq!(error.code, exit::NOT_FOUND);
        assert_eq!(error.error_kind, Some(ERROR_NEEDS_USBIPD_ATTACH));
        assert_eq!(error.data.expect("guidance data")["busid"], "9-99");
        assert!(!log.exists(), "the deprecated no-op must not invoke usbipd");
    }

    #[test]
    fn tc_2614_attach_env_does_not_change_wsl_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usbipd.log");
        fake_usbipd(
            dir.path(),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 0\n",
                log.display()
            ),
        );
        let device = dir.path().join("missing-device");
        let mut config = SerialConfig::new(&device, 115_200);
        config.usbipd_busid = Some("1-2".to_owned());
        let env = SerialEnv {
            wsl2: true,
            trusted_busid: None,
        };
        let error = with_path(dir.path(), || {
            unsafe {
                std::env::set_var("DMUXAI_USBIPD_ATTACH", "0");
                std::env::set_var("DMUXAI_NO_USBIPD_ATTACH", "1");
            }
            open_with_env(&config, &env, None).expect_err("device missing")
        });
        assert_eq!(error.error_kind, Some(ERROR_NEEDS_USBIPD_ATTACH));
        assert!(
            !log.exists(),
            "attach-disable env vars are not preconditions"
        );
    }

    #[test]
    fn tc_2614_no_attach_defaults_false_and_serializes_for_the_host() {
        let config: SerialConfig =
            serde_json::from_str(r#"{"device":"/dev/ttyUSB0","baud":115200}"#).unwrap();
        assert!(!config.no_usbipd_attach);
        let mut config = config;
        config.no_usbipd_attach = true;
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["no_usbipd_attach"], true);
    }

    #[test]
    fn tc_2614_non_wsl_missing_path_does_not_run_usbipd() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("usbipd.log");
        fake_usbipd(
            dir.path(),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 0\n",
                log.display()
            ),
        );
        let device = dir.path().join("missing-device");
        let config = SerialConfig::new(&device, 115_200);
        let env = SerialEnv {
            wsl2: false,
            trusted_busid: None,
        };
        let error = with_path(dir.path(), || {
            open_with_env(&config, &env, None).expect_err("device missing")
        });
        assert_eq!(error.code, exit::NOT_FOUND);
        assert_eq!(error.error_kind, Some(ERROR_MISSING));
        assert!(
            error.message.contains(&device.display().to_string()),
            "missing-path message must name the path: {}",
            error.message
        );
        assert!(error.data.is_none());
        assert!(!log.exists(), "usbipd must not run outside WSL2");
    }

    #[test]
    fn tc_2613_serial_backend_marks_lost_when_master_closes() {
        let mut master = -1;
        let mut slave = -1;
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(result, 0);
        let path = fs::read_link(format!("/proc/self/fd/{slave}")).unwrap();
        let config = SerialConfig::new(&path, 115_200);
        let env = SerialEnv {
            wsl2: false,
            trusted_busid: None,
        };
        let backend = open_with_env(&config, &env, None).expect("open pty slave");
        unsafe { libc::close(master) };
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && !backend.is_lost() {
            thread::sleep(Duration::from_millis(20));
        }
        let lost = backend.is_lost();
        unsafe { libc::close(slave) };
        assert!(
            lost,
            "closing the PTY master must mark the serial backend lost"
        );
    }
}
