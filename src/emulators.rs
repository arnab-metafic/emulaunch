use crate::config;

use std::fmt;
use std::process::Stdio;

// State constants
pub const STATE_BOOTED: &str = "Booted";
pub const STATE_SHUTDOWN: &str = "Shutdown";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const STATE_AVAILABLE: &str = "Available";

// Device types
pub const DEVICE_TYPE_AVD: &str = "AVD";
pub const DEVICE_TYPE_RUNNING: &str = "Running Device";

// Error messages
pub const ERR_IOS_ONLY_MACOS: &str = "iOS simulators are only available on macOS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
  Android,
  Ios,
}

impl Platform {
  /// Heading for this platform's group in lists.
  pub fn section_title(self) -> &'static str {
    match self {
      Platform::Android => "Android Emulators",
      Platform::Ios => "iOS Simulators",
    }
  }

  /// What a device of this platform is called in the detail view.
  pub fn kind(self) -> &'static str {
    match self {
      Platform::Android => "Android Emulator",
      Platform::Ios => "iOS Simulator",
    }
  }

  pub fn id_label(self) -> &'static str {
    match self {
      Platform::Android => "ID",
      Platform::Ios => "UDID",
    }
  }

  /// Label for [`Device::meta`]: the AVD kind on Android, the runtime on iOS.
  pub fn meta_label(self) -> &'static str {
    match self {
      Platform::Android => "Type",
      Platform::Ios => "Runtime",
    }
  }
}

/// An Android emulator (AVD) or iOS simulator.
#[derive(Debug, Clone)]
pub struct Device {
  pub platform: Platform,
  /// Display name.
  pub name: String,
  /// What the platform tooling identifies it by: the AVD id or the simulator UDID.
  pub id: String,
  pub state: String,
  /// Platform-specific extra: the AVD kind on Android, the runtime on iOS.
  pub meta: String,
}

impl Device {
  /// Launch this device.
  pub fn open(&self) -> Result<String, String> {
    match self.platform {
      Platform::Android => open_android_emulator(&self.id),
      Platform::Ios => open_ios_simulator(&self.id),
    }
  }
}

impl fmt::Display for Device {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{} [{}] ({})", self.name, self.state, self.meta)
  }
}

/// A row of the TUI list: either a platform heading or a device.
pub enum EmulatorEntry {
  SectionHeader(String),
  Device(Device),
}

impl EmulatorEntry {
  pub fn display_name(&self) -> &str {
    match self {
      EmulatorEntry::SectionHeader(s) => s,
      EmulatorEntry::Device(d) => &d.name,
    }
  }

  pub fn is_header(&self) -> bool {
    matches!(self, EmulatorEntry::SectionHeader(_))
  }
}

fn get_android_emulator_cmd() -> Result<String, String> {
  config::get_android_emulator_cmd().map_err(|e: config::CommandNotFoundError| e.to_string())
}

fn get_adb_cmd() -> Result<String, String> {
  config::get_adb_cmd().map_err(|e: config::CommandNotFoundError| e.to_string())
}

#[cfg(target_os = "macos")]
fn get_xcrun_cmd() -> Result<String, String> {
  config::get_xcrun_cmd().map_err(|e: config::CommandNotFoundError| e.to_string())
}

/// Directory holding AVD definitions, following the Android tooling rules:
/// `$ANDROID_AVD_HOME`, else `$ANDROID_USER_HOME/avd`, else `~/.android/avd`.
fn avd_home() -> Option<std::path::PathBuf> {
  let env_dir = |var| std::env::var_os(var).filter(|v| !v.is_empty());
  if let Some(dir) = env_dir("ANDROID_AVD_HOME") {
    return Some(dir.into());
  }
  if let Some(dir) = env_dir("ANDROID_USER_HOME") {
    return Some(std::path::PathBuf::from(dir).join("avd"));
  }
  dirs::home_dir().map(|h| h.join(".android").join("avd"))
}

/// Read the display name from an AVD's config.ini
fn get_avd_display_name(avd_id: &str) -> Option<String> {
  let config_path = avd_home()?
    .join(format!("{}.avd", avd_id))
    .join("config.ini");
  let contents = std::fs::read_to_string(config_path).ok()?;
  contents
    .lines()
    .find(|line| line.starts_with("avd.ini.displayname="))
    .and_then(|line| line.strip_prefix("avd.ini.displayname="))
    .map(|s| s.trim().to_string())
}

/// Get the set of AVD names that are currently running via adb
fn get_running_avd_names() -> Result<Vec<String>, String> {
  let adb_cmd = get_adb_cmd()?;

  let output = std::process::Command::new(&adb_cmd)
    .args(["devices"])
    .output()
    .map_err(|e| format!("Failed to run adb devices: {}", e))?;

  if !output.status.success() {
    return Ok(Vec::new());
  }

  let stdout = String::from_utf8_lossy(&output.stdout);
  let serials: Vec<String> = stdout
    .lines()
    .skip(1)
    .filter_map(|line| {
      let parts: Vec<&str> = line.split_whitespace().collect();
      if parts.len() >= 2 && parts[1] == "device" && parts[0].starts_with("emulator-") {
        Some(parts[0].to_string())
      } else {
        None
      }
    })
    .collect();

  let names = serials
    .iter()
    .filter_map(|serial| {
      let result = std::process::Command::new(&adb_cmd)
        .args(["-s", serial, "emu", "avd", "name"])
        .output()
        .ok()?;
      if result.status.success() {
        let stdout = String::from_utf8_lossy(&result.stdout);
        stdout.lines().next().map(|s| s.trim().to_string())
      } else {
        None
      }
    })
    .collect();

  Ok(names)
}

/// List AVDs by scanning ~/.android/avd/ directory
fn list_avds_from_directory() -> Result<Vec<Device>, String> {
  let avd_dir = avd_home().ok_or_else(|| "Cannot determine AVD directory".to_string())?;

  let entries =
    std::fs::read_dir(&avd_dir).map_err(|e| format!("Cannot read AVD directory: {}", e))?;

  let mut emulators = Vec::new();
  for entry in entries.flatten() {
    let path = entry.path();
    if path.extension().and_then(|e| e.to_str()) == Some("ini") && !path.is_dir() {
      if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        // Verify the .avd directory exists
        if avd_dir.join(format!("{}.avd", stem)).is_dir() {
          let display_name = get_avd_display_name(stem).unwrap_or_else(|| stem.to_string());
          emulators.push(Device {
            platform: Platform::Android,
            name: display_name,
            id: stem.to_string(),
            state: STATE_SHUTDOWN.to_string(),
            meta: DEVICE_TYPE_AVD.to_string(),
          });
        }
      }
    }
  }

  if emulators.is_empty() {
    Err(format!("No AVDs found in {}", avd_dir.display()))
  } else {
    Ok(emulators)
  }
}

pub fn list_android_emulators() -> Result<Vec<Device>, String> {
  let emulator_cmd = get_android_emulator_cmd()?;
  let running_names = get_running_avd_names().unwrap_or_default();

  let output = std::process::Command::new(&emulator_cmd)
    .arg("-list-avds")
    .output();

  let mut emulators = match output {
    Ok(result) if result.status.success() => {
      let stdout = String::from_utf8_lossy(&result.stdout);
      Ok(
        stdout
          .lines()
          .filter(|line| !line.is_empty())
          .map(|line| {
            let id = line.trim().to_string();
            let name = get_avd_display_name(&id).unwrap_or_else(|| id.clone());
            let state = if running_names.contains(&id) {
              STATE_BOOTED.to_string()
            } else {
              STATE_SHUTDOWN.to_string()
            };
            Device {
              platform: Platform::Android,
              name,
              id,
              state,
              meta: DEVICE_TYPE_AVD.to_string(),
            }
          })
          .collect(),
      )
    }
    Ok(_) | Err(_) => {
      // emulator command failed or not found — try scanning AVD directory, then adb
      list_avds_from_directory().or_else(|_| list_android_devices_via_adb())
    }
  }?;

  // Sort: booted first
  emulators.sort_by(|a, b| {
    let a_booted = a.state == STATE_BOOTED;
    let b_booted = b.state == STATE_BOOTED;
    b_booted.cmp(&a_booted)
  });

  Ok(emulators)
}

fn list_android_devices_via_adb() -> Result<Vec<Device>, String> {
  let adb_cmd = get_adb_cmd()?;

  let output = std::process::Command::new(&adb_cmd)
    .args(["devices", "-l"])
    .output()
    .map_err(|e| format!("Failed to run adb command: {}", e))?;

  if !output.status.success() {
    return Err(format!(
      "adb devices failed: {}",
      String::from_utf8_lossy(&output.stderr)
    ));
  }

  let stdout = String::from_utf8_lossy(&output.stdout);
  Ok(
    stdout
      .lines()
      .skip(1)
      .filter(|line| !line.is_empty() && line.contains("device"))
      .filter_map(|line| {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let id = parts.first().map(|s| s.to_string())?;
        let name = parts
          .iter()
          .find(|p| p.starts_with("model:"))
          .and_then(|p| p.strip_prefix("model:"))
          .map(|s| s.to_string())
          .unwrap_or_else(|| id.clone());

        Some(Device {
          platform: Platform::Android,
          name,
          id,
          state: STATE_BOOTED.to_string(),
          meta: DEVICE_TYPE_RUNNING.to_string(),
        })
      })
      .collect(),
  )
}

#[cfg(target_os = "macos")]
pub fn list_ios_simulators() -> Result<Vec<Device>, String> {
  let xcrun = get_xcrun_cmd()?;

  let output = std::process::Command::new(&xcrun)
    .args(["simctl", "list", "devices", "available", "--json"])
    .output()
    .map_err(|e| format!("Failed to run xcrun simctl: {}", e))?;

  if !output.status.success() {
    return Err(format!(
      "xcrun simctl failed: {}",
      String::from_utf8_lossy(&output.stderr)
    ));
  }

  let json = String::from_utf8_lossy(&output.stdout);
  parse_ios_simulators(&json)
}

#[cfg(not(target_os = "macos"))]
pub fn list_ios_simulators() -> Result<Vec<Device>, String> {
  Err(ERR_IOS_ONLY_MACOS.to_string())
}

/// Turn a simctl runtime identifier such as
/// `com.apple.CoreSimulator.SimRuntime.iOS-17-0` into `iOS 17.0`. Any
/// non-numeric trailing part (device-specific runtimes such as
/// `iOS-27-2-iPhone19-4`) is appended after a dash. Unrecognised identifiers
/// are returned unchanged.
#[cfg(target_os = "macos")]
fn pretty_runtime(identifier: &str) -> String {
  let Some(rest) = identifier.strip_prefix("com.apple.CoreSimulator.SimRuntime.") else {
    return identifier.to_string();
  };
  let mut parts = rest.split('-');
  let platform = parts.next().unwrap_or_default();
  let parts: Vec<&str> = parts.collect();
  let numeric = parts
    .iter()
    .take_while(|p| p.chars().all(|c| c.is_ascii_digit()))
    .count();
  let version = parts[..numeric].join(".");
  let suffix = parts[numeric..].join("-");
  match (version.is_empty(), suffix.is_empty()) {
    (true, true) => platform.to_string(),
    (true, false) => format!("{} - {}", platform, suffix),
    (false, true) => format!("{} {}", platform, version),
    (false, false) => format!("{} {} - {}", platform, version, suffix),
  }
}

#[cfg(target_os = "macos")]
fn parse_ios_simulators(json: &str) -> Result<Vec<Device>, String> {
  #[derive(serde::Deserialize)]
  struct DevicesResponse {
    devices: serde_json::Value,
  }

  let response: DevicesResponse =
    serde_json::from_str(json).map_err(|e| format!("Failed to parse simctl JSON: {}", e))?;

  let mut simulators = Vec::new();

  if let Some(devices_map) = response.devices.as_object() {
    for (runtime, devices) in devices_map {
      if let Some(device_list) = devices.as_array() {
        for device in device_list {
          let Some(name) = device.get("name").and_then(|v| v.as_str()) else {
            continue;
          };
          let Some(udid) = device.get("udid").and_then(|v| v.as_str()) else {
            continue;
          };
          let Some(state) = device.get("state").and_then(|v| v.as_str()) else {
            continue;
          };

          if matches!(state, STATE_BOOTED | STATE_SHUTDOWN | STATE_AVAILABLE) {
            simulators.push(Device {
              platform: Platform::Ios,
              name: name.to_string(),
              id: udid.to_string(),
              state: state.to_string(),
              meta: pretty_runtime(runtime),
            });
          }
        }
      }
    }
  }

  Ok(simulators)
}

/// Detach the child from our terminal's process group (and, on Windows, from
/// our console) so closing the terminal or pressing Ctrl+C does not kill it.
fn detach(cmd: &mut std::process::Command) {
  #[cfg(unix)]
  {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
  }
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
  }
}

/// How long to watch a freshly spawned emulator for an immediate failure.
const LAUNCH_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// Last few lines of the emulator log, for error messages.
fn log_tail(path: &std::path::Path) -> String {
  let contents = std::fs::read_to_string(path).unwrap_or_default();
  let lines: Vec<&str> = contents.lines().collect();
  lines[lines.len().saturating_sub(8)..].join("\n")
}

pub fn open_android_emulator(avd: &str) -> Result<String, String> {
  let emulator_cmd = get_android_emulator_cmd()?;

  // A second `emulator -avd` for the same AVD refuses to start
  if get_running_avd_names()
    .unwrap_or_default()
    .iter()
    .any(|n| n == avd)
  {
    return Err(format!("Android emulator '{}' is already running", avd));
  }

  // Send output to a log file rather than a pipe: a pipe would break (SIGPIPE)
  // once we exit while the emulator keeps running.
  let log_path = std::env::temp_dir().join(format!("emulaunch-{}.log", avd));
  let log = std::fs::File::create(&log_path)
    .map_err(|e| format!("Cannot create log file {}: {}", log_path.display(), e))?;
  let log_err = log
    .try_clone()
    .map_err(|e| format!("Cannot open log file {}: {}", log_path.display(), e))?;

  let mut cmd = std::process::Command::new(&emulator_cmd);
  cmd
    .args(["-avd", avd])
    .stdin(Stdio::null())
    .stdout(log)
    .stderr(log_err);
  detach(&mut cmd);

  let mut child = cmd
    .spawn()
    .map_err(|e| format!("Failed to launch emulator '{}': {}", avd, e))?;

  // Real startup takes much longer than this, but configuration errors (bad
  // AVD, missing system image, ...) make the emulator exit almost immediately.
  let deadline = std::time::Instant::now() + LAUNCH_GRACE;
  while std::time::Instant::now() < deadline {
    match child.try_wait() {
      Ok(Some(status)) => {
        return Err(format!(
          "Emulator '{}' exited immediately ({}):\n{}\n(full log: {})",
          avd,
          status,
          log_tail(&log_path),
          log_path.display()
        ));
      }
      Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
      Err(_) => break,
    }
  }

  Ok(format!(
    "Launching Android emulator: {} (log: {})",
    avd,
    log_path.display()
  ))
}

/// Bring up the app that displays booted simulators and report whether one
/// could be opened.
///
/// Xcode 27 replaced Simulator.app with Device Hub, so try Simulator.app
/// first (older Xcode; it honours -CurrentDeviceUDID when it is not already
/// running) and fall back to Device Hub, which we cannot point at a device.
/// Apps are addressed by bundle identifier because the app name differs.
#[cfg(target_os = "macos")]
fn open_simulator_ui(udid: &str) -> bool {
  let open = |args: &[&str]| {
    std::process::Command::new("open")
      .args(args)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status()
      .map(|s| s.success())
      .unwrap_or(false)
  };
  open(&[
    "-b",
    "com.apple.iphonesimulator",
    "--args",
    "-CurrentDeviceUDID",
    udid,
  ]) || open(&["-b", "com.apple.dt.Devices"])
}

#[cfg(target_os = "macos")]
pub fn open_ios_simulator(udid: &str) -> Result<String, String> {
  let xcrun = get_xcrun_cmd()?;

  let boot_output = std::process::Command::new(&xcrun)
    .args(["simctl", "boot", udid])
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .output();

  match boot_output {
    Ok(result) => {
      let stderr = String::from_utf8_lossy(&result.stderr);
      if !result.status.success()
        && !stderr.contains("Unable to boot device in current state: Booted")
      {
        return Err(format!("Failed to boot simulator: {}", stderr));
      }
    }
    Err(e) => return Err(format!("Failed to run simctl boot: {}", e)),
  }

  if open_simulator_ui(udid) {
    Ok(format!("Opening iOS simulator: {}", udid))
  } else {
    Ok(format!(
      "Booted iOS simulator: {} (could not open Simulator or Device Hub)",
      udid
    ))
  }
}

#[cfg(not(target_os = "macos"))]
pub fn open_ios_simulator(_udid: &str) -> Result<String, String> {
  Err(ERR_IOS_ONLY_MACOS.to_string())
}

/// Find the single device whose display name or id equals `query`.
///
/// Fails when nothing matches, or when several do (the same name on both
/// platforms, or on two iOS runtimes) so we never launch the wrong one.
pub fn find_device(query: &str) -> Result<Device, String> {
  let mut matches: Vec<Device> = list_android_emulators()
    .unwrap_or_default()
    .into_iter()
    .chain(list_ios_simulators().unwrap_or_default())
    .filter(|d| d.name == query || d.id == query)
    .collect();

  match matches.len() {
    0 => Err(format!("Emulator '{}' not found", query)),
    1 => Ok(matches.remove(0)),
    _ => {
      let choices: Vec<String> = matches
        .iter()
        .map(|d| {
          format!(
            "  {} ({}, {}: {})",
            d.name,
            d.meta,
            d.platform.id_label(),
            d.id
          )
        })
        .collect();
      Err(format!(
        "'{}' matches {} devices; open one by its id instead:\n{}",
        query,
        matches.len(),
        choices.join("\n")
      ))
    }
  }
}

/// Result of gathering every platform's devices for the TUI.
pub struct Collected {
  pub entries: Vec<EmulatorEntry>,
  /// Why a platform could not be listed (e.g. a missing `adb`). Platforms that
  /// are simply unsupported on this OS are not reported.
  pub errors: Vec<String>,
}

/// Collect all emulators into a unified list with section headers
pub fn collect_all_entries() -> Collected {
  let mut entries = Vec::new();
  let mut errors = Vec::new();

  match list_android_emulators() {
    Ok(android) if !android.is_empty() => {
      entries.push(EmulatorEntry::SectionHeader(
        Platform::Android.section_title().to_string(),
      ));
      entries.extend(android.into_iter().map(EmulatorEntry::Device));
    }
    Ok(_) => {}
    Err(e) => errors.push(e),
  }

  match list_ios_simulators() {
    Ok(ios) if !ios.is_empty() => {
      entries.push(EmulatorEntry::SectionHeader(
        Platform::Ios.section_title().to_string(),
      ));
      entries.extend(ios.into_iter().map(EmulatorEntry::Device));
    }
    Ok(_) => {}
    Err(e) if e == ERR_IOS_ONLY_MACOS => {}
    Err(e) => errors.push(e),
  }

  Collected { entries, errors }
}

/// Format a plain text list for the `list` subcommand
pub fn format_emulator_list() -> String {
  let mut output = String::new();

  let sections = [
    (
      Platform::Android,
      list_android_emulators(),
      "No Android emulators found",
      "Android emulators",
    ),
    (
      Platform::Ios,
      list_ios_simulators(),
      "No iOS simulators found",
      "iOS simulators",
    ),
  ];
  for (i, (platform, result, none_found, error_prefix)) in sections.into_iter().enumerate() {
    if i > 0 {
      output.push('\n');
    }
    match result {
      Ok(devices) if !devices.is_empty() => {
        output.push_str(platform.section_title());
        output.push_str(":\n");
        for device in devices {
          output.push_str(&format!("  {}\n", device));
        }
      }
      Ok(_) => output.push_str(&format!("{}\n", none_found)),
      Err(e) => output.push_str(&format!("{} error: {}\n", error_prefix, e)),
    }
  }

  output
}
