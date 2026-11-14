use ratv::companion::{Event, MediaControlFlags};
use ratv::discovery::{self, ScanOptions};
use ratv::mrp::{Command, PlaybackState};
use ratv::{
    AppleTv, ClientOptions, Credentials, Endpoints, ExposeSecret, Key, PairOptions, PinPrompt,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Mutex, task::JoinHandle, time::timeout};

#[derive(Clone, Serialize, Deserialize)]
struct Device {
    id: String,
    name: String,
    address: IpAddr,
    companion: Option<SocketAddr>,
    airplay: Option<SocketAddr>,
    #[serde(default)]
    credentials: Option<String>,
}
impl Device {
    fn public(&self) -> Value {
        json!({"id":self.id,"name":self.name,"address":self.address})
    }
    fn endpoints(&self) -> Endpoints {
        Endpoints {
            companion: self.companion,
            airplay: self.airplay,
        }
    }
}
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    last_device: Option<String>,
    devices: BTreeMap<String, Device>,
}
#[derive(Default)]
struct Live {
    text: AtomicBool,
    controls: AtomicU32,
}
struct Session {
    tv: AppleTv,
    live: Arc<Live>,
    events: Option<JoinHandle<()>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(task) = self.events.take() {
            task.abort();
        }
    }
}
#[derive(Default)]
pub struct Remote {
    path: Option<PathBuf>,
    saved: Saved,
    found: BTreeMap<String, Device>,
    pairing: Option<(String, PinPrompt)>,
    session: Option<Session>,
}
pub type State = Mutex<Remote>;

impl Remote {
    pub fn initialize(&mut self, path: PathBuf) -> Result<(), String> {
        if self.path.is_some() {
            return Ok(());
        }
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        let file = path.join("native-state.json");
        if file.exists() {
            self.saved = serde_json::from_slice(&fs::read(file).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
        self.path = Some(path);
        Ok(())
    }
    fn persist(&self) -> Result<(), String> {
        let directory = self
            .path
            .as_ref()
            .ok_or("Application data path is unavailable")?;
        let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        serde_json::to_writer(&mut file, &self.saved).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        file.persist(directory.join("native-state.json"))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn legacy(&self, id: &str) -> Option<String> {
        let path = self.path.as_ref()?.join("credentials.json");
        let saved: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        saved
            .get(id)?
            .get("Companion")
            .or_else(|| saved.get(id)?.get("AirPlay"))?
            .as_str()
            .map(str::to_owned)
    }
    fn last_id(&self) -> Option<String> {
        self.saved.last_device.clone().or_else(|| {
            let value: Value = serde_json::from_slice(
                &fs::read(self.path.as_ref()?.join("preferences.json")).ok()?,
            )
            .ok()?;
            value.get("last_device")?.as_str().map(str::to_owned)
        })
    }
    async fn scan(&mut self, wanted: Option<&str>) -> Result<Value, String> {
        let mut options = ScanOptions::default();
        options.timeout = Duration::from_secs(3);
        if let Some(id) = wanted {
            let address = self.saved.devices.get(id).map(|d| d.address).or_else(|| {
                let v: Value = serde_json::from_slice(
                    &fs::read(self.path.as_ref()?.join("preferences.json")).ok()?,
                )
                .ok()?;
                v.get("last_address")?.as_str()?.parse().ok()
            });
            if let Some(address) = address {
                options.targets = vec![SocketAddr::new(address, 5353)];
            }
        }
        let devices = if let Some(id) = wanted {
            discovery::scan_until(&options, |d| {
                d.identifier() == Some(id) && Endpoints::from(d).companion.is_some()
            })
            .await
        } else {
            discovery::scan(&options).await
        }
        .map_err(|e| e.to_string())?;
        let own = if_addrs::get_if_addrs()
            .unwrap_or_default()
            .into_iter()
            .map(|i| i.ip())
            .collect::<Vec<_>>();
        self.found.clear();
        for device in devices {
            let Some(address) = device.addresses.first().copied() else {
                continue;
            };
            if address.is_loopback()
                || own.contains(&address)
                || device.model().is_some_and(|m| !m.starts_with("AppleTV"))
            {
                continue;
            }
            let endpoints = Endpoints::from(&device);
            if endpoints.companion.is_none() && endpoints.airplay.is_none() {
                continue;
            }
            let id = device.identifier().unwrap_or(&device.host).to_owned();
            let credentials = self
                .saved
                .devices
                .get(&id)
                .and_then(|d| d.credentials.clone())
                .or_else(|| self.legacy(&id));
            self.found.insert(
                id.clone(),
                Device {
                    id,
                    name: device.name().into(),
                    address,
                    companion: endpoints.companion,
                    airplay: endpoints.airplay,
                    credentials,
                },
            );
        }
        Ok(json!({"devices":self.found.values().map(Device::public).collect::<Vec<_>>()}))
    }
    async fn connect(&mut self, id: &str) -> Result<Value, String> {
        let device = self
            .found
            .get(id)
            .or_else(|| self.saved.devices.get(id))
            .cloned()
            .ok_or("Scan for devices again.")?;
        let Some(text) = device.credentials.as_ref() else {
            return Ok(json!({"paired":false}));
        };
        let credentials: Credentials = text
            .parse()
            .map_err(|_| "Stored pairing data is invalid. Pair the device again.".to_string())?;
        if let Some(old) = self.session.take() {
            let _ = timeout(Duration::from_millis(300), old.tv.close()).await;
        }
        let mut options = ClientOptions::default();
        options.name = "ATV Remote Desktop".into();
        options.timeout = Duration::from_secs(2);
        options.stall_timeout = Some(Duration::from_millis(800));
        let tv = timeout(
            Duration::from_secs(7),
            AppleTv::connect(&device.endpoints(), &credentials, &options),
        )
        .await
        .map_err(|_| "Connection timed out. Make sure your Apple TV is awake.".to_string())?
        .map_err(|e| e.to_string())?;
        let live = Arc::new(Live::default());
        let events = tv.companion().map(|client| {
            let mut events = client.events();
            let live = live.clone();
            let client = client.clone();
            tokio::spawn(async move {
                if let Ok(Ok(text)) = timeout(Duration::from_millis(500), client.text()).await {
                    live.text.store(text.is_some(), Ordering::Relaxed);
                }
                loop {
                    match events.recv().await {
                        Ok(Event::TextInputStarted) => live.text.store(true, Ordering::Relaxed),
                        Ok(Event::TextInputStopped) => live.text.store(false, Ordering::Relaxed),
                        Ok(Event::MediaControl(flags)) => {
                            live.controls.store(flags.0, Ordering::Relaxed)
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            live.text.store(false, Ordering::Relaxed);
                            live.controls.store(0, Ordering::Relaxed);
                        }
                        Err(_) => break,
                    }
                }
            })
        });
        self.session = Some(Session { tv, live, events });
        self.saved.last_device = Some(id.into());
        self.saved.devices.insert(id.into(), device.clone());
        self.persist()?;
        Ok(json!({"paired":true,"device":device.public()}))
    }
    pub async fn handle(&mut self, request: &Value) -> Result<Value, String> {
        match request["action"].as_str().unwrap_or("") {
            "scan" => self.scan(None).await,
            "reconnect" => {
                let Some(id) = self.last_id() else {
                    return Ok(json!({"paired":false}));
                };
                if self.saved.devices.contains_key(&id)
                    && let Ok(result) = self.connect(&id).await
                {
                    return Ok(result);
                }
                self.scan(Some(&id)).await?;
                if !self.found.contains_key(&id) {
                    self.scan(None).await?;
                }
                if self.found.contains_key(&id) {
                    self.connect(&id).await
                } else {
                    Ok(json!({"paired":false,"reason":"not_found"}))
                }
            }
            "connect" => {
                self.connect(request["id"].as_str().ok_or("Select a device first.")?)
                    .await
            }
            "pair_begin" => {
                let id = request["id"].as_str().ok_or("Select a device first.")?;
                let device = self.found.get(id).ok_or("Scan for devices again.")?;
                let mut options = PairOptions::default();
                options.name = "ATV Remote Desktop".into();
                let prompt = ratv::pair(&device.endpoints(), &options)
                    .await
                    .map_err(|e| e.to_string())?;
                self.pairing = Some((id.into(), prompt));
                Ok(json!({"ok":true}))
            }
            "pair_finish" => {
                let pin = request["pin"]
                    .as_str()
                    .ok_or("Enter the PIN shown on your TV.")?;
                if pin.len() != 4 || !pin.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("Enter a four-digit PIN.".into());
                }
                let (id, prompt) = self.pairing.take().ok_or("Start pairing first.")?;
                let credentials = prompt.finish(pin).await.map_err(|e| e.to_string())?;
                let device = self.found.get_mut(&id).ok_or("Scan for devices again.")?;
                device.credentials =
                    Some(credentials.to_secret_string().expose_secret().to_owned());
                self.saved.devices.insert(id.clone(), device.clone());
                self.saved.last_device = Some(id);
                self.persist()?;
                Ok(json!({"ok":true}))
            }
            _ => Err("Unknown action".into()),
        }
    }
    fn active(&self) -> Result<(AppleTv, Arc<Live>), String> {
        let session = self
            .session
            .as_ref()
            .filter(|s| s.tv.is_connected())
            .ok_or("No Apple TV is connected.")?;
        Ok((session.tv.clone(), session.live.clone()))
    }
}
fn context(tv: &AppleTv, live: &Live) -> Value {
    let playing = tv.now_playing().map(|rx| rx.borrow().clone());
    snapshot(tv.is_connected(), playing.as_ref(), live)
}
fn snapshot(connected: bool, playing: Option<&ratv::mrp::NowPlaying>, live: &Live) -> Value {
    if !connected {
        return json!({"connected":false,"skip_backward":false,"skip_forward":false,"text_input":false,"media":null});
    }
    let active = playing.as_ref().is_some_and(|p| {
        matches!(
            p.state,
            Some(PlaybackState::Playing | PlaybackState::Paused | PlaybackState::Seeking)
        )
    });
    let flags = MediaControlFlags(live.controls.load(Ordering::Relaxed));
    let supports = |command, flag| {
        playing.as_ref().is_some_and(|p| p.supports(command)) || flags.contains(flag)
    };
    let media=playing.as_ref().filter(|p|active&&(p.title.is_some()||p.artist.is_some()||p.album.is_some())).map(|p|json!({"title":p.title,"artist":p.artist,"album":p.album,"position":p.elapsed,"total_time":p.duration,"state":p.state.map(|s|format!("{s:?}"))}));
    json!({"connected":true,"skip_backward":active&&supports(Command::SkipBackward,MediaControlFlags::SKIP_BACKWARD),"skip_forward":active&&supports(Command::SkipForward,MediaControlFlags::SKIP_FORWARD),"text_input":live.text.load(Ordering::Relaxed),"media":media})
}
pub async fn dispatch(state: &State, path: PathBuf, request: Value) -> Result<Value, String> {
    let action = request["action"].as_str().unwrap_or("");
    if matches!(action, "context" | "command") {
        let active = state.lock().await.active();
        if action == "context" {
            return Ok(match active {
                Ok((tv, live)) => context(&tv, &live),
                Err(_) => {
                    json!({"connected":false,"skip_backward":false,"skip_forward":false,"text_input":false,"media":null})
                }
            });
        }
        let (tv, live) = active?;
        let result = match request["command"].as_str().unwrap_or("") {
            "up" => tv.press(Key::Up).await,
            "down" => tv.press(Key::Down).await,
            "left" => tv.press(Key::Left).await,
            "right" => tv.press(Key::Right).await,
            "select" => tv.press(Key::Select).await,
            "menu" => tv.press(Key::Menu).await,
            "home" => tv.press(Key::Home).await,
            "play_pause" => tv.play_pause().await,
            "skip_backward" | "skip_forward" => {
                let forward = request["command"] == "skip_forward";
                let available = context(&tv, &live);
                if available[if forward {
                    "skip_forward"
                } else {
                    "skip_backward"
                }] != true
                {
                    return Err("Seeking is not available for the current media.".into());
                }
                tv.skip(if forward { 10.0 } else { -10.0 }).await
            }
            "text" => {
                if !live.text.load(Ordering::Relaxed) {
                    return Err("Select a text field on your Apple TV first.".into());
                }
                tv.set_text(request["text"].as_str().unwrap_or("")).await
            }
            "power" => {
                let client = tv.companion().ok_or("Power control is unavailable.")?;
                if client
                    .power_state()
                    .await
                    .map_err(|e| e.to_string())?
                    .is_on()
                {
                    tv.turn_off().await
                } else {
                    tv.turn_on().await
                }
            }
            "mute" => {
                let volume = tv.volume().ok_or("Mute is not supported by this device.")?;
                let muted = volume
                    .borrow()
                    .muted
                    .ok_or("Mute is unavailable for the current output.")?;
                tv.set_muted(!muted).await
            }
            _ => return Err("Unknown command".into()),
        };
        result.map_err(|e| e.to_string())?;
        return Ok(json!({"ok":true}));
    }
    let mut remote = state.lock().await;
    remote.initialize(path)?;
    remote.handle(&request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device() -> Device {
        Device {
            id: "tv".into(),
            name: "Living Room".into(),
            address: "192.0.2.11".parse().unwrap(),
            companion: Some("192.0.2.11:49153".parse().unwrap()),
            airplay: Some("192.0.2.11:7000".parse().unwrap()),
            credentials: Some("secret".into()),
        }
    }
    #[test]
    fn device_response_never_exposes_pairing_keys() {
        let public = device().public();
        assert!(public.get("credentials").is_none());
        assert!(!public.to_string().contains("secret"));
    }
    #[test]
    fn persisted_endpoints_and_selection_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut remote = Remote::default();
        remote.initialize(directory.path().to_path_buf()).unwrap();
        remote.saved.last_device = Some("tv".into());
        remote.saved.devices.insert("tv".into(), device());
        remote.persist().unwrap();
        let mut restored = Remote::default();
        restored.initialize(directory.path().to_path_buf()).unwrap();
        assert_eq!(restored.last_id().as_deref(), Some("tv"));
        assert_eq!(restored.saved.devices["tv"].companion, device().companion);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(directory.path().join("native-state.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn controls_follow_playback_and_focus_without_network_calls() {
        let live = Live::default();
        live.controls
            .store(MediaControlFlags::SKIP_FORWARD, Ordering::Relaxed);
        let mut playing = ratv::mrp::NowPlaying::default();
        playing.state = Some(PlaybackState::Playing);
        playing.title = Some("Movie".into());
        assert_eq!(snapshot(true, Some(&playing), &live)["skip_forward"], true);
        assert_eq!(
            snapshot(true, Some(&playing), &live)["skip_backward"],
            false
        );
        assert_eq!(snapshot(true, Some(&playing), &live)["text_input"], false);
        live.text.store(true, Ordering::Relaxed);
        assert_eq!(snapshot(true, Some(&playing), &live)["text_input"], true);
        playing.state = Some(PlaybackState::Stopped);
        let idle = snapshot(true, Some(&playing), &live);
        assert_eq!(idle["skip_forward"], false);
        assert!(idle["media"].is_null());
        let disconnected = snapshot(false, Some(&playing), &live);
        assert_eq!(disconnected["text_input"], false);
        assert!(disconnected["media"].is_null());
    }
    #[tokio::test]
    #[ignore = "requires the owner's paired Apple TV on the local network"]
    async fn reconnect_real_saved_device() {
        let path = PathBuf::from(std::env::var("ATV_TEST_DATA").expect("ATV_TEST_DATA required"));
        let mut remote = Remote::default();
        remote.initialize(path).unwrap();
        let start = std::time::Instant::now();
        let result = remote.handle(&json!({"action":"reconnect"})).await.unwrap();
        assert_eq!(result["paired"], true);
        println!(
            "Native reconnect completed in {:.2}s",
            start.elapsed().as_secs_f64()
        );
        let session = remote.session.take().unwrap();
        assert!(session.tv.is_connected());
        session.tv.close().await.unwrap();
    }
}
