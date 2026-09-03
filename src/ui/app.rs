//! Phase 1 demo application: text, buttons, a slider, a scroll area and an
//! animated spinner so repaint pacing can be observed.

pub struct App {
    pub counter: i32,
    pub slider: f32,
    pub checked: bool,
    pub text: String,
    pub frame_ms: f32,
    pub transport: &'static str,
    pub scale: f32,
    pub animate: bool,
}

impl App {
    pub fn new(transport: &'static str, scale: f32) -> Self {
        Self {
            counter: 0,
            slider: 0.4,
            checked: true,
            text: "Type here".to_owned(),
            frame_ms: 0.0,
            transport,
            scale,
            animate: true,
        }
    }

    pub fn ui(&mut self, root: &mut egui::Ui) {
        egui::Panel::left("sidebar").default_size(220.0).show(root, |ui| {
            ui.heading("gitgui");
            ui.label("Phase 1 rendering demo");
            ui.separator();
            ui.label(format!("transport: {}", self.transport));
            ui.label(format!("scale: {}", self.scale));
            ui.label(format!("frame: {:.2} ms", self.frame_ms));
            ui.separator();
            if ui.button("Increment").clicked() {
                self.counter += 1;
            }
            if ui.button("Reset").clicked() {
                self.counter = 0;
            }
            ui.checkbox(&mut self.checked, "Checkbox");
            ui.checkbox(&mut self.animate, "Animate spinner");
            ui.separator();
            ui.small("q quits, Ctrl+C quits");
        });

        egui::Panel::bottom("status").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.label("main");
                ui.separator();
                ui.label("0 staged, 0 unstaged");
                ui.separator();
                ui.label(format!("counter {}", self.counter));
                if self.animate {
                    ui.separator();
                    ui.spinner();
                }
            });
        });

        egui::CentralPanel::default().show(root, |ui| {
            ui.heading("The quick brown fox jumps over the lazy dog");
            ui.label("Regular text at 13 pt. Grumpy wizards make toxic brew for the evil queen and jack.");
            ui.monospace("monospace: fn main() { println!(\"hello\"); } 0123456789");
            ui.add(egui::Slider::new(&mut self.slider, 0.0..=1.0).text("slider"));
            ui.text_edit_singleline(&mut self.text);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for i in 0..200 {
                    ui.horizontal(|ui| {
                        let short = format!("{:07x}", (i as u32).wrapping_mul(2654435761) & 0xfff_ffff);
                        ui.monospace(short);
                        ui.label(format!("commit {i}: a summary line for the commit list"));
                        ui.weak("author");
                    });
                }
            });
        });
    }
}
