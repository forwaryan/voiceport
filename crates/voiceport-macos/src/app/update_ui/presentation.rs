use super::*;

/// Native UI snapshot. It owns only display data and carries no update actions.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct UpdateViewSnapshot {
    pub phase: UpdateViewPhase,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub latest_version_stale: bool,
    pub transfer: Option<UpdateTransfer>,
    pub reason: Option<&'static str>,
    pub refresh_error: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum UpdateViewPhase {
    Idle,
    Disabled,
    Checking,
    Available,
    Current,
    LocalNewer,
    Incompatible,
    CheckFailed,
    Connecting,
    Downloading,
    Verifying,
    DownloadFailed,
    Ready,
    Installing,
    Quitting,
}

impl UpdateViewPhase {
    pub fn can_check(self) -> bool {
        !matches!(
            self,
            Self::Disabled
                | Self::Checking
                | Self::Connecting
                | Self::Downloading
                | Self::Verifying
                | Self::Installing
                | Self::Quitting
        )
    }

    pub fn can_download(self) -> bool {
        matches!(self, Self::Available | Self::DownloadFailed)
    }

    pub fn can_install(self) -> bool {
        self == Self::Ready
    }

    pub fn can_view_release(self) -> bool {
        matches!(self, Self::CheckFailed | Self::Incompatible)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct UpdateTransfer {
    pub received_bytes: u64,
    pub percent: Option<u64>,
    pub bytes_per_second: Option<f64>,
    pub stalled: bool,
}

impl UpdateTransfer {
    pub(super) fn read(progress: DownloadSnapshot, receiving: bool, now: Instant) -> Self {
        Self {
            received_bytes: progress.received,
            percent: progress.percent(),
            bytes_per_second: if receiving { progress.rate(now) } else { None },
            stalled: receiving && progress.stalled(now),
        }
    }
}

impl UpdateUi {
    pub(super) fn latest_version_stale(&self) -> bool {
        self.last_check.latest.is_some()
            && (matches!(self.state, State::Checking { .. })
                || self.last_check.state == Some("failed"))
    }

    pub(super) fn view_snapshot(&self, quitting: bool, now: Instant) -> UpdateViewSnapshot {
        use UpdateViewPhase::*;
        let mut snapshot = UpdateViewSnapshot {
            phase: Idle,
            current_version: self.running_version(),
            latest_version: self.last_check.latest.clone(),
            latest_version_stale: self.latest_version_stale(),
            transfer: None,
            reason: None,
            refresh_error: None,
        };
        snapshot.phase = if quitting {
            Quitting
        } else if test_build::active() {
            Disabled
        } else {
            match &self.state {
                State::Checking { .. } => Checking,
                State::Offered(offer) => {
                    snapshot.refresh_error = offer.refresh_error;
                    if let Some(error) = &offer.download_error {
                        snapshot.reason = Some(error.reason);
                        snapshot.transfer = Some(UpdateTransfer::read(error.progress, false, now));
                        DownloadFailed
                    } else {
                        Available
                    }
                }
                State::Downloading {
                    progress, offer, ..
                } => {
                    let progress = progress.snapshot();
                    let receiving = !progress.verifying;
                    snapshot.transfer = Some(UpdateTransfer::read(progress, receiving, now));
                    snapshot.refresh_error = offer.refresh_error;
                    if progress.verifying {
                        Verifying
                    } else if progress.received == 0 && progress.percent().is_none() {
                        Connecting
                    } else {
                        Downloading
                    }
                }
                State::Ready(_, refresh_error, progress) => {
                    snapshot.transfer = Some(UpdateTransfer::read(*progress, false, now));
                    snapshot.refresh_error = *refresh_error;
                    Ready
                }
                State::Installing => Installing,
                State::Idle | State::Notice(_) => {
                    snapshot.reason = self.last_check.reason;
                    match self.last_check.state {
                        Some("up_to_date") => Current,
                        Some("local_newer") => LocalNewer,
                        Some("incompatible") => Incompatible,
                        Some("failed") => CheckFailed,
                        _ => Idle,
                    }
                }
            }
        };
        snapshot
    }
}

impl Delegate {
    pub(in crate::app) fn update_view_snapshot(&self) -> UpdateViewSnapshot {
        self.ivars()
            .update
            .borrow()
            .view_snapshot(self.ivars().quitting.get(), Instant::now())
    }
}
