//! Patchloom inventory and explicit recovery, 2026-10-28; original GPLv3 applies.
//! Inventory and recovery inspection do not change files. Restoring originals
//! requires a validated preview and fresh confirmation for the selected scope.
use ajour_core::discovery::{self, Discovery, Installation, Product};
use iced::{
    button, container, scrollable, text_input, Application, Button, Clipboard, Column, Command,
    Container, Element, Length, Row, Scrollable, Settings, Subscription, Text, TextInput,
};
use serde::Serialize;
use std::{path::PathBuf, sync::Arc, time::Instant};

#[derive(Debug, Clone)]
struct ScanRequest {
    root: PathBuf,
    generation: Arc<()>,
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
// Modified 2026-10-28; original GPLv3 applies.
#[derive(Debug, Clone)]
struct ReleaseRequest {
    root: PathBuf,
    repository: String,
    product: Product,
    inventory: Arc<Discovery>,
    token: Arc<()>,
}

// Added 2026-10-28; original GPLv3 applies.
#[derive(Debug, Clone)]
enum RecoveryAction {
    Inspect,
    Preview,
    Restore(Arc<ajour_core::fs::RecoveryApproval>),
}

#[derive(Debug, Clone)]
struct RecoveryRequest {
    root: PathBuf,
    addons: PathBuf,
    product: Product,
    generation: Arc<()>,
    token: Arc<()>,
    action: RecoveryAction,
}

impl RecoveryRequest {
    fn same_request(&self, other: &Self) -> bool {
        self.root == other.root
            && self.addons == other.addons
            && self.product == other.product
            && Arc::ptr_eq(&self.generation, &other.generation)
            && Arc::ptr_eq(&self.token, &other.token)
            && match (&self.action, &other.action) {
                (RecoveryAction::Inspect, RecoveryAction::Inspect)
                | (RecoveryAction::Preview, RecoveryAction::Preview) => true,
                (RecoveryAction::Restore(a), RecoveryAction::Restore(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

#[derive(Debug, Clone)]
struct RecoveryPreview {
    request: RecoveryRequest,
    approval: Arc<ajour_core::fs::RecoveryApproval>,
}

#[derive(Debug, Clone)]
enum RecoveryResult {
    Inspection(Result<Arc<ajour_core::fs::RecoveryInspection>, String>),
    Preview(Result<Arc<ajour_core::fs::RecoveryApproval>, String>),
    Restore {
        outcome: Result<ajour_core::fs::RecoveryRestoreOutcome, String>,
        inspection: Result<Arc<ajour_core::fs::RecoveryInspection>, String>,
    },
}

impl RecoveryResult {
    fn matches_action(&self, action: &RecoveryAction) -> bool {
        matches!(
            (self, action),
            (Self::Inspection(_), RecoveryAction::Inspect)
                | (Self::Preview(_), RecoveryAction::Preview)
                | (Self::Restore { .. }, RecoveryAction::Restore(_))
        )
    }
}

#[derive(Debug, Clone)]
struct ReleaseSummary {
    repository: String,
    product: Product,
    version: String,
}

// Modified 2026-10-28; original GPLv3 applies.
// Modified 2026-10-28; original GPLv3 applies.
// Declaration order makes Blocked dominate finite deadlines when merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RetryDeadline {
    At(Instant),
    Blocked,
}

impl RetryDeadline {
    fn from_wait(now: Instant, wait: std::time::Duration) -> Self {
        if wait == std::time::Duration::MAX {
            return Self::Blocked;
        }
        now.checked_add(wait).map_or(Self::Blocked, Self::At)
    }

    fn allows(self, now: Instant) -> bool {
        matches!(self, Self::At(deadline) if now >= deadline)
    }
}

#[derive(Debug)]
enum ReleaseFailure {
    Other(String),
    RateLimited(RetryDeadline),
}

fn release_failure(error: Box<dyn std::error::Error + Send + Sync>) -> ReleaseFailure {
    if let Some(ajour_core::error::DownloadError::RateLimited { retry_after, .. }) =
        error.downcast_ref::<ajour_core::error::DownloadError>()
    {
        return ReleaseFailure::RateLimited(RetryDeadline::from_wait(Instant::now(), *retry_after));
    }
    ReleaseFailure::Other(clip(&error.to_string(), ERROR_CHARS))
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
    #[serde(skip)]
    generation: Arc<()>,
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
    #[serde(skip)]
    repository: String,
    #[serde(skip)]
    release_result: Option<Result<ReleaseSummary, String>>,
    #[serde(skip)]
    pending_release: Option<ReleaseRequest>,
    #[serde(skip)]
    release_worker: Option<Arc<()>>,
    #[serde(skip)]
    release_retry_at: Option<RetryDeadline>,
    // Recovery borrows receipt_worker's existing physical filesystem slot.
    // Logical invalidation clears pending_recovery, never recovery_job.
    #[serde(skip)]
    recovery_job: Option<RecoveryRequest>,
    #[serde(skip)]
    pending_recovery: Option<RecoveryRequest>,
    #[serde(skip)]
    recovery_inspection: Option<Arc<ajour_core::fs::RecoveryInspection>>,
    #[serde(skip)]
    recovery_preview: Option<RecoveryPreview>,
    #[serde(skip)]
    recovery_ack: String,
    #[serde(skip)]
    recovery_page: usize,
    #[serde(skip)]
    recovery_error: Option<String>,
    #[serde(skip)]
    recovery_outcome: Option<Result<ajour_core::fs::RecoveryRestoreOutcome, String>>,
    #[serde(skip)]
    recovery_post_error: Option<String>,
}
impl Model {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            generation: Arc::new(()),
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
            repository: String::new(),
            release_result: None,
            pending_release: None,
            release_worker: None,
            release_retry_at: None,
            recovery_job: None,
            pending_recovery: None,
            recovery_inspection: None,
            recovery_preview: None,
            recovery_ack: String::new(),
            recovery_page: 0,
            recovery_error: None,
            recovery_outcome: None,
            recovery_post_error: None,
        }
    }
    // Modified 2026-10-28; original GPLv3 applies.
    // Logical invalidation never releases a physical worker or provider wait.
    // Logical invalidation never releases a physical worker or provider wait.
    fn invalidate_snapshot(&mut self) {
        if self.restoring() {
            return;
        }
        self.generation = Arc::new(());
        self.scan_requested = false;
        self.inventory = None;
        self.selected = None;
        self.error = None;
        self.clear_receipt();
        self.clear_release();
        self.clear_recovery();
    }

    fn set_root(&mut self, root: PathBuf) {
        if self.restoring() {
            return;
        }
        self.invalidate_snapshot();
        self.root = root;
    }

    // Synchronous only for the deterministic headless check and fixture tests.
    fn refresh(&mut self) {
        if let Some(request) = self.begin_scan() {
            let result = inventory_snapshot(&request);
            self.complete_scan(request, result);
        }
    }
    fn begin_scan(&mut self) -> Option<ScanRequest> {
        if self.restoring() {
            return None;
        }
        self.invalidate_snapshot();
        if self.root.as_os_str().is_empty() {
            self.error = Some("Enter a copied WoW folder path, then click Open.".into());
            return None;
        }
        self.scan_requested = true;
        self.start_scan()
    }
    // Modified 2026-10-28; original GPLv3 applies.
    // ponytail: one snapshot worker; split only if measured latency requires it.
    fn start_scan(&mut self) -> Option<ScanRequest> {
        if !self.scan_requested
            || self.root.as_os_str().is_empty()
            || self.scan.is_some()
            || self.receipt_worker.is_some()
        {
            return None;
        }
        self.scan_requested = false;
        let request = ScanRequest {
            root: self.root.clone(),
            generation: self.generation.clone(),
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
        if self.root != request.root
            || self.scan_requested
            || !Arc::ptr_eq(&self.generation, &request.generation)
        {
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
        // Recovery uses this physical slot too. A receipt message cannot
        // complete a recovery job, even if supplied with its token.
        if self.recovery_job.is_some() {
            return false;
        }
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
        if self.restoring() {
            return;
        }
        if self
            .inventory
            .as_ref()
            .is_some_and(|r| r.installations.iter().any(|i| i.product == product))
        {
            if self.selected != Some(product) {
                self.clear_receipt();
                self.clear_release();
                self.clear_recovery();
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
        if self.recovery_job.is_some() {
            None
        } else {
            self.receipt_worker.as_ref().map(|_| "Checking receipt...")
        }
    }

    fn can_inspect_receipt(&self) -> bool {
        !self.receipt_id.is_empty()
            && self.installation().is_some()
            && self.scan.is_none()
            && !self.scan_requested
            && self.receipt_worker.is_none()
    }

    // Invalidation never releases physical provider capacity.
    fn clear_release(&mut self) {
        self.pending_release = None;
        self.release_result = None;
    }

    fn set_repository(&mut self, repository: String) {
        if self.repository != repository {
            self.clear_release();
            self.repository = repository;
        }
    }

    fn release_status(&self) -> Option<&'static str> {
        self.release_worker
            .as_ref()
            .map(|_| "Checking GitHub release...")
    }

    fn release_inputs_ready(&self) -> bool {
        valid_repository(&self.repository)
            && self.installation().is_some()
            && self.scan.is_none()
            && !self.scan_requested
            && !self.restoring()
            && self.release_worker.is_none()
    }

    fn can_check_release(&self) -> bool {
        self.can_check_release_at(Instant::now())
    }

    fn can_check_release_at(&self, now: Instant) -> bool {
        self.release_inputs_ready()
            && self
                .release_retry_at
                .is_none_or(|deadline| deadline.allows(now))
    }

    fn begin_release(&mut self) -> Option<ReleaseRequest> {
        if !self.can_check_release() {
            return None;
        }
        let installation = self.installation()?;
        let request = ReleaseRequest {
            root: self.root.clone(),
            repository: self.repository.clone(),
            product: installation.product,
            inventory: self.inventory.as_ref()?.clone(),
            token: Arc::new(()),
        };
        self.clear_release();
        self.release_worker = Some(request.token.clone());
        self.pending_release = Some(request.clone());
        Some(request)
    }

    fn complete_release(
        &mut self,
        request: ReleaseRequest,
        result: Result<String, String>,
    ) -> bool {
        if !self
            .release_worker
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &request.token))
        {
            return false;
        }
        // Only the matching physical completion may release this reservation.
        self.release_worker = None;
        let pending = self.pending_release.take();
        if self.root != request.root
            || self.repository != request.repository
            || self.selected != Some(request.product)
            || self.scan.is_some()
            || self.scan_requested
            || !pending
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &request.token))
            || !self
                .inventory
                .as_ref()
                .is_some_and(|report| Arc::ptr_eq(report, &request.inventory))
        {
            return false;
        }
        self.release_result = Some(
            result
                .map(|version| ReleaseSummary {
                    repository: request.repository,
                    product: request.product,
                    version: clip(&version, SOURCE_FIELD_CHARS),
                })
                .map_err(|error| clip(&error, ERROR_CHARS)),
        );
        true
    }

    fn complete_release_rate_limited(
        &mut self,
        request: ReleaseRequest,
        deadline: RetryDeadline,
    ) -> bool {
        if !self
            .release_worker
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &request.token))
        {
            return false;
        }

        // Only the matching physical completion may establish or extend a
        // wait. Stale display sources still establish it; foreign/duplicate
        // completions cannot change it. Blocked dominates every finite wait.
        self.release_retry_at = Some(
            self.release_retry_at
                .map_or(deadline, |existing| existing.max(deadline)),
        );
        self.complete_release(
            request,
            Err("The server returned a rate limit. Release checks are paused.".into()),
        )
    }

    // Added 2026-10-28; original GPLv3 applies.
    fn restoring(&self) -> bool {
        self.recovery_job
            .as_ref()
            .is_some_and(|job| matches!(&job.action, RecoveryAction::Restore(_)))
    }

    fn clear_recovery(&mut self) {
        self.pending_recovery = None;
        self.recovery_inspection = None;
        self.recovery_preview = None;
        self.recovery_ack.clear();
        self.recovery_page = 0;
        self.recovery_error = None;
        self.recovery_outcome = None;
        self.recovery_post_error = None;
        // recovery_job and receipt_worker describe physical work, not display.
    }

    fn recovery_scope_current(&self, request: &RecoveryRequest) -> bool {
        if self.root != request.root
            || self.selected != Some(request.product)
            || !Arc::ptr_eq(&self.generation, &request.generation)
        {
            return false;
        }
        // Restore admission deliberately invalidates the old inventory.
        // Root/product/generation cannot change through admitted GUI actions
        // until the physical restore and post-inspection have completed.
        if matches!(&request.action, RecoveryAction::Restore(_)) {
            return true;
        }
        self.installation().is_some_and(|installation| {
            installation.directory.join("Interface").join("AddOns") == request.addons
        })
    }

    fn recovery_slot_available(&self) -> bool {
        self.scan.is_none()
            && !self.scan_requested
            && self.receipt_worker.is_none()
            && self.recovery_job.is_none()
    }

    fn can_inspect_recovery(&self) -> bool {
        self.installation().is_some() && self.recovery_slot_available()
    }

    fn can_preview_recovery(&self) -> bool {
        self.can_inspect_recovery()
            && self
                .recovery_inspection
                .as_ref()
                .is_some_and(|inspection| inspection.phase != "none")
    }

    fn begin_recovery(&mut self, preview: bool) -> Option<RecoveryRequest> {
        if if preview {
            !self.can_preview_recovery()
        } else {
            !self.can_inspect_recovery()
        } {
            return None;
        }
        let installation = self.installation()?;
        let request = RecoveryRequest {
            root: self.root.clone(),
            addons: installation.directory.join("Interface").join("AddOns"),
            product: installation.product,
            generation: self.generation.clone(),
            token: Arc::new(()),
            action: if preview {
                RecoveryAction::Preview
            } else {
                RecoveryAction::Inspect
            },
        };
        if !preview {
            self.clear_recovery();
        } else {
            self.recovery_preview = None;
            self.recovery_ack.clear();
            self.recovery_page = 0;
            self.recovery_error = None;
            self.recovery_outcome = None;
            self.recovery_post_error = None;
        }

        // Reserve synchronously, before constructing the Command. Existing
        // scan/receipt admission already checks this physical reservation.
        self.receipt_worker = Some(request.token.clone());
        self.recovery_job = Some(request.clone());
        self.pending_recovery = Some(request.clone());
        Some(request)
    }

    fn current_recovery_preview(&self, preview: &RecoveryPreview) -> bool {
        self.recovery_scope_current(&preview.request)
            && self.recovery_preview.as_ref().is_some_and(|current| {
                current.request.same_request(&preview.request)
                    && Arc::ptr_eq(&current.approval, &preview.approval)
                    && current.approval.root() == preview.approval.root()
            })
    }

    fn set_recovery_ack(&mut self, preview: &RecoveryPreview, answer: String) {
        if self.recovery_slot_available() && self.current_recovery_preview(preview) {
            // No trimming, case folding or reuse across preview identities.
            self.recovery_ack = answer.chars().take(7).collect();
        }
    }

    fn cancel_recovery_preview(&mut self, preview: &RecoveryPreview) {
        if self.recovery_slot_available() && self.current_recovery_preview(preview) {
            self.recovery_preview = None;
            self.recovery_ack.clear();
            self.recovery_page = 0;
            self.recovery_error = Some("Preview cancelled. No addon folders were changed.".into());
        }
    }

    fn can_restore_recovery(&self, preview: &RecoveryPreview) -> bool {
        self.recovery_slot_available()
            && self.recovery_ack == "RESTORE"
            && self.current_recovery_preview(preview)
    }

    fn begin_restore_recovery(&mut self, preview: &RecoveryPreview) -> Option<RecoveryRequest> {
        if !self.can_restore_recovery(preview) {
            return None;
        }
        let request = RecoveryRequest {
            root: preview.request.root.clone(),
            addons: preview.request.addons.clone(),
            product: preview.request.product,
            generation: preview.request.generation.clone(),
            token: Arc::new(()),
            action: RecoveryAction::Restore(preview.approval.clone()),
        };

        // Consume consent before dispatch. A duplicate click cannot reuse it.
        self.recovery_preview = None;
        self.recovery_ack.clear();
        self.recovery_page = 0;
        self.recovery_inspection = None;
        self.recovery_error = None;
        self.recovery_outcome = None;
        self.recovery_post_error = None;

        // This attempt can write. Old inventory/ownership and provider display
        // assumptions cease to apply even if core later returns an error.
        self.inventory = None;
        self.error = None;
        self.clear_receipt();
        self.clear_release();
        // Preserve selected scope, generation, physical provider reservation
        // and release_retry_at. No provider request is queued or retried.
        self.receipt_worker = Some(request.token.clone());
        self.recovery_job = Some(request.clone());
        self.pending_recovery = Some(request.clone());
        Some(request)
    }

    fn complete_recovery(&mut self, request: RecoveryRequest, result: RecoveryResult) -> bool {
        if !self
            .recovery_job
            .as_ref()
            .is_some_and(|job| job.same_request(&request) && result.matches_action(&job.action))
            || !self
                .receipt_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token))
        {
            return false;
        }

        // Only the matching physical completion releases the borrowed slot.
        // A stale matching completion releases it without publishing.
        self.recovery_job = None;
        self.receipt_worker = None;
        let pending = self.pending_recovery.take();
        if !pending
            .as_ref()
            .is_some_and(|pending| pending.same_request(&request))
            || !self.recovery_scope_current(&request)
        {
            return false;
        }

        match result {
            RecoveryResult::Inspection(inspection) => match inspection {
                Ok(inspection) => self.recovery_inspection = Some(inspection),
                Err(error) => self.recovery_error = Some(error),
            },
            RecoveryResult::Preview(approval) => match approval {
                Ok(approval) => {
                    // Both readings came from this selected product scope.
                    // Canonical-root disagreement requires another inspection.
                    if self
                        .recovery_inspection
                        .as_ref()
                        .is_none_or(|inspection| inspection.root != approval.root())
                    {
                        self.recovery_inspection = None;
                        self.recovery_error =
                            Some("The AddOns folder changed. Inspect recovery again.".into());
                    } else {
                        self.recovery_preview = Some(RecoveryPreview { request, approval });
                    }
                }
                Err(error) => self.recovery_error = Some(error),
            },
            RecoveryResult::Restore {
                outcome,
                inspection,
            } => {
                // Keep the restore result even when post-inspection failed.
                self.recovery_outcome = Some(outcome);
                match inspection {
                    Ok(inspection) => self.recovery_inspection = Some(inspection),
                    Err(error) => self.recovery_post_error = Some(error),
                }
            }
        }
        true
    }

    fn recovery_status(&self) -> Option<&'static str> {
        self.recovery_job.as_ref().map(|job| match &job.action {
            RecoveryAction::Inspect => "Reading recovery state...",
            RecoveryAction::Preview => "Checking the restore plan...",
            RecoveryAction::Restore(_) => {
                "Restoring originals and checking the result. Keep Patchloom open and wait before changing folders."
            }
        })
    }

    fn set_recovery_page(&mut self, preview: &RecoveryPreview, page: usize) {
        if self.recovery_slot_available() && self.current_recovery_preview(preview) {
            let count = preview
                .approval
                .originals()
                .len()
                .max(preview.approval.move_aside_if_present().len());
            let last = count.saturating_sub(1) / FOLDERS_SHOWN;
            self.recovery_page = page.min(last);
        }
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
// Added 2026-10-28; original GPLv3 applies.
fn recovery_snapshot(request: &RecoveryRequest) -> RecoveryResult {
    match &request.action {
        RecoveryAction::Inspect => RecoveryResult::Inspection(
            ajour_core::fs::inspect_recovery(&request.addons)
                .map(Arc::new)
                .map_err(|error| error.to_string()),
        ),
        RecoveryAction::Preview => RecoveryResult::Preview(
            ajour_core::fs::preflight_restore_originals(&request.addons)
                .map(Arc::new)
                .map_err(|error| error.to_string()),
        ),
        RecoveryAction::Restore(approval) => {
            // Reaching this worker means fresh explicit consent was admitted
            // synchronously. Never show a dialog or wait under the core lock.
            let outcome = ajour_core::fs::restore_originals_checked(
                &request.addons,
                approval.as_ref(),
                |originals, move_aside| {
                    originals == approval.originals()
                        && move_aside == approval.move_aside_if_present()
                },
            )
            .map_err(|error| error.to_string());

            // Run after every checked call, including Err. Completion is not
            // delivered, and physical GUI capacity is not released, until
            // this inspection finishes.
            let inspection = ajour_core::fs::inspect_recovery(&request.addons)
                .map(Arc::new)
                .map_err(|error| error.to_string());
            RecoveryResult::Restore {
                outcome,
                inspection,
            }
        }
    }
}

fn recovery_command(request: RecoveryRequest) -> Command<Message> {
    Command::perform(
        async move {
            let worker = request.clone();
            let result = async_std::task::spawn_blocking(move || recovery_snapshot(&worker)).await;
            (request, result)
        },
        |(request, result)| Message::RecoveryDone(request, result),
    )
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
// GUI admission mirrors the existing core grammar; core validation remains
// authoritative. Raw input is neither trimmed nor converted from a URL.
fn valid_repository(repository: &str) -> bool {
    if repository.len() > 201 {
        return false;
    }
    let Some((owner, name)) = repository.split_once('/') else {
        return false;
    };
    [owner, name].iter().all(|part| {
        !part.is_empty()
            && part.len() <= 100
            && *part != "."
            && *part != ".."
            && part
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch))
    })
}

fn release_command(request: ReleaseRequest) -> Command<Message> {
    // ponytail: one independent metadata check, no queue; local snapshot
    // capacity stays unchanged. Revisit only for measured concurrency needs.
    Command::perform(
        async move {
            let result = match request
                .inventory
                .installations
                .iter()
                .find(|installation| installation.product == request.product)
            {
                Some(installation) => {
                    ajour_core::github_releases::check(&request.repository, installation)
                        .await
                        .map(|candidate| clip(&candidate.version, SOURCE_FIELD_CHARS))
                        .map_err(release_failure)
                }
                None => Err(ReleaseFailure::Other(
                    "The selected WoW version is no longer available.".into(),
                )),
            };
            (request, result)
        },
        |(request, result)| match result {
            Ok(version) => Message::ReleaseDone(request, Ok(version)),
            Err(ReleaseFailure::Other(error)) => Message::ReleaseDone(request, Err(error)),
            Err(ReleaseFailure::RateLimited(deadline)) => {
                Message::ReleaseRateLimited(request, deadline)
            }
        },
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
    RootInputChanged(String),
    OpenRoot,
    Select(Product),
    Rescan,
    InventoryDone(ScanRequest, Result<Arc<Discovery>, String>),
    ReceiptIdChanged(String),
    InspectReceipt,
    ReceiptDone(
        ReceiptRequest,
        Result<Arc<ajour_core::fs::OwnershipPreview>, String>,
    ),
    RepositoryChanged(String),
    CheckRelease,
    ReleaseDone(ReleaseRequest, Result<String, String>),
    ReleaseRateLimited(ReleaseRequest, RetryDeadline),
    InspectRecovery,
    PreviewRecovery,
    RecoveryAckChanged(RecoveryPreview, String),
    RecoveryPage(RecoveryPreview, usize),
    RecoveryRootPage(RecoveryPreview, usize),
    CancelRecoveryPreview(RecoveryPreview),
    RestoreRecovery(RecoveryPreview),
    RecoveryDone(RecoveryRequest, RecoveryResult),
    NativeCheckDone,
}
struct Flags {
    root: PathBuf,
    native_check: bool,
}
// Added 2026-10-28; original GPLv3 applies.
// Added 2026-10-28; original GPLv3 applies.
fn recovery_root_pages(root: &std::path::Path) -> usize {
    // OS-string debug formatting preserves non-UTF-8 path distinctions.
    // Page the representation, not a lossy or clipped approval root.
    let text = format!("{:?}", root.as_os_str());
    text.chars().count().saturating_sub(1) / ERROR_CHARS + 1
}

fn recovery_root_chunk(root: &std::path::Path, page: usize) -> String {
    let page = page.min(recovery_root_pages(root).saturating_sub(1));
    let text = format!("{:?}", root.as_os_str());

    // Bound input characters before escaping. escape_default emits ASCII,
    // including visible escapes for controls, bidi characters and backslashes.
    // The complete path remains accessible across pages.
    text.chars()
        .skip(page.saturating_mul(ERROR_CHARS))
        .take(ERROR_CHARS)
        .flat_map(char::escape_default)
        .collect()
}

fn recovery_folder_page(folders: &[String], page: usize) -> String {
    if folders.is_empty() {
        return "None.".into();
    }

    let start = page.saturating_mul(FOLDERS_SHOWN);
    let lines: Vec<_> = folders
        .iter()
        .skip(start)
        .take(FOLDERS_SHOWN)
        .enumerate()
        .map(|(offset, name)| {
            // Approval names have already passed core's 255-byte component
            // validation. Escape each complete name; do not use FOLDER_CHARS.
            let name: String = name.chars().flat_map(char::escape_default).collect();
            format!(
                "{}. \"{}\"",
                start.saturating_add(offset).saturating_add(1),
                name
            )
        })
        .collect();

    if lines.is_empty() {
        "No folders on this page.".into()
    } else {
        lines.join("\n")
    }
}

fn recovery_outcome_text(
    outcome: &Result<ajour_core::fs::RecoveryRestoreOutcome, String>,
) -> String {
    use ajour_core::fs::RecoveryRestoreOutcome;

    match outcome {
        Ok(RecoveryRestoreOutcome::Cancelled) => {
            "Restore cancelled. No addon folders were changed.".into()
        }
        Ok(RecoveryRestoreOutcome::Changed) => {
            "Recovery state changed. No restore was started. Inspect recovery and preview a new plan before confirming again.".into()
        }
        Ok(RecoveryRestoreOutcome::RestoredRetained) => {
            "Original folders restored. Replacement folders and recovery journals are retained. Updates remain blocked pending cleanup.".into()
        }
        Err(error) => format!(
            "Restore failed: {}. Some folders may already have moved. Review the recovery state before trying again.",
            clip(error, ERROR_CHARS)
        ),
    }
}

struct RecoveryControls {
    inspect: button::State,
    preview: button::State,
    restore: button::State,
    cancel: button::State,
    previous: button::State,
    next: button::State,
    root_previous: button::State,
    root_next: button::State,
    acknowledgement: text_input::State,
    root_page: usize,
}

impl RecoveryControls {
    fn new() -> Self {
        Self {
            inspect: button::State::new(),
            preview: button::State::new(),
            restore: button::State::new(),
            cancel: button::State::new(),
            previous: button::State::new(),
            next: button::State::new(),
            root_previous: button::State::new(),
            root_next: button::State::new(),
            acknowledgement: text_input::State::new(),
            root_page: 0,
        }
    }
}

// Added 2026-10-28; original GPLv3 applies.
impl RecoveryControls {
    fn view<'a>(&'a mut self, model: &'a Model, root_edited: bool) -> Element<'a, Message> {
        let mut inspect = Button::new(&mut self.inspect, Text::new("Inspect recovery"));
        if !root_edited && model.can_inspect_recovery() {
            inspect = inspect.on_press(Message::InspectRecovery);
        }

        let mut preview_button = Button::new(&mut self.preview, Text::new("Preview restore"));
        if !root_edited && model.can_preview_recovery() {
            preview_button = preview_button.on_press(Message::PreviewRecovery);
        }

        let mut content = Column::new()
            .spacing(10)
            .push(Text::new("Recovery").size(22))
            .push(Text::new(
                "Inspection and preview do not change files. Restoring originals moves addon folders.",
            ))
            .push(
                Row::new()
                    .spacing(12)
                    .push(inspect)
                    .push(preview_button),
            );

        if let Some(product) = model.selected {
            content = content.push(Text::new(format!(
                "Recovery WoW version: {}",
                product_label(product)
            )));
        }

        if let Some(status) = model.recovery_status() {
            content = content.push(Text::new(status));
        }

        if let Some(error) = &model.recovery_error {
            content = content
                .push(Text::new("Recovery message:"))
                .push(Text::new(clip(error, ERROR_CHARS)).size(14));
        }

        if let Some(outcome) = &model.recovery_outcome {
            content = content
                .push(Text::new("Restore result").size(18))
                .push(Text::new(recovery_outcome_text(outcome)))
                .push(Text::new(
                    "Rescan folder to refresh inventory before another recovery check.",
                ));
        }

        if let Some(error) = &model.recovery_post_error {
            content = content
                .push(Text::new(
                    "The follow-up recovery check failed. The restore result above still applies, but the current recovery state could not be read.",
                ))
                .push(Text::new(clip(error, ERROR_CHARS)).size(14));
        }

        if let Some(inspection) = &model.recovery_inspection {
            content = content
                .push(Text::new("Recovery inspection").size(18))
                .push(
                    Text::new(format!(
                        "AddOns path summary: {}",
                        clip(&format!("{:?}", inspection.root.as_os_str()), ERROR_CHARS)
                    ))
                    .size(14),
                );

            if inspection.phase == "none" {
                content = content.push(Text::new("No recovery journal was found."));
            } else {
                content = content
                    .push(Text::new(format!(
                        "Journal phase: {}",
                        clip(&inspection.phase, SOURCE_FIELD_CHARS)
                    )))
                    .push(Text::new(format!(
                        "Recorded original folders: {}; incoming folders: {}.",
                        inspection.previous.len(),
                        inspection.incoming.len()
                    )))
                    .push(Text::new(format!(
                        "Backups found: {}; staged folders: {}; affected destinations present: {}.",
                        inspection.backups.len(),
                        inspection.staged.len(),
                        inspection.destinations.len()
                    )));
            }

            if let Some(phase) = &inspection.restore_phase {
                content = content.push(Text::new(format!(
                    "Restore phase: {}",
                    clip(phase, SOURCE_FIELD_CHARS)
                )));
            }

            content = content
                .push(Text::new(format!(
                    "Retained replacement folders: {}.",
                    inspection.retained_replacements.len()
                )))
                .push(Text::new(
                    "This inspection does not authorize a restore. Preview restore checks whether a restore plan is available.",
                ));
        }

        if let Some(preview) = model
            .recovery_preview
            .as_ref()
            .filter(|preview| model.current_recovery_preview(preview))
        {
            let ready = !root_edited && model.recovery_slot_available();
            let root_pages = recovery_root_pages(preview.approval.root());
            let root_page = self.root_page.min(root_pages.saturating_sub(1));
            let count = preview
                .approval
                .originals()
                .len()
                .max(preview.approval.move_aside_if_present().len());
            let pages = count.saturating_sub(1) / FOLDERS_SHOWN + 1;
            let page = model.recovery_page.min(pages.saturating_sub(1));

            content = content
                .push(Text::new("Restore plan").size(18))
                .push(Text::new(format!(
                    "WoW version: {}",
                    product_label(preview.request.product)
                )))
                .push(Text::new(format!(
                    "AddOns path, part {} of {} (escaped):",
                    root_page + 1,
                    root_pages
                )))
                .push(Text::new(recovery_root_chunk(
                    preview.approval.root(),
                    root_page,
                )).size(14))
                .push(Text::new(
                    "The path includes its recorded quotes and escapes. Each part continues the previous one.",
                ));

            let mut root_previous =
                Button::new(&mut self.root_previous, Text::new("Previous path part"));
            if ready && root_page > 0 {
                root_previous = root_previous
                    .on_press(Message::RecoveryRootPage(preview.clone(), root_page - 1));
            }

            let mut root_next = Button::new(&mut self.root_next, Text::new("Next path part"));
            if ready && root_page + 1 < root_pages {
                root_next =
                    root_next.on_press(Message::RecoveryRootPage(preview.clone(), root_page + 1));
            }

            content = content
                .push(Row::new().spacing(12).push(root_previous).push(root_next))
                .push(Text::new(format!(
                    "Folder page {} of {}. Names use quoted escapes.",
                    page + 1,
                    pages
                )))
                .push(Text::new(format!(
                    "Original folders to restore ({}):",
                    preview.approval.originals().len()
                )))
                .push(Text::new(recovery_folder_page(preview.approval.originals(), page)).size(14))
                .push(Text::new(format!(
                    "Current folder names checked by this plan ({}):",
                    preview.approval.move_aside_if_present().len()
                )))
                .push(
                    Text::new(recovery_folder_page(
                        preview.approval.move_aside_if_present(),
                        page,
                    ))
                    .size(14),
                );

            let mut previous = Button::new(&mut self.previous, Text::new("Previous folder page"));
            if ready && page > 0 {
                previous = previous.on_press(Message::RecoveryPage(preview.clone(), page - 1));
            }

            let mut next = Button::new(&mut self.next, Text::new("Next folder page"));
            if ready && page + 1 < pages {
                next = next.on_press(Message::RecoveryPage(preview.clone(), page + 1));
            }

            content = content
                .push(Row::new().spacing(12).push(previous).push(next))
                .push(Text::new(
                    "Folders moved aside are kept. A resumed restore continues from its recorded stage. Replacement folders and recovery journals remain; updates stay blocked pending cleanup.",
                ))
                .push(Text::new(
                    "WTF, SavedVariables and unrelated addons are outside this restore.",
                ))
                .push(Text::new(
                    "Type RESTORE for this plan, then click Restore originals. Keep Patchloom open until restoration finishes.",
                ));

            if ready {
                let acknowledged_preview = preview.clone();
                content = content.push(TextInput::new(
                    &mut self.acknowledgement,
                    "RESTORE",
                    &model.recovery_ack,
                    move |answer| Message::RecoveryAckChanged(acknowledged_preview.clone(), answer),
                ));
            } else {
                // No editable acknowledgement or actionable confirmation while
                // another physical filesystem job owns the shared slot.
                content = content.push(Text::new(
                    "Wait for the current folder check to finish before confirming.",
                ));
            }

            let mut restore = Button::new(&mut self.restore, Text::new("Restore originals"));
            if !root_edited && model.can_restore_recovery(preview) {
                restore = restore.on_press(Message::RestoreRecovery(preview.clone()));
            }

            let mut cancel = Button::new(&mut self.cancel, Text::new("Cancel preview"));
            if ready {
                cancel = cancel.on_press(Message::CancelRecoveryPreview(preview.clone()));
            }

            content = content.push(Row::new().spacing(12).push(restore).push(cancel));
        }

        content.into()
    }
}

struct Preview {
    model: Model,
    root_input: String,
    root_edited: bool,
    root_input_state: text_input::State,
    open_root: button::State,
    choices: Vec<button::State>,
    rescan: button::State,
    receipt_input: text_input::State,
    inspect: button::State,
    repository_input: text_input::State,
    check_release: button::State,
    recovery: RecoveryControls,
    scroll: scrollable::State,
    native_check: bool,
    exit: bool,
}
impl Preview {
    fn from_model(model: Model, native_check: bool) -> Self {
        let root_input = model.root.to_string_lossy().into_owned();
        let mut app = Self {
            model,
            root_input,
            root_edited: false,
            root_input_state: text_input::State::new(),
            open_root: button::State::new(),
            choices: vec![],
            rescan: button::State::new(),
            receipt_input: text_input::State::new(),
            inspect: button::State::new(),
            repository_input: text_input::State::new(),
            check_release: button::State::new(),
            recovery: RecoveryControls::new(),
            scroll: scrollable::State::new(),
            native_check,
            exit: false,
        };
        app.synchronize_choices();
        app
    }
    // Modified 2026-10-28; original GPLv3 applies.
    fn dispatch(&mut self, message: Message) -> Command<Message> {
        if self.model.restoring()
            && matches!(
                &message,
                Message::RootInputChanged(_)
                    | Message::OpenRoot
                    | Message::Select(_)
                    | Message::Rescan
            )
        {
            return Command::none();
        }

        match message {
            Message::RootInputChanged(input) => {
                if self.root_input != input {
                    self.root_input = input;
                    self.root_edited = true;
                    self.model.invalidate_snapshot();
                    self.recovery.root_page = 0;
                    self.synchronize_choices();
                }
                // Logical invalidation does not release physical capacity.
                return Command::none();
            }
            Message::OpenRoot => {
                let root = if self.root_edited {
                    PathBuf::from(&self.root_input)
                } else {
                    self.model.root.clone()
                };
                self.model.set_root(root);
                self.root_edited = false;
                self.recovery.root_page = 0;
                let request = self.model.begin_scan();
                self.synchronize_choices();
                return request.map_or_else(Command::none, scan_command);
            }
            Message::Select(product) => {
                let selected = self.model.selected;
                self.model.select(product);
                if self.model.selected != selected {
                    self.recovery.root_page = 0;
                }
            }
            Message::Rescan => {
                if self.root_edited {
                    return Command::none();
                }
                let request = self.model.begin_scan();
                self.recovery.root_page = 0;
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
            Message::RepositoryChanged(repository) => {
                self.model.set_repository(repository);
            }
            Message::CheckRelease => {
                if let Some(request) = self.model.begin_release() {
                    return release_command(request);
                }
            }
            Message::ReleaseDone(request, result) => {
                self.model.complete_release(request, result);
            }
            Message::ReleaseRateLimited(request, deadline) => {
                self.model.complete_release_rate_limited(request, deadline);
            }
            Message::InspectRecovery => {
                if !self.root_edited {
                    if let Some(request) = self.model.begin_recovery(false) {
                        self.recovery.root_page = 0;
                        return recovery_command(request);
                    }
                }
            }
            Message::PreviewRecovery => {
                if !self.root_edited {
                    if let Some(request) = self.model.begin_recovery(true) {
                        self.recovery.root_page = 0;
                        return recovery_command(request);
                    }
                }
            }
            Message::RecoveryAckChanged(preview, answer) => {
                if !self.root_edited {
                    // Bound inspection of input, with no prefix acceptance.
                    let answer = if answer.chars().take(8).count() <= 7 {
                        answer
                    } else {
                        String::new()
                    };
                    self.model.set_recovery_ack(&preview, answer);
                }
            }
            Message::RecoveryPage(preview, page) => {
                if !self.root_edited {
                    self.model.set_recovery_page(&preview, page);
                }
            }
            Message::RecoveryRootPage(preview, page) => {
                if !self.root_edited
                    && self.model.recovery_slot_available()
                    && self.model.current_recovery_preview(&preview)
                {
                    self.recovery.root_page =
                        page.min(recovery_root_pages(preview.approval.root()).saturating_sub(1));
                }
            }
            Message::CancelRecoveryPreview(preview) => {
                if !self.root_edited
                    && self.model.recovery_slot_available()
                    && self.model.current_recovery_preview(&preview)
                {
                    self.model.cancel_recovery_preview(&preview);
                    self.recovery.root_page = 0;
                }
            }
            Message::RestoreRecovery(preview) => {
                if !self.root_edited {
                    if let Some(request) = self.model.begin_restore_recovery(&preview) {
                        self.recovery.root_page = 0;
                        self.synchronize_choices();
                        return recovery_command(request);
                    }
                }
            }
            Message::RecoveryDone(request, result) => {
                // Matching completion releases physical capacity inside Model.
                // Invalidated results cannot publish or reset current controls.
                if self.model.complete_recovery(request, result) {
                    self.recovery.root_page = 0;
                    self.synchronize_choices();
                }
            }
            Message::NativeCheckDone => {
                if self.model.scan.is_none()
                    && !self.model.scan_requested
                    && self.model.receipt_worker.is_none()
                    && self.model.recovery_job.is_none()
                {
                    self.exit = true;
                }
            }
        }
        self.model
            .start_scan()
            .map_or_else(Command::none, scan_command)
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
        let command = if model.root.as_os_str().is_empty() {
            Command::none()
        } else {
            model.begin_scan().map_or_else(Command::none, scan_command)
        };
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
        self.dispatch(message)
    }
    // Modified 2026-10-28; original GPLv3 applies.
    fn view(&mut self) -> Element<'_, Message> {
        let restoring = self.model.restoring();

        // Do not offer editable scope controls during restore or post-inspection.
        // Dispatch independently rejects these actions for the same interval.
        let root_input: Element<'_, Message> = if restoring {
            Text::new(clip(
                &format!("{:?}", self.model.root.as_os_str()),
                ERROR_CHARS,
            ))
            .width(Length::Fill)
            .into()
        } else {
            TextInput::new(
                &mut self.root_input_state,
                "Copied World of Warcraft folder",
                &self.root_input,
                Message::RootInputChanged,
            )
            .width(Length::Fill)
            .into()
        };

        let mut open = Button::new(&mut self.open_root, Text::new("Open"));
        if !restoring {
            open = open.on_press(Message::OpenRoot);
        }

        let mut rescan = Button::new(&mut self.rescan, Text::new("Rescan folder"));
        if !restoring && !self.root_edited && !self.model.root.as_os_str().is_empty() {
            rescan = rescan.on_press(Message::Rescan);
        }

        let mut content = Column::new()
            .spacing(14)
            .push(Text::new("Patchloom").size(32))
            .push(Text::new("WoW addon inventory").size(22))
            .push(Text::new(
                "Inventory and receipt checks do not change files. Restore originals requires a reviewed plan and confirmation.",
            ).size(16))
            .push(Text::new(
                "Development build. Enter a copied WoW folder path, then click Open.",
            ))
            .push(Row::new().spacing(12).push(root_input).push(open))
            .push(rescan);

        if !self.root_edited && !self.model.root.as_os_str().is_empty() {
            content = content.push(
                Text::new(clip(
                    &format!("{:?}", self.model.root.as_os_str()),
                    ERROR_CHARS,
                ))
                .size(14),
            );
        }
        if let Some(status) = self.model.scan_status() {
            content = content.push(Text::new(status));
        }
        if let Some(status) = self.model.receipt_status() {
            content = content.push(Text::new(status));
        }
        if let Some(status) = self.model.release_status() {
            content = content.push(Text::new(status));
        }
        if let Some(error) = &self.model.error {
            content = content
                .push(Text::new("We couldn't read this WoW folder.").size(22))
                .push(Text::new(
                    "Check that the copied World of Warcraft folder is complete and readable.",
                ))
                .push(Text::new(clip(error, ERROR_CHARS)).size(14));
        }

        if let Some(report) = &self.model.inventory {
            if report.installations.is_empty() {
                content =
                    content.push(Text::new("No supported WoW versions found in this folder."));
            }
            for issue in &report.issues {
                content = content.push(Text::new(clip(&issue_text(issue), ERROR_CHARS)).size(15));
            }
            for (installation, state) in report.installations.iter().zip(&mut self.choices) {
                let label = format!(
                    "{}  {}",
                    product_label(installation.product),
                    clip(&installation.version, SOURCE_FIELD_CHARS)
                );
                let mut choice = Button::new(state, Text::new(label));
                if !restoring && !self.root_edited {
                    choice = choice.on_press(Message::Select(installation.product));
                }
                content = content.push(choice);
            }

            if let Some(installation) = self.model.installation() {
                content = content
                    .push(
                        Text::new(format!(
                            "{} {}",
                            product_label(installation.product),
                            clip(&installation.version, SOURCE_FIELD_CHARS)
                        ))
                        .size(22),
                    )
                    .push(
                        Text::new(clip(
                            &format!("{:?}", installation.directory.as_os_str()),
                            ERROR_CHARS,
                        ))
                        .size(14),
                    )
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
                            .push(
                                Text::new(clip(&addon.addon, FOLDER_CHARS))
                                    .width(Length::FillPortion(1)),
                            )
                            .push(Text::new(status).width(Length::FillPortion(3))),
                    );
                }

                content = content
                    .push(Text::new("GitHub release check").size(18))
                    .push(Text::new(
                        "Enter a public GitHub owner/repository. Check release contacts GitHub.",
                    ));
                if !restoring {
                    content = content.push(TextInput::new(
                        &mut self.repository_input,
                        "owner/repository",
                        &self.model.repository,
                        Message::RepositoryChanged,
                    ));
                }
                match self.model.release_retry_at {
                    Some(RetryDeadline::Blocked) => {
                        content = content.push(Text::new(
                            "The server's wait cannot be represented safely. \
                             Release checks remain paused for this preview session. \
                             Local inventory and receipt checks remain available.",
                        ));
                    }
                    Some(RetryDeadline::At(deadline)) => {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if !remaining.is_zero() {
                            let seconds = remaining
                                .as_secs()
                                .saturating_add(u64::from(remaining.subsec_nanos() != 0));
                            content = content.push(Text::new(format!(
                                "Release checks are paused. At this refresh, about \
                                 {seconds} seconds remained. Early clicks send no \
                                 request. After the wait, click Check release again. \
                                 Local inventory and receipt checks remain available."
                            )));
                        }
                    }
                    None => {}
                }

                let mut check = Button::new(&mut self.check_release, Text::new("Check release"));
                // Keep the existing manual-click behavior during a displayed wait.
                // begin_release remains authoritative and refuses early requests.
                if !self.root_edited && !restoring && self.model.release_inputs_ready() {
                    check = check.on_press(Message::CheckRelease);
                }
                content = content
                    .push(check)
                    .push(Text::new(format!(
                        "Selected WoW version: {}",
                        product_label(installation.product)
                    )))
                    .push(Text::new(
                        "This pilot checks the latest stable release. It requires a matching release.json manifest and asset digests.",
                    ))
                    .push(Text::new(
                        "A compatible release does not establish whether an addon is installed, owned or up to date. This check does not download addon ZIPs, install addons or adopt existing folders.",
                    ));
                if !self.model.repository.is_empty() && !valid_repository(&self.model.repository) {
                    content = content.push(Text::new(
                        "Enter owner/repository using letters, numbers, periods, hyphens or underscores. Each part must be 1 to 100 characters and cannot be . or ..",
                    ));
                }
                if let Some(status) = self.model.release_status() {
                    content = content.push(Text::new(status));
                } else {
                    match &self.model.release_result {
                        Some(Ok(summary)) => {
                            content = content
                                .push(Text::new(format!(
                                    "Repository: {}",
                                    clip(&summary.repository, SOURCE_FIELD_CHARS)
                                )))
                                .push(Text::new(format!(
                                    "{} compatible release: {}",
                                    product_label(summary.product),
                                    summary.version
                                )));
                        }
                        Some(Err(error)) => {
                            content = content
                                .push(Text::new("We couldn't check this release."))
                                .push(Text::new(clip(error, ERROR_CHARS)).size(14))
                                .push(Text::new(
                                    "Review the error before trying Check release again. Any active rate-limit wait still applies. Local inventory and receipt checks remain available.",
                                ));
                        }
                        None => {
                            content = content.push(Text::new("No release has been checked."));
                        }
                    }
                }

                content = content
                    .push(Text::new("Ownership receipt").size(18))
                    .push(Text::new(
                    "Enter a receipt ID to check its recorded folders against this WoW version.",
                ));
                if !restoring {
                    content = content.push(TextInput::new(
                        &mut self.receipt_input,
                        "Receipt ID",
                        &self.model.receipt_id,
                        Message::ReceiptIdChanged,
                    ));
                }
                let mut inspect = Button::new(&mut self.inspect, Text::new("Check receipt"));
                if !self.root_edited && !restoring && self.model.can_inspect_receipt() {
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

        // Keep recovery outside the inventory branch: restore admission clears
        // inventory, but progress, outcome and post-inspection must stay visible.
        // RecoveryControls pages the complete canonical approval root and both
        // complete folder slices. Host summaries above are not approval scope.
        content = content.push(self.recovery.view(&self.model, self.root_edited));

        // Reuse existing neutral project surfaces without changing approved art.
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

// Added 2026-10-28; original GPLv3 applies.
// Runs the actual Application constructor and local completion dispatch.
// Does not start a native application, executor, subscription or clipboard.
// Modified 2026-10-28; original GPLv3 applies.
// Runs the actual Application constructor and local completion dispatch.
// Does not start a native application, executor, subscription or clipboard.
fn check_startup(root: PathBuf, explicit_root: bool) -> crate::Result<()> {
    let empty_root = root.as_os_str().is_empty();
    let (mut app, command) = Preview::new(Flags {
        root,
        native_check: false,
    });
    let mut futures = command.futures();
    let initial_commands = futures.len();
    let expected_commands = if empty_root { 0 } else { 1 };

    let no_recovery_work = |model: &Model| {
        model.recovery_job.is_none()
            && model.pending_recovery.is_none()
            && model.recovery_inspection.is_none()
            && model.recovery_preview.is_none()
            && model.recovery_ack.is_empty()
            && model.recovery_page == 0
            && model.recovery_error.is_none()
            && model.recovery_outcome.is_none()
            && model.recovery_post_error.is_none()
    };

    if initial_commands != expected_commands
        || app.model.receipt_worker.is_some()
        || app.model.release_worker.is_some()
        || app.model.pending_release.is_some()
        || !no_recovery_work(&app.model)
    {
        anyhow::bail!("Unexpected work during safe startup");
    }

    if empty_root && explicit_root {
        // An explicitly supplied empty path must not discover the working
        // directory or count as a successful rootless landing.
        if !app.dispatch(Message::OpenRoot).futures().is_empty() {
            anyhow::bail!("Unexpected work for an empty WoW path");
        }
    }

    if initial_commands == 1 {
        let message = async_std::task::block_on(futures.remove(0));
        match message {
            Message::InventoryDone(request, result) => {
                let next = app.dispatch(Message::InventoryDone(request, result));
                if !next.futures().is_empty() {
                    anyhow::bail!("Unexpected follow-up work during safe startup");
                }
            }
            _ => anyhow::bail!("Unexpected completion during safe startup"),
        }
    }

    drop(app.view());
    if !no_recovery_work(&app.model)
        || app.model.receipt_worker.is_some()
        || app.model.release_worker.is_some()
        || app.model.pending_release.is_some()
    {
        anyhow::bail!("Unexpected recovery or provider work after startup");
    }

    serde_json::to_writer(
        std::io::stdout(),
        &serde_json::json!({
            "entry": "safe-startup",
            "initial_commands": initial_commands,
            "model": &app.model,
        }),
    )?;
    if explicit_root && app.model.needs_review() {
        anyhow::bail!("Some inventory items need review");
    }
    Ok(())
}

pub fn run(opts: &crate::cli::Opts) -> crate::Result<()> {
    if !cfg!(feature = "no-self-update") {
        anyhow::bail!("This development build requires the no-self-update feature");
    }
    if opts.command.is_some() || opts.data_directory.is_some() || opts.self_update_temp.is_some() {
        anyhow::bail!(
            "Preview cannot be combined with legacy commands, --data or --self-update-temp"
        );
    }
    if (opts.preview_check && opts.preview_window_check)
        || (opts.startup_check && (opts.preview_check || opts.preview_window_check))
    {
        anyhow::bail!("Choose one preview check mode");
    }
    if (opts.preview_check || opts.preview_window_check) && opts.preview.is_none() {
        anyhow::bail!("An explicit copied WoW path is required");
    }

    let root = opts.preview.clone().unwrap_or_default();
    if opts.startup_check {
        return check_startup(root, opts.preview.is_some());
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
                    model.set_root(model.root.join("another-root"));
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
            model.set_root(temp.path().into());
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
                    model.set_root(model.root.join("another-root"));
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
        model.set_root(model.root.join("different-root"));
        assert!(!deliver(&mut model, scan_command(changed)));
        assert!(model.scan.is_none());
        let failing = model.begin_scan().unwrap();
        assert!(deliver(&mut model, scan_command(failing)));
        assert!(model.error.is_some());
        assert!(model.inventory.is_none());
        model.set_root(original);
        let retry = model.begin_scan().unwrap();
        assert!(deliver(&mut model, scan_command(retry)));
        assert!(model.error.is_none());
        assert!(model.inventory.is_some());
    }

    #[test]
    fn receipt_root_guard_rejects_a_changed_root_before_another_scan() {
        let (_temp, mut model) = receipt_model();
        let request = model.begin_receipt().unwrap();
        model.set_root(model.root.join("different-root"));
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

    // Added 2026-10-28; original GPLv3 applies.
    // Provider results below are injected model results, not HTTP responses.
    fn release_fixture() -> (tempfile::TempDir, Model) {
        let (temp, mut model) = receipt_model();
        model.set_repository("fixture/addon".into());
        (temp, model)
    }

    fn assert_no_release_work(model: &Model) {
        assert!(model.release_worker.is_none());
        assert!(model.pending_release.is_none());
        assert!(model.release_result.is_none());
        assert_eq!(model.release_status(), None);
    }

    #[test]
    fn release_work_is_not_started_by_construction_headless_or_local_actions() {
        let (temp, _) = receipt_model();
        let (mut app, command) = Preview::new(Flags {
            root: temp.path().into(),
            native_check: false,
        });
        assert_no_release_work(&app.model);

        // Existing deliver checks exactly one future and accepts only the
        // actual inventory/receipt messages. Drive only the startup scan.
        assert!(deliver(&mut app.model, command));
        app.synchronize_choices();
        assert_no_release_work(&app.model);

        app.model.select(Product::Retail);
        app.model.set_repository("fixture/addon".into());
        assert!(app.model.can_check_release());
        assert_no_release_work(&app.model);
        app.model.select(Product::ClassicProgression);
        assert_no_release_work(&app.model);
        app.model.set_receipt_id("b".repeat(64));
        assert_no_release_work(&app.model);
        drop(app.view());
        assert_no_release_work(&app.model);

        let scan = app.model.begin_scan().unwrap();
        app.synchronize_choices();
        assert_no_release_work(&app.model);
        assert!(deliver(&mut app.model, scan_command(scan)));
        app.synchronize_choices();
        assert_no_release_work(&app.model);

        // Follow the supplied headless branch's model/view/serialization
        // operations. This does not invoke run() or construct CLI options.
        let mut headless = Model::new(temp.path().into());
        headless.refresh();
        let mut headless = Preview::from_model(headless, false);
        drop(headless.view());
        let report = serde_json::to_value(&headless.model).unwrap();
        assert!(!headless.model.needs_review());
        assert!(report.get("repository").is_none());
        assert!(report.get("release_result").is_none());
        assert_no_release_work(&headless.model);
    }

    #[test]
    fn release_admission_busy_success_error_and_retry_are_explicit() {
        let (_temp, mut model) = release_fixture();
        let first = model.begin_release().unwrap();
        assert_eq!(model.release_status(), Some("Checking GitHub release..."));
        assert!(!model.can_check_release());
        for _ in 0..8 {
            assert!(model.begin_release().is_none());
        }

        // Construct and drop the actual provider future without polling it.
        // This checks Command shape, not HTTP transport or resolver success.
        assert_eq!(release_command(first.clone()).futures().len(), 1);

        assert!(model.complete_release(first.clone(), Ok("v2".into())));
        assert_eq!(model.release_status(), None);
        assert!(model.release_worker.is_none());
        assert!(model.pending_release.is_none());
        assert!(model.can_check_release());
        let summary = model.release_result.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(summary.repository, "fixture/addon");
        assert_eq!(summary.product, Product::Retail);
        assert_eq!(summary.version, "v2");

        // A completed result remains visible until explicit admission or
        // invalidation. Rendering and unchanged input do not retry.
        model.set_repository("fixture/addon".into());
        let mut app = Preview::from_model(model, false);
        drop(app.view());
        assert!(app.model.release_worker.is_none());
        assert!(app.model.release_result.as_ref().unwrap().is_ok());

        let failed = app.model.begin_release().unwrap();
        assert!(app.model.release_result.is_none());
        assert!(app.model.complete_release(failed, Err("HTTP 429".into())));
        assert_eq!(
            app.model
                .release_result
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap_err(),
            "HTTP 429"
        );
        assert!(app.model.release_worker.is_none());
        assert!(app.model.can_check_release());
        drop(app.view());
        assert!(app.model.release_worker.is_none());
        assert!(app.model.pending_release.is_none());

        let retry = app.model.begin_release().unwrap();
        assert!(app.model.release_result.is_none());
        assert!(!app
            .model
            .complete_release(first, Err("old duplicate".into())));
        assert!(app
            .model
            .release_worker
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &retry.token)));
        assert!(app.model.complete_release(retry, Ok("v3".into())));
        assert_eq!(
            app.model
                .release_result
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .version,
            "v3"
        );
    }

    #[test]
    fn release_invalidated_before_completion_cannot_publish() {
        for change in 0..4 {
            let (_temp, mut model) = release_fixture();
            let request = model.begin_release().unwrap();
            match change {
                0 => model.set_repository("other/addon".into()),
                1 => model.select(Product::ClassicProgression),
                2 => {
                    model.set_root(model.root.join("different-root"));
                    assert!(model.pending_release.is_none());
                }
                _ => {
                    // A local rescan can complete while provider capacity
                    // stays reserved. It never queues another provider job.
                    model.refresh();
                    model.select(Product::Retail);
                    assert!(!Arc::ptr_eq(
                        model.inventory.as_ref().unwrap(),
                        &request.inventory
                    ));
                }
            }
            assert!(model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token)));
            assert_eq!(model.release_status(), Some("Checking GitHub release..."));
            assert!(model.begin_release().is_none());
            assert!(!model.complete_release(request, Ok("stale".into())));
            assert_no_release_work(&model);
        }
    }

    #[test]
    fn release_results_are_cleared_by_input_product_and_rescan_transitions() {
        for failure in [false, true] {
            for change in 0..4 {
                let (_temp, mut model) = release_fixture();
                let request = model.begin_release().unwrap();
                let result = if failure {
                    Err("offline".into())
                } else {
                    Ok("v2".into())
                };
                assert!(model.complete_release(request, result));
                assert!(model.release_result.is_some());

                match change {
                    0 => model.set_repository("other/addon".into()),
                    1 => model.select(Product::ClassicProgression),
                    2 => {
                        model.set_root(model.root.join("different-root"));
                        model.refresh();
                        assert!(model.error.is_some());
                    }
                    _ => model.refresh(),
                }
                assert_no_release_work(&model);
            }
        }
    }

    #[test]
    fn release_aba_and_old_completions_cannot_release_a_new_worker() {
        for change in 0..4 {
            let (_temp, mut model) = release_fixture();
            let original_root = model.root.clone();
            let old = model.begin_release().unwrap();

            match change {
                0 => {
                    model.set_repository("other/addon".into());
                    model.set_repository(old.repository.clone());
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
                    model.set_root(original_root.join("different-root"));
                    model.refresh();
                    model.set_root(original_root);
                    model.refresh();
                    model.select(Product::Retail);
                }
            }

            assert_eq!(model.repository, old.repository);
            assert_eq!(model.selected, Some(old.product));
            assert_eq!(model.root, old.root);
            assert!(model.pending_release.is_none());
            assert!(model.begin_release().is_none());

            let mut unrelated = old.clone();
            unrelated.token = Arc::new(());
            assert!(!model.complete_release(unrelated, Ok("unrelated".into())));
            assert!(model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &old.token)));

            assert!(!model.complete_release(old.clone(), Ok("ABA stale".into())));
            assert_no_release_work(&model);

            let current = model.begin_release().unwrap();
            assert!(!Arc::ptr_eq(&old.token, &current.token));
            assert!(!model.complete_release(old.clone(), Err("late".into())));
            assert!(model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &current.token)));
            assert!(model
                .pending_release
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &current.token)));

            assert!(model.complete_release(current.clone(), Ok("current".into())));
            assert!(!model.complete_release(current, Err("duplicate".into())));
            assert!(!model.complete_release(old, Ok("older duplicate".into())));
            assert_eq!(
                model
                    .release_result
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .version,
                "current"
            );
            assert!(model.release_worker.is_none());
        }
    }

    #[test]
    fn release_validation_uses_raw_repository_without_url_or_whitespace_fallback() {
        let (_temp, mut model) = release_fixture();
        let report = model.inventory.as_ref().unwrap().clone();
        for raw in [
            "",
            "owner",
            "/repo",
            "owner/",
            "owner/repo/extra",
            "owner//repo",
            "./repo",
            "../repo",
            "owner/.",
            "owner/..",
            " owner/repo",
            "owner/repo ",
            "owner/repo\n",
            "owner/\u{202e}repo",
            "ownér/repo",
            "https://github.com/owner/repo",
            "owner/repo?token=x",
            "owner/repo#fragment",
        ] {
            assert!(!valid_repository(raw), "{:?}", raw);
            model.set_repository(raw.into());
            assert_eq!(model.repository, raw);
            assert!(!model.can_check_release());
            assert!(model.begin_release().is_none());
            assert_no_release_work(&model);
            assert!(Arc::ptr_eq(model.inventory.as_ref().unwrap(), &report));
        }

        let maximum = format!("{}/{}", "a".repeat(100), "b".repeat(100));
        assert!(valid_repository(&maximum));
        let too_long = format!("{}/repo", "a".repeat(101));
        assert!(!valid_repository(&too_long));
        model.set_repository(too_long.clone());
        assert_eq!(model.repository, too_long);
        assert!(model.begin_release().is_none());

        for raw in ["fixture/addon", "Owner-1/repo_2.v3", &maximum] {
            assert!(valid_repository(raw));
            model.set_repository(raw.into());
            assert!(model.can_check_release());
            assert_no_release_work(&model);
        }

        model.selected = None;
        assert!(!model.can_check_release());
        assert!(model.begin_release().is_none());
    }

    #[test]
    fn release_summaries_are_bounded_control_safe_and_do_not_change_input() {
        let (_temp, mut model) = release_fixture();
        let raw = format!("界\n\u{202e}{}", "z".repeat(10_000));
        let repository = model.repository.clone();
        let request = model.begin_release().unwrap();
        assert!(model.complete_release(request, Ok(raw.clone())));
        let summary = model.release_result.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(summary.version, clip(&raw, SOURCE_FIELD_CHARS));
        assert!(summary.version.chars().count() <= SOURCE_FIELD_CHARS + 3);
        assert!(summary.version.contains(r"\u{a}"));
        assert!(summary.version.contains(r"\u{202e}"));
        assert!(!summary.version.chars().any(char::is_control));
        assert!(!summary.version.contains('\u{202e}'));
        assert_eq!(summary.repository, repository);
        assert_eq!(model.repository, repository);

        let retry = model.begin_release().unwrap();
        assert!(model.complete_release(retry, Err(raw.clone())));
        let error = model.release_result.as_ref().unwrap().as_ref().unwrap_err();
        assert_eq!(error, &clip(&raw, ERROR_CHARS));
        assert!(error.chars().count() <= ERROR_CHARS + 3);
        assert!(!error.chars().any(char::is_control));
        assert!(!error.contains('\u{202e}'));
        assert_eq!(model.repository, repository);
        assert!(raw.contains('\n') && raw.contains('\u{202e}'));
    }

    #[test]
    fn release_completion_preserves_inventory_receipt_and_fixture_bytes() {
        let (temp, mut model) = release_fixture();
        let app_data = tempfile::tempdir().unwrap();
        let config = app_data.path().join("config-sentinel.json");
        std::fs::write(&config, b"{\"fixture\":\"unchanged\"}\n").unwrap();
        let addon = temp.path().join("_retail_/Interface/AddOns/Unowned");
        std::fs::create_dir_all(&addon).unwrap();
        let user_file = addon.join("keep.lua");
        std::fs::write(&user_file, b"fixture user bytes\n").unwrap();

        // Synthetic receipt only for this model-isolation assertion.
        // The existing real-writer paired bridge is preserved separately.
        let receipt_request = model.begin_receipt().unwrap();
        let receipt = matching_receipt(&receipt_request);
        assert!(model.complete_receipt(receipt_request, Ok(receipt.clone())));
        let inventory = model.inventory.as_ref().unwrap().clone();
        let inventory_bytes = serde_json::to_vec(inventory.as_ref()).unwrap();
        let receipt_id = model.receipt_id.clone();
        let paths = [temp.path().join(".build.info"), user_file, config];
        let before: Vec<Vec<u8>> = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect();

        for result in [Err("offline / HTTP 403".into()), Ok("v2".into())] {
            let request = model.begin_release().unwrap();
            // Independent provider admission does not consume snapshot
            // capacity or invalidate an already displayed receipt.
            assert!(model.scan.is_none());
            assert!(model.receipt_worker.is_none());
            assert!(model.can_inspect_receipt());
            assert!(model.complete_release(request, result));
            assert!(Arc::ptr_eq(model.inventory.as_ref().unwrap(), &inventory));
            assert_eq!(
                serde_json::to_vec(model.inventory.as_deref().unwrap()).unwrap(),
                inventory_bytes
            );
            assert!(Arc::ptr_eq(model.receipt.as_ref().unwrap(), &receipt));
            assert_eq!(model.receipt_id, receipt_id);
            assert!(model.error.is_none());
            assert!(model.receipt_error.is_none());
            for (path, bytes) in paths.iter().zip(&before) {
                assert_eq!(&std::fs::read(path).unwrap(), bytes);
            }
        }

        // Receipt inspection can still be admitted after provider completion.
        let next_receipt = model.begin_receipt().unwrap();
        assert!(model.complete_receipt(next_receipt.clone(), Ok(matching_receipt(&next_receipt))));
    }

    // Added 2026-10-28; original GPLv3 applies.
    #[cfg(test)]
    mod rate_error_boundary_tests {
        use super::*;
        use ajour_core::error::DownloadError;
        use isahc::http::StatusCode;
        use std::time::Duration;

        #[test]
        fn question_mark_boxing_preserves_rate_type_and_wait_for_gui_conversion() {
            type Boxed = Box<dyn std::error::Error + Send + Sync>;

            for code in [StatusCode::TOO_MANY_REQUESTS, StatusCode::FORBIDDEN] {
                // Exercise the same concrete-error-to-box conversion used by `?`.
                let result: Result<(), Boxed> = (|| {
                    Err::<(), DownloadError>(DownloadError::RateLimited {
                        code,
                        retry_after: Duration::from_secs(120),
                    })?;
                    Ok(())
                })();
                let error = result.unwrap_err();

                match error.downcast_ref::<DownloadError>() {
                    Some(DownloadError::RateLimited {
                        code: actual,
                        retry_after,
                    }) => {
                        assert_eq!(*actual, code);
                        assert_eq!(*retry_after, Duration::from_secs(120));
                    }
                    other => panic!("typed rate metadata lost: {:?}", other),
                }

                // Bracket the real clock read; no sleep or exact timing assumption.
                let before = Instant::now();
                let failure = release_failure(error);
                let after = Instant::now();
                match failure {
                    ReleaseFailure::RateLimited(RetryDeadline::At(deadline)) => {
                        assert!(deadline >= before + Duration::from_secs(120));
                        assert!(deadline <= after + Duration::from_secs(120));
                    }
                    other => panic!("GUI lost typed rate outcome: {:?}", other),
                }
            }
        }

        #[test]
        fn status_or_rate_words_in_an_untyped_error_do_not_start_a_wait() {
            for code in [StatusCode::FORBIDDEN, StatusCode::TOO_MANY_REQUESTS] {
                let error = DownloadError::InvalidStatusCode {
                    code,
                    url: "RateLimited retry-after 3600".into(),
                };
                assert!(matches!(
                    release_failure(Box::new(error)),
                    ReleaseFailure::Other(_),
                ));
            }

            let error = std::io::Error::other("Rate limit response (429); retry-after 3600");
            assert!(matches!(
                release_failure(Box::new(error)),
                ReleaseFailure::Other(_),
            ));
        }

        #[test]
        fn checked_deadlines_preserve_long_waits_or_fail_closed() {
            let now = Instant::now();
            for wait in [
                Duration::from_secs(86_401),
                Duration::from_secs(172_800),
                Duration::from_secs(u64::MAX),
            ] {
                let actual = RetryDeadline::from_wait(now, wait);
                match now.checked_add(wait) {
                    Some(deadline) => {
                        assert_eq!(actual, RetryDeadline::At(deadline));
                        assert!(!actual.allows(now));
                        assert!(actual.allows(deadline));
                    }
                    None => {
                        assert_eq!(actual, RetryDeadline::Blocked);
                        assert!(!actual.allows(now));
                    }
                }
            }
            assert_eq!(
                RetryDeadline::from_wait(now, Duration::MAX),
                RetryDeadline::Blocked,
            );
        }

        #[test]
        fn deadline_merge_never_shortens_or_unblocks_a_wait() {
            let now = Instant::now();
            let short = RetryDeadline::At(now);
            let long = RetryDeadline::from_wait(now, Duration::from_secs(172_800));
            assert_eq!(short.max(long), long);
            assert_eq!(long.max(short), long);
            assert_eq!(long.max(RetryDeadline::Blocked), RetryDeadline::Blocked);
            assert_eq!(RetryDeadline::Blocked.max(long), RetryDeadline::Blocked);
            assert!(!RetryDeadline::Blocked.allows(now));
            assert!(short.allows(now));
        }

        #[test]
        fn boxed_unrepresentable_rate_wait_remains_typed_and_blocked() {
            let failure = release_failure(Box::new(DownloadError::RateLimited {
                code: StatusCode::TOO_MANY_REQUESTS,
                retry_after: Duration::MAX,
            }));
            assert!(matches!(
                failure,
                ReleaseFailure::RateLimited(RetryDeadline::Blocked),
            ));
        }
    }

    // Added 2026-10-28; original GPLv3 applies.
    mod rate_dispatch_tests {
        use super::*;
        use ajour_core::error::DownloadError;
        use isahc::http::StatusCode;
        use std::time::Duration;

        fn no_command(command: Command<Message>) {
            assert!(command.futures().is_empty());
        }

        fn app_fixture() -> (tempfile::TempDir, Preview) {
            let (temp, model) = release_fixture();
            (temp, Preview::from_model(model, false))
        }

        fn owns(app: &Preview, request: &ReleaseRequest) {
            assert!(app
                .model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token)));
            assert!(app
                .model
                .pending_release
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &request.token)));
        }

        // Exercise actual CheckRelease dispatch and actual release_command
        // construction. Never poll a provider future.
        fn click_once(app: &mut Preview) -> ReleaseRequest {
            let futures = app.dispatch(Message::CheckRelease).futures();
            assert_eq!(futures.len(), 1);
            let request = app.model.pending_release.as_ref().unwrap().clone();
            owns(app, &request);
            drop(futures);
            request
        }

        // Poll only real local inventory/receipt commands, using the same
        // supplied futures API as deliver(). Route results through dispatch.
        fn local_message(command: Command<Message>) -> Message {
            let mut futures = command.futures();
            assert_eq!(futures.len(), 1);
            let message = async_std::task::block_on(futures.remove(0));
            assert!(matches!(
                &message,
                Message::InventoryDone(..) | Message::ReceiptDone(..)
            ));
            message
        }

        fn finish_local(app: &mut Preview, command: Command<Message>) {
            no_command(app.dispatch(local_message(command)));
        }

        // Synthetic typed provider failure -> real error conversion -> real
        // completion handler. This does not execute release_command's mapper.
        fn rate_completion(
            app: &mut Preview,
            request: ReleaseRequest,
            code: StatusCode,
            wait: Duration,
        ) -> RetryDeadline {
            let deadline = match release_failure(Box::new(DownloadError::RateLimited {
                code,
                retry_after: wait,
            })) {
                ReleaseFailure::RateLimited(deadline) => deadline,
                other => panic!("Lost typed rate error: {:?}", other),
            };
            no_command(app.dispatch(Message::ReleaseRateLimited(request, deadline)));
            deadline
        }

        fn invalidate(app: &mut Preview, change: usize) {
            match change {
                0 => {
                    let original = app.model.repository.clone();
                    no_command(app.dispatch(Message::RepositoryChanged("other/addon".into())));
                    no_command(app.dispatch(Message::RepositoryChanged(original)));
                }
                1 => {
                    no_command(app.dispatch(Message::Select(Product::ClassicProgression)));
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                }
                2 => {
                    let command = app.dispatch(Message::Rescan);
                    finish_local(app, command);
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                }
                3 => {
                    let original = app.model.root.clone();
                    let missing = original.join("missing-fixture-root");
                    no_command(app.dispatch(Message::RootInputChanged(
                        missing.to_str().expect("UTF-8 fixture path").into(),
                    )));
                    let command = app.dispatch(Message::OpenRoot);
                    finish_local(app, command);
                    assert!(app.model.error.is_some());
                    no_command(app.dispatch(Message::RootInputChanged(
                        original.to_str().expect("UTF-8 fixture path").into(),
                    )));
                    let command = app.dispatch(Message::OpenRoot);
                    finish_local(app, command);
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                }
                _ => {
                    let original = app.model.root.clone();
                    let away = original.join("draft-only-root");
                    no_command(app.dispatch(Message::RootInputChanged(
                        away.to_str().expect("UTF-8 fixture path").into(),
                    )));
                    no_command(app.dispatch(Message::RootInputChanged(
                        original.to_str().expect("UTF-8 fixture path").into(),
                    )));
                    assert!(app.model.pending_release.is_none());
                    let command = app.dispatch(Message::OpenRoot);
                    finish_local(app, command);
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                }
            }
        }

        #[test]
        fn typed_long_and_unrepresentable_completions_block_actual_manual_dispatch() {
            for code in [StatusCode::TOO_MANY_REQUESTS, StatusCode::FORBIDDEN] {
                for wait in [Duration::from_secs(172_800), Duration::MAX] {
                    let (_temp, mut app) = app_fixture();
                    let inventory = app.model.inventory.as_ref().unwrap().clone();
                    let receipt_id = app.model.receipt_id.clone();
                    let request = click_once(&mut app);
                    let before = Instant::now();
                    let deadline = rate_completion(&mut app, request, code, wait);
                    let after = Instant::now();

                    match deadline {
                        RetryDeadline::At(at) => {
                            assert_ne!(wait, Duration::MAX);
                            assert!(at >= before.checked_add(wait).unwrap());
                            assert!(at <= after.checked_add(wait).unwrap());
                            assert!(at.duration_since(before) > Duration::from_secs(86_400));
                        }
                        RetryDeadline::Blocked => {
                            assert!(
                                wait == Duration::MAX
                                    || before.checked_add(wait).is_none()
                                    || after.checked_add(wait).is_none()
                            );
                        }
                    }
                    assert_eq!(app.model.release_retry_at, Some(deadline));
                    assert!(app.model.release_result.as_ref().unwrap().is_err());
                    assert!(app.model.release_worker.is_none());
                    assert!(app.model.pending_release.is_none());
                    assert!(Arc::ptr_eq(
                        app.model.inventory.as_ref().unwrap(),
                        &inventory
                    ));
                    assert_eq!(app.model.receipt_id, receipt_id);

                    for _ in 0..4 {
                        no_command(app.dispatch(Message::CheckRelease));
                        assert_eq!(app.model.release_retry_at, Some(deadline));
                        assert!(app.model.release_worker.is_none());
                        assert!(app.model.pending_release.is_none());
                    }
                    drop(app.view());
                    assert!(app.subscription().recipes().is_empty());
                    let report = serde_json::to_value(&app.model).unwrap();
                    assert!(report.get("release_retry_at").is_none());
                }
            }
        }

        #[test]
        fn stale_physical_rate_completion_establishes_wait_across_invalidations() {
            for wait in [Duration::from_secs(172_800), Duration::MAX] {
                for change in 0..5 {
                    let (_temp, mut app) = app_fixture();
                    let request = click_once(&mut app);
                    invalidate(&mut app, change);
                    assert!(app
                        .model
                        .release_worker
                        .as_ref()
                        .is_some_and(|token| Arc::ptr_eq(token, &request.token)));
                    assert!(app.model.pending_release.is_none());
                    no_command(app.dispatch(Message::CheckRelease));

                    let deadline = rate_completion(
                        &mut app,
                        request.clone(),
                        StatusCode::TOO_MANY_REQUESTS,
                        wait,
                    );
                    assert!(app.model.release_worker.is_none());
                    assert!(app.model.pending_release.is_none());
                    assert!(app.model.release_result.is_none());
                    assert_eq!(app.model.release_retry_at, Some(deadline));

                    // Later input/product/discovery changes also retain it.
                    invalidate(&mut app, change);
                    assert!(app.model.release_inputs_ready());
                    assert_eq!(app.model.release_retry_at, Some(deadline));
                    no_command(app.dispatch(Message::CheckRelease));

                    no_command(
                        app.dispatch(Message::ReleaseRateLimited(request, RetryDeadline::Blocked)),
                    );
                    assert_eq!(app.model.release_retry_at, Some(deadline));
                    assert!(app.model.release_worker.is_none());
                    assert!(app.model.release_result.is_none());
                }
            }
        }

        #[test]
        fn foreign_duplicate_and_old_aba_responses_cannot_mutate_wait_or_capacity() {
            let (_temp, mut app) = app_fixture();
            let old = click_once(&mut app);
            invalidate(&mut app, 0);
            let mut foreign = old.clone();
            foreign.token = Arc::new(());

            no_command(app.dispatch(Message::ReleaseRateLimited(foreign, RetryDeadline::Blocked)));
            assert!(app.model.release_retry_at.is_none());
            assert!(app
                .model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &old.token)));
            assert!(app.model.pending_release.is_none());

            let deadline = rate_completion(
                &mut app,
                old.clone(),
                StatusCode::FORBIDDEN,
                Duration::from_secs(172_800),
            );
            assert!(app.model.release_result.is_none());
            assert!(app.model.release_worker.is_none());
            no_command(app.dispatch(Message::ReleaseRateLimited(
                old.clone(),
                RetryDeadline::Blocked,
            )));
            assert_eq!(app.model.release_retry_at, Some(deadline));

            // Controlled expired fixture state; no sleep or expiry callback.
            let expired = RetryDeadline::At(Instant::now());
            app.model.release_retry_at = Some(expired);
            let current = click_once(&mut app);
            assert!(!Arc::ptr_eq(&old.token, &current.token));
            let mut foreign = current.clone();
            foreign.token = Arc::new(());

            for message in [
                Message::ReleaseRateLimited(old.clone(), RetryDeadline::Blocked),
                Message::ReleaseDone(old.clone(), Ok("old".into())),
                Message::ReleaseRateLimited(foreign.clone(), RetryDeadline::Blocked),
                Message::ReleaseDone(foreign, Err("foreign".into())),
            ] {
                no_command(app.dispatch(message));
                owns(&app, &current);
                assert_eq!(app.model.release_retry_at, Some(expired));
                assert!(app.model.release_result.is_none());
            }

            no_command(app.dispatch(Message::ReleaseDone(current.clone(), Ok("current".into()))));
            assert!(app.model.release_worker.is_none());
            for message in [
                Message::ReleaseRateLimited(current, RetryDeadline::Blocked),
                Message::ReleaseRateLimited(old, RetryDeadline::Blocked),
            ] {
                no_command(app.dispatch(message));
                assert_eq!(app.model.release_retry_at, Some(expired));
                assert!(app.model.release_worker.is_none());
                assert!(app.model.pending_release.is_none());
                assert_eq!(
                    app.model
                        .release_result
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .version,
                    "current",
                );
            }
        }

        #[test]
        fn expiry_is_idle_until_explicit_click_and_each_admission_is_single() {
            let (_temp, mut app) = app_fixture();
            let request = click_once(&mut app);
            rate_completion(
                &mut app,
                request,
                StatusCode::TOO_MANY_REQUESTS,
                Duration::from_secs(172_800),
            );
            no_command(app.dispatch(Message::CheckRelease));

            // Simulate elapsed time by supplying an already-expired deadline.
            // This deliberately does not claim a real two-day clock advance.
            app.model.release_retry_at = Some(RetryDeadline::At(Instant::now()));
            drop(app.view());
            assert!(app.subscription().recipes().is_empty());
            let repository = app.model.repository.clone();
            no_command(app.dispatch(Message::RepositoryChanged(repository)));
            assert!(app.model.release_worker.is_none());
            assert!(app.model.pending_release.is_none());

            let request = click_once(&mut app);
            for _ in 0..4 {
                no_command(app.dispatch(Message::CheckRelease));
                owns(&app, &request);
            }
            no_command(app.dispatch(Message::ReleaseDone(request, Ok("v2".into()))));
            assert!(app.model.release_worker.is_none());
            drop(app.view());
            assert!(app.model.release_worker.is_none());
            let next = click_once(&mut app);
            owns(&app, &next);
        }

        #[test]
        fn native_timer_messages_never_schedule_or_release_provider_work() {
            for state in 0..4 {
                let (_temp, mut app) = app_fixture();
                assert!(app.subscription().recipes().is_empty());
                let request = click_once(&mut app);
                match state {
                    0 => {
                        rate_completion(
                            &mut app,
                            request,
                            StatusCode::TOO_MANY_REQUESTS,
                            Duration::from_secs(172_800),
                        );
                    }
                    1 => {
                        rate_completion(
                            &mut app,
                            request,
                            StatusCode::TOO_MANY_REQUESTS,
                            Duration::MAX,
                        );
                    }
                    2 => {
                        rate_completion(
                            &mut app,
                            request,
                            StatusCode::TOO_MANY_REQUESTS,
                            Duration::from_secs(172_800),
                        );
                        app.model.release_retry_at = Some(RetryDeadline::At(Instant::now()));
                    }
                    _ => {}
                }
                let deadline = app.model.release_retry_at;
                let worker = app.model.release_worker.clone();
                let pending = app.model.pending_release.clone();
                app.native_check = true;
                assert!(!app.subscription().recipes().is_empty());

                for _ in 0..3 {
                    no_command(app.dispatch(Message::NativeCheckDone));
                    assert!(app.should_exit());
                    assert_eq!(app.model.release_retry_at, deadline);
                    match (&worker, &app.model.release_worker) {
                        (Some(before), Some(after)) => assert!(Arc::ptr_eq(before, after)),
                        (None, None) => {}
                        other => panic!("Timer changed provider capacity: {:?}", other),
                    }
                    match &pending {
                        Some(request) => owns(&app, request),
                        None => assert!(app.model.pending_release.is_none()),
                    }
                }
            }
        }

        #[test]
        fn local_commands_and_queued_rescan_work_while_release_is_busy_or_blocked() {
            let (temp, mut app) = app_fixture();
            let app_data = tempfile::tempdir().unwrap();
            let config = app_data.path().join("config-sentinel.json");
            let user_file = temp.path().join("_retail_/Interface/AddOns/keep.lua");
            std::fs::write(&config, b"fixture config\n").unwrap();
            std::fs::write(&user_file, b"fixture user bytes\n").unwrap();
            let paths = [temp.path().join(".build.info"), user_file, config];
            let before: Vec<_> = paths
                .iter()
                .map(|path| std::fs::read(path).unwrap())
                .collect();

            let request = click_once(&mut app);
            let receipt = app.dispatch(Message::InspectReceipt);
            assert!(app.model.receipt_worker.is_some());
            finish_local(&mut app, receipt);
            assert!(app.model.receipt_error.is_some());
            owns(&app, &request);

            let scan = app.dispatch(Message::Rescan);
            finish_local(&mut app, scan);
            no_command(app.dispatch(Message::Select(Product::Retail)));
            assert!(app
                .model
                .release_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token)));

            rate_completion(
                &mut app,
                request,
                StatusCode::TOO_MANY_REQUESTS,
                Duration::MAX,
            );
            assert_eq!(app.model.release_retry_at, Some(RetryDeadline::Blocked));
            assert!(app.model.release_result.is_none());
            assert!(app.model.can_inspect_receipt());

            let receipt = app.dispatch(Message::InspectReceipt);
            no_command(app.dispatch(Message::Rescan));
            assert!(app.model.scan_requested);
            assert!(app.model.scan.is_none());
            no_command(app.dispatch(Message::NativeCheckDone));
            assert!(!app.should_exit());
            no_command(app.dispatch(Message::CheckRelease));

            // The real receipt completion releases snapshot capacity and the
            // unchanged handler schedules exactly the queued inventory scan.
            let scan = app.dispatch(local_message(receipt));
            assert!(app.model.receipt_worker.is_none());
            assert!(app.model.scan.is_some());
            no_command(app.dispatch(Message::NativeCheckDone));
            assert!(!app.should_exit());
            finish_local(&mut app, scan);
            no_command(app.dispatch(Message::Select(Product::Retail)));

            assert!(app.model.inventory.is_some());
            assert!(app.model.can_inspect_receipt());
            assert_eq!(app.model.release_retry_at, Some(RetryDeadline::Blocked));
            no_command(app.dispatch(Message::CheckRelease));
            assert!(app.model.release_worker.is_none());
            drop(app.view());
            for (path, bytes) in paths.iter().zip(&before) {
                assert_eq!(&std::fs::read(path).unwrap(), bytes);
            }
        }
    }

    // Added 2026-10-28; original GPLv3 applies.
    mod recovery_dispatch_tests {
        use super::*;
        use ajour_core::fs::{
            preflight_restore_originals, restore_originals_checked,
            RecoveryRestoreOutcome as Outcome,
        };
        use std::{collections::BTreeMap, fs, path::Path};

        #[derive(Debug, PartialEq, Eq)]
        enum Entry {
            Directory,
            File(Vec<u8>),
            Link(PathBuf),
        }

        // Complete fixture membership/types/bytes, including empty directories.
        // Never follows links. Call only before work or after its future finishes,
        // and never while this test holds the root mutex.
        fn tree(root: &Path) -> BTreeMap<PathBuf, Entry> {
            fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
                let metadata = fs::symlink_metadata(path).unwrap();
                let link = metadata.file_type().is_symlink();
                #[cfg(windows)]
                let link = {
                    use std::os::windows::fs::MetadataExt;
                    link || metadata.file_attributes() & 0x400 != 0
                };
                let entry = if link {
                    Entry::Link(fs::read_link(path).unwrap())
                } else if metadata.is_dir() {
                    Entry::Directory
                } else {
                    assert!(metadata.is_file(), "unexpected fixture entry type");
                    Entry::File(fs::read(path).unwrap())
                };
                let directory = matches!(&entry, Entry::Directory);
                entries.insert(path.strip_prefix(root).unwrap().into(), entry);
                if directory {
                    for child in fs::read_dir(path).unwrap() {
                        visit(root, &child.unwrap().path(), entries);
                    }
                }
            }

            let mut entries = BTreeMap::new();
            visit(root, root, &mut entries);
            entries
        }

        fn no_command(command: Command<Message>) {
            assert!(command.futures().is_empty());
        }

        // Call only with local constructor/scan/receipt/recovery commands.
        // Never pass CheckRelease: no provider future is polled by these tests.
        fn local_message(command: Command<Message>) -> Message {
            let mut futures = command.futures();
            assert_eq!(futures.len(), 1);
            let message = async_std::task::block_on(futures.remove(0));
            assert!(matches!(
                &message,
                Message::InventoryDone(..) | Message::ReceiptDone(..) | Message::RecoveryDone(..)
            ));
            message
        }

        fn finish(app: &mut Preview, command: Command<Message>) {
            let message = local_message(command);
            no_command(app.dispatch(message));
        }

        fn owns(app: &Preview, request: &RecoveryRequest) {
            assert!(app
                .model
                .recovery_job
                .as_ref()
                .is_some_and(|job| job.same_request(request)));
            assert!(app
                .model
                .receipt_worker
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &request.token)));
        }

        fn fixture(interrupted: bool) -> (tempfile::TempDir, tempfile::TempDir, Preview, PathBuf) {
            let (wow, model) = release_fixture();
            let data = tempfile::tempdir().unwrap();
            fs::create_dir(data.path().join("config")).unwrap();
            fs::write(
                data.path().join("config/settings.json"),
                b"{\"fixture\":\"preserve\"}\0\n",
            )
            .unwrap();

            // Selected WoW root and canonical AddOns root are different scopes.
            let addons = model
                .installation()
                .unwrap()
                .directory
                .join("Interface")
                .join("AddOns")
                .canonicalize()
                .unwrap();

            for directory in [
                wow.path().join("_retail_/WTF/Account/SavedVariables"),
                addons.join("WTF"),
                addons.join("SavedVariables"),
                addons.join("Unrelated"),
            ] {
                fs::create_dir_all(&directory).unwrap();
                fs::write(directory.join("keep.lua"), b"unrelated settings\0\n").unwrap();
            }

            if interrupted {
                let transaction = addons.join(".patchloom-install");
                fs::create_dir_all(transaction.join("backup/Bar")).unwrap();
                fs::create_dir(transaction.join("backup/Foo")).unwrap();
                fs::create_dir(transaction.join("staged")).unwrap();
                // Same accepted journal format as the supplied CLI/core fixtures.
                fs::write(
                    transaction.join("plan.yml"),
                    b"version: 1\nprevious: [Bar, Foo]\nincoming: [Foo, New]\n",
                )
                .unwrap();
                fs::write(transaction.join("ready"), b"").unwrap();
                fs::write(transaction.join("backup/Bar/old.lua"), b"original-bar").unwrap();
                fs::write(transaction.join("backup/Foo/old.lua"), b"original").unwrap();
                for (name, bytes) in [
                    ("Foo", b"replacement".as_slice()),
                    ("New", b"additional".as_slice()),
                ] {
                    fs::create_dir(addons.join(name)).unwrap();
                    fs::write(addons.join(name).join("new.lua"), bytes).unwrap();
                }
                // Nonempty mutex bytes are included in test inventories, but
                // are deliberately not approval authority.
                fs::write(
                    addons.join(".patchloom-install.lock"),
                    b"fixture mutex bytes\n",
                )
                .unwrap();
            }

            (wow, data, Preview::from_model(model, false), addons)
        }

        fn plan(app: &mut Preview) -> RecoveryPreview {
            let command = app.dispatch(Message::InspectRecovery);
            let request = app.model.pending_recovery.as_ref().unwrap().clone();
            owns(app, &request);
            assert!(matches!(&request.action, RecoveryAction::Inspect));
            assert!(app.model.scan.is_none());
            finish(app, command);
            assert!(app.model.recovery_error.is_none());
            assert!(app.model.recovery_inspection.is_some());

            let command = app.dispatch(Message::PreviewRecovery);
            let request = app.model.pending_recovery.as_ref().unwrap().clone();
            owns(app, &request);
            assert!(matches!(&request.action, RecoveryAction::Preview));
            finish(app, command);
            assert!(app.model.recovery_error.is_none());
            assert!(app.model.pending_recovery.is_none());
            assert!(app.model.recovery_job.is_none());
            assert!(app.model.receipt_worker.is_none());
            assert!(app.model.recovery_ack.is_empty());
            app.model.recovery_preview.as_ref().unwrap().clone()
        }

        fn restore(app: &mut Preview, preview: &RecoveryPreview) -> Command<Message> {
            no_command(app.dispatch(Message::RecoveryAckChanged(
                preview.clone(),
                "RESTORE".into(),
            )));
            assert!(app.model.can_restore_recovery(preview));
            let command = app.dispatch(Message::RestoreRecovery(preview.clone()));
            let request = app.model.pending_recovery.as_ref().unwrap();
            owns(app, request);
            assert!(matches!(&request.action, RecoveryAction::Restore(_)));
            assert!(app.model.restoring());
            assert!(app.model.inventory.is_none());
            assert!(app.model.recovery_preview.is_none());
            assert!(app.model.recovery_ack.is_empty());
            assert!(app.model.scan.is_none());
            assert!(!app.model.scan_requested);
            command
        }

        #[test]
        fn inspect_preflight_core_cancel_and_preview_cancel_preserve_complete_trees() {
            let (wow, data, mut app, addons) = fixture(true);
            let before = tree(wow.path());
            let config_before = tree(data.path());

            // Construction, rendering and the native-check message do not recover.
            assert!(app.model.recovery_job.is_none());
            assert!(app.subscription().recipes().is_empty());
            drop(app.view());
            no_command(app.dispatch(Message::NativeCheckDone));
            assert!(app.model.recovery_job.is_none());
            assert_no_release_work(&app.model);

            let preview = plan(&mut app);
            assert_eq!(preview.request.root, app.model.root);
            assert_eq!(preview.request.product, Product::Retail);
            assert_eq!(preview.request.addons.canonicalize().unwrap(), addons);
            assert_eq!(preview.approval.root(), addons.as_path());
            assert_ne!(preview.approval.root(), app.model.root.as_path());
            assert_eq!(preview.approval.originals(), ["Bar", "Foo"]);
            assert_eq!(
                preview.approval.move_aside_if_present(),
                ["Bar", "Foo", "New"]
            );
            // Bar is approved as affected, not falsely asserted to exist.
            assert!(!addons.join("Bar").exists());
            assert_eq!(tree(wow.path()), before);
            assert_eq!(tree(data.path()), config_before);

            // Exercise actual checked-core cancellation. The GUI's restore
            // worker always supplies admitted consent; it has no cancel dialog.
            let mut calls = 0;
            let outcome = restore_originals_checked(
                &addons,
                preview.approval.as_ref(),
                |originals, retained| {
                    calls += 1;
                    assert_eq!(originals, preview.approval.originals());
                    assert_eq!(retained, preview.approval.move_aside_if_present());
                    false
                },
            )
            .unwrap();
            assert_eq!(calls, 1);
            assert_eq!(outcome, Outcome::Cancelled);
            assert_eq!(tree(wow.path()), before);
            assert_eq!(tree(data.path()), config_before);

            no_command(app.dispatch(Message::RecoveryAckChanged(
                preview.clone(),
                "RESTORE".into(),
            )));
            no_command(app.dispatch(Message::CancelRecoveryPreview(preview.clone())));
            assert!(app.model.recovery_preview.is_none());
            assert!(app.model.recovery_ack.is_empty());
            assert_eq!(
                app.model.recovery_error.as_deref(),
                Some("Preview cancelled. No addon folders were changed.")
            );
            no_command(app.dispatch(Message::RestoreRecovery(preview)));
            assert!(app.model.recovery_outcome.is_none());
            assert!(app.model.recovery_job.is_none());
            assert!(app.model.receipt_worker.is_none());
            assert!(!addons.join(".patchloom-restore").exists());
            drop(app.view());
            assert_no_release_work(&app.model);
            assert_eq!(tree(wow.path()), before);
            assert_eq!(tree(data.path()), config_before);
        }

        #[test]
        fn no_journal_is_reported_without_creating_a_mutex_or_offering_a_plan() {
            let (wow, data, mut app, addons) = fixture(false);
            let before = tree(wow.path());
            let config_before = tree(data.path());

            let command = app.dispatch(Message::InspectRecovery);
            finish(&mut app, command);
            let inspection = app.model.recovery_inspection.as_ref().unwrap();
            assert_eq!(inspection.root, addons);
            assert_eq!(inspection.phase, "none");
            assert!(app.model.recovery_error.is_none());
            assert!(!app.model.can_preview_recovery());
            no_command(app.dispatch(Message::PreviewRecovery));
            assert!(app.model.recovery_preview.is_none());

            assert!(preflight_restore_originals(&addons).is_err());
            assert!(!addons.join(".patchloom-install.lock").exists());
            assert!(!addons.join(".patchloom-install").exists());
            assert!(!addons.join(".patchloom-restore").exists());
            drop(app.view());
            assert_no_release_work(&app.model);
            assert_eq!(tree(wow.path()), before);
            assert_eq!(tree(data.path()), config_before);
        }

        #[test]
        fn checked_restore_command_restores_bundle_and_retains_versions_and_journals() {
            let (wow, data, mut app, addons) = fixture(true);
            let untouched = [
                wow.path().join("_classic_"),
                wow.path().join("_retail_/WTF"),
                addons.join("WTF"),
                addons.join("SavedVariables"),
                addons.join("Unrelated"),
            ];
            let before: Vec<_> = untouched.iter().map(|path| tree(path)).collect();
            let config_before = tree(data.path());
            let build_before = fs::read(wow.path().join(".build.info")).unwrap();
            let transaction = addons.join(".patchloom-install");
            let journal_before = fs::read(transaction.join("plan.yml")).unwrap();
            let mutex_before = fs::read(addons.join(".patchloom-install.lock")).unwrap();

            let preview = plan(&mut app);
            let command = restore(&mut app, &preview);
            let request = app.model.pending_recovery.as_ref().unwrap().clone();

            // Admission consumes consent and reserves capacity before polling.
            no_command(app.dispatch(Message::RestoreRecovery(preview.clone())));
            owns(&app, &request);
            assert!(addons.join("Foo/new.lua").is_file());
            assert!(!addons.join(".patchloom-restore").exists());

            // Actual Command -> spawn_blocking -> checked core -> post-inspection.
            let message = local_message(command);
            assert!(matches!(
                &message,
                Message::RecoveryDone(
                    _,
                    RecoveryResult::Restore {
                        outcome: Ok(Outcome::RestoredRetained),
                        inspection: Ok(_),
                    }
                )
            ));
            // Completed worker output has not yet released UI capacity.
            owns(&app, &request);
            assert!(app.model.restoring());
            no_command(app.dispatch(message.clone()));

            assert!(!app.model.restoring());
            assert!(app.model.recovery_job.is_none());
            assert!(app.model.pending_recovery.is_none());
            assert!(app.model.receipt_worker.is_none());
            assert!(app.model.inventory.is_none());
            assert!(app.model.receipt.is_none());
            assert!(app.model.recovery_post_error.is_none());
            assert!(matches!(
                app.model.recovery_outcome.as_ref(),
                Some(Ok(Outcome::RestoredRetained))
            ));
            assert_eq!(fs::read(addons.join("Foo/old.lua")).unwrap(), b"original");
            assert_eq!(
                fs::read(addons.join("Bar/old.lua")).unwrap(),
                b"original-bar"
            );
            assert!(!addons.join("Foo/new.lua").exists());
            assert!(!addons.join("New").exists());
            assert_eq!(
                fs::read(addons.join(".patchloom-restore/backup/Foo/new.lua")).unwrap(),
                b"replacement"
            );
            assert_eq!(
                fs::read(addons.join(".patchloom-restore/backup/New/new.lua")).unwrap(),
                b"additional"
            );
            assert_eq!(
                fs::read(transaction.join("plan.yml")).unwrap(),
                journal_before
            );
            assert!(transaction.join("ready").is_file());
            assert!(transaction.join("backup").is_dir());
            assert!(transaction.join("staged").is_dir());
            assert!(addons.join(".patchloom-restore/plan.yml").is_file());
            assert!(addons.join(".patchloom-restore/committed").is_file());
            let inspection = app.model.recovery_inspection.as_ref().unwrap();
            assert_eq!(
                inspection.restore_phase.as_deref(),
                Some("restoring-or-restored")
            );
            assert_eq!(inspection.retained_replacements, ["Foo", "New"]);

            // Retained recovery state is not cleaned up or made reusable.
            assert!(preflight_restore_originals(&addons).is_err());
            no_command(app.dispatch(message));
            no_command(app.dispatch(Message::RestoreRecovery(preview)));
            no_command(app.dispatch(Message::NativeCheckDone));
            drop(app.view());
            assert!(app.model.recovery_job.is_none());
            assert!(app.model.scan.is_none());
            assert!(!app.model.scan_requested);
            assert_no_release_work(&app.model);
            assert!(addons.join(".patchloom-restore/committed").is_file());
            for (path, expected) in untouched.iter().zip(&before) {
                assert_eq!(&tree(path), expected);
            }
            assert_eq!(tree(data.path()), config_before);
            assert_eq!(
                fs::read(wow.path().join(".build.info")).unwrap(),
                build_before
            );
            assert_eq!(
                fs::read(addons.join(".patchloom-install.lock")).unwrap(),
                mutex_before
            );
        }

        #[test]
        fn journal_backup_and_current_changes_are_refused_by_the_real_worker() {
            for change in 0..3 {
                let (wow, data, mut app, addons) = fixture(true);
                let preview = plan(&mut app);
                let path = match change {
                    0 => addons.join(".patchloom-install/plan.yml"),
                    1 => addons.join(".patchloom-install/backup/Foo/old.lua"),
                    _ => addons.join("Foo/new.lua"),
                };
                let mut bytes = fs::read(&path).unwrap();
                if change == 0 {
                    // Same valid parsed plan, different authority bytes.
                    bytes.extend_from_slice(b"\n# changed after preview\n");
                } else {
                    bytes[0] ^= 1;
                }
                fs::write(path, bytes).unwrap();
                let before = tree(wow.path());
                let config_before = tree(data.path());

                let command = restore(&mut app, &preview);
                let message = local_message(command);
                assert!(matches!(
                    &message,
                    Message::RecoveryDone(
                        _,
                        RecoveryResult::Restore {
                            outcome: Ok(Outcome::Changed),
                            inspection: Ok(_),
                        }
                    )
                ));
                no_command(app.dispatch(message));
                assert!(matches!(
                    app.model.recovery_outcome.as_ref(),
                    Some(Ok(Outcome::Changed))
                ));
                assert!(app.model.recovery_preview.is_none());
                assert!(app.model.recovery_ack.is_empty());
                assert!(app.model.recovery_post_error.is_none());
                assert!(app.model.inventory.is_none());
                no_command(app.dispatch(Message::RecoveryAckChanged(
                    preview.clone(),
                    "RESTORE".into(),
                )));
                no_command(app.dispatch(Message::RestoreRecovery(preview)));
                drop(app.view());
                assert!(!addons.join(".patchloom-restore").exists());
                assert!(app.model.recovery_job.is_none());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        #[test]
        fn consent_rejects_prefixes_and_foreign_scope_generation_token_and_ticket() {
            let (wow, data, mut app, addons) = fixture(true);
            let (_other_wow, _other_data, _other_app, other_addons) = fixture(true);
            let preview = plan(&mut app);
            let before = tree(wow.path());
            let config_before = tree(data.path());

            no_command(app.dispatch(Message::RestoreRecovery(preview.clone())));
            for answer in [
                "",
                "restore",
                " RESTORE",
                "RESTORE ",
                "RESTORE\n",
                "RESTOREx",
                "RESTORE\u{202e}",
            ] {
                // An invalid replacement must also remove previously valid text.
                no_command(app.dispatch(Message::RecoveryAckChanged(
                    preview.clone(),
                    "RESTORE".into(),
                )));
                no_command(
                    app.dispatch(Message::RecoveryAckChanged(preview.clone(), answer.into())),
                );
                assert!(!app.model.can_restore_recovery(&preview));
                no_command(app.dispatch(Message::RestoreRecovery(preview.clone())));
                assert!(app.model.recovery_job.is_none());
            }

            for mismatch in 0..7 {
                let mut foreign = preview.clone();
                match mismatch {
                    0 => foreign.request.root = foreign.request.root.join("other"),
                    1 => foreign.request.addons = foreign.request.addons.join("other"),
                    2 => foreign.request.product = Product::ClassicProgression,
                    3 => foreign.request.generation = Arc::new(()),
                    4 => foreign.request.token = Arc::new(()),
                    5 => {
                        // A real fresh ticket with identical content is still
                        // a different in-memory approval identity.
                        foreign.approval = Arc::new(preflight_restore_originals(&addons).unwrap());
                    }
                    _ => {
                        foreign.approval =
                            Arc::new(preflight_restore_originals(&other_addons).unwrap());
                    }
                }

                no_command(app.dispatch(Message::RecoveryAckChanged(
                    preview.clone(),
                    "RESTORE".into(),
                )));
                no_command(
                    app.dispatch(Message::RecoveryAckChanged(foreign.clone(), String::new())),
                );
                assert_eq!(app.model.recovery_ack, "RESTORE");
                no_command(app.dispatch(Message::RestoreRecovery(foreign.clone())));
                no_command(app.dispatch(Message::CancelRecoveryPreview(foreign)));
                assert!(app.model.current_recovery_preview(&preview));
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.receipt_worker.is_none());
                assert!(app.model.recovery_outcome.is_none());
            }

            assert!(!addons.join(".patchloom-restore").exists());
            assert_no_release_work(&app.model);
            assert_eq!(tree(wow.path()), before);
            assert_eq!(tree(data.path()), config_before);
        }
        // Added 2026-10-28; original GPLv3 applies.
        fn finish_recovery_failure(app: &mut Preview, command: Command<Message>) {
            let message = local_message(command);
            let expected = match &message {
                Message::RecoveryDone(
                    request,
                    RecoveryResult::Inspection(Err(error)) | RecoveryResult::Preview(Err(error)),
                ) => {
                    owns(app, request);
                    assert!(!error.is_empty());
                    error.clone()
                }
                other => panic!("Expected actual recovery failure: {:?}", other),
            };
            no_command(app.dispatch(message));
            assert_eq!(app.model.recovery_error.as_deref(), Some(expected.as_str()));
            assert!(app.model.recovery_preview.is_none());
            assert!(app.model.recovery_ack.is_empty());
            assert!(app.model.recovery_outcome.is_none());
            assert!(app.model.recovery_post_error.is_none());
            assert!(app.model.pending_recovery.is_none());
            assert!(app.model.recovery_job.is_none());
            assert!(app.model.receipt_worker.is_none());
        }

        #[test]
        fn product_rescan_draft_and_open_aba_require_a_new_preview_and_ack() {
            for change in 0..4 {
                let (wow, data, mut app, addons) = fixture(true);
                let (other, _) = receipt_model();
                let before = tree(wow.path());
                let config_before = tree(data.path());
                let other_before = tree(other.path());
                let old = plan(&mut app);
                no_command(
                    app.dispatch(Message::RecoveryAckChanged(old.clone(), "RESTORE".into())),
                );
                let original = app.model.root.clone();
                let generation = app.model.generation.clone();

                match change {
                    0 => {
                        no_command(app.dispatch(Message::Select(Product::ClassicProgression)));
                        no_command(app.dispatch(Message::Select(Product::Retail)));
                        // Selection ABA must fail even without a new generation.
                        assert!(Arc::ptr_eq(&generation, &app.model.generation));
                    }
                    1 => {
                        let command = app.dispatch(Message::Rescan);
                        finish(&mut app, command);
                        no_command(app.dispatch(Message::Select(Product::Retail)));
                    }
                    2 => {
                        let away = original.join("unopened-draft");
                        no_command(app.dispatch(Message::RootInputChanged(
                            away.to_str().expect("UTF-8 fixture path").into(),
                        )));
                        no_command(app.dispatch(Message::RootInputChanged(
                            original.to_str().expect("UTF-8 fixture path").into(),
                        )));
                        assert!(app.root_edited);
                        no_command(app.dispatch(Message::RestoreRecovery(old.clone())));
                        let command = app.dispatch(Message::OpenRoot);
                        finish(&mut app, command);
                        no_command(app.dispatch(Message::Select(Product::Retail)));
                    }
                    _ => {
                        for root in [other.path(), original.as_path()] {
                            no_command(app.dispatch(Message::RootInputChanged(
                                root.to_str().expect("UTF-8 fixture path").into(),
                            )));
                            let command = app.dispatch(Message::OpenRoot);
                            finish(&mut app, command);
                        }
                        no_command(app.dispatch(Message::Select(Product::Retail)));
                    }
                }

                assert_eq!(app.model.root, original);
                assert_eq!(app.model.selected, Some(Product::Retail));
                if change != 0 {
                    assert!(!Arc::ptr_eq(&generation, &app.model.generation));
                }
                assert!(!app.root_edited);
                assert!(app.model.recovery_preview.is_none());
                assert!(app.model.recovery_ack.is_empty());
                assert!(!app.model.current_recovery_preview(&old));
                no_command(
                    app.dispatch(Message::RecoveryAckChanged(old.clone(), "RESTORE".into())),
                );
                no_command(app.dispatch(Message::RestoreRecovery(old.clone())));
                assert!(app.model.recovery_job.is_none());

                let fresh = plan(&mut app);
                assert_eq!(fresh.approval.root(), addons.as_path());
                assert!(!Arc::ptr_eq(&old.request.token, &fresh.request.token));
                assert!(!Arc::ptr_eq(&old.approval, &fresh.approval));
                assert!(app.model.recovery_ack.is_empty());
                no_command(app.dispatch(Message::RestoreRecovery(fresh.clone())));

                // Stale events cannot consume, clear or reuse fresh consent.
                no_command(
                    app.dispatch(Message::RecoveryAckChanged(fresh.clone(), "RESTORE".into())),
                );
                no_command(app.dispatch(Message::RecoveryAckChanged(old.clone(), String::new())));
                no_command(app.dispatch(Message::CancelRecoveryPreview(old.clone())));
                no_command(app.dispatch(Message::RestoreRecovery(old)));
                assert_eq!(app.model.recovery_ack, "RESTORE");
                assert!(app.model.current_recovery_preview(&fresh));
                assert!(app.model.can_restore_recovery(&fresh));
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.receipt_worker.is_none());
                assert!(!addons.join(".patchloom-restore").exists());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
                assert_eq!(tree(other.path()), other_before);
            }
        }

        #[test]
        fn invalid_unknown_and_missing_original_states_fail_without_fixture_changes() {
            for change in 0..8 {
                let (wow, data, mut app, addons) = fixture(true);
                let command = app.dispatch(Message::InspectRecovery);
                finish(&mut app, command);
                assert!(app.model.can_preview_recovery());
                let transaction = addons.join(".patchloom-install");

                match change {
                    0 => fs::write(transaction.join("plan.yml"), b"version: [invalid").unwrap(),
                    1 => fs::write(
                        transaction.join("plan.yml"),
                        b"version: 99\nprevious: [Bar, Foo]\nincoming: [Foo, New]\n",
                    )
                    .unwrap(),
                    2 => fs::write(transaction.join("unknown-entry"), b"retain me").unwrap(),
                    3 => {
                        fs::create_dir(transaction.join("backup/Unexpected")).unwrap();
                        fs::write(
                            transaction.join("backup/Unexpected/keep.lua"),
                            b"unknown backup",
                        )
                        .unwrap();
                    }
                    4 => fs::write(transaction.join("ready"), b"invalid marker").unwrap(),
                    5 => fs::write(transaction.join("committed"), b"").unwrap(),
                    6 => fs::rename(
                        transaction.join("backup/Foo"),
                        wow.path().join("preserved-original"),
                    )
                    .unwrap(),
                    _ => {
                        fs::remove_file(addons.join(".patchloom-install.lock")).unwrap();
                        fs::create_dir(addons.join(".patchloom-install.lock")).unwrap();
                    }
                }
                let before = tree(wow.path());
                let config_before = tree(data.path());

                // A formerly readable inspection is not authority. Actual
                // preflight must reject the changed invalid state.
                let command = app.dispatch(Message::PreviewRecovery);
                finish_recovery_failure(&mut app, command);

                let command = app.dispatch(Message::InspectRecovery);
                if change == 6 {
                    // Inspection reports missing backups; it does not certify
                    // that the original bundle can be restored.
                    finish(&mut app, command);
                    let inspection = app.model.recovery_inspection.as_ref().unwrap();
                    assert_eq!(inspection.phase, "ready-marker");
                    assert_eq!(inspection.backups, ["Bar"]);
                    assert!(app.model.recovery_error.is_none());
                    let command = app.dispatch(Message::PreviewRecovery);
                    finish_recovery_failure(&mut app, command);
                } else {
                    finish_recovery_failure(&mut app, command);
                    assert!(app.model.recovery_inspection.is_none());
                }

                drop(app.view());
                no_command(app.dispatch(Message::NativeCheckDone));
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.recovery_preview.is_none());
                assert!(app.model.scan.is_none());
                assert!(!addons.join(".patchloom-restore").exists());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        #[test]
        fn busy_mutex_rejects_inspect_preflight_and_checked_restore_without_retry() {
            for action in 0..3 {
                let (wow, data, mut app, addons) = fixture(true);
                let preview = if action == 2 {
                    Some(plan(&mut app))
                } else {
                    if action == 1 {
                        let command = app.dispatch(Message::InspectRecovery);
                        finish(&mut app, command);
                    }
                    None
                };
                let before = tree(wow.path());
                let config_before = tree(data.path());
                let lock = fs::File::options()
                    .read(true)
                    .write(true)
                    .open(addons.join(".patchloom-install.lock"))
                    .unwrap();
                lock.try_lock().unwrap();

                let command = match action {
                    0 => app.dispatch(Message::InspectRecovery),
                    1 => app.dispatch(Message::PreviewRecovery),
                    _ => restore(&mut app, preview.as_ref().unwrap()),
                };
                let request = app.model.pending_recovery.as_ref().unwrap().clone();
                owns(&app, &request);

                if action == 2 {
                    let message = local_message(command);
                    assert!(matches!(
                        &message,
                        Message::RecoveryDone(
                            _,
                            RecoveryResult::Restore {
                                outcome: Err(_),
                                inspection: Err(_),
                            }
                        )
                    ));
                    // Error and follow-up error have both been obtained, but
                    // capacity remains reserved until matching dispatch.
                    owns(&app, &request);
                    assert!(app.model.restoring());
                    no_command(app.dispatch(message.clone()));
                    no_command(app.dispatch(message));
                    assert!(matches!(app.model.recovery_outcome.as_ref(), Some(Err(_))));
                    assert!(app.model.recovery_post_error.is_some());
                    assert!(app.model.recovery_inspection.is_none());
                    assert!(app.model.inventory.is_none());
                } else {
                    finish_recovery_failure(&mut app, command);
                }

                // Never inventory or reopen mutex bytes while this handle
                // holds the exclusive OS lock, including on Windows.
                drop(lock);
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.pending_recovery.is_none());
                assert!(app.model.receipt_worker.is_none());
                drop(app.view());
                no_command(app.dispatch(Message::NativeCheckDone));
                assert!(app.model.recovery_job.is_none());
                assert_no_release_work(&app.model);
                assert!(!addons.join(".patchloom-restore").exists());
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);

                // Lock release permits a fresh real preflight. It does not
                // trigger an implicit GUI retry or reuse the consumed consent.
                let fresh = preflight_restore_originals(&addons).unwrap();
                assert_eq!(fresh.root(), addons.as_path());
                assert!(app.model.recovery_preview.is_none());
                assert!(app.model.recovery_ack.is_empty());
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        #[cfg(any(unix, windows))]
        fn fixture_directory_link(alias: &Path, target: &Path) {
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, alias).unwrap();
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let output = std::process::Command::new("cmd.exe")
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(alias.components().collect::<PathBuf>())
                    .arg(target.components().collect::<PathBuf>())
                    .creation_flags(0x08000000)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                use std::os::windows::fs::MetadataExt;
                assert_ne!(
                    fs::symlink_metadata(alias).unwrap().file_attributes() & 0x400,
                    0
                );
            }
        }

        #[cfg(any(unix, windows))]
        #[test]
        fn linked_root_journal_backup_and_current_content_cannot_get_approval() {
            for location in 0..4 {
                let (wow, data, mut app, addons) = fixture(true);
                let command = app.dispatch(Message::InspectRecovery);
                finish(&mut app, command);
                let transaction = addons.join(".patchloom-install");
                let outside = data.path().join("link-target");
                fs::create_dir(&outside).unwrap();
                fs::write(outside.join("keep.lua"), b"outside sentinel").unwrap();
                let (alias, target) = match location {
                    0 => {
                        let saved = wow.path().join("preserved-addons");
                        fs::rename(&addons, &saved).unwrap();
                        (addons.clone(), saved)
                    }
                    1 => {
                        let saved = wow.path().join("preserved-journal");
                        fs::rename(&transaction, &saved).unwrap();
                        (transaction.clone(), saved)
                    }
                    2 => (transaction.join("backup/Foo/link"), outside.clone()),
                    _ => (addons.join("Foo/link"), outside.clone()),
                };
                fixture_directory_link(&alias, &target);
                let before = tree(wow.path());
                let config_before = tree(data.path());
                let target_before = tree(&target);

                let command = app.dispatch(Message::PreviewRecovery);
                finish_recovery_failure(&mut app, command);
                let command = app.dispatch(Message::InspectRecovery);
                if location == 3 {
                    // Inspection lists destinations without recursively
                    // approving current content. Preflight above rejected it.
                    finish(&mut app, command);
                    assert!(app.model.recovery_error.is_none());
                    assert!(app.model.recovery_inspection.is_some());
                    assert!(app.model.recovery_preview.is_none());
                } else {
                    finish_recovery_failure(&mut app, command);
                }
                assert!(app.model.recovery_outcome.is_none());
                assert!(app.model.recovery_job.is_none());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
                assert_eq!(tree(&target), target_before);

                #[cfg(windows)]
                fs::remove_dir(&alias).unwrap(); // Unlink only this fixture junction.
                assert_eq!(tree(&target), target_before);
                assert_eq!(
                    fs::read(outside.join("keep.lua")).unwrap(),
                    b"outside sentinel"
                );
            }
        }

        #[cfg(unix)]
        #[test]
        fn linked_mutex_plan_and_marker_fail_actual_dispatch_without_touching_targets() {
            for location in 0..3 {
                let (wow, data, mut app, addons) = fixture(true);
                let command = app.dispatch(Message::InspectRecovery);
                finish(&mut app, command);
                let path = match location {
                    0 => addons.join(".patchloom-install.lock"),
                    1 => addons.join(".patchloom-install/plan.yml"),
                    _ => addons.join(".patchloom-install/ready"),
                };
                let saved = wow.path().join("preserved-linked-file");
                fs::rename(&path, &saved).unwrap();
                std::os::unix::fs::symlink(&saved, &path).unwrap();
                let before = tree(wow.path());
                let config_before = tree(data.path());

                let command = app.dispatch(Message::PreviewRecovery);
                finish_recovery_failure(&mut app, command);
                let command = app.dispatch(Message::InspectRecovery);
                finish_recovery_failure(&mut app, command);
                assert!(!addons.join(".patchloom-restore").exists());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        // Added 2026-10-28; original GPLv3 applies.
        #[test]
        fn scan_and_receipt_capacity_exclude_recovery_and_drain_one_queued_scan() {
            for receipt_owner in [false, true] {
                let (wow, data, mut app, _) = fixture(true);
                let preview = plan(&mut app);
                no_command(app.dispatch(Message::RecoveryAckChanged(
                    preview.clone(),
                    "RESTORE".into(),
                )));
                no_command(app.dispatch(Message::ReceiptIdChanged("a".repeat(64))));
                let before = tree(wow.path());
                let config_before = tree(data.path());
                let command = app.dispatch(if receipt_owner {
                    Message::InspectReceipt
                } else {
                    Message::Rescan
                });
                let token = if receipt_owner {
                    app.model.receipt_worker.as_ref().unwrap().clone()
                } else {
                    app.model.scan.as_ref().unwrap().clone()
                };

                for message in [
                    Message::InspectRecovery,
                    Message::PreviewRecovery,
                    Message::RestoreRecovery(preview.clone()),
                    Message::InspectReceipt,
                ] {
                    no_command(app.dispatch(message));
                }
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.pending_recovery.is_none());
                no_command(app.dispatch(Message::Rescan));
                no_command(app.dispatch(Message::Rescan));
                assert!(app.model.scan_requested);
                let owner = if receipt_owner {
                    app.model.receipt_worker.as_ref().unwrap()
                } else {
                    app.model.scan.as_ref().unwrap()
                };
                assert!(Arc::ptr_eq(owner, &token));

                let old = local_message(command);
                let queued = app.dispatch(old.clone());
                assert!(app.model.receipt_worker.is_none());
                assert!(!app.model.scan_requested);
                let scan = app.model.scan.as_ref().unwrap().clone();
                no_command(app.dispatch(old));
                assert!(Arc::ptr_eq(app.model.scan.as_ref().unwrap(), &scan));
                no_command(app.dispatch(Message::InspectRecovery));
                no_command(app.dispatch(Message::PreviewRecovery));
                no_command(app.dispatch(Message::InspectReceipt));
                finish(&mut app, queued);
                no_command(app.dispatch(Message::Select(Product::Retail)));
                assert!(app.model.recovery_slot_available());
                assert!(app.model.recovery_inspection.is_none());
                assert!(app.model.recovery_preview.is_none());
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), before);
                assert_eq!(tree(data.path()), config_before);
            }
        }

        #[test]
        fn invalidation_keeps_recovery_capacity_until_stale_matching_completion() {
            for preview_action in [false, true] {
                for change in 0..4 {
                    let (wow, data, mut app, _) = fixture(true);
                    let (other, _) = receipt_model();
                    let approved = plan(&mut app);
                    let before = tree(wow.path());
                    let config_before = tree(data.path());
                    let other_before = tree(other.path());
                    let command = app.dispatch(if preview_action {
                        Message::PreviewRecovery
                    } else {
                        Message::InspectRecovery
                    });
                    let request = app.model.pending_recovery.as_ref().unwrap().clone();
                    let message = local_message(command);
                    let root = app.model.root.clone();

                    match change {
                        0 => {
                            no_command(app.dispatch(Message::Select(Product::ClassicProgression)));
                            no_command(app.dispatch(Message::Select(Product::Retail)));
                        }
                        1 => no_command(app.dispatch(Message::Rescan)),
                        2 => {
                            no_command(app.dispatch(Message::Rescan));
                            for path in [root.join("draft-only"), root.clone()] {
                                no_command(app.dispatch(Message::RootInputChanged(
                                    path.to_str().unwrap().into(),
                                )));
                            }
                            assert!(!app.model.scan_requested);
                            assert!(app.root_edited);
                        }
                        _ => {
                            for path in [other.path(), root.as_path()] {
                                no_command(app.dispatch(Message::RootInputChanged(
                                    path.to_str().unwrap().into(),
                                )));
                                no_command(app.dispatch(Message::OpenRoot));
                            }
                        }
                    }
                    assert!(app.model.pending_recovery.is_none());
                    assert!(app
                        .model
                        .recovery_job
                        .as_ref()
                        .unwrap()
                        .same_request(&request));
                    assert!(Arc::ptr_eq(
                        app.model.receipt_worker.as_ref().unwrap(),
                        &request.token,
                    ));
                    assert!(app.model.scan.is_none());
                    for rejected in [
                        Message::InspectRecovery,
                        Message::PreviewRecovery,
                        Message::InspectReceipt,
                        Message::RestoreRecovery(approved),
                        Message::NativeCheckDone,
                    ] {
                        no_command(app.dispatch(rejected));
                    }
                    assert!(!app.should_exit());

                    let queued = app.model.scan_requested;
                    let next = app.dispatch(message.clone());
                    assert!(app.model.recovery_job.is_none());
                    assert!(app.model.receipt_worker.is_none());
                    assert!(app.model.recovery_inspection.is_none());
                    assert!(app.model.recovery_preview.is_none());
                    assert!(app.model.recovery_error.is_none());
                    assert!(app.model.recovery_outcome.is_none());
                    assert!(app.model.recovery_ack.is_empty());
                    if queued {
                        let token = app.model.scan.as_ref().unwrap().clone();
                        no_command(app.dispatch(message));
                        assert!(Arc::ptr_eq(app.model.scan.as_ref().unwrap(), &token,));
                        finish(&mut app, next);
                    } else {
                        no_command(next);
                        no_command(app.dispatch(message));
                        assert!(app.model.scan.is_none());
                    }
                    if app.root_edited {
                        let command = app.dispatch(Message::OpenRoot);
                        finish(&mut app, command);
                    }
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                    assert!(app.model.recovery_slot_available());
                    assert!(app.model.recovery_inspection.is_none());
                    assert!(app.model.recovery_preview.is_none());
                    assert_no_release_work(&app.model);
                    assert_eq!(tree(wow.path()), before);
                    assert_eq!(tree(data.path()), config_before);
                    assert_eq!(tree(other.path()), other_before);
                }
            }
        }

        fn refuse_restore_scope_changes(app: &mut Preview, preview: &RecoveryPreview) {
            let request = app.model.pending_recovery.as_ref().unwrap().clone();
            let root = app.model.root.clone();
            let input = app.root_input.clone();
            let edited = app.root_edited;
            let selected = app.model.selected;
            let generation = app.model.generation.clone();
            assert!(app.model.restoring());
            for message in [
                Message::RootInputChanged(root.join("refused-draft").to_str().unwrap().into()),
                Message::OpenRoot,
                Message::Select(Product::ClassicProgression),
                Message::Rescan,
                Message::CancelRecoveryPreview(preview.clone()),
                Message::RecoveryAckChanged(preview.clone(), "RESTORE".into()),
                Message::RestoreRecovery(preview.clone()),
                Message::InspectRecovery,
                Message::PreviewRecovery,
                Message::InspectReceipt,
                Message::CheckRelease,
            ] {
                no_command(app.dispatch(message));
                owns(app, &request);
                assert_eq!(app.model.root, root);
                assert_eq!(app.root_input, input);
                assert_eq!(app.root_edited, edited);
                assert_eq!(app.model.selected, selected);
                assert!(Arc::ptr_eq(&app.model.generation, &generation));
                assert!(app.model.scan.is_none());
                assert!(!app.model.scan_requested);
                assert!(app.model.inventory.is_none());
                assert!(app.model.receipt.is_none());
                assert!(app.model.pending_receipt.is_none());
                assert!(app.model.recovery_ack.is_empty());
            }
            drop(app.view());
            no_command(app.dispatch(Message::NativeCheckDone));
            assert!(!app.should_exit());
            owns(app, &request);
        }

        #[test]
        fn recovery_completions_require_full_identity_action_and_slot_ownership() {
            for action in 0..3 {
                let (wow, data, mut app, _) = fixture(true);
                // Obtain genuine historical scan and receipt completions.
                let command = app.dispatch(Message::Rescan);
                let scan_message = local_message(command);
                no_command(app.dispatch(scan_message.clone()));
                no_command(app.dispatch(Message::Select(Product::Retail)));
                no_command(app.dispatch(Message::ReceiptIdChanged("a".repeat(64))));
                let command = app.dispatch(Message::InspectReceipt);
                let receipt_message = local_message(command);
                no_command(app.dispatch(receipt_message.clone()));
                let preview = plan(&mut app);
                let alternate =
                    Arc::new(preflight_restore_originals(preview.approval.root()).unwrap());
                assert!(!Arc::ptr_eq(&alternate, &preview.approval));

                let command = match action {
                    0 => app.dispatch(Message::InspectRecovery),
                    1 => app.dispatch(Message::PreviewRecovery),
                    _ => restore(&mut app, &preview),
                };
                let request = app.model.pending_recovery.as_ref().unwrap().clone();
                owns(&app, &request);
                if action == 2 {
                    refuse_restore_scope_changes(&mut app, &preview);
                }
                let message = local_message(command);
                let result = match &message {
                    Message::RecoveryDone(echo, result) => {
                        assert!(echo.same_request(&request));
                        result.clone()
                    }
                    other => panic!("Expected recovery completion: {:?}", other),
                };
                if action == 2 {
                    assert!(matches!(
                        &result,
                        RecoveryResult::Restore {
                            outcome: Ok(ajour_core::fs::RecoveryRestoreOutcome::RestoredRetained),
                            inspection: Ok(_),
                        }
                    ));
                    // Worker has returned both results; dispatch still owns
                    // the reservation and continues refusing scope changes.
                    refuse_restore_scope_changes(&mut app, &preview);
                }
                let after_worker = tree(wow.path());
                let config_after_worker = tree(data.path());

                for changed_field in 0..6 {
                    let mut foreign = request.clone();
                    match changed_field {
                        0 => foreign.root = foreign.root.join("foreign"),
                        1 => foreign.addons = foreign.addons.join("foreign"),
                        2 => foreign.product = Product::ClassicProgression,
                        3 => foreign.generation = Arc::new(()),
                        4 => foreign.token = Arc::new(()),
                        _ => {
                            foreign.action = if action == 0 {
                                RecoveryAction::Preview
                            } else {
                                RecoveryAction::Inspect
                            }
                        }
                    }
                    no_command(app.dispatch(Message::RecoveryDone(foreign, result.clone())));
                    owns(&app, &request);
                }
                if action == 2 {
                    let mut wrong_ticket = request.clone();
                    wrong_ticket.action = RecoveryAction::Restore(alternate);
                    no_command(app.dispatch(Message::RecoveryDone(wrong_ticket, result.clone())));
                    owns(&app, &request);
                }
                // Deliberately malformed completion, not a claimed core result.
                let wrong_kind = if action == 0 {
                    RecoveryResult::Preview(Err("wrong result variant".into()))
                } else {
                    RecoveryResult::Inspection(Err("wrong result variant".into()))
                };
                no_command(app.dispatch(Message::RecoveryDone(request.clone(), wrong_kind)));
                owns(&app, &request);

                // A receipt/scan envelope cannot complete recovery merely by
                // borrowing its token. Payloads came from actual local workers.
                if let Message::ReceiptDone(mut receipt, result) = receipt_message.clone() {
                    receipt.token = request.token.clone();
                    no_command(app.dispatch(Message::ReceiptDone(receipt, result)));
                } else {
                    panic!("Expected receipt completion");
                }
                if let Message::InventoryDone(mut scan, result) = scan_message {
                    scan.token = request.token.clone();
                    no_command(app.dispatch(Message::InventoryDone(scan, result)));
                } else {
                    panic!("Expected inventory completion");
                }
                owns(&app, &request);
                assert_eq!(tree(wow.path()), after_worker);
                assert_eq!(tree(data.path()), config_after_worker);

                no_command(app.dispatch(message.clone()));
                assert!(app.model.recovery_job.is_none());
                assert!(app.model.pending_recovery.is_none());
                assert!(app.model.receipt_worker.is_none());

                let command = app.dispatch(Message::Rescan);
                let token = app.model.scan.as_ref().unwrap().clone();
                no_command(app.dispatch(message.clone()));
                assert!(Arc::ptr_eq(app.model.scan.as_ref().unwrap(), &token));
                finish(&mut app, command);
                no_command(app.dispatch(Message::Select(Product::Retail)));

                let command = app.dispatch(Message::InspectRecovery);
                let newer = app.model.pending_recovery.as_ref().unwrap().clone();
                assert!(!Arc::ptr_eq(&newer.token, &request.token));
                no_command(app.dispatch(message));
                no_command(app.dispatch(receipt_message));
                owns(&app, &newer);
                let current = local_message(command);
                no_command(app.dispatch(current.clone()));
                let published = app.model.recovery_inspection.as_ref().unwrap().clone();
                no_command(app.dispatch(current));
                assert!(Arc::ptr_eq(
                    app.model.recovery_inspection.as_ref().unwrap(),
                    &published,
                ));
                assert!(app.model.recovery_slot_available());
                assert_no_release_work(&app.model);
                assert_eq!(tree(wow.path()), after_worker);
                assert_eq!(tree(data.path()), config_after_worker);
            }
        }

        #[test]
        fn recovery_preserves_provider_capacity_and_typed_waits_without_http() {
            for action in 0..3 {
                for state in 0..4 {
                    for rate_result in [false, true] {
                        let (wow, data, mut app, _) = fixture(true);
                        let preview = plan(&mut app);
                        no_command(
                            app.dispatch(Message::RepositoryChanged("fixture/addon".into())),
                        );
                        // Construct real provider commands; never poll them.
                        let futures = app.dispatch(Message::CheckRelease).futures();
                        assert_eq!(futures.len(), 1);
                        let first = app.model.pending_release.as_ref().unwrap().clone();
                        drop(futures);
                        let active = if state == 0 {
                            Some(first.clone())
                        } else {
                            let now = Instant::now();
                            let wait = match state {
                                1 => RetryDeadline::At(now),
                                2 => RetryDeadline::from_wait(
                                    now,
                                    std::time::Duration::from_secs(172_800),
                                ),
                                _ => RetryDeadline::Blocked,
                            };
                            no_command(
                                app.dispatch(Message::ReleaseRateLimited(first.clone(), wait)),
                            );
                            if state == 1 {
                                let futures = app.dispatch(Message::CheckRelease).futures();
                                assert_eq!(futures.len(), 1);
                                let current = app.model.pending_release.as_ref().unwrap().clone();
                                drop(futures);
                                Some(current)
                            } else {
                                None
                            }
                        };
                        let deadline = app.model.release_retry_at;
                        let command = match action {
                            0 => app.dispatch(Message::InspectRecovery),
                            1 => app.dispatch(Message::PreviewRecovery),
                            _ => restore(&mut app, &preview),
                        };
                        let recovery = app.model.pending_recovery.as_ref().unwrap().clone();
                        owns(&app, &recovery);
                        assert_eq!(app.model.release_retry_at, deadline);
                        if let Some(provider) = &active {
                            assert!(Arc::ptr_eq(
                                app.model.release_worker.as_ref().unwrap(),
                                &provider.token,
                            ));
                            assert_eq!(app.model.pending_release.is_some(), action != 2);
                        } else {
                            assert!(app.model.release_worker.is_none());
                        }
                        no_command(app.dispatch(Message::CheckRelease));
                        let message = local_message(command);
                        owns(&app, &recovery);
                        no_command(app.dispatch(message));
                        assert_eq!(app.model.release_retry_at, deadline);
                        let after = tree(wow.path());
                        let config_after = tree(data.path());

                        let mut foreign = active.as_ref().unwrap_or(&first).clone();
                        foreign.token = Arc::new(());
                        no_command(app.dispatch(Message::ReleaseRateLimited(
                            foreign,
                            RetryDeadline::Blocked,
                        )));
                        assert_eq!(app.model.release_retry_at, deadline);

                        let expected = if let Some(provider) = active {
                            assert!(Arc::ptr_eq(
                                app.model.release_worker.as_ref().unwrap(),
                                &provider.token,
                            ));
                            let expected = if rate_result {
                                let incoming = if state == 0 {
                                    RetryDeadline::from_wait(
                                        Instant::now(),
                                        std::time::Duration::from_secs(172_800),
                                    )
                                } else {
                                    RetryDeadline::Blocked
                                };
                                no_command(app.dispatch(Message::ReleaseRateLimited(
                                    provider.clone(),
                                    incoming,
                                )));
                                Some(deadline.map_or(incoming, |old| old.max(incoming)))
                            } else {
                                no_command(app.dispatch(Message::ReleaseDone(
                                    provider.clone(),
                                    Ok("fixture-version".into()),
                                )));
                                deadline
                            };
                            assert_eq!(app.model.release_result.is_some(), action != 2);
                            no_command(app.dispatch(Message::ReleaseRateLimited(
                                provider.clone(),
                                RetryDeadline::Blocked,
                            )));
                            no_command(app.dispatch(Message::ReleaseDone(
                                provider,
                                Err("duplicate provider completion".into()),
                            )));
                            expected
                        } else {
                            no_command(app.dispatch(Message::ReleaseRateLimited(
                                first,
                                RetryDeadline::At(Instant::now()),
                            )));
                            deadline
                        };
                        assert_eq!(app.model.release_retry_at, expected);
                        assert!(app.model.release_worker.is_none());
                        assert!(app.model.pending_release.is_none());

                        let command = app.dispatch(Message::Rescan);
                        no_command(app.dispatch(local_message(command)));
                        no_command(app.dispatch(Message::Select(Product::Retail)));
                        drop(app.view());
                        no_command(app.dispatch(Message::NativeCheckDone));
                        assert_no_release_work(&app.model);
                        assert_eq!(app.model.release_retry_at, expected);
                        if expected.is_some_and(|wait| !wait.allows(Instant::now())) {
                            no_command(app.dispatch(Message::CheckRelease));
                            assert_no_release_work(&app.model);
                            assert_eq!(app.model.release_retry_at, expected);
                        }
                        assert_eq!(tree(wow.path()), after);
                        assert_eq!(tree(data.path()), config_after);
                    }
                }
            }
        }

        // Added 2026-10-28; original GPLv3 applies.
        #[test]
        fn restore_outcome_survives_independent_postinspection_failure() {
            use ajour_core::fs::{
                inspect_recovery, restore_originals_checked, RecoveryRestoreOutcome as Outcome,
            };
            use std::fs;

            for kind in 0..4 {
                for post_failure in [false, true] {
                    let (wow, data, mut app, _) = fixture(true);
                    let preview = plan(&mut app);
                    let command = restore(&mut app, &preview);
                    let request = app.model.pending_recovery.as_ref().unwrap().clone();
                    let addons = request.addons.clone();

                    match kind {
                        2 => {
                            let journal = addons.join(".patchloom-install/plan.yml");
                            let mut bytes = fs::read(&journal).unwrap();
                            bytes.extend_from_slice(b"\n# changed after GUI preview\n");
                            fs::write(journal, bytes).unwrap();
                        }
                        3 => {
                            // Preserve the original elsewhere in this fixture.
                            // Inspection can report the missing backup; checked
                            // restore must refuse to guess at its contents.
                            fs::rename(
                                addons
                                    .join(".patchloom-install/backup")
                                    .join(&preview.approval.originals()[0]),
                                wow.path().join("preserved-original-for-error"),
                            )
                            .unwrap();
                        }
                        _ => {}
                    }
                    let before_worker = tree(wow.path());
                    let config_before = tree(data.path());

                    let message = if kind == 1 {
                        // GUI's admitted worker callback always approves its
                        // matching slices. Exercise real core cancellation
                        // separately, then inject its completion envelope.
                        drop(command.futures());
                        let outcome = restore_originals_checked(
                            &addons,
                            preview.approval.as_ref(),
                            |originals, retained| {
                                assert_eq!(originals, preview.approval.originals());
                                assert_eq!(retained, preview.approval.move_aside_if_present(),);
                                false
                            },
                        )
                        .map_err(|error| error.to_string());
                        let inspection = inspect_recovery(&addons)
                            .map(Arc::new)
                            .map_err(|error| error.to_string());
                        Message::RecoveryDone(
                            request.clone(),
                            RecoveryResult::Restore {
                                outcome,
                                inspection,
                            },
                        )
                    } else {
                        // Actual GUI Command -> blocking worker -> checked core
                        // restore -> post-inspection -> completion message.
                        local_message(command)
                    };
                    let (outcome, inspection) = match message {
                        Message::RecoveryDone(
                            echo,
                            RecoveryResult::Restore {
                                outcome,
                                inspection,
                            },
                        ) => {
                            assert!(echo.same_request(&request));
                            (outcome, inspection)
                        }
                        other => panic!("Expected restore completion: {:?}", other),
                    };
                    match kind {
                        0 => assert_eq!(outcome, Ok(Outcome::RestoredRetained)),
                        1 => assert_eq!(outcome, Ok(Outcome::Cancelled)),
                        2 => assert_eq!(outcome, Ok(Outcome::Changed)),
                        _ => assert!(outcome.is_err()),
                    }
                    let inspection = inspection.unwrap();
                    if kind != 0 {
                        assert_eq!(tree(wow.path()), before_worker);
                    }
                    owns(&app, &request);
                    assert!(app.model.recovery_outcome.is_none());
                    assert!(app.model.recovery_post_error.is_none());

                    let expected = outcome.clone();
                    let after_worker = tree(wow.path());
                    // Only this follow-up failure is synthetic. No phase pause
                    // or failed filesystem inspection is claimed by this branch.
                    let follow_up = if post_failure {
                        Err("injected follow-up inspection failure".into())
                    } else {
                        Ok(inspection.clone())
                    };
                    no_command(app.dispatch(Message::RecoveryDone(
                        request.clone(),
                        RecoveryResult::Restore {
                            outcome,
                            inspection: follow_up,
                        },
                    )));
                    assert_eq!(app.model.recovery_outcome, Some(expected.clone()));
                    assert_eq!(
                        app.model.recovery_post_error.as_deref(),
                        post_failure.then_some("injected follow-up inspection failure"),
                    );
                    if post_failure {
                        assert!(app.model.recovery_inspection.is_none());
                    } else {
                        assert!(Arc::ptr_eq(
                            app.model.recovery_inspection.as_ref().unwrap(),
                            &inspection,
                        ));
                    }
                    assert!(app.model.recovery_error.is_none());
                    assert!(app.model.recovery_preview.is_none());
                    assert!(app.model.recovery_ack.is_empty());
                    assert!(app.model.inventory.is_none());
                    assert!(app.model.receipt.is_none());
                    assert!(app.model.receipt_error.is_none());
                    assert!(app.model.recovery_slot_available());

                    let text = recovery_outcome_text(&expected);
                    match kind {
                        0 => {
                            assert!(text.starts_with("Original folders restored."));
                            assert!(text.contains("Updates remain blocked"));
                        }
                        1 => assert_eq!(text, "Restore cancelled. No addon folders were changed.",),
                        2 => {
                            assert!(text.starts_with("Recovery state changed."));
                            assert!(text.contains("No restore was started."));
                        }
                        _ => {
                            assert!(text.starts_with("Restore failed:"));
                            assert!(text.contains("Some folders may already have moved."));
                            assert!(!text.starts_with("Original folders restored."));
                        }
                    }

                    // A duplicate cannot replace either saved result.
                    no_command(app.dispatch(Message::RecoveryDone(
                        request,
                        RecoveryResult::Restore {
                            outcome: Err("duplicate outcome".into()),
                            inspection: Err("duplicate inspection".into()),
                        },
                    )));
                    drop(app.view());
                    assert_eq!(app.model.recovery_outcome, Some(expected));
                    assert_eq!(
                        app.model.recovery_post_error.as_deref(),
                        post_failure.then_some("injected follow-up inspection failure"),
                    );
                    assert_eq!(tree(wow.path()), after_worker);
                    assert_eq!(tree(data.path()), config_before);
                    assert_no_release_work(&app.model);
                }
            }
        }

        #[test]
        fn restore_admission_clears_ownership_displays_and_rejects_delayed_receipts() {
            for prior_error in [false, true] {
                let (wow, data, mut app, _) = fixture(true);
                let preview = plan(&mut app);
                let command = app.dispatch(Message::InspectReceipt);
                let receipt = app.model.pending_receipt.as_ref().unwrap().clone();
                drop(command.futures());
                let result = if prior_error {
                    Err("prior ownership error".into())
                } else {
                    Ok(matching_receipt(&receipt))
                };
                let delayed = Message::ReceiptDone(receipt, result);
                no_command(app.dispatch(delayed.clone()));
                assert_eq!(app.model.receipt.is_some(), !prior_error);
                assert_eq!(app.model.receipt_error.is_some(), prior_error);
                assert!(app.model.inventory.is_some());

                let command = restore(&mut app, &preview);
                let request = app.model.pending_recovery.as_ref().unwrap().clone();
                // These assertions precede polling the restore command.
                assert!(app.model.inventory.is_none());
                assert!(app.model.error.is_none());
                assert!(app.choices.is_empty());
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());
                assert!(app.model.pending_receipt.is_none());
                no_command(app.dispatch(delayed.clone()));
                owns(&app, &request);
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());

                let completed = local_message(command);
                no_command(app.dispatch(delayed.clone()));
                owns(&app, &request);
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());
                no_command(app.dispatch(completed));
                assert!(matches!(
                    app.model.recovery_outcome.as_ref(),
                    Some(Ok(ajour_core::fs::RecoveryRestoreOutcome::RestoredRetained))
                ));
                let report = app.model.recovery_inspection.as_ref().unwrap().clone();
                let after = tree(wow.path());
                let config_after = tree(data.path());
                no_command(app.dispatch(delayed.clone()));
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());
                assert!(Arc::ptr_eq(
                    app.model.recovery_inspection.as_ref().unwrap(),
                    &report,
                ));

                let scan = app.dispatch(Message::Rescan);
                let token = app.model.scan.as_ref().unwrap().clone();
                no_command(app.dispatch(delayed.clone()));
                assert!(Arc::ptr_eq(app.model.scan.as_ref().unwrap(), &token));
                finish(&mut app, scan);
                no_command(app.dispatch(Message::Select(Product::Retail)));

                let command = app.dispatch(Message::InspectReceipt);
                let current = app.model.pending_receipt.as_ref().unwrap().clone();
                no_command(app.dispatch(delayed));
                assert!(Arc::ptr_eq(
                    app.model.receipt_worker.as_ref().unwrap(),
                    &current.token,
                ));
                assert!(Arc::ptr_eq(
                    &app.model.pending_receipt.as_ref().unwrap().token,
                    &current.token,
                ));
                assert!(app.model.receipt.is_none());
                assert!(app.model.receipt_error.is_none());
                // Finish the actual current ownership worker, whatever its
                // honest fixture result; the injected old display cannot win.
                finish(&mut app, command);
                assert!(app.model.receipt_worker.is_none());
                assert!(app.model.pending_receipt.is_none());
                assert_eq!(tree(wow.path()), after);
                assert_eq!(tree(data.path()), config_after);
                assert_no_release_work(&app.model);
            }
        }

        #[test]
        fn recovery_paging_preserves_complete_approval_and_rejects_stale_messages() {
            use std::fs;

            let (wow, data, mut app, _) = fixture(true);
            let old_root_preview = plan(&mut app);
            let container = tempfile::tempdir().unwrap();
            let mut root = container.path().canonicalize().unwrap();
            for _ in 0..8 {
                root = root.join("paging-scope-é-界-\u{202e}-abcdefghijklmnop");
            }
            fs::create_dir_all(&root).unwrap();
            for entry in fs::read_dir(wow.path()).unwrap() {
                let entry = entry.unwrap();
                fs::rename(entry.path(), root.join(entry.file_name())).unwrap();
            }
            let addons = root
                .join("_retail_/Interface/AddOns")
                .canonicalize()
                .unwrap();
            let state = ajour_core::fs::inspect_recovery(&addons).unwrap();
            let mut previous = state.previous;
            for index in 0..(2 * FOLDERS_SHOWN + 3) {
                let name = format!("Paging{index:03}_{}_\u{202e}_end", "界".repeat(65));
                assert!(name.len() <= 255);
                let backup = addons.join(".patchloom-install/backup").join(&name);
                fs::create_dir(&backup).unwrap();
                fs::write(backup.join("original.lua"), b"paging original").unwrap();
                previous.push(name);
            }
            fs::write(
                addons.join(".patchloom-install/plan.yml"),
                serde_json::to_vec(&serde_json::json!({
                    "version": 1,
                    "previous": previous,
                    "incoming": state.incoming,
                }))
                .unwrap(),
            )
            .unwrap();
            no_command(app.dispatch(Message::RootInputChanged(root.to_str().unwrap().into())));
            let command = app.dispatch(Message::OpenRoot);
            finish(&mut app, command);
            no_command(app.dispatch(Message::Select(Product::Retail)));
            let command = app.dispatch(Message::InspectRecovery);
            finish(&mut app, command);
            let command = app.dispatch(Message::PreviewRecovery);
            finish(&mut app, command);
            let old = app.model.recovery_preview.as_ref().unwrap().clone();
            // A fresh real preflight replaces request/ticket identity.
            let command = app.dispatch(Message::PreviewRecovery);
            finish(&mut app, command);
            let preview = app.model.recovery_preview.as_ref().unwrap().clone();
            let originals = preview.approval.originals().to_vec();
            let retained = preview.approval.move_aside_if_present().to_vec();
            let canonical = preview.approval.root().to_path_buf();
            let before = tree(container.path());
            let config_before = tree(data.path());
            no_command(app.dispatch(Message::RecoveryAckChanged(
                preview.clone(),
                "RESTORE".into(),
            )));

            let root_pages = recovery_root_pages(&canonical);
            assert!(root_pages > 1);
            let mut displayed_root = String::new();
            for page in 0..root_pages {
                no_command(app.dispatch(Message::RecoveryRootPage(preview.clone(), page)));
                assert_eq!(app.recovery.root_page, page);
                let chunk = recovery_root_chunk(&canonical, page);
                assert!(chunk.is_ascii());
                assert!(chunk.len() <= 10 * ERROR_CHARS);
                displayed_root.push_str(&chunk);
                drop(app.view());
            }
            assert_eq!(
                displayed_root,
                format!("{:?}", canonical.as_os_str())
                    .escape_default()
                    .to_string(),
            );
            let count = originals.len().max(retained.len());
            let last = count.saturating_sub(1) / FOLDERS_SHOWN;
            assert!(last >= 2);
            for page in 0..=last {
                no_command(app.dispatch(Message::RecoveryPage(preview.clone(), page)));
                assert_eq!(app.model.recovery_page, page);
                for folders in [&originals, &retained] {
                    let text = recovery_folder_page(folders, page);
                    let start = page * FOLDERS_SHOWN;
                    let names: Vec<_> = folders.iter().skip(start).take(FOLDERS_SHOWN).collect();
                    if names.is_empty() {
                        assert_eq!(text, "No folders on this page.");
                    } else {
                        assert_eq!(text.lines().count(), names.len());
                        for (offset, (line, name)) in text.lines().zip(names).enumerate() {
                            assert_eq!(
                                line,
                                format!("{}. \"{}\"", start + offset + 1, name.escape_default(),)
                            );
                        }
                    }
                    assert!(text.is_ascii());
                    assert!(!text.contains('\u{202e}'));
                }
                drop(app.view());
            }
            no_command(app.dispatch(Message::RecoveryPage(preview.clone(), usize::MAX)));
            no_command(app.dispatch(Message::RecoveryRootPage(preview.clone(), usize::MAX)));
            assert_eq!(app.model.recovery_page, last);
            assert_eq!(app.recovery.root_page, root_pages - 1);

            no_command(app.dispatch(Message::RecoveryPage(preview.clone(), 1)));
            no_command(app.dispatch(Message::RecoveryRootPage(preview.clone(), 1)));
            let mut stale = vec![old, old_root_preview.clone()];
            for field in 0..7 {
                let mut foreign = preview.clone();
                match field {
                    0 => foreign.request.root = foreign.request.root.join("foreign"),
                    1 => foreign.request.addons = foreign.request.addons.join("foreign"),
                    2 => foreign.request.product = Product::ClassicProgression,
                    3 => foreign.request.generation = Arc::new(()),
                    4 => foreign.request.token = Arc::new(()),
                    5 => foreign.request.action = RecoveryAction::Inspect,
                    _ => {
                        foreign.approval =
                            Arc::new(ajour_core::fs::preflight_restore_originals(&addons).unwrap())
                    }
                }
                stale.push(foreign);
            }
            for stale in stale {
                no_command(app.dispatch(Message::RecoveryPage(stale.clone(), 0)));
                no_command(app.dispatch(Message::RecoveryRootPage(stale, 0)));
                assert_eq!(app.model.recovery_page, 1);
                assert_eq!(app.recovery.root_page, 1);
            }

            // Even current paging messages cannot change controls while the
            // shared physical filesystem slot belongs to a receipt worker.
            let command = app.dispatch(Message::InspectReceipt);
            no_command(app.dispatch(Message::RecoveryPage(preview.clone(), 0)));
            no_command(app.dispatch(Message::RecoveryRootPage(preview.clone(), 0)));
            assert_eq!(app.model.recovery_page, 1);
            assert_eq!(app.recovery.root_page, 1);
            finish(&mut app, command);
            assert_eq!(preview.approval.root(), canonical);
            assert_eq!(preview.approval.originals(), originals);
            assert_eq!(preview.approval.move_aside_if_present(), retained);
            assert!(Arc::ptr_eq(
                &app.model.recovery_preview.as_ref().unwrap().approval,
                &preview.approval,
            ));
            assert_eq!(app.model.recovery_ack, "RESTORE");
            assert_eq!(tree(container.path()), before);
            assert_eq!(tree(data.path()), config_before);

            let command = app.dispatch(Message::RestoreRecovery(preview.clone()));
            let request = app.model.pending_recovery.as_ref().unwrap().clone();
            owns(&app, &request);
            no_command(app.dispatch(Message::RecoveryPage(preview.clone(), 1)));
            no_command(app.dispatch(Message::RecoveryRootPage(preview.clone(), 1)));
            assert_eq!(app.model.recovery_page, 0);
            assert_eq!(app.recovery.root_page, 0);
            finish(&mut app, command);
            assert!(matches!(
                app.model.recovery_outcome.as_ref(),
                Some(Ok(ajour_core::fs::RecoveryRestoreOutcome::RestoredRetained))
            ));
            assert_eq!(preview.approval.originals(), originals);
            assert_eq!(preview.approval.move_aside_if_present(), retained);
            assert_eq!(tree(data.path()), config_before);
            assert_no_release_work(&app.model);
        }

        #[test]
        fn recovery_display_helpers_escape_controls_and_handle_empty_or_extreme_pages() {
            for (name, expected) in [
                ("A\nB", "1. \"A\\nB\""),
                ("界\u{202e}", "1. \"\\u{754c}\\u{202e}\""),
                ("\"\\", r#"1. "\"\\""#),
            ] {
                let names = vec![name.to_string()];
                assert_eq!(recovery_folder_page(&names, 0), expected);
                assert_eq!(names[0], name);
                assert_eq!(
                    recovery_folder_page(&names, usize::MAX),
                    "No folders on this page.",
                );
            }
            assert_eq!(recovery_folder_page(&[], 0), "None.");
            assert_eq!(recovery_folder_page(&[], usize::MAX), "None.");
            let path = PathBuf::from("root\nwith\tcontrols\u{202e}");
            let expected = format!("{:?}", path.as_os_str())
                .escape_default()
                .to_string();
            assert_eq!(recovery_root_pages(&path), 1);
            assert_eq!(recovery_root_chunk(&path, usize::MAX), expected);
            #[cfg(unix)]
            {
                use std::{ffi::OsString, os::unix::ffi::OsStringExt};
                let path = PathBuf::from(OsString::from_vec(b"root-\xff".to_vec()));
                let shown: String = (0..recovery_root_pages(&path))
                    .map(|page| recovery_root_chunk(&path, page))
                    .collect();
                assert_eq!(
                    shown,
                    format!("{:?}", path.as_os_str())
                        .escape_default()
                        .to_string(),
                );
                assert!(!shown.contains('\u{fffd}'));
            }
        }

        #[test]
        fn recovery_stays_idle_on_rootless_startup_render_and_native_timer_messages() {
            let data = tempfile::tempdir().unwrap();
            std::fs::write(data.path().join("config-sentinel"), b"unchanged").unwrap();
            let before = tree(data.path());
            for native_check in [false, true] {
                let (mut app, command) = Preview::new(Flags {
                    root: PathBuf::new(),
                    native_check,
                });
                no_command(command);
                let model_before = format!("{:?}", app.model);
                let generation = app.model.generation.clone();
                for _ in 0..3 {
                    drop(app.view());
                    assert_eq!(app.subscription().recipes().is_empty(), !native_check);
                    no_command(app.dispatch(Message::InspectRecovery));
                    no_command(app.dispatch(Message::PreviewRecovery));
                    no_command(app.dispatch(Message::NativeCheckDone));
                    assert!(app.should_exit());
                    assert_eq!(format!("{:?}", app.model), model_before);
                    assert!(Arc::ptr_eq(&app.model.generation, &generation));
                    assert!(app.model.recovery_slot_available());
                    assert_no_release_work(&app.model);
                }
            }
            assert_eq!(tree(data.path()), before);
        }

        #[test]
        fn recovery_preview_and_completed_states_do_not_act_on_render_or_timer() {
            for native_check in [false, true] {
                for state in 0..5 {
                    let (wow, data, _, _) = fixture(true);
                    let before_startup = tree(wow.path());
                    let config_before = tree(data.path());
                    let (mut app, command) = Preview::new(Flags {
                        root: wow.path().into(),
                        native_check,
                    });
                    assert!(app.model.recovery_job.is_none());
                    assert!(app.model.pending_recovery.is_none());
                    assert!(app.model.recovery_inspection.is_none());
                    assert!(app.model.recovery_preview.is_none());
                    assert_no_release_work(&app.model);
                    let message = local_message(command);
                    assert!(matches!(&message, Message::InventoryDone(..)));
                    no_command(app.dispatch(message));
                    no_command(app.dispatch(Message::Select(Product::Retail)));
                    assert_eq!(tree(wow.path()), before_startup);
                    assert!(app.model.recovery_inspection.is_none());
                    assert!(app.model.recovery_outcome.is_none());

                    if state != 0 {
                        let preview = plan(&mut app);
                        match state {
                            1 => {
                                no_command(app.dispatch(Message::RecoveryAckChanged(
                                    preview,
                                    "RESTORE".into(),
                                )))
                            }
                            2 => {
                                let command = restore(&mut app, &preview);
                                finish(&mut app, command);
                                assert!(matches!(
                                    app.model.recovery_outcome.as_ref(),
                                    Some(Ok(
                                        ajour_core::fs::RecoveryRestoreOutcome::RestoredRetained
                                    ))
                                ));
                            }
                            3 => {
                                no_command(app.dispatch(Message::CancelRecoveryPreview(preview)));
                                assert!(app.model.recovery_preview.is_none());
                                assert!(app.model.recovery_error.is_some());
                            }
                            _ => {
                                std::fs::write(
                                    preview.approval.root().join(".patchloom-install/plan.yml"),
                                    b"version: [invalid",
                                )
                                .unwrap();
                                let command = restore(&mut app, &preview);
                                finish(&mut app, command);
                                assert!(matches!(
                                    app.model.recovery_outcome.as_ref(),
                                    Some(Err(_))
                                ));
                                assert!(app.model.recovery_post_error.is_some());
                            }
                        }
                    }
                    let after = tree(wow.path());
                    let model_before = format!("{:?}", app.model);
                    let generation = app.model.generation.clone();
                    let ticket = app
                        .model
                        .recovery_preview
                        .as_ref()
                        .map(|preview| preview.approval.clone());
                    for _ in 0..3 {
                        drop(app.view());
                        assert_eq!(app.subscription().recipes().is_empty(), !native_check);
                        no_command(app.dispatch(Message::NativeCheckDone));
                        assert!(app.should_exit());
                        assert_eq!(format!("{:?}", app.model), model_before);
                        assert!(Arc::ptr_eq(&app.model.generation, &generation));
                        if let Some(ticket) = &ticket {
                            assert!(Arc::ptr_eq(
                                &app.model.recovery_preview.as_ref().unwrap().approval,
                                ticket,
                            ));
                            assert_eq!(app.model.recovery_ack, "RESTORE");
                        }
                        assert!(app.model.recovery_slot_available());
                        assert!(app.model.pending_recovery.is_none());
                        assert!(app.model.pending_receipt.is_none());
                        assert!(app.model.scan.is_none());
                        assert!(!app.model.scan_requested);
                        assert_no_release_work(&app.model);
                        assert_eq!(tree(wow.path()), after);
                        assert_eq!(tree(data.path()), config_before);
                    }
                }
            }
        }
    }

    // Added 2026-10-28; original GPLv3 applies.
    mod startup_root_tests {
        use super::*;
        use std::{path::Path, time::Duration};

        fn no_command(command: Command<Message>) {
            assert!(command.futures().is_empty());
        }

        // Call only with a command returned by constructor/Open/Rescan or
        // InspectReceipt. Never pass CheckRelease commands here.
        fn local_message(command: Command<Message>) -> Message {
            let mut futures = command.futures();
            assert_eq!(futures.len(), 1);
            let message = async_std::task::block_on(futures.remove(0));
            assert!(matches!(
                &message,
                Message::InventoryDone(..) | Message::ReceiptDone(..)
            ));
            message
        }

        fn finish_local(app: &mut Preview, command: Command<Message>) {
            let message = local_message(command);
            no_command(app.dispatch(message));
        }

        fn edit(app: &mut Preview, root: &Path) {
            no_command(app.dispatch(Message::RootInputChanged(
                root.to_str().expect("UTF-8 fixture path").into(),
            )));
        }

        fn open(app: &mut Preview, root: &Path) -> Command<Message> {
            edit(app, root);
            app.dispatch(Message::OpenRoot)
        }

        fn owns(worker: Option<&Arc<()>>, token: &Arc<()>) {
            assert!(worker.is_some_and(|worker| Arc::ptr_eq(worker, token)));
        }

        fn invalidated(app: &Preview) {
            assert!(app.model.inventory.is_none());
            assert!(app.model.selected.is_none());
            assert!(app.model.error.is_none());
            assert!(app.choices.is_empty());
            assert!(app.model.receipt.is_none());
            assert!(app.model.receipt_error.is_none());
            assert!(app.model.pending_receipt.is_none());
            assert!(app.model.release_result.is_none());
            assert!(app.model.pending_release.is_none());
        }

        fn click_release(app: &mut Preview) -> ReleaseRequest {
            let futures = app.dispatch(Message::CheckRelease).futures();
            assert_eq!(futures.len(), 1);
            let request = app.model.pending_release.as_ref().unwrap().clone();
            owns(app.model.release_worker.as_ref(), &request.token);
            drop(futures);
            request
        }

        fn tree(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
            fn visit(root: &Path, path: &Path, entries: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
                entries.push((
                    path.strip_prefix(root).unwrap().into(),
                    if path.is_file() {
                        Some(std::fs::read(path).unwrap())
                    } else {
                        None
                    },
                ));
                if path.is_dir() {
                    for entry in std::fs::read_dir(path).unwrap() {
                        visit(root, &entry.unwrap().path(), entries);
                    }
                }
            }
            let mut entries = Vec::new();
            visit(root, root, &mut entries);
            entries.sort();
            entries
        }

        #[test]
        fn empty_constructor_view_and_empty_open_do_no_work_or_fixture_writes() {
            for config_state in 0..3 {
                let temp = tempfile::tempdir().unwrap();
                let config = temp.path().join("isolated-config");
                if config_state != 0 {
                    std::fs::create_dir(&config).unwrap();
                }
                if config_state == 2 {
                    std::fs::write(config.join("existing-config"), b"preserve\n").unwrap();
                }
                let before = tree(temp.path());

                let (mut app, command) = Preview::new(Flags {
                    root: PathBuf::new(),
                    native_check: false,
                });
                no_command(command);
                invalidated(&app);
                assert!(app.model.root.as_os_str().is_empty());
                assert!(app.root_input.is_empty());
                assert!(!app.root_edited);
                assert!(app.model.scan.is_none());
                assert!(app.model.receipt_worker.is_none());
                assert!(app.model.release_worker.is_none());
                assert!(!app.model.scan_requested);
                assert!(app.model.release_retry_at.is_none());
                assert!(app.subscription().recipes().is_empty());
                drop(app.view());

                no_command(app.dispatch(Message::CheckRelease));
                no_command(app.dispatch(Message::InspectReceipt));
                no_command(app.dispatch(Message::Rescan));
                assert!(app.model.error.is_some());
                assert!(app.model.scan.is_none());

                edit(&mut app, &temp.path().join("not-opened"));
                no_command(app.dispatch(Message::RootInputChanged(String::new())));
                invalidated(&app);
                no_command(app.dispatch(Message::OpenRoot));
                assert!(app.model.error.is_some());
                assert!(app.model.inventory.is_none());
                assert!(app.model.scan.is_none());
                assert!(!app.model.scan_requested);
                assert!(app.model.start_scan().is_none());
                assert_no_release_work(&app.model);
                assert_eq!(tree(temp.path()), before);
            }
        }

        #[test]
        fn explicit_open_discovers_all_modern_products_and_rejects_bad_roots() {
            let (temp, _) = receipt_model();
            for folder in ["_classic_era_", "_anniversary_"] {
                std::fs::create_dir_all(temp.path().join(folder).join("Interface").join("AddOns"))
                    .unwrap();
            }
            std::fs::write(
                temp.path().join(".build.info"),
                concat!(
                    "Active!DEC:1|Product!STRING:0|Version!STRING:0\n",
                    "1|wow|12.1.5.70575\n",
                    "1|wow_classic|5.5.4.70579\n",
                    "1|wow_classic_era|1.15.7.60000\n",
                    "1|wow_anniversary|2.5.5.60000\n",
                ),
            )
            .unwrap();
            let before = tree(temp.path());
            let (mut app, command) = Preview::new(Flags {
                root: PathBuf::new(),
                native_check: false,
            });
            no_command(command);

            edit(&mut app, temp.path());
            assert!(app.model.root.as_os_str().is_empty());
            assert!(app.model.scan.is_none());
            assert_no_release_work(&app.model);
            let command = app.dispatch(Message::OpenRoot);
            finish_local(&mut app, command);

            let report = app.model.inventory.as_ref().unwrap();
            assert!(!app.model.needs_review());
            assert_eq!(report.installations.len(), 4);
            for product in [
                Product::Retail,
                Product::ClassicProgression,
                Product::ClassicEra,
                Product::Anniversary,
            ] {
                assert!(report.installations.iter().any(|i| i.product == product));
            }
            assert_eq!(app.choices.len(), 4);
            assert!(app.model.selected.is_none());
            no_command(app.dispatch(Message::Select(Product::Anniversary)));
            assert_eq!(
                app.model.installation().unwrap().product,
                Product::Anniversary
            );
            assert_no_release_work(&app.model);
            assert_eq!(tree(temp.path()), before);

            let missing = temp.path().join("missing-root");
            let command = open(&mut app, &missing);
            finish_local(&mut app, command);
            assert!(app.model.error.is_some());
            assert!(app.model.inventory.is_none());
            assert!(app.model.selected.is_none());
            assert!(app.choices.is_empty());
            assert!(!missing.exists());
            assert_no_release_work(&app.model);
            assert_eq!(tree(temp.path()), before);

            std::fs::write(
                temp.path().join(".build.info"),
                concat!(
                    "Active!DEC:1|Product!STRING:0|Version!STRING:0\n",
                    "1|wow_unknown|13.0.0.1\n",
                ),
            )
            .unwrap();
            let before = tree(temp.path());
            let command = open(&mut app, temp.path());
            finish_local(&mut app, command);
            assert!(app.model.needs_review());
            let report = app.model.inventory.as_ref().unwrap();
            assert!(report.installations.is_empty());
            assert!(report
                .issues
                .iter()
                .any(|issue| issue.contains("Unknown product/version family")));
            assert!(app.model.selected.is_none());
            assert!(app.choices.is_empty());
            drop(app.view());
            assert_no_release_work(&app.model);
            assert_eq!(tree(temp.path()), before);
        }

        #[test]
        fn draft_edits_immediately_clear_success_and_failure_displays() {
            for failure in [false, true] {
                let (temp, model) = release_fixture();
                let mut app = Preview::from_model(model, false);
                let receipt = app.model.begin_receipt().unwrap();
                let receipt_result = if failure {
                    Err("old receipt failure".into())
                } else {
                    Ok(matching_receipt(&receipt))
                };
                no_command(app.dispatch(Message::ReceiptDone(receipt, receipt_result)));
                let release = click_release(&mut app);
                let release_result = if failure {
                    Err("old release failure".into())
                } else {
                    Ok("old version".into())
                };
                no_command(app.dispatch(Message::ReleaseDone(release, release_result)));
                assert!(app.model.release_result.is_some());
                assert!(app.model.receipt.is_some() || app.model.receipt_error.is_some());

                let original = app.model.root.clone();
                let generation = app.model.generation.clone();
                let repository = app.model.repository.clone();
                let receipt_id = app.model.receipt_id.clone();
                edit(&mut app, &temp.path().join("new-root"));
                invalidated(&app);
                assert_eq!(app.model.root, original);
                assert!(!Arc::ptr_eq(&generation, &app.model.generation));
                assert_eq!(app.model.repository, repository);
                assert_eq!(app.model.receipt_id, receipt_id);
                assert!(app.root_edited);
                assert!(app.model.scan.is_none());
                assert!(!app.model.scan_requested);
                no_command(app.dispatch(Message::Rescan));
                no_command(app.dispatch(Message::Select(Product::Retail)));
                no_command(app.dispatch(Message::CheckRelease));
                assert_no_release_work(&app.model);
                drop(app.view());
            }
        }

        #[test]
        fn busy_scan_coalesces_latest_open_and_rejects_foreign_and_duplicate_results() {
            let (a, _) = receipt_model();
            let (b, _) = receipt_model();
            let (c, _) = receipt_model();
            let (mut app, command) = Preview::new(Flags {
                root: a.path().into(),
                native_check: false,
            });
            let old_message = local_message(command);
            let old = match &old_message {
                Message::InventoryDone(request, _) => request.clone(),
                other => panic!("Expected scan: {:?}", other),
            };

            no_command(open(&mut app, b.path()));
            no_command(open(&mut app, c.path()));
            invalidated(&app);
            assert_eq!(app.model.root, c.path());
            assert!(app.model.scan_requested);
            owns(app.model.scan.as_ref(), &old.token);

            let mut foreign = old.clone();
            foreign.token = Arc::new(());
            no_command(app.dispatch(Message::InventoryDone(foreign, Err("foreign scan".into()))));
            owns(app.model.scan.as_ref(), &old.token);
            assert!(app.model.scan_requested);

            let latest_command = app.dispatch(old_message.clone());
            invalidated(&app);
            assert!(!app.model.scan_requested);
            let latest_token = app.model.scan.as_ref().unwrap().clone();
            assert!(!Arc::ptr_eq(&latest_token, &old.token));

            no_command(app.dispatch(old_message));
            owns(app.model.scan.as_ref(), &latest_token);
            let latest_message = local_message(latest_command);
            match &latest_message {
                Message::InventoryDone(request, _) => {
                    assert_eq!(request.root, c.path());
                    owns(app.model.scan.as_ref(), &request.token);
                    let mut foreign = request.clone();
                    foreign.token = Arc::new(());
                    no_command(app.dispatch(Message::InventoryDone(
                        foreign,
                        Err("foreign latest scan".into()),
                    )));
                }
                other => panic!("Expected latest scan: {:?}", other),
            }
            owns(app.model.scan.as_ref(), &latest_token);
            no_command(app.dispatch(latest_message.clone()));
            let published = app.model.inventory.as_ref().unwrap().clone();
            assert_eq!(published.root, c.path().canonicalize().unwrap());
            assert_eq!(app.choices.len(), 2);
            assert!(app.model.selected.is_none());
            no_command(app.dispatch(latest_message));
            assert!(Arc::ptr_eq(
                app.model.inventory.as_ref().unwrap(),
                &published
            ));
            assert_no_release_work(&app.model);
        }

        #[test]
        fn draft_away_back_aba_rejects_scan_even_without_a_queued_open() {
            let (a, _) = receipt_model();
            let (b, _) = receipt_model();
            let (mut app, command) = Preview::new(Flags {
                root: a.path().into(),
                native_check: false,
            });
            let message = local_message(command);
            let old_generation = app.model.generation.clone();

            edit(&mut app, b.path());
            edit(&mut app, a.path());
            assert_eq!(app.model.root, a.path());
            assert!(!app.model.scan_requested);
            assert!(!Arc::ptr_eq(&old_generation, &app.model.generation));
            no_command(app.dispatch(message.clone()));
            invalidated(&app);
            assert!(app.model.scan.is_none());
            no_command(app.dispatch(message));
            invalidated(&app);

            let command = app.dispatch(Message::OpenRoot);
            finish_local(&mut app, command);
            assert_eq!(
                app.model.inventory.as_ref().unwrap().root,
                a.path().canonicalize().unwrap()
            );
            assert!(app.model.selected.is_none());
            assert_no_release_work(&app.model);
        }

        #[test]
        fn receipt_holds_shared_slot_until_matching_completion_then_latest_root_scans() {
            let (_a, model) = release_fixture();
            let (b, _) = receipt_model();
            let (c, _) = receipt_model();
            let mut app = Preview::from_model(model, false);
            let receipt_command = app.dispatch(Message::InspectReceipt);
            let old = app.model.pending_receipt.as_ref().unwrap().clone();

            no_command(open(&mut app, b.path()));
            no_command(open(&mut app, c.path()));
            invalidated(&app);
            owns(app.model.receipt_worker.as_ref(), &old.token);
            assert!(app.model.scan.is_none());
            assert!(app.model.scan_requested);
            no_command(app.dispatch(Message::InspectReceipt));
            no_command(app.dispatch(Message::CheckRelease));

            let mut foreign = old.clone();
            foreign.token = Arc::new(());
            no_command(app.dispatch(Message::ReceiptDone(foreign, Err("foreign receipt".into()))));
            owns(app.model.receipt_worker.as_ref(), &old.token);
            assert!(app.model.scan.is_none());

            let message = local_message(receipt_command);
            let scan = app.dispatch(message.clone());
            assert!(app.model.receipt_worker.is_none());
            assert!(app.model.receipt.is_none());
            assert!(app.model.receipt_error.is_none());
            let scan_token = app.model.scan.as_ref().unwrap().clone();
            no_command(app.dispatch(message));
            owns(app.model.scan.as_ref(), &scan_token);
            finish_local(&mut app, scan);
            assert_eq!(
                app.model.inventory.as_ref().unwrap().root,
                c.path().canonicalize().unwrap()
            );

            no_command(app.dispatch(Message::Select(Product::Retail)));
            let current_command = app.dispatch(Message::InspectReceipt);
            let current = app.model.pending_receipt.as_ref().unwrap().clone();
            no_command(app.dispatch(Message::ReceiptDone(
                old.clone(),
                Ok(matching_receipt(&old)),
            )));
            owns(app.model.receipt_worker.as_ref(), &current.token);
            assert!(app
                .model
                .pending_receipt
                .as_ref()
                .is_some_and(|pending| Arc::ptr_eq(&pending.token, &current.token)));
            finish_local(&mut app, current_command);
            assert!(app.model.receipt.is_none());
            assert!(app.model.receipt_error.is_some());
            assert_no_release_work(&app.model);
        }

        #[test]
        fn provider_reservation_survives_root_aba_and_only_manual_click_readmits() {
            let (a, model) = release_fixture();
            let (b, _) = receipt_model();
            let mut app = Preview::from_model(model, false);
            let old = click_release(&mut app);

            for root in [b.path(), a.path()] {
                let command = open(&mut app, root);
                owns(app.model.release_worker.as_ref(), &old.token);
                assert!(app.model.pending_release.is_none());
                finish_local(&mut app, command);
                no_command(app.dispatch(Message::Select(Product::Retail)));
                no_command(app.dispatch(Message::CheckRelease));
                owns(app.model.release_worker.as_ref(), &old.token);
            }
            assert_eq!(app.model.root, old.root);
            assert_eq!(app.model.selected, Some(old.product));
            assert!(!Arc::ptr_eq(
                app.model.inventory.as_ref().unwrap(),
                &old.inventory
            ));

            let mut foreign = old.clone();
            foreign.token = Arc::new(());
            no_command(app.dispatch(Message::ReleaseDone(foreign, Ok("foreign".into()))));
            owns(app.model.release_worker.as_ref(), &old.token);
            no_command(app.dispatch(Message::ReleaseDone(old.clone(), Ok("stale ABA".into()))));
            assert_no_release_work(&app.model);

            drop(app.view());
            no_command(app.dispatch(Message::RepositoryChanged(old.repository.clone())));
            no_command(app.dispatch(Message::ReceiptIdChanged("b".repeat(64))));
            assert_no_release_work(&app.model);
            let current = click_release(&mut app);
            no_command(app.dispatch(Message::ReleaseRateLimited(old, RetryDeadline::Blocked)));
            owns(app.model.release_worker.as_ref(), &current.token);
            assert!(app.model.release_retry_at.is_none());
            no_command(app.dispatch(Message::CheckRelease));
            no_command(app.dispatch(Message::NativeCheckDone));
            owns(app.model.release_worker.as_ref(), &current.token);
            no_command(app.dispatch(Message::ReleaseDone(current.clone(), Ok("current".into()))));
            no_command(app.dispatch(Message::ReleaseDone(current, Err("duplicate".into()))));
            assert_eq!(
                app.model
                    .release_result
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .version,
                "current"
            );
        }

        #[test]
        fn stale_rate_completion_preserves_finite_or_blocked_wait_across_root_flow() {
            let finite = RetryDeadline::from_wait(Instant::now(), Duration::from_secs(172_800));
            for deadline in [finite, RetryDeadline::Blocked] {
                let (a, model) = release_fixture();
                let (b, _) = receipt_model();
                let (c, _) = receipt_model();
                let roots = [a.path(), b.path(), c.path()];
                let before: Vec<_> = roots.iter().map(|root| tree(root)).collect();
                let mut app = Preview::from_model(model, false);
                let old = click_release(&mut app);
                let scan_b = open(&mut app, b.path());
                owns(app.model.release_worker.as_ref(), &old.token);
                assert!(app.model.pending_release.is_none());

                let mut foreign = old.clone();
                foreign.token = Arc::new(());
                no_command(
                    app.dispatch(Message::ReleaseRateLimited(foreign, RetryDeadline::Blocked)),
                );
                owns(app.model.release_worker.as_ref(), &old.token);
                assert!(app.model.release_retry_at.is_none());

                no_command(app.dispatch(Message::ReleaseRateLimited(old.clone(), deadline)));
                assert!(app.model.release_worker.is_none());
                assert!(app.model.release_result.is_none());
                assert_eq!(app.model.release_retry_at, Some(deadline));

                no_command(open(&mut app, c.path()));
                assert_eq!(app.model.release_retry_at, Some(deadline));
                no_command(app.dispatch(Message::ReleaseRateLimited(
                    old.clone(),
                    RetryDeadline::Blocked,
                )));
                assert_eq!(app.model.release_retry_at, Some(deadline));

                let scan_c = app.dispatch(local_message(scan_b));
                assert!(app.model.inventory.is_none());
                assert_eq!(app.model.root, c.path());
                finish_local(&mut app, scan_c);
                no_command(app.dispatch(Message::Select(Product::Retail)));
                no_command(app.dispatch(Message::CheckRelease));
                assert_no_release_work(&app.model);

                let command = open(&mut app, a.path());
                assert_eq!(app.model.release_retry_at, Some(deadline));
                finish_local(&mut app, command);
                no_command(app.dispatch(Message::Select(Product::Retail)));
                no_command(app.dispatch(Message::CheckRelease));
                no_command(app.dispatch(Message::ReleaseDone(old, Ok("late success".into()))));
                assert_no_release_work(&app.model);

                no_command(app.dispatch(Message::RootInputChanged(String::new())));
                assert_eq!(app.model.release_retry_at, Some(deadline));
                no_command(app.dispatch(Message::OpenRoot));
                assert!(app.model.error.is_some());
                assert!(app.model.scan.is_none());
                assert_eq!(app.model.release_retry_at, Some(deadline));
                assert_no_release_work(&app.model);
                for (root, bytes) in roots.iter().zip(&before) {
                    assert_eq!(&tree(root), bytes);
                }
            }
        }
    }
}
