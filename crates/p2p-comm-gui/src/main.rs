use eframe::egui;

use p2p_comm_core::{has_stored_identity, unlock, Error, Identity};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "p2p-comm",
        options,
        Box::new(|_cc| Ok(Box::new(App::default()))),
    )
}

enum Screen {
    Unlock(UnlockForm),
    Main(Identity),
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

struct App {
    screen: Screen,
}

impl Default for App {
    fn default() -> Self {
        Self {
            screen: Screen::Unlock(UnlockForm::default()),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let next = match &mut self.screen {
            Screen::Unlock(form) => {
                if unlock_ui(ctx, form) {
                    match unlock(&form.password) {
                        Ok(identity) => Some(Screen::Main(identity)),
                        Err(err) => {
                            form.error = Some(error_text(err).to_owned());
                            None
                        }
                    }
                } else {
                    None
                }
            }
            Screen::Main(identity) => {
                main_ui(ctx, identity.peer_id_hex());
                None
            }
        };
        if let Some(screen) = next {
            self.screen = screen;
        }
    }
}

fn error_text(err: Error) -> &'static str {
    match err {
        Error::EmptyPassword => "Password cannot be empty.",
        Error::WrongPassword => "Wrong password. Existing identity was not changed.",
        Error::DataDirUnavailable => "Could not locate the data directory.",
        Error::Io => "Could not read or write the identity file.",
        Error::CorruptStore => "Identity file is corrupt.",
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
                .button(if form.returning { "Unlock" } else { "Set password" })
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

fn main_ui(ctx: &egui::Context, peer_id_hex: &str) {
    egui::SidePanel::left("sidebar")
        .default_width(220.0)
        .show(ctx, |ui| {
            ui.heading("Chats");
            ui.label("No conversations yet.");
        });
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.heading("Chat");
        ui.label(format!("Your Peer ID: {peer_id_hex}"));
        ui.separator();
        ui.label("Select a conversation from the sidebar.");
    });
}
