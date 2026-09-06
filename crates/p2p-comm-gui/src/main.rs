use eframe::egui;

use p2p_comm_core::{
    default_data_dir, has_stored_identity, short_id, Error, Node, PeerStatus, Snapshot,
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
                let snap = main.node.snapshot();
                if changed || snap.selected_status == Some(PeerStatus::Connecting) {
                    ctx.request_repaint();
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                main_ui(ctx, main, &snap);
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
        let response = ui.selectable_label(selected, &item.label);
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
}

fn status_text(status: Option<PeerStatus>) -> &'static str {
    match status {
        Some(PeerStatus::Connecting) => "Connecting…",
        Some(PeerStatus::Connected) => "Connected",
        Some(PeerStatus::Failed) => "Disconnected",
        None => "",
    }
}
