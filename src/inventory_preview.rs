//! Isolated read-only Patchloom inventory preview, 2026-10-27; original GPLv3 applies.
use ajour_core::discovery::{self, Discovery, Installation, Product};
use iced::{
    button, container, scrollable, text_input, Application, Button, Clipboard, Column, Command,
    Container, Element, Length, Row, Scrollable, Settings, Subscription, Text, TextInput,
};
use serde::Serialize;
use std::{path::PathBuf, sync::Arc};

#[derive(Debug, Clone)]
struct ScanRequest {
    root: PathBuf,
    token: Arc<()>,
}
#[derive(Debug, Clone)]
struct ReceiptRequest {
    root: PathBuf,
    addons: PathBuf,
    product: Product,
    id: String,
    token: Arc<()>,
}
fn serialize_inventory<S: serde::Serializer>(
    inventory: &Option<Arc<Discovery>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    inventory.as_deref().serialize(serializer)
}
#[derive(Debug, Serialize)]
struct Model {
    root: PathBuf,
    #[serde(serialize_with = "serialize_inventory")]
    inventory: Option<Arc<Discovery>>,
    error: Option<String>,
    selected: Option<Product>,
    #[serde(skip)]
    scan: Option<Arc<()>>,
    #[serde(skip)]
    receipt_id: String,
    #[serde(skip)]
    receipt: Option<Arc<ajour_core::fs::OwnershipPreview>>,
    #[serde(skip)]
    receipt_error: Option<String>,
    #[serde(skip)]
    pending_receipt: Option<ReceiptRequest>,
    #[serde(skip)]
    receipt_worker: Option<Arc<()>>,
    #[serde(skip)]
    scan_requested: bool,
}
impl Model {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            inventory: None,
            error: None,
            selected: None,
            scan: None,
            receipt_id: String::new(),
            receipt: None,
            receipt_error: None,
            pending_receipt: None,
            receipt_worker: None,
            scan_requested: false,
        }
    }
    // Synchronous only for the deterministic headless check and fixture tests.
    fn refresh(&mut self) {
        if let Some(request) = self.begin_scan() {
            let result = inventory_snapshot(&request);
            self.complete_scan(request, result);
        }
    }
    fn begin_scan(&mut self) -> Option<ScanRequest> {
        self.selected = None;
        self.inventory = None;
        self.error = None;
        self.clear_receipt();
        self.scan_requested = true;
        self.start_scan()
    }
    // Modified 2026-10-28; original GPLv3 applies.
    // ponytail: one snapshot worker; split only if measured latency requires it.
    fn start_scan(&mut self) -> Option<ScanRequest> {
        if !self.scan_requested || self.scan.is_some() || self.receipt_worker.is_some() {
            return None;
        }
        self.scan_requested = false;
        let request = ScanRequest {
            root: self.root.clone(),
            token: Arc::new(()),
        };
        self.scan = Some(request.token.clone());
        Some(request)
    }

    fn complete_scan(
        &mut self,
        request: ScanRequest,
        result: Result<Arc<Discovery>, String>,
    ) -> bool {
        if !self
            .scan
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &request.token))
        {
            return false;
        }
        // Only this physical completion may release this reservation.
        self.scan = None;
        if self.root != request.root || self.scan_requested {
            return false;
        }
        match result {
            Ok(report) => {
                self.inventory = Some(report);
                self.error = None;
            }
            Err(error) => {
                self.inventory = None;
                self.error = Some(error);
            }
        }
        true
    }
    fn clear_receipt(&mut self) {
        self.pending_receipt = None;
        self.receipt = None;
        self.receipt_error = None;
    }
    fn set_receipt_id(&mut self, id: String) {
        if self.receipt_id != id {
            self.clear_receipt();
            self.receipt_id = id;
        }
    }
    fn begin_receipt(&mut self) -> Option<ReceiptRequest> {
        if self.scan.is_some() || self.scan_requested || self.receipt_worker.is_some() {
            return None;
        }
        let installation = self.installation()?;
        let request = ReceiptRequest {
            root: self.root.clone(),
            addons: installation.directory.join("Interface").join("AddOns"),
            product: installation.product,
            id: self.receipt_id.clone(),
            token: Arc::new(()),
        };
        self.clear_receipt();
        self.receipt_worker = Some(request.token.clone());
        self.pending_receipt = Some(request.clone());
        Some(request)
    }
    fn complete_receipt(
        &mut self,
        request: ReceiptRequest,
        result: Result<Arc<ajour_core::fs::OwnershipPreview>, String>,
    ) -> bool {
        if !self
            .receipt_worker
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &request.token))
        {
            return false;
        }
        // Invalidating displayed data never releases physical capacity.
        self.receipt_worker = None;
        let pending = self.pending_receipt.take();
        if self.root != request.root
            || self.selected != Some(request.product)
            || self.receipt_id != request.id
            || !pending
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &request.token))
            || !self.installation().is_some_and(|installation| {
                installation.directory.join("Interface").join("AddOns") == request.addons
            })
        {
            return false;
        }
        let result = result.and_then(|preview| {
            let product =
                serde_json::to_value(request.product).map_err(|error| error.to_string())?;
            if preview.source.get("product") != Some(&product) {
                return Err("Ownership source/product mismatch".into());
            }
            Ok(preview)
        });
        match result {
            Ok(preview) => self.receipt = Some(preview),
            Err(error) => self.receipt_error = Some(error),
        }
        true
    }
    fn select(&mut self, product: Product) {
        if self
            .inventory
            .as_ref()
            .is_some_and(|r| r.installations.iter().any(|i| i.product == product))
        {
            if self.selected != Some(product) {
                self.clear_receipt();
            }
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
    // Modified 2026-10-28; original GPLv3 applies.
    fn scan_status(&self) -> Option<&'static str> {
        if self.scan.is_some() {
            Some("Reading addon folders...")
        } else if self.scan_requested {
            Some("Waiting to read addon folders...")
        } else {
            None
        }
    }

    fn receipt_status(&self) -> Option<&'static str> {
        self.receipt_worker.as_ref().map(|_| "Checking receipt...")
    }

    fn can_inspect_receipt(&self) -> bool {
        !self.receipt_id.is_empty()
            && self.installation().is_some()
            && self.scan.is_none()
            && !self.scan_requested
            && self.receipt_worker.is_none()
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
fn inventory_snapshot(request: &ScanRequest) -> Result<Arc<Discovery>, String> {
    discovery::discover(&request.root)
        .map(Arc::new)
        .map_err(|error| error.to_string())
}
fn receipt_snapshot(
    request: &ReceiptRequest,
) -> Result<Arc<ajour_core::fs::OwnershipPreview>, String> {
    ajour_core::fs::inspect_ownership(&request.addons, &request.id)
        .map(Arc::new)
        .map_err(|error| error.to_string())
}
fn scan_command(request: ScanRequest) -> Command<Message> {
    Command::perform(
        async move {
            let worker = request.clone();
            let result = async_std::task::spawn_blocking(move || inventory_snapshot(&worker)).await;
            (request, result)
        },
        |(request, result)| Message::InventoryDone(request, result),
    )
}
fn receipt_command(request: ReceiptRequest) -> Command<Message> {
    Command::perform(
        async move {
            let worker = request.clone();
            let result = async_std::task::spawn_blocking(move || receipt_snapshot(&worker)).await;
            (request, result)
        },
        |(request, result)| Message::ReceiptDone(request, result),
    )
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

// Modified 2026-10-28: bounded display of untrusted receipt text; original GPLv3 applies.
const SOURCE_FIELD_CHARS: usize = 80;
const FOLDER_CHARS: usize = 64;
const FOLDERS_SHOWN: usize = 20;
const ERROR_CHARS: usize = 300;

// Modified 2026-10-28; original GPLv3 applies.
// Bounds emitted characters before the existing optional "...", and examines
// at most max + 1 input characters. Stored strings remain unchanged.
fn clip(text: &str, max: usize) -> String {
    let mut output = String::new();
    let mut used = 0;

    for ch in text.chars() {
        let escape = ch.is_control()
            || matches!(
                ch,
                '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{2028}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            );
        let escaped = escape.then(|| ch.escape_unicode().to_string());
        let width = escaped.as_ref().map_or(1, String::len);
        if width > max - used {
            output.push_str("...");
            return output;
        }
        if let Some(escaped) = escaped {
            output.push_str(&escaped);
        } else {
            output.push(ch);
        }
        used += width;
    }

    output
}

// Reads two fields only. The source JSON is never formatted whole.
fn source_summary(source: &serde_json::Value) -> String {
    let field = |key: &str| {
        source
            .get(key)
            .and_then(|value| value.as_str())
            .map_or_else(
                || "not recorded".to_owned(),
                |value| clip(value, SOURCE_FIELD_CHARS),
            )
    };
    format!(
        "Recorded source: {} ({})",
        field("repository"),
        field("product")
    )
}

fn folder_summary(folders: &[String]) -> String {
    let mut text = folders
        .iter()
        .take(FOLDERS_SHOWN)
        .map(|folder| clip(folder, FOLDER_CHARS))
        .collect::<Vec<_>>()
        .join(", ");
    if folders.len() > FOLDERS_SHOWN {
        text.push_str(&format!(", and {} more", folders.len() - FOLDERS_SHOWN));
    }
    text
}

#[derive(Debug, Clone)]
enum Message {
    Select(Product),
    Rescan,
    InventoryDone(ScanRequest, Result<Arc<Discovery>, String>),
    ReceiptIdChanged(String),
    InspectReceipt,
    ReceiptDone(
        ReceiptRequest,
        Result<Arc<ajour_core::fs::OwnershipPreview>, String>,
    ),
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
    receipt_input: text_input::State,
    inspect: button::State,
    scroll: scrollable::State,
    native_check: bool,
    exit: bool,
}
impl Preview {
    fn from_model(model: Model, native_check: bool) -> Self {
        let mut app = Self {
            model,
            choices: vec![],
            rescan: button::State::new(),
            receipt_input: text_input::State::new(),
            inspect: button::State::new(),
            scroll: scrollable::State::new(),
            native_check,
            exit: false,
        };
        app.synchronize_choices();
        app
    }
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
        let mut model = Model::new(flags.root);
        let command = model.begin_scan().map_or_else(Command::none, scan_command);
        (Self::from_model(model, flags.native_check), command)
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
                let request = self.model.begin_scan();
                self.synchronize_choices();
                return request.map_or_else(Command::none, scan_command);
            }
            Message::InventoryDone(request, result) => {
                if self.model.complete_scan(request, result) {
                    self.synchronize_choices();
                }
            }
            Message::ReceiptIdChanged(id) => self.model.set_receipt_id(id),
            Message::InspectReceipt => {
                if let Some(request) = self.model.begin_receipt() {
                    return receipt_command(request);
                }
            }
            Message::ReceiptDone(request, result) => {
                self.model.complete_receipt(request, result);
            }
            Message::NativeCheckDone => {
                if self.model.scan.is_none() && !self.model.scan_requested {
                    self.exit = true;
                }
            }
        }
        self.model
            .start_scan()
            .map_or_else(Command::none, scan_command)
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
        if let Some(status) = self.model.scan_status() {
            content = content.push(Text::new(status));
        }
        if let Some(status) = self.model.receipt_status() {
            content = content.push(Text::new(status));
        }
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
                content = content
                    .push(Text::new("Ownership receipt").size(18))
                    .push(Text::new(
                        "Enter a receipt ID to check its recorded folders against this WoW version.",
                    ))
                    .push(TextInput::new(
                        &mut self.receipt_input,
                        "Receipt ID",
                        &self.model.receipt_id,
                        Message::ReceiptIdChanged,
                    ));
                let mut inspect = Button::new(&mut self.inspect, Text::new("Check receipt"));
                if self.model.can_inspect_receipt() {
                    inspect = inspect.on_press(Message::InspectReceipt);
                }
                content = content.push(inspect).push(Text::new(
                    "Receipts record matching file contents. They do not authenticate the source or authorize changes.",
                ));
                if let Some(preview) = &self.model.receipt {
                    content = content
                        .push(Text::new("Receipt matches these folders:"))
                        .push(Text::new(folder_summary(&preview.folders)))
                        .push(Text::new(source_summary(&preview.source)));
                } else if let Some(error) = &self.model.receipt_error {
                    content = content
                        .push(Text::new(
                            "We couldn't verify this receipt. It may be missing, invalid, or out of date.",
                        ))
                        .push(Text::new(clip(error, ERROR_CHARS)).size(14));
                } else if let Some(status) = self.model.receipt_status() {
                    content = content.push(Text::new(status));
                } else {
                    content = content.push(Text::new("No receipt has been checked."));
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
        // Complete the headless snapshot without starting an executor.
        let mut model = Model::new(root);
        model.refresh();
        let mut app = Preview::from_model(model, false);
        // Construct the same view as the native entry; no window or executor is started.
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
    fn deliver(model: &mut Model, command: Command<Message>) -> bool {
        let mut futures = command.futures();
        assert_eq!(futures.len(), 1);
        match async_std::task::block_on(futures.remove(0)) {
            Message::InventoryDone(request, result) => model.complete_scan(request, result),
            Message::ReceiptDone(request, result) => model.complete_receipt(request, result),
            other => panic!("Unexpected worker message: {:?}", other),
        }
    }

    fn scanned_preview(root: PathBuf, native_check: bool) -> Preview {
        let (mut app, command) = Preview::new(Flags { root, native_check });
        assert!(deliver(&mut app.model, command));
        app.synchronize_choices();
        app
    }
    #[test]
    fn selection_never_falls_back_to_another_product_or_survives_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let mut model = Model {
            root: temp.path().join("missing-root"),
            inventory: Some(Arc::new(Discovery {
                root: PathBuf::new(),
                installations: vec![],
                issues: vec!["unknown product".into()],
            })),
            error: None,
            selected: None,
            ..Model::new(PathBuf::new())
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
        assert!(app.model.error.is_none());
        assert!(app.model.inventory.is_none());
        assert!(app.model.scan.is_some());
        assert_eq!(command.futures().len(), 1);
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
        let mut app = scanned_preview(temp.path().into(), false);
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
        assert_eq!(command.futures().len(), 1);
        assert_eq!(smoke.mode(), iced::window::Mode::Hidden);
    }

    fn receipt_model() -> (tempfile::TempDir, Model) {
        let temp = tempfile::tempdir().unwrap();
        for directory in ["_retail_", "_classic_"] {
            std::fs::create_dir_all(temp.path().join(directory).join("Interface").join("AddOns"))
                .unwrap();
        }
        std::fs::write(
            temp.path().join(".build.info"),
            "Active!DEC:1|Product!STRING:0|Version!STRING:0\n1|wow|12.1.5.70575\n1|wow_classic|5.5.4.70579\n",
        )
        .unwrap();
        let mut model = Model::new(temp.path().into());
        model.refresh();
        model.select(Product::Retail);
        model.set_receipt_id("a".repeat(64));
        (temp, model)
    }

    fn matching_receipt(request: &ReceiptRequest) -> Arc<ajour_core::fs::OwnershipPreview> {
        Arc::new(ajour_core::fs::OwnershipPreview {
            id: request.id.clone(),
            root: request.addons.clone(),
            source: serde_json::json!({
                "repository": "fixture/addon",
                "product": request.product,
            }),
            folders: vec!["Foo".into()],
        })
    }

    #[test]
    fn receipt_result_checks_product_and_preserves_reported_fields() {
        let (_temp, mut model) = receipt_model();
        let request = model.begin_receipt().unwrap();
        assert_eq!(
            request.addons,
            model
                .installation()
                .unwrap()
                .directory
                .join("Interface")
                .join("AddOns")
        );
        assert!(model.complete_receipt(request.clone(), Ok(matching_receipt(&request))));
        let preview = model.receipt.as_ref().unwrap();
        assert_eq!(preview.folders, vec!["Foo"]);
        assert_eq!(preview.source["repository"], "fixture/addon");
        assert!(model.receipt_error.is_none());
        let retry = model.begin_receipt().unwrap();
        let wrong = ajour_core::fs::OwnershipPreview {
            id: retry.id.clone(),
            root: retry.addons.clone(),
            source: serde_json::json!({"product": "ClassicEra"}),
            folders: vec!["Foo".into()],
        };
        assert!(model.complete_receipt(retry, Ok(Arc::new(wrong))));
        assert!(model.receipt.is_none());
        assert_eq!(
            model.receipt_error.as_deref(),
            Some("Ownership source/product mismatch")
        );
    }

    #[test]
    fn pending_receipts_are_invalidated_by_input_selection_rescan_and_root_changes() {
        for change in 0..4 {
            let (temp, mut model) = receipt_model();
            let request = model.begin_receipt().unwrap();
            for _ in 0..8 {
                assert!(model.begin_receipt().is_none());
            }
            match change {
                0 => {
                    model.set_receipt_id("b".repeat(64));
                    model.set_receipt_id(request.id.clone());
                }
                1 => {
                    model.select(Product::ClassicProgression);
                    model.select(Product::Retail);
                }
                2 => {
                    model.refresh();
                    model.select(Product::Retail);
                }
                _ => {
                    model.root = model.root.join("another-root");
                    assert!(model.begin_scan().is_none());
                }
            }
            assert!(model.receipt.is_none());
            assert!(model.receipt_error.is_none());
            assert!(model.pending_receipt.is_none());
            assert!(model.receipt_worker.is_some());
            assert!(model.begin_receipt().is_none());
            let mut unrelated = request.clone();
            unrelated.token = Arc::new(());
            assert!(!model.complete_receipt(unrelated, Err("unrelated".into())));
            assert!(model.receipt_worker.is_some());
            assert!(!deliver(&mut model, receipt_command(request.clone())));
            assert!(model.receipt_worker.is_none());
            assert!(model.receipt.is_none());
            assert!(!model.complete_receipt(request.clone(), Ok(matching_receipt(&request))));
            if model.scan_requested {
                let scan = model.start_scan().unwrap();
                assert!(!model.complete_receipt(request, Err("duplicate".into())));
                assert!(model.scan.is_some());
                assert!(deliver(&mut model, scan_command(scan)));
            }
            model.root = temp.path().into();
            model.refresh();
            model.select(Product::Retail);
            assert!(model.begin_receipt().is_some());
        }
    }

    #[test]
    fn completed_receipts_are_cleared_before_new_work() {
        for change in 0..4 {
            let (_temp, mut model) = receipt_model();
            let request = model.begin_receipt().unwrap();
            assert!(model.complete_receipt(request.clone(), Ok(matching_receipt(&request))));
            match change {
                0 => model.set_receipt_id("b".repeat(64)),
                1 => model.select(Product::ClassicProgression),
                2 => {
                    assert!(model.begin_scan().is_some());
                }
                _ => {
                    model.root = model.root.join("another-root");
                    assert!(model.begin_scan().is_some());
                }
            }
            assert!(model.receipt.is_none());
            assert!(model.receipt_error.is_none());
        }
    }

    #[test]
    fn receipt_capacity_rejects_duplicates_and_errors_allow_retry() {
        let (_temp, mut model) = receipt_model();
        let failure = model.begin_receipt().unwrap();
        assert!(model.begin_receipt().is_none());
        assert!(deliver(&mut model, receipt_command(failure.clone())));
        assert!(model.receipt.is_none());
        assert!(model.receipt_error.is_some());
        assert!(model.receipt_worker.is_none());
        let retry = model.begin_receipt().unwrap();
        assert!(model.receipt_error.is_none());
        assert!(!model.complete_receipt(failure, Err("late failure".into())));
        assert!(model
            .receipt_worker
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &retry.token)));
        assert!(model.begin_receipt().is_none());
        assert!(model.complete_receipt(retry.clone(), Ok(matching_receipt(&retry))));
        assert!(!model.complete_receipt(retry, Err("duplicate".into())));
        assert!(model.receipt.is_some());
        assert!(model.receipt_worker.is_none());
    }

    #[test]
    fn inventory_completions_reject_old_requests_and_changed_roots() {
        let (_temp, mut model) = receipt_model();
        let original = model.root.clone();
        let older = model.begin_scan().unwrap();
        for _ in 0..8 {
            assert!(model.begin_scan().is_none());
            assert!(model.begin_receipt().is_none());
        }
        assert!(!deliver(&mut model, scan_command(older.clone())));
        assert!(model.scan.is_none());
        let newer = model.start_scan().unwrap();
        assert!(!model.complete_scan(older, Err("late failure".into())));
        assert!(model
            .scan
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &newer.token)));
        assert!(deliver(&mut model, scan_command(newer.clone())));
        assert!(model.inventory.is_some());
        assert!(!model.complete_scan(newer, Err("duplicate".into())));
        let changed = model.begin_scan().unwrap();
        model.root = model.root.join("different-root");
        assert!(!deliver(&mut model, scan_command(changed)));
        assert!(model.scan.is_none());
        let failing = model.begin_scan().unwrap();
        assert!(deliver(&mut model, scan_command(failing)));
        assert!(model.error.is_some());
        assert!(model.inventory.is_none());
        model.root = original;
        let retry = model.begin_scan().unwrap();
        assert!(deliver(&mut model, scan_command(retry)));
        assert!(model.error.is_none());
        assert!(model.inventory.is_some());
    }

    #[test]
    fn receipt_root_guard_rejects_a_changed_root_before_another_scan() {
        let (_temp, mut model) = receipt_model();
        let request = model.begin_receipt().unwrap();
        model.root = model.root.join("different-root");
        assert!(!model.complete_receipt(request.clone(), Ok(matching_receipt(&request))));
        assert!(model.receipt.is_none());
    }

    #[test]
    fn receipt_display_is_unicode_safe_and_bounded() {
        assert_eq!(clip("界界界界", 3), "界界界...");
        assert_eq!(clip("界界界", 3), "界界界");
        assert_eq!(clip("", 3), "");
        // Large, nested, arbitrary source: only repository and product are shown.
        let big = serde_json::Value::Array(vec![serde_json::Value::from("x".repeat(1000)); 1000]);
        let source = serde_json::json!({
            "repository": "界".repeat(5000),
            "product": "Retail",
            "extra": {"nested": big},
        });
        let text = source_summary(&source);
        assert!(text.chars().count() < 200, "{}", text.chars().count());
        assert!(text.contains("(Retail)") && !text.contains("xxxx"));
        assert!(text.contains(&"界".repeat(SOURCE_FIELD_CHARS)));
        // Missing or non-string fields are reported, not hidden.
        assert_eq!(
            source_summary(&serde_json::json!({"repository": 7})),
            "Recorded source: not recorded (not recorded)"
        );
        assert_eq!(
            source_summary(&serde_json::Value::Null),
            "Recorded source: not recorded (not recorded)"
        );
        // Folder list: count and per-name length are both capped.
        let folders: Vec<String> = (0..10_000)
            .map(|i| format!("界{i}{}", "z".repeat(500)))
            .collect();
        let text = folder_summary(&folders);
        assert!(text.chars().count() < FOLDERS_SHOWN * (FOLDER_CHARS + 5) + 40);
        assert!(text.ends_with(", and 9980 more"));
        assert_eq!(folder_summary(&["Foo".into(), "Bar".into()]), "Foo, Bar");
    }

    // GUI: append inside the existing #[cfg(test)] mod tests containing
    // deliver() and scanned_preview(). No existing code changes.
    // Added 2026-10-28; original GPLv3 applies.

    #[test]
    #[ignore = "child of the explicit real-writer bridge parent; never run standalone"]
    fn real_writer_receipt_reaches_gui_completion_child() {
        use std::io::Write;

        let root = PathBuf::from(
            std::env::var_os("PATCHLOOM_GUI_BRIDGE_ROOT")
                .expect("Run through the core bridge parent"),
        );
        let id = std::env::var("PATCHLOOM_GUI_BRIDGE_ID")
            .expect("Missing installer-returned receipt ID");
        let source: serde_json::Value = serde_json::from_str(
            &std::env::var("PATCHLOOM_GUI_BRIDGE_SOURCE")
                .expect("Missing writer-source expectation"),
        )
        .unwrap();
        let ack = root.parent().unwrap().join("gui-bridge-complete");
        assert!(!ack.exists(), "Completion acknowledgement must be fresh");
        assert_eq!(source["repository"], "fixture/addon");
        assert_eq!(source["product"], "Retail");

        // Real discovery through the existing scan Command helper.
        let mut app = scanned_preview(root.clone(), false);
        assert!(app.model.error.is_none());
        assert!(!app.model.needs_review());
        app.model.select(Product::Retail);
        app.model.set_receipt_id(id.clone());

        let request = app.model.begin_receipt().unwrap();
        let addons = root.join("_retail_").join("Interface").join("AddOns");
        assert_eq!(request.root, root);
        assert_eq!(request.addons, addons);
        assert_eq!(request.product, Product::Retail);
        assert_eq!(request.id, id);
        assert!(app.model.pending_receipt.is_some());
        assert!(app.model.receipt_worker.is_some());

        // Existing real Command future -> spawn_blocking -> receipt_snapshot
        // -> inspect_ownership -> Message::ReceiptDone -> complete_receipt.
        assert!(deliver(&mut app.model, receipt_command(request)));

        assert!(app.model.receipt_error.is_none());
        assert!(app.model.pending_receipt.is_none());
        assert!(app.model.receipt_worker.is_none());
        let preview = app
            .model
            .receipt
            .as_ref()
            .expect("GUI success receipt missing");
        assert_eq!(preview.id, id);
        assert_eq!(preview.root, addons.canonicalize().unwrap());
        assert_eq!(preview.source, source);
        assert_eq!(preview.folders, vec!["Foo".to_owned()]);
        assert_eq!(folder_summary(&preview.folders), "Foo");
        assert_eq!(
            source_summary(&preview.source),
            "Recorded source: fixture/addon (Retail)",
        );

        // Construct the existing success view without a window or native executor.
        drop(app.view());

        // Outside the copied WoW tree; never replace an existing acknowledgement.
        let mut completed = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&ack)
            .unwrap();
        completed
            .write_all(b"writer-receipt/gui-completion/v1\n")
            .unwrap();
    }

    // Added 2026-10-28; original GPLv3 applies.
    #[test]
    fn receipt_view_stays_busy_after_id_or_product_invalidation() {
        for change_product in [false, true] {
            let (_temp, model) = receipt_model();
            let mut app = Preview::from_model(model, false);
            assert!(app.model.can_inspect_receipt());
            assert_eq!(app.model.receipt_status(), None);

            let request = app.model.begin_receipt().unwrap();
            assert!(!app.model.can_inspect_receipt());
            assert_eq!(app.model.receipt_status(), Some("Checking receipt..."));

            if change_product {
                app.model.select(Product::ClassicProgression);
            } else {
                app.model.set_receipt_id("b".repeat(64));
            }

            assert!(app.model.pending_receipt.is_none());
            assert!(app
                .model
                .receipt_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token)));
            assert!(!app.model.can_inspect_receipt());
            assert_eq!(app.model.receipt_status(), Some("Checking receipt..."));
            assert!(app.model.begin_receipt().is_none());
            drop(app.view());

            // The invalidated result releases capacity but cannot populate
            // the newly selected receipt/product.
            assert!(!deliver(&mut app.model, receipt_command(request)));
            assert!(app.model.receipt_worker.is_none());
            assert!(app.model.receipt.is_none());
            assert!(app.model.receipt_error.is_none());
            assert_eq!(app.model.receipt_status(), None);
            assert!(app.model.can_inspect_receipt());
            drop(app.view());

            app.model.set_receipt_id(String::new());
            assert!(!app.model.can_inspect_receipt());
        }
    }

    #[test]
    fn scan_view_shows_queued_then_running_then_completed_status() {
        let (_temp, model) = receipt_model();
        let mut app = Preview::from_model(model, false);
        let receipt = app.model.begin_receipt().unwrap();

        assert!(app.model.begin_scan().is_none());
        app.synchronize_choices();
        assert!(app.model.inventory.is_none());
        assert!(app.model.scan.is_none());
        assert!(app.model.scan_requested);
        assert_eq!(
            app.model.scan_status(),
            Some("Waiting to read addon folders...")
        );
        assert_eq!(app.model.receipt_status(), Some("Checking receipt..."));
        assert!(!app.model.can_inspect_receipt());
        drop(app.view());

        assert!(!deliver(&mut app.model, receipt_command(receipt)));
        assert_eq!(app.model.receipt_status(), None);
        assert_eq!(
            app.model.scan_status(),
            Some("Waiting to read addon folders...")
        );
        assert!(!app.model.can_inspect_receipt());

        let scan = app.model.start_scan().unwrap();
        assert!(!app.model.scan_requested);
        assert_eq!(app.model.scan_status(), Some("Reading addon folders..."));
        assert!(!app.model.can_inspect_receipt());
        drop(app.view());

        assert!(deliver(&mut app.model, scan_command(scan)));
        app.synchronize_choices();
        assert_eq!(app.model.scan_status(), None);
        assert_eq!(app.model.receipt_status(), None);
        assert!(app.model.inventory.is_some());
        assert!(!app.model.can_inspect_receipt());
        app.model.select(Product::Retail);
        assert!(app.model.can_inspect_receipt());
        drop(app.view());
    }

    #[test]
    fn receipt_display_escapes_controls_within_existing_output_budgets() {
        assert_eq!(clip("界é🙂", 3), "界é🙂");
        assert_eq!(clip("界é🙂x", 3), "界é🙂...");
        assert_eq!(clip("", 0), "");
        assert_eq!(clip("x", 0), "...");
        assert_eq!(clip("A\n\u{202e}界", 40), r"A\u{a}\u{202e}界");

        // Never split an escape to fill the remaining budget.
        assert_eq!(clip("\u{202e}", 7), "...");
        assert_eq!(clip("\u{202e}", 8), r"\u{202e}");
        assert_eq!(clip("\u{202e}x", 8), r"\u{202e}...");

        let controls = [
            '\0', '\t', '\n', '\r', '\u{001b}', '\u{0085}', '\u{061c}', '\u{200e}', '\u{200f}',
            '\u{2028}', '\u{2029}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}',
            '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ];
        for ch in controls {
            let raw = ch.to_string();
            assert_eq!(clip(&raw, 20), ch.escape_unicode().to_string());
        }

        let raw = "界\n\u{202e}".repeat(1000);
        for limit in [SOURCE_FIELD_CHARS, FOLDER_CHARS, ERROR_CHARS] {
            let displayed = clip(&raw, limit);
            assert!(displayed.chars().count() <= limit + 3);
            assert!(displayed.starts_with("界"));
            assert!(displayed.ends_with("..."));
            assert!(!displayed.chars().any(char::is_control));
            assert!(!displayed.contains('\u{202e}'));
        }

        let source = serde_json::json!({
            "repository": raw.clone(),
            "product": "Retail",
        });
        let source_before = source.clone();
        let folders = vec![raw.clone()];
        let source_text = source_summary(&source);
        let folder_text = folder_summary(&folders);
        assert!(source_text.contains(r"\u{a}"));
        assert!(source_text.contains(r"\u{202e}"));
        assert!(source_text.ends_with(" (Retail)"));
        assert!(folder_text.chars().count() <= FOLDER_CHARS + 3);
        assert!(!folder_text.contains('\n'));
        assert!(!folder_text.contains('\u{202e}'));
        assert_eq!(source, source_before);
        assert_eq!(folders[0], raw);
    }
}
