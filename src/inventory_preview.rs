//! Isolated read-only Patchloom inventory preview, 2026-10-27; original GPLv3 applies.
use ajour_core::discovery::{self, Discovery, Installation, Product};
use iced::{
    button, container, scrollable, Application, Button, Clipboard, Column, Command, Container,
    Element, Length, Row, Scrollable, Settings, Subscription, Text,
};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
struct Model {
    root: PathBuf,
    inventory: Option<Discovery>,
    error: Option<String>,
    selected: Option<Product>,
}
impl Model {
    fn new(root: PathBuf) -> Self {
        let mut model = Self {
            root,
            inventory: None,
            error: None,
            selected: None,
        };
        model.refresh();
        model
    }
    // ponytail: bounded synchronous snapshots; move to a worker if large copied installs block the window.
    fn refresh(&mut self) {
        self.selected = None;
        match discovery::discover(&self.root) {
            Ok(report) => {
                self.inventory = Some(report);
                self.error = None;
            }
            Err(error) => {
                self.inventory = None;
                self.error = Some(error.to_string());
            }
        }
    }
    fn select(&mut self, product: Product) {
        if self
            .inventory
            .as_ref()
            .is_some_and(|r| r.installations.iter().any(|i| i.product == product))
        {
            self.selected = Some(product);
        }
    }
    fn installation(&self) -> Option<&Installation> {
        let selected = self.selected?;
        self.inventory
            .as_ref()?
            .installations
            .iter()
            .find(|i| i.product == selected)
    }
    fn needs_review(&self) -> bool {
        self.error.is_some()
            || self.inventory.as_ref().is_none_or(|report| {
                !report.issues.is_empty()
                    || report
                        .installations
                        .iter()
                        .any(|i| i.addons.iter().any(|a| a.issue.is_some()))
            })
    }
}
fn product_label(product: Product) -> &'static str {
    match product {
        Product::Retail => "Retail",
        Product::ClassicProgression => "Mists of Pandaria Classic",
        Product::ClassicEra => "Classic Era",
        Product::Anniversary => "Burning Crusade Classic Anniversary",
    }
}

fn issue_text(issue: &str) -> String {
    let (product, detail) = issue
        .split_once(": ")
        .unwrap_or(("this installation", issue));
    if detail.contains("Unknown product/version family") {
        format!("Unsupported WoW version: {product}.")
    } else if detail.contains("Ambiguous active product builds") {
        format!("More than one active build was found for {product}.")
    } else if detail.contains("No active supported WoW products") {
        "No supported active WoW versions were found.".into()
    } else {
        format!("This WoW installation needs review: {issue}")
    }
}

#[derive(Debug, Clone)]
enum Message {
    Select(Product),
    Rescan,
    NativeCheckDone,
}
struct Flags {
    root: PathBuf,
    native_check: bool,
}
struct Preview {
    model: Model,
    choices: Vec<button::State>,
    rescan: button::State,
    scroll: scrollable::State,
    native_check: bool,
    exit: bool,
}
impl Preview {
    fn synchronize_choices(&mut self) {
        self.choices = (0..self
            .model
            .inventory
            .as_ref()
            .map_or(0, |r| r.installations.len()))
            .map(|_| button::State::new())
            .collect();
    }
}
impl Application for Preview {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Flags = Flags;
    fn new(flags: Flags) -> (Self, Command<Message>) {
        let mut app = Self {
            model: Model::new(flags.root),
            choices: vec![],
            rescan: button::State::new(),
            scroll: scrollable::State::new(),
            native_check: flags.native_check,
            exit: false,
        };
        app.synchronize_choices();
        (app, Command::none())
    }
    fn title(&self) -> String {
        "Patchloom | Inventory preview".into()
    }
    fn subscription(&self) -> Subscription<Message> {
        if self.native_check {
            iced_futures::time::every(std::time::Duration::from_millis(200))
                .map(|_| Message::NativeCheckDone)
        } else {
            Subscription::none()
        }
    }
    fn mode(&self) -> iced::window::Mode {
        if self.native_check {
            iced::window::Mode::Hidden
        } else {
            iced::window::Mode::Windowed
        }
    }
    fn should_exit(&self) -> bool {
        self.exit
    }
    fn update(&mut self, message: Message, _: &mut Clipboard) -> Command<Message> {
        match message {
            Message::Select(product) => self.model.select(product),
            Message::Rescan => {
                self.model.refresh();
                self.synchronize_choices();
            }
            Message::NativeCheckDone => self.exit = true,
        }
        Command::none()
    }
    fn view(&mut self) -> Element<'_, Message> {
        let mut content = Column::new()
            .spacing(14)
            .push(Text::new("Patchloom").size(32))
            .push(Text::new("WoW addon inventory").size(22))
            .push(Text::new("Read-only preview. Your files will not be changed.").size(16))
            .push(Text::new(self.model.root.display().to_string()).size(14))
            .push(
                Button::new(&mut self.rescan, Text::new("Rescan folder")).on_press(Message::Rescan),
            );
        if let Some(error) = &self.model.error {
            content = content
                .push(Text::new("We couldn't read this WoW folder.").size(22))
                .push(Text::new(
                    "Check that the copied World of Warcraft folder is complete and readable.",
                ))
                .push(Text::new(error).size(14));
        }
        if let Some(report) = &self.model.inventory {
            if report.installations.is_empty() {
                content =
                    content.push(Text::new("No supported WoW versions found in this folder."));
            }
            for issue in &report.issues {
                content = content.push(Text::new(issue_text(issue)).size(15));
            }
            for (installation, state) in report.installations.iter().zip(&mut self.choices) {
                let label = format!(
                    "{}  {}",
                    product_label(installation.product),
                    installation.version
                );
                content = content.push(
                    Button::new(state, Text::new(label))
                        .on_press(Message::Select(installation.product)),
                );
            }
            if let Some(installation) = self.model.installation() {
                content = content
                    .push(
                        Text::new(format!(
                            "{} {}",
                            product_label(installation.product),
                            installation.version
                        ))
                        .size(22),
                    )
                    .push(Text::new(installation.directory.display().to_string()).size(14))
                    .push(Text::new(format!(
                        "Addon folders: {}",
                        installation.addons.len()
                    )));
                if installation.addons.is_empty() {
                    content = content.push(Text::new("No addon folders found."));
                }
                for addon in &installation.addons {
                    let status = match addon.issue.as_deref() {
                        None => "Detected",
                        Some(issue) if issue.contains("Ambiguous") => {
                            "Needs review: more than one matching addon file."
                        }
                        Some(_) => "Needs review: addon metadata couldn't be matched.",
                    };
                    content = content.push(
                        Row::new()
                            .spacing(20)
                            .push(Text::new(&addon.addon).width(Length::FillPortion(1)))
                            .push(Text::new(status).width(Length::FillPortion(3))),
                    );
                }
            } else if !report.installations.is_empty() {
                content =
                    content.push(Text::new("Choose a WoW version to view its addon folders."));
            }
        }
        // Reuse existing neutral project surfaces; no unapproved logo or palette adaptation.
        let palette = ajour_core::theme::Theme::dark().palette;
        Container::new(Scrollable::new(&mut self.scroll).padding(24).push(content))
            .style(Surface(palette))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
struct Surface(ajour_core::theme::ColorPalette);
impl container::StyleSheet for Surface {
    fn style(&self) -> container::Style {
        container::Style {
            background: Some(self.0.base.background.into()),
            text_color: Some(self.0.bright.surface),
            ..Default::default()
        }
    }
}

pub fn run(opts: &crate::cli::Opts) -> crate::Result<()> {
    let root = opts
        .preview
        .clone()
        .ok_or_else(|| anyhow::anyhow!("An explicit copied WoW path is required"))?;
    if opts.command.is_some() || opts.data_directory.is_some() || opts.self_update_temp.is_some() {
        anyhow::bail!(
            "Preview cannot be combined with legacy commands, --data or --self-update-temp"
        );
    }
    if opts.preview_check && opts.preview_window_check {
        anyhow::bail!("Choose one preview check mode");
    }
    if opts.preview_check {
        let (mut app, command) = Preview::new(Flags {
            root,
            native_check: false,
        });
        // Construct the same view as the native entry; no window or executor is started.
        drop(command);
        drop(app.view());
        serde_json::to_writer(std::io::stdout(), &app.model)?;
        if app.model.needs_review() {
            anyhow::bail!("Some inventory items need review");
        }
        return Ok(());
    }
    let mut settings = Settings::with_flags(Flags {
        root,
        native_check: opts.preview_window_check,
    });
    settings.window.size = (960, 680);
    settings.default_text_size = 16;
    settings.antialiasing = opts.antialiasing.unwrap_or(false);
    Preview::run(settings).map_err(|e| anyhow::anyhow!(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_never_falls_back_to_another_product_or_survives_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let mut model = Model {
            root: temp.path().join("missing-root"),
            inventory: Some(Discovery {
                root: PathBuf::new(),
                installations: vec![],
                issues: vec!["unknown product".into()],
            }),
            error: None,
            selected: None,
        };
        assert!(model.needs_review());
        model.select(Product::Retail);
        assert!(model.installation().is_none());
        model.selected = Some(Product::Retail);
        model.refresh();
        assert!(model.selected.is_none());
        assert!(model.error.is_some());
        assert!(model.inventory.is_none());
        let (app, command) = Preview::new(Flags {
            root: temp.path().join("another-missing-root"),
            native_check: false,
        });
        assert!(app.model.error.is_some());
        assert!(command.futures().is_empty());
        assert!(app.subscription().recipes().is_empty());
    }

    #[test]
    fn real_products_selection_refresh_and_view_schedule_no_legacy_work() {
        let temp = tempfile::tempdir().unwrap();
        for folder in ["_retail_", "_classic_"] {
            std::fs::create_dir(temp.path().join(folder)).unwrap();
        }
        let info = temp.path().join(".build.info");
        std::fs::write(&info,"Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.1.5.70575\n1|wow_classic|5.5.4.70579\n").unwrap();
        let (mut app, command) = Preview::new(Flags {
            root: temp.path().into(),
            native_check: false,
        });
        assert!(command.futures().is_empty());
        assert!(app.subscription().recipes().is_empty());
        assert!(!app.model.needs_review());
        assert!(app.model.installation().is_none());
        app.model.select(Product::ClassicProgression);
        assert_eq!(
            app.model.installation().unwrap().product,
            Product::ClassicProgression
        );
        app.model.select(Product::Anniversary);
        assert_eq!(
            app.model.installation().unwrap().product,
            Product::ClassicProgression
        );
        drop(app.view());
        std::fs::write(
            &info,
            "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow_unknown|13.0.0.1\n",
        )
        .unwrap();
        app.model.refresh();
        app.synchronize_choices();
        assert!(app.model.needs_review());
        assert!(app.model.installation().is_none());
        assert!(app.choices.is_empty());
        drop(app.view());
        assert!(app.subscription().recipes().is_empty());
        let (smoke, command) = Preview::new(Flags {
            root: temp.path().into(),
            native_check: true,
        });
        assert!(command.futures().is_empty());
        assert_eq!(smoke.mode(), iced::window::Mode::Hidden);
    }
}
