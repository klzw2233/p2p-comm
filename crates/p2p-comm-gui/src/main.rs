use eframe::egui;

use p2p_comm_core::{
    default_data_dir, has_stored_identity, short_id, CallPhase, CallResult, Error, FileProgress,
    MediaType, Node, PeerStatus, PendingInvite, PendingOffer, Snapshot, TransferStatus, VideoFrame,
};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "p2p-comm",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}

enum Screen {
    Unlock(UnlockForm),
    Main(Box<MainState>),
}

struct UnlockForm {
    password: String,
    error: Option<String>,
    returning: bool,
}

impl Default for UnlockForm {
    fn default() -> Self {
        Self {
            password: String::new(),
            error: None,
            returning: has_stored_identity(),
        }
    }
}

struct MainState {
    node: Node,
    dial: String,
    dial_error: Option<String>,
    nickname_draft: String,
    nickname_error: Option<String>,
    compose: String,
    send_error: Option<String>,
    video_tex: Option<egui::TextureHandle>,
}

struct App {
    rt: tokio::runtime::Runtime,
    screen: Screen,
}

impl App {
    fn new() -> Self {
        Self {
            rt: tokio::runtime::Runtime::new().expect("tokio runtime"),
            screen: Screen::Unlock(UnlockForm::default()),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let next = match &mut self.screen {
            Screen::Unlock(form) => {
                if unlock_ui(ctx, form) {
                    match start_node(&self.rt, &form.password) {
                        Ok(node) => Some(Screen::Main(Box::new(MainState {
                            node,
                            dial: String::new(),
                            dial_error: None,
                            nickname_draft: String::new(),
                            nickname_error: None,
                            compose: String::new(),
                            send_error: None,
                            video_tex: None,
                        }))),
                        Err(err) => {
                            form.error = Some(error_text(err).to_owned());
                            None
                        }
                    }
                } else {
                    None
                }
            }
            Screen::Main(main) => {
                let changed = main.node.poll();
                if let Some(frame) = main.node.take_video_frame() {
                    apply_video_frame(ctx, main, &frame);
                }
                let snap = main.node.snapshot();
                let transferring = snap.transfer.as_ref().is_some_and(|t| {
                    matches!(
                        t.status,
                        TransferStatus::Offered | TransferStatus::Transferring
                    )
                });
                let in_call = snap.call.is_some();
                if changed
                    || transferring
                    || in_call
                    || snap.pending_offer.is_some()
                    || snap.pending_invite.is_some()
                    || snap.selected_status == Some(PeerStatus::Connecting)
                    || snap.selected_status == Some(PeerStatus::Connected)
                {
                    ctx.request_repaint();
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                main_ui(ctx, main, &snap);
                if snap.call.is_none() {
                    main.video_tex = None;
                }
                None
            }
        };
        if let Some(screen) = next {
            self.screen = screen;
        }
    }
}

fn start_node(rt: &tokio::runtime::Runtime, password: &str) -> Result<Node, Error> {
    let dir = default_data_dir()?;
    rt.block_on(Node::start(&dir, password))
}

fn error_text(err: Error) -> &'static str {
    match err {
        Error::EmptyPassword => "Password cannot be empty.",
        Error::WrongPassword => "Wrong password. Existing identity was not changed.",
        Error::DataDirUnavailable => "Could not locate the data directory.",
        Error::Io => "Could not read or write the data directory.",
        Error::CorruptStore => "Stored data is corrupt.",
        Error::InvalidPeerId => "Enter a 64-character hex Peer ID or a known nickname.",
        Error::UnknownNickname => "Unknown nickname.",
        Error::EmptyNickname => "Nickname cannot be empty.",
        Error::DuplicateNickname => "That nickname is already used.",
        Error::Bind => "Could not start the network endpoint.",
        Error::InvalidFrame => "Malformed message.",
        Error::NotConnected => "Not connected to this Peer.",
        Error::DecryptFailed => "Could not decrypt chat history.",
        Error::Busy => "Already in a call.",
    }
}

fn unlock_ui(ctx: &egui::Context, form: &mut UnlockForm) -> bool {
    let mut submitted = false;
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(120.0);
            ui.heading("p2p-comm");
            ui.label(if form.returning {
                "Enter identity password"
            } else {
                "Set identity password"
            });
            ui.add_space(12.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut form.password)
                    .password(true)
                    .hint_text("identity password"),
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submitted = true;
            }
            if ui
                .button(if form.returning {
                    "Unlock"
                } else {
                    "Set password"
                })
                .clicked()
            {
                submitted = true;
            }
            if let Some(error) = &form.error {
                ui.colored_label(egui::Color32::from_rgb(200, 80, 80), error);
            }
        });
    });
    submitted
}

fn main_ui(ctx: &egui::Context, main: &mut MainState, snap: &Snapshot) {
    egui::SidePanel::left("sidebar")
        .default_width(220.0)
        .show(ctx, |ui| sidebar_ui(ui, main, snap));
    egui::CentralPanel::default().show(ctx, |ui| chat_ui(ui, main, snap));
    if let Some(offer) = &snap.pending_offer {
        offer_window(ctx, main, offer);
    }
    if let Some(invite) = &snap.pending_invite {
        invite_window(ctx, main, invite);
    }
}

fn sidebar_ui(ui: &mut egui::Ui, main: &mut MainState, snap: &Snapshot) {
    ui.heading("Chats");
    if snap.sidebar.is_empty() {
        ui.label("No conversations yet.");
    }
    let mut select = None;
    let mut set_nick = None;
    let mut del_nick = None;
    for item in &snap.sidebar {
        let selected = snap.selected.as_deref() == Some(item.peer_id_hex.as_str());
        let label = if item.unread > 0 {
            format!("{} ({})", item.label, item.unread)
        } else {
            item.label.clone()
        };
        let response = ui.selectable_label(selected, label);
        if response.clicked() {
            select = Some(item.peer_id_hex.clone());
        }
        response.context_menu(|ui| {
            if ui.button("Set nickname").clicked() {
                set_nick = Some(item.peer_id_hex.clone());
                ui.close_menu();
            }
            if ui.button("Delete nickname").clicked() {
                del_nick = Some(item.peer_id_hex.clone());
                ui.close_menu();
            }
        });
    }
    if let Some(peer) = select {
        main.node.select(&peer);
        main.nickname_draft = main.node.display_name(&peer);
        if main.nickname_draft == short_id(&peer) {
            main.nickname_draft.clear();
        }
        main.nickname_error = None;
    }
    if let Some(peer) = set_nick {
        main.node.select(&peer);
        main.nickname_draft = main.node.display_name(&peer);
        if main.nickname_draft == short_id(&peer) {
            main.nickname_draft.clear();
        }
        main.nickname_error = None;
    }
    if let Some(peer) = del_nick {
        if let Err(err) = main.node.remove_nickname(&peer) {
            main.nickname_error = Some(error_text(err).to_owned());
        } else {
            main.nickname_draft.clear();
            main.nickname_error = None;
        }
    }

    ui.separator();
    ui.label("Dial");
    let dial = ui.add(egui::TextEdit::singleline(&mut main.dial).hint_text("Peer ID or nickname"));
    let submitted = ui.button("Dial").clicked()
        || (dial.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
    if submitted {
        match main.node.dial(&main.dial) {
            Ok(()) => {
                main.dial.clear();
                main.dial_error = None;
            }
            Err(err) => main.dial_error = Some(error_text(err).to_owned()),
        }
    }
    if let Some(error) = &main.dial_error {
        ui.colored_label(egui::Color32::from_rgb(200, 80, 80), error);
    }
}

fn chat_ui(ui: &mut egui::Ui, main: &mut MainState, snap: &Snapshot) {
    ui.heading("Chat");
    ui.label(format!("Your Peer ID: {}", snap.local_peer_id_hex));
    ui.separator();
    let Some(peer) = snap.selected.as_deref() else {
        ui.label("Select a conversation from the sidebar.");
        return;
    };
    ui.label(format!("Peer: {}", main.node.display_name(peer)));
    ui.label(format!("Peer ID: {peer}"));
    ui.label(status_text(snap.selected_status));
    video_pane(ui, main, snap, peer);
    call_bar(ui, main, snap, peer);
    ui.horizontal(|ui| {
        ui.label("Nickname");
        ui.add(egui::TextEdit::singleline(&mut main.nickname_draft).hint_text("local nickname"));
        if ui.button("Save").clicked() {
            match main.node.set_nickname(peer, &main.nickname_draft) {
                Ok(()) => main.nickname_error = None,
                Err(err) => main.nickname_error = Some(error_text(err).to_owned()),
            }
        }
        if ui.button("Clear").clicked() {
            match main.node.remove_nickname(peer) {
                Ok(()) => {
                    main.nickname_draft.clear();
                    main.nickname_error = None;
                }
                Err(err) => main.nickname_error = Some(error_text(err).to_owned()),
            }
        }
    });
    if let Some(error) = &main.nickname_error {
        ui.colored_label(egui::Color32::from_rgb(200, 80, 80), error);
    }
    if let Some(err) = &snap.selected_error {
        ui.colored_label(egui::Color32::from_rgb(200, 80, 80), &err.message);
        if ui.button("Retry").clicked() {
            if let Err(dial_err) = main.node.dial(peer) {
                main.dial_error = Some(error_text(dial_err).to_owned());
            }
        }
    }
    ui.separator();
    if let Some(xfer) = &snap.transfer {
        transfer_ui(ui, xfer);
        ui.separator();
    }
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .max_height(ui.available_height() - 40.0)
        .show(ui, |ui| {
            for msg in &snap.messages {
                ui.label(format_message(msg));
            }
        });
    ui.horizontal(|ui| {
        let edit = ui.add(
            egui::TextEdit::singleline(&mut main.compose)
                .hint_text("message")
                .desired_width(ui.available_width() - 150.0),
        );
        let send = ui.button("Send").clicked()
            || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if send {
            match main.node.send_text(peer, &main.compose) {
                Ok(()) => {
                    main.compose.clear();
                    main.send_error = None;
                }
                Err(err) => main.send_error = Some(error_text(err).to_owned()),
            }
        }
        if ui.button("Send file").clicked() {
            if let Some(path) = rfd::FileDialog::new().pick_file() {
                if let Err(err) = main.node.send_file(peer, &path) {
                    main.send_error = Some(error_text(err).to_owned());
                }
            }
        }
    });
    if let Some(error) = &main.send_error {
        ui.colored_label(egui::Color32::from_rgb(200, 80, 80), error);
    }
}

fn transfer_ui(ui: &mut egui::Ui, xfer: &FileProgress) {
    let (status, color) = match xfer.status {
        TransferStatus::Offered => ("Offering…", egui::Color32::GRAY),
        TransferStatus::Transferring => ("Transferring…", egui::Color32::LIGHT_BLUE),
        TransferStatus::Complete => ("Complete", egui::Color32::from_rgb(90, 160, 90)),
        TransferStatus::Rejected => ("Rejected", egui::Color32::from_rgb(200, 80, 80)),
        TransferStatus::Failed => ("Failed", egui::Color32::from_rgb(200, 80, 80)),
    };
    let arrow = match xfer.direction {
        p2p_comm_core::Direction::Outgoing => "↑",
        p2p_comm_core::Direction::Incoming => "↓",
    };
    ui.label(format!(
        "{arrow} {} ({})",
        xfer.name,
        format_size(xfer.size)
    ));
    let fraction = if xfer.size == 0 {
        1.0
    } else {
        let done = xfer.transferred.min(xfer.size);
        let percent = u16::try_from(done.saturating_mul(100) / xfer.size).unwrap_or(100);
        f32::from(percent) / 100.0
    };
    ui.add(
        egui::ProgressBar::new(fraction)
            .text(format!("{status} — {}", format_size(xfer.transferred)))
            .fill(color),
    );
}

fn offer_window(ctx: &egui::Context, main: &mut MainState, offer: &PendingOffer) {
    let peer = offer.peer_id_hex.clone();
    egui::Window::new("Incoming file")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(format!(
                "{} wants to send a file.",
                main.node.display_name(&peer)
            ));
            ui.label(format!("Name: {}", offer.name));
            ui.label(format!("Size: {}", format_size(offer.size)));
            ui.horizontal(|ui| {
                if ui.button("Accept").clicked() {
                    main.node.accept_file(&peer);
                }
                if ui.button("Reject").clicked() {
                    main.node.reject_file(&peer);
                }
            });
        });
}

fn video_pane(ui: &mut egui::Ui, main: &MainState, snap: &Snapshot, peer: &str) {
    let Some(call) = snap.call.as_ref() else {
        return;
    };
    if call.peer_id_hex != peer
        || call.media != MediaType::AudioVideo
        || call.phase != CallPhase::Active
    {
        return;
    }
    if let Some(tex) = &main.video_tex {
        let available = ui.available_width();
        let height = available * 480.0 / 640.0;
        ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(available, height)));
    } else {
        ui.label("Waiting for video…");
    }
}

fn apply_video_frame(ctx: &egui::Context, main: &mut MainState, frame: &VideoFrame) {
    let size = [frame.width as usize, frame.height as usize];
    let image = egui::ColorImage::from_rgb(size, &frame.rgb);
    match &mut main.video_tex {
        Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
        None => {
            main.video_tex =
                Some(ctx.load_texture("remote-video", image, egui::TextureOptions::LINEAR));
        }
    }
}

fn call_bar(ui: &mut egui::Ui, main: &mut MainState, snap: &Snapshot, peer: &str) {
    ui.horizontal(|ui| match snap.call.as_ref() {
        Some(call) if call.peer_id_hex == peer => {
            let label = match (call.phase, call.media) {
                (CallPhase::Outgoing, MediaType::AudioVideo) => "Calling (video)…",
                (CallPhase::Outgoing, MediaType::Audio) => "Calling…",
                (CallPhase::Incoming, MediaType::AudioVideo) => "Incoming video call",
                (CallPhase::Incoming, MediaType::Audio) => "Incoming call",
                (CallPhase::Active, MediaType::AudioVideo) => "In video call",
                (CallPhase::Active, MediaType::Audio) => "In call",
            };
            ui.colored_label(egui::Color32::from_rgb(90, 160, 90), label);
            if ui.button("Hang up").clicked() {
                main.node.hangup();
                main.video_tex = None;
            }
        }
        Some(_) => {
            ui.label("Busy on another call");
        }
        None => {
            let connected = snap.selected_status == Some(PeerStatus::Connected);
            if ui
                .add_enabled(connected, egui::Button::new("Voice"))
                .clicked()
            {
                if let Err(err) = main.node.invite_audio(peer) {
                    main.send_error = Some(error_text(err).to_owned());
                }
            }
            if ui
                .add_enabled(connected, egui::Button::new("Video"))
                .clicked()
            {
                if let Err(err) = main.node.invite_video(peer) {
                    main.send_error = Some(error_text(err).to_owned());
                }
            }
        }
    });
    if let Some(result) = snap.call_result {
        let text = match result {
            CallResult::Rejected => "Call declined.",
            CallResult::TimedOut => "Call timed out.",
        };
        ui.colored_label(egui::Color32::from_rgb(200, 80, 80), text);
    }
}

fn invite_window(ctx: &egui::Context, main: &mut MainState, invite: &PendingInvite) {
    let peer = invite.peer_id_hex.clone();
    egui::Window::new("Incoming call")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(format!(
                "{} is calling ({}).",
                main.node.display_name(&peer),
                if invite.media == MediaType::AudioVideo {
                    "video"
                } else {
                    "voice"
                }
            ));
            ui.horizontal(|ui| {
                if ui.button("Accept").clicked() {
                    main.node.accept_call(&peer);
                }
                if ui.button("Reject").clicked() {
                    main.node.reject_call(&peer);
                }
            });
        });
}

fn format_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < MIB {
        format!("{} KiB", bytes / KIB)
    } else {
        format!("{} MiB", bytes / MIB)
    }
}

fn format_message(msg: &p2p_comm_core::ChatMessage) -> String {
    let who = match msg.direction {
        p2p_comm_core::Direction::Outgoing => "Me",
        p2p_comm_core::Direction::Incoming => "Peer",
    };
    let fail = if msg.failed { " [failed]" } else { "" };
    format!(
        "{} {who}: {}{fail}",
        format_time(msg.timestamp),
        msg.content
    )
}

fn format_time(unix_millis: u64) -> String {
    let secs = unix_millis / 1000;
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn status_text(status: Option<PeerStatus>) -> &'static str {
    match status {
        Some(PeerStatus::Connecting) => "Connecting…",
        Some(PeerStatus::Connected) => "Connected",
        Some(PeerStatus::Failed) => "Disconnected",
        None => "",
    }
}
