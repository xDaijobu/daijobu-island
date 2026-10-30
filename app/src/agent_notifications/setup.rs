//! Serialized opt-in setup. Development tests use temporary configs and fake versions.
use super::*;
use dj_core::term_agents::Agent;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

pub struct Runtime {
    pub controller: AgentNotifications,
    pub providers: Vec<VerifiedProvider>,
    pub executable: PathBuf,
}

impl Runtime {
    pub fn launch(&mut self, tab: TabId, agent: Agent) -> Result<(u64, String), String> {
        let verified = self
            .providers
            .iter()
            .find(|p| p.provider().agent() == agent)
            .copied()
            .ok_or("Agent notifications unavailable; see Settings > Terminal")?;
        let env = self.controller.prepare(tab, verified)?;
        let script = dj_core::agent_notifications::powershell_launch(
            &env.grant,
            &env.endpoint,
            self.executable
                .to_str()
                .ok_or("bridge path is not Unicode")?,
        );
        Ok((env.grant.generation, script))
    }
}

pub struct Ready {
    pub epoch: u64,
    pub runtime: Option<Runtime>,
    pub status: Vec<String>,
}

enum Request {
    Configure(u64, bool),
    Stop,
}
#[derive(Default)]
struct Shared {
    request: Option<Request>,
    result: Option<Ready>,
    stopping: bool,
}

pub struct SetupWorker {
    shared: Arc<Mutex<Shared>>,
    tx: mpsc::SyncSender<()>,
    thread: Option<JoinHandle<()>>,
    epoch: u64,
    pub status: Vec<String>,
}

impl SetupWorker {
    pub fn new(executable: PathBuf, hwnd: isize) -> Self {
        let paths = Paths::user();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let worker_shared = shared.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("agent-hook-setup".into())
            .spawn(move || {
                while rx.recv().is_ok() {
                    let request = worker_shared.lock().unwrap().request.take();
                    match request {
                        Some(Request::Configure(epoch, enabled)) => {
                            let result = configure(
                                epoch,
                                enabled,
                                &paths,
                                &executable,
                                hwnd,
                                ipc::provider_version,
                            );
                            worker_shared.lock().unwrap().result = Some(result);
                            dj_win::window::wake(hwnd);
                        }
                        Some(Request::Stop) => break,
                        None => {}
                    }
                    let state = worker_shared.lock().unwrap();
                    if state.stopping && state.request.is_none() {
                        break;
                    }
                }
            })
            .ok();
        let status = if thread.is_some() {
            vec!["Agent notifications are off".into()]
        } else {
            vec!["Agent notification setup is unavailable".into()]
        };
        Self {
            shared,
            tx,
            thread,
            epoch: 0,
            status,
        }
    }

    pub fn request(&mut self, enabled: bool) {
        self.epoch = self.epoch.checked_add(1).expect("setup epoch exhausted");
        self.status = vec![if enabled {
            "Setting up agent notifications...".into()
        } else {
            "Removing Daijobu Island notification hooks...".into()
        }];
        if self.thread.is_none() {
            self.status = vec!["Agent notification setup is unavailable".into()];
            return;
        }
        // Coalesce repeated edits; at most one desired configuration and one result.
        self.shared.lock().unwrap().request = Some(Request::Configure(self.epoch, enabled));
        let _ = self.tx.try_send(());
    }

    pub fn take(&mut self) -> Option<Ready> {
        let result = self.shared.lock().unwrap().result.take()?;
        if result.epoch != self.epoch {
            return None;
        }
        self.status = result.status.clone();
        Some(result)
    }
}

impl Drop for SetupWorker {
    fn drop(&mut self) {
        // Quitting retains the user's opt-in hooks. Only a Configure(false) removes them.
        {
            let mut state = self.shared.lock().unwrap();
            state.stopping = true;
            if state.request.is_none() {
                state.request = Some(Request::Stop);
            }
        }
        let _ = self.tx.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
struct Paths {
    configs: [PathBuf; 2],
    receipts: [PathBuf; 2],
}
impl Paths {
    fn user() -> Self {
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_default();
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"));
        let codex = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let data = crate::storage::data_dir();
        Self {
            configs: [claude.join("settings.json"), codex.join("hooks.json")],
            receipts: [
                data.join("agent-hooks-claude.json"),
                data.join("agent-hooks-codex.json"),
            ],
        }
    }
}

fn configure(
    epoch: u64,
    enabled: bool,
    paths: &Paths,
    executable: &Path,
    hwnd: isize,
    probe: impl Fn(Provider) -> Result<String, String>,
) -> Ready {
    let mut providers = Vec::new();
    let mut status = Vec::new();
    for (i, provider) in [Provider::Claude, Provider::Codex].into_iter().enumerate() {
        if !enabled {
            status.push(match remove(&paths.receipts[i]) {
                Ok(()) => format!("{}: notification hooks removed", provider.name()),
                Err(_) => format!("{}: cleanup failed; settings were kept", provider.name()),
            });
            continue;
        }
        let verified = probe(provider)
            .ok()
            .and_then(|version| VerifiedProvider::from_version_output(provider, &version).ok());
        let Some(verified) = verified else {
            status.push(format!(
                "{}: unavailable (missing or unverified version)",
                provider.name()
            ));
            continue;
        };
        match install(
            &paths.configs[i],
            &paths.receipts[i],
            &HookPlan::new(verified, executable),
        ) {
            Ok(()) => {
                providers.push(verified);
                status.push(match provider {
                    Provider::Claude => "Claude: hooks installed for new agent tabs".into(),
                    Provider::Codex => "Codex: review /hooks, then open a new agent tab".into(),
                });
            }
            Err(_) => status.push(format!(
                "{}: setup failed; existing settings were kept",
                provider.name()
            )),
        }
    }
    if enabled {
        status.push("Codex question alerts unavailable; approval alerts supported".into());
    }
    let runtime = if enabled && !providers.is_empty() {
        match AgentNotifications::start(move || dj_win::window::wake(hwnd)) {
            Ok(controller) => Some(Runtime {
                controller,
                providers,
                executable: executable.into(),
            }),
            Err(_) => {
                status.push("Local agent notification bridge unavailable".into());
                None
            }
        }
    } else {
        None
    };
    Ready {
        epoch,
        runtime,
        status,
    }
}

#[derive(Serialize, Deserialize)]
struct SavedReceipt {
    path: PathBuf,
    receipts: Vec<SetupReceipt>,
}

fn read(path: &Path) -> Result<Option<String>, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("config cannot be read".into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "config read failed")?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err("config too large".into());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "config is not UTF-8".into())
}

fn atomic_write(path: &Path, expected: Option<&str>, output: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| p.is_absolute())
        .ok_or("config path is not absolute")?;
    fs::create_dir_all(parent).map_err(|_| "config directory unavailable")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "cannot stage config")?;
    temp.write_all(output.as_bytes())
        .map_err(|_| "cannot write staged config")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "cannot sync staged config")?;
    if expected.is_some() {
        ipc::preserve_config_acl(path, temp.path())?;
    }
    if read(path)?.as_deref() != expected {
        return Err("config changed during setup; retry from Settings".into());
    }
    if expected.is_none() {
        temp.persist_noclobber(path)
            .map_err(|_| "config appeared during setup")?;
    } else {
        temp.persist(path)
            .map_err(|_| "cannot replace config atomically")?;
    }
    Ok(())
}

fn saved(path: &Path) -> Result<Option<(String, SavedReceipt)>, String> {
    let Some(text) = read(path)? else {
        return Ok(None);
    };
    let saved: SavedReceipt = serde_json::from_str(&text).map_err(|_| "invalid setup receipt")?;
    if saved.receipts.len() > 8 || !saved.path.is_absolute() {
        return Err("invalid setup receipt".into());
    }
    Ok(Some((text, saved)))
}

fn install(
    config: &Path,
    receipt_path: &Path,
    plan: &Result<HookPlan, String>,
) -> Result<(), String> {
    let plan = plan.as_ref().map_err(Clone::clone)?;
    if let Some((_, old)) = saved(receipt_path)? {
        if old.path != config {
            remove(receipt_path)?;
        }
    }
    let previous = saved(receipt_path)?;
    let before = read(config)?;
    let mut baseline = before.clone().unwrap_or_else(|| "{}".into());
    if let Some((_, old)) = &previous {
        for receipt in &old.receipts {
            baseline = uninstall_hooks(&baseline, receipt)?;
        }
    }
    let edit = plan.install(&baseline)?;
    if before.as_deref() == Some(edit.output.as_str()) {
        return Ok(());
    }
    let Some(receipt) = edit.receipt else {
        return Ok(());
    };
    let mut receipts = previous
        .as_ref()
        .map(|(_, s)| s.receipts.clone())
        .unwrap_or_default();
    if receipts.len() >= 8 {
        return Err("setup journal requires cleanup".into());
    }
    receipts.push(receipt.clone());
    // Journal both old and new ownership BEFORE changing provider config. A crash or
    // failed replacement leaves sufficient exact receipts to remove only our hooks.
    let journal = SavedReceipt {
        path: config.into(),
        receipts,
    };
    atomic_write(
        receipt_path,
        previous.as_ref().map(|(text, _)| text.as_str()),
        &serde_json::to_string_pretty(&journal).map_err(|_| "cannot serialize receipt")?,
    )?;
    atomic_write(config, before.as_deref(), &edit.output)?;
    let journal_text = read(receipt_path)?;
    let final_receipt = SavedReceipt {
        path: config.into(),
        receipts: vec![receipt],
    };
    // The larger journal remains a valid cleanup receipt if compaction fails.
    let _ = atomic_write(
        receipt_path,
        journal_text.as_deref(),
        &serde_json::to_string_pretty(&final_receipt).map_err(|_| "cannot serialize receipt")?,
    );
    Ok(())
}

fn remove(receipt_path: &Path) -> Result<(), String> {
    let Some((_, saved)) = saved(receipt_path)? else {
        return Ok(());
    };
    if let Some(before) = read(&saved.path)? {
        let mut after = before.clone();
        for receipt in &saved.receipts {
            after = uninstall_hooks(&after, receipt)?;
        }
        if after != before {
            atomic_write(&saved.path, Some(&before), &after)?;
        }
    }
    fs::remove_file(receipt_path).map_err(|_| "cannot remove setup receipt".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Paths, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            configs: [
                dir.path().join("Claude Space/settings.json"),
                dir.path().join("Codex's 你好/hooks.json"),
            ],
            receipts: [
                dir.path().join("state/claude.json"),
                dir.path().join("state/codex.json"),
            ],
        };
        let exe = dir.path().join("App's & Unicode 你好/daijobu.exe");
        (dir, paths, exe)
    }

    #[test]
    fn fixture_only_setup_disable_and_upgrade_preserve_third_party_config() {
        let (_dir, paths, exe) = fixture();
        let original = r#"{"permissions":{"deny":["Bash(rm *)"]},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"third-party.exe"}]}]},"unknown":true}"#;
        for config in &paths.configs {
            fs::create_dir_all(config.parent().unwrap()).unwrap();
            fs::write(config, original).unwrap();
        }
        let probe = |p: Provider| Ok(p.verified_version().into());
        let ready = configure(1, true, &paths, &exe, 0, probe);
        assert_eq!(ready.runtime.as_ref().unwrap().providers.len(), 2);
        drop(ready);
        let ready = configure(
            2,
            true,
            &paths,
            &exe.with_file_name("updated.exe"),
            0,
            probe,
        );
        drop(ready);
        let disabled = configure(3, false, &paths, &exe, 0, probe);
        assert!(disabled.runtime.is_none());
        for config in &paths.configs {
            assert_eq!(
                parse_config(&fs::read_to_string(config).unwrap()).unwrap(),
                parse_config(original).unwrap()
            );
        }
        assert!(paths.receipts.iter().all(|p| !p.exists()));
    }

    #[test]
    fn unverified_versions_and_invalid_config_never_get_repaired() {
        let (_dir, paths, exe) = fixture();
        let ready = configure(1, true, &paths, &exe, 0, |_| Ok("unverified".into()));
        assert!(ready.runtime.is_none());
        assert!(paths.configs.iter().all(|p| !p.exists()));
        fs::create_dir_all(paths.configs[0].parent().unwrap()).unwrap();
        fs::write(&paths.configs[0], "invalid secret config").unwrap();
        let ready = configure(2, true, &paths, &exe, 0, |p| {
            Ok(p.verified_version().into())
        });
        assert_eq!(
            fs::read_to_string(&paths.configs[0]).unwrap(),
            "invalid secret config"
        );
        assert!(ready
            .status
            .iter()
            .any(|s| s.contains("Claude: setup failed")));
        assert!(!paths.receipts[0].exists());
        drop(ready);
        configure(3, false, &paths, &exe, 0, |_| unreachable!());
    }

    #[test]
    fn concurrent_edit_aborts_atomic_replace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "user changed this").unwrap();
        assert!(atomic_write(&path, Some("old"), "replacement").is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "user changed this");
    }
}
