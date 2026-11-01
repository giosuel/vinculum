//! Files over Bluetooth: "Send files…" on a device, with its progress dialog, and the dialog for
//! a file a device sends. A thread talks to obexd on the session bus; received files are moved
//! from obexd's folder to ~/Downloads once complete.

use std::path::PathBuf;
use std::time::Duration;

use lituus_bt::{IncomingFile, Obex, ObexEvent, Transfer};
use tokio::sync::mpsc;

use crate::{FileRow, FilesView, MainWindow, model, update};

pub enum Command {
    Send {
        address: String,
        device: String,
        files: Vec<PathBuf>,
    },
    Cancel,
    /// The answer to the incoming file: accept it, and whether to always accept from its sender.
    Answer {
        accept: bool,
        always: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sending {
    pub device: String,
    pub transfers: Vec<Transfer>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Incoming {
    pub device: String,
    pub address: String,
    pub name: String,
    pub size: u64,
    pub kind: String,
}

#[derive(Default)]
pub struct State {
    pub sending: Option<Sending>,
    /// The dialog for the transfer is hidden; it carries on.
    pub hidden: bool,
    pub incoming: Option<Incoming>,
    pub always: bool,
    /// The last thing to tell: a file received, or why sending failed.
    pub note: Option<String>,
    commands: Option<mpsc::UnboundedSender<Command>>,
}

impl State {
    pub fn send(&self, command: Command) {
        if let Some(tx) = &self.commands {
            let _ = tx.send(command);
        }
    }
}

fn post(weak: &slint::Weak<MainWindow>, change: impl FnOnce(&mut State) + Send + 'static) {
    let w = weak.clone();
    let _ = weak.upgrade_in_event_loop(move |_| update(&w, |app| change(&mut app.files)));
}

/// Where received files go: `$XDG_DOWNLOAD_DIR`, else ~/Downloads.
pub fn downloads() -> PathBuf {
    std::env::var_os("XDG_DOWNLOAD_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Downloads"))
}

/// Asks for files (the desktop's dialog), then sends them.
pub fn choose_and_send(weak: slint::Weak<MainWindow>, address: String, device: String) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
        let chosen = runtime.block_on(crate::portal::open_files(&format!("Send files to {device}")));
        let w = weak.clone();
        let _ = weak.upgrade_in_event_loop(move |_| {
            update(&w, |app| match chosen {
                Ok(files) if !files.is_empty() => app.files.send(Command::Send { address, device, files }),
                Ok(_) => {}
                Err(e) => app.files.note = Some(format!("{e}.")),
            })
        });
    });
}

/// The name the Bluetooth page knows a device by.
fn device_name(app: &crate::App, address: &str) -> String {
    app.bt
        .snapshot
        .as_ref()
        .and_then(|s| s.devices.iter().find(|d| d.address.eq_ignore_ascii_case(address)))
        .map_or_else(|| address.to_owned(), |d| d.alias.clone())
}

/// Starts the thread; without obexd (or a session bus) files simply can't be sent or received.
pub fn spawn(weak: slint::Weak<MainWindow>, always: Vec<String>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    crate::APP.with(|app| app.borrow_mut().files.commands = Some(tx));
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
        runtime.block_on(async move {
            let Ok(obex) = Obex::session().await else { return };
            let mut agent = obex.register_agent().await.ok();
            let mut always = always;
            let mut waiting: Option<IncomingFile> = None;
            // Received transfers to move once complete: (transfer, name).
            let mut receiving: Vec<(String, String)> = Vec::new();
            // What's being sent, and its obexd session.
            let mut sending: Vec<Transfer> = Vec::new();
            let mut session: Option<String> = None;
            let mut tick = tokio::time::interval(Duration::from_millis(300));
            loop {
                tokio::select! {
                    Some(event) = async { match agent.as_mut() { Some(a) => a.recv().await, None => std::future::pending().await } } => match event {
                        ObexEvent::Push(file) => {
                            if always.iter().any(|a| a.eq_ignore_ascii_case(&file.address)) {
                                receiving.push((file.transfer.clone(), file.name.clone()));
                                let name = file.name.clone();
                                file.accept(&name);
                                continue;
                            }
                            let incoming = Incoming { device: String::new(), address: file.address.clone(), name: file.name.clone(), size: file.size, kind: file.kind.clone() };
                            if let Some(old) = waiting.replace(file) {
                                old.reject();
                            }
                            let w = weak.clone();
                            let _ = weak.upgrade_in_event_loop(move |_| update(&w, move |app| {
                                let device = device_name(app, &incoming.address);
                                app.files.incoming = Some(Incoming { device, ..incoming });
                                app.files.always = false;
                            }));
                        }
                        ObexEvent::Cancel => {
                            waiting = None;
                            post(&weak, |s| s.incoming = None);
                        }
                    },
                    Some(command) = rx.recv() => match command {
                        Command::Answer { accept, always: keep } => {
                            if let Some(file) = waiting.take() {
                                if accept {
                                    if keep && !always.contains(&file.address) {
                                        always.push(file.address.clone());
                                        let _ = crate::config::set_list(&crate::config::path(), "bluetooth", "accept_files_from", &always);
                                    }
                                    receiving.push((file.transfer.clone(), file.name.clone()));
                                    let name = file.name.clone();
                                    file.accept(&name);
                                } else {
                                    file.reject();
                                }
                            }
                            post(&weak, |s| s.incoming = None);
                        }
                        Command::Send { address, device, files } => match obex.send_files(&address, &files).await {
                            Ok((s, transfers)) => {
                                session = Some(s);
                                sending.clear();
                                for (t, file) in transfers.iter().zip(&files) {
                                    let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
                                    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                    sending.push(obex.transfer(t).await.unwrap_or(Transfer { path: t.clone(), name, size, status: "queued".into(), ..Transfer::default() }));
                                }
                                let list = sending.clone();
                                post(&weak, move |s| { s.sending = Some(Sending { device, transfers: list }); s.hidden = false; });
                            }
                            Err(e) => {
                                let note = format!("Couldn't send to {device}: {e}");
                                post(&weak, move |s| s.note = Some(note));
                            }
                        },
                        Command::Cancel => {
                            for t in sending.iter().filter(|t| !t.done()) {
                                let _ = obex.cancel(&t.path).await;
                            }
                            if let Some(s) = session.take() {
                                let _ = obex.close(&s).await;
                            }
                            sending.clear();
                            post(&weak, |s| s.sending = None);
                        }
                    },
                    _ = tick.tick() => {
                        // Progress of what's being sent.
                        if sending.iter().any(|t| !t.done()) {
                            for t in sending.iter_mut().filter(|t| !t.done()) {
                                if let Ok(now) = obex.transfer(&t.path).await {
                                    t.status = now.status;
                                    t.transferred = if t.status == "complete" { t.size } else { now.transferred };
                                    if now.size > 0 {
                                        t.size = now.size;
                                    }
                                }
                            }
                            if sending.iter().all(Transfer::done) && let Some(s) = session.take() {
                                let _ = obex.close(&s).await;
                            }
                            let list = sending.clone();
                            post(&weak, move |s| if let Some(sending) = &mut s.sending { sending.transfers = list; });
                        }
                        // Received files: move them when they're complete.
                        let mut still = Vec::new();
                        for (path, name) in receiving.drain(..) {
                            match obex.transfer(&path).await {
                                Ok(t) if t.status == "complete" => {
                                    let from = if t.filename.is_empty() { lituus_bt::obex::cache_dir().join(lituus_bt::obex::safe_name(&name)) } else { PathBuf::from(&t.filename) };
                                    let note = match move_to_downloads(&from, &name) {
                                        Ok(dir) => format!("Received {}, in {}", lituus_bt::obex::safe_name(&name), shorten(&dir)),
                                        Err(e) => format!("Received {name}, but couldn't move it to {}: {e}", shorten(&downloads())),
                                    };
                                    post(&weak, move |s| s.note = Some(note));
                                }
                                Ok(t) if t.status == "error" => {
                                    let note = format!("Receiving {name} failed");
                                    post(&weak, move |s| s.note = Some(note));
                                }
                                Ok(_) => still.push((path, name)),
                                Err(_) => {}
                            }
                        }
                        receiving = still;
                    },
                }
            }
        });
    });
}

fn move_to_downloads(from: &std::path::Path, name: &str) -> std::io::Result<PathBuf> {
    let dir = downloads();
    std::fs::create_dir_all(&dir)?;
    let to = lituus_bt::obex::free_path(&dir, &lituus_bt::obex::safe_name(name));
    std::fs::rename(from, &to).or_else(|_| std::fs::copy(from, &to).and_then(|_| std::fs::remove_file(from)))?;
    Ok(dir)
}

fn shorten(path: &std::path::Path) -> String {
    let p = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && p.starts_with(&home) => format!("~{}", &p[home.len()..]),
        _ => p,
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / f64::from(1u32 << 30)),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / f64::from(1u32 << 20)),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} bytes"),
    }
}

/// "JPEG image", from a MIME type or the name.
fn kind(mime: &str, name: &str) -> String {
    let ext = std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let known = match (mime, ext.as_str()) {
        (m, _) if m.starts_with("image/jpeg") => "JPEG image",
        (_, "jpg" | "jpeg") => "JPEG image",
        (m, _) if m.starts_with("image/png") => "PNG image",
        (_, "png") => "PNG image",
        (m, _) if m.starts_with("image/") => "Image",
        (m, _) if m.starts_with("video/") => "Video",
        (_, "mp4" | "mkv" | "mov" | "webm") => "Video",
        (m, _) if m.starts_with("audio/") => "Audio",
        (_, "pdf") => "PDF document",
        (m, _) if m.contains("vcard") => "Contact card",
        (_, "vcf") => "Contact card",
        _ => "",
    };
    if known.is_empty() { ext.to_uppercase() + " file" } else { known.to_owned() }
}

pub fn view(state: &State) -> FilesView {
    let mut v = FilesView::default();
    if let Some(s) = &state.sending {
        v.sending = !state.hidden;
        v.send_title = format!("Sending files to {}", s.device).into();
        let total: u64 = s.transfers.iter().map(|t| t.size).sum();
        let sent: u64 = s.transfers.iter().map(|t| if t.status == "complete" { t.size } else { t.transferred }).sum();
        let done = s.transfers.iter().filter(|t| t.status == "complete").count();
        let failed = s.transfers.iter().any(|t| t.status == "error");
        v.send_done = s.transfers.iter().all(Transfer::done);
        v.send_summary = if v.send_done && !failed {
            format!("All {} sent · {}", s.transfers.len(), size(total)).into()
        } else {
            format!("{done} of {} sent · {} of {}", s.transfers.len(), size(sent), size(total)).into()
        };
        v.send_fraction = if total > 0 { sent as f32 / total as f32 } else { 0.0 };
        v.files = model(
            s.transfers
                .iter()
                .map(|t| FileRow {
                    name: lituus_bt::obex::safe_name(if t.name.is_empty() { &t.filename } else { &t.name }).into(),
                    size: size(t.size).into(),
                    status: match t.status.as_str() {
                        "complete" => "Sent".into(),
                        "error" => "Failed".into(),
                        "active" if t.size > 0 => format!("{} %", t.transferred * 100 / t.size).into(),
                        _ => "Waiting".into(),
                    },
                    fraction: if t.size > 0 { t.transferred as f32 / t.size as f32 } else { 0.0 },
                    active: t.status == "active",
                    tone: match t.status.as_str() {
                        "complete" => "accent".into(),
                        "error" => "danger".into(),
                        _ => "muted".into(),
                    },
                })
                .collect(),
        );
    }
    if let Some(i) = &state.incoming {
        v.incoming = true;
        v.in_title = format!("{} wants to send you a file", i.device).into();
        v.in_name = lituus_bt::obex::safe_name(&i.name).into();
        v.in_meta = format!("{} · {}", kind(&i.kind, &i.name), size(i.size)).into();
        v.in_folder = shorten(&downloads()).into();
        v.in_always_label = format!("Always accept files from {}", i.device).into();
        v.in_always = state.always;
    }
    v.note = state.note.clone().unwrap_or_default().into();
    v
}

pub fn render(window: &MainWindow, state: &State) {
    window.set_files(view(state));
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn t(name: &str, size: u64, transferred: u64, status: &str) -> Transfer {
        Transfer {
            path: format!("/t/{name}"),
            name: name.into(),
            size,
            transferred,
            status: status.into(),
            filename: String::new(),
        }
    }

    #[test]
    fn the_send_dialog_adds_up() {
        let state = State {
            sending: Some(Sending {
                device: "Pixel 8".into(),
                transfers: vec![
                    t("a.jpg", 4 << 20, 4 << 20, "complete"),
                    t("b.mp4", 10 << 20, 5 << 20, "active"),
                    t("c.md", 6 << 10, 0, "queued"),
                ],
            }),
            ..State::default()
        };
        let v = view(&state);
        assert!(v.sending && !v.send_done);
        assert_eq!(v.send_title, "Sending files to Pixel 8");
        assert_eq!(v.send_summary, "1 of 3 sent · 9.0 MB of 14.0 MB");
        let statuses: Vec<String> = v.files.iter().map(|f| f.status.to_string()).collect();
        assert_eq!(statuses, ["Sent", "50 %", "Waiting"]);
    }

    #[test]
    fn an_incoming_file_says_what_it_is() {
        let state = State {
            incoming: Some(Incoming {
                device: "Pixel 8".into(),
                address: "AA".into(),
                name: "../PXL_1.jpg".into(),
                size: 3_565_158,
                kind: "image/jpeg".into(),
            }),
            ..State::default()
        };
        let v = view(&state);
        assert_eq!(v.in_title, "Pixel 8 wants to send you a file");
        assert_eq!(v.in_name, "PXL_1.jpg");
        assert_eq!(v.in_meta, "JPEG image · 3.4 MB");
    }
}
