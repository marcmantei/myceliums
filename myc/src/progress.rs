//! Progress reporting for `myc analyze` on stderr.

/// Progress bar reporter using `indicatif`. Draws on stderr so stdout
/// stays clean for JSON hook output.
pub(crate) struct IndicatifReporter {
    bar: std::sync::Mutex<Option<indicatif::ProgressBar>>,
}

impl IndicatifReporter {
    pub(crate) fn new() -> Self {
        Self {
            bar: std::sync::Mutex::new(None),
        }
    }

    fn set_bar(&self, bar: indicatif::ProgressBar) {
        let mut guard = self.bar.lock().unwrap();
        if let Some(old) = guard.take() {
            old.finish_and_clear();
        }
        *guard = Some(bar);
    }

    fn finish(&self) {
        let mut guard = self.bar.lock().unwrap();
        if let Some(bar) = guard.take() {
            bar.finish_and_clear();
        }
    }
}

impl myceliums_core::progress::ProgressReporter for IndicatifReporter {
    fn report(&self, phase: myceliums_core::progress::AnalysisPhase) {
        use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
        use myceliums_core::progress::AnalysisPhase;

        match phase {
            AnalysisPhase::Discovering => {
                let bar = ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Discovering files...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                self.set_bar(bar);
            }
            AnalysisPhase::Parsing { current, total } => {
                let mut guard = self.bar.lock().unwrap();
                if let Some(ref bar) = *guard {
                    if bar.length() == Some(total as u64) {
                        // Update existing bar
                        bar.set_position(current as u64);
                        return;
                    }
                }
                // Create new bar for parsing phase
                if let Some(old) = guard.take() {
                    old.finish_and_clear();
                }
                let bar = ProgressBar::new(total as u64)
                    .with_style(
                        ProgressStyle::with_template(
                            "{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} files ({eta}) {msg}",
                        )
                        .unwrap()
                        .progress_chars("=>-")
                        .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Parsing...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                bar.set_position(current as u64);
                *guard = Some(bar);
            }
            AnalysisPhase::BuildingRelationships => {
                let bar = ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Building relationships...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                self.set_bar(bar);
            }
            AnalysisPhase::DetectingCommunities => {
                let bar = ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Detecting communities...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                self.set_bar(bar);
            }
            AnalysisPhase::TracingProcesses => {
                let bar = ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Tracing processes...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                self.set_bar(bar);
            }
            AnalysisPhase::GeneratingEmbeddings { current, total } => {
                let mut guard = self.bar.lock().unwrap();
                if let Some(ref bar) = *guard {
                    if bar.length() == Some(total as u64) {
                        bar.set_position(current as u64);
                        return;
                    }
                }
                if let Some(old) = guard.take() {
                    old.finish_and_clear();
                }
                let bar = ProgressBar::new(total as u64)
                    .with_style(
                        ProgressStyle::with_template(
                            "{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} embeddings ({eta}) {msg}",
                        )
                        .unwrap()
                        .progress_chars("=>-")
                        .tick_chars("\u{25d0}\u{25d3}\u{25d1}\u{25d2}\u{2714}"),
                    )
                    .with_message("Generating embeddings (a first index takes the longest)...");
                bar.set_draw_target(ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                bar.set_position(current as u64);
                *guard = Some(bar);
            }
            AnalysisPhase::Complete { symbols, files } => {
                self.finish();
                eprintln!(
                    "\u{2714} Analysis complete: {} symbols, {} files",
                    symbols, files
                );
            }
        }
    }
}
