use eframe::egui;

fn main() -> eframe::Result {
    eframe::run_native(
        "Not News Aggregator",
        eframe::NativeOptions::default(),
        Box::new(|_| Ok(Box::new(RewriteShell))),
    )
}

struct RewriteShell;

impl eframe::App for RewriteShell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Not News Aggregator · Rust rewrite");
            ui.label("The compatibility reader and placement domain are under construction.");
        });
    }
}
