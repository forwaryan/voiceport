use crate::local_models::{
    Catalog, OrcaDiscovery, SessionSource, unavailable_reason, validate_runtime,
};
use crate::model_library::{Installation, MODELS, Paths};
use std::time::{Duration, Instant};
use std::{borrow::Cow, cell::OnceCell, path::Path};

// The AppKit timer keeps polling jobs every 100 ms. Only the visible local page
// pays for disk classification, with explicit invalidation at lifecycle events.
pub(super) const DISK_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RefreshReason {
    Tick,
    Open,
    JobFinished,
    Action,
}

#[derive(Default)]
pub(super) struct LocalRefresh {
    pub disk: Option<LocalDisk>,
    next_probe: Option<Instant>,
}

impl LocalRefresh {
    pub fn invalidate(&mut self) {
        self.next_probe = None;
    }

    pub fn sample(
        &mut self,
        now: Instant,
        visible: bool,
        reason: RefreshReason,
        probe: impl FnOnce() -> LocalDisk,
    ) -> bool {
        if !visible {
            self.invalidate();
            self.disk = None;
            return false;
        }
        if reason == RefreshReason::Tick && self.next_probe.is_some_and(|deadline| now < deadline) {
            return false;
        }
        self.disk = Some(probe());
        self.next_probe = Some(now + DISK_REFRESH_INTERVAL);
        true
    }
}

pub(super) struct LocalDisk {
    pub engine_ready: bool,
    pub rows: Vec<ModelDisk>,
}

impl LocalDisk {
    pub fn probe(paths: &Paths, orca_root: Option<&Path>) -> Self {
        #[cfg(test)]
        tests::count(|counts| counts.engine += 1);
        let engine_ready = validate_runtime(&paths.runtime(), std::env::consts::ARCH).is_ok();
        let orca = OnceCell::new();
        let rows = MODELS
            .iter()
            .map(|definition| {
                #[cfg(test)]
                tests::count(|counts| counts.installation += 1);
                let (installation, owned) = paths.inspect_installation(definition);
                // An incompatible owned copy still shadows the Orca copy.
                let source = owned.unwrap_or_else(|| {
                    orca.get_or_init(|| {
                        #[cfg(test)]
                        if orca_root.is_some() {
                            tests::count(|counts| counts.orca_root += 1);
                        }
                        OrcaDiscovery::system(orca_root, Some(paths))
                    })
                    .discover(definition.directory, definition.kind)
                });
                ModelDisk {
                    installation,
                    source: source
                        .map(SessionSource::Local)
                        .map_err(|error| unavailable_reason(definition.id, &error)),
                }
            })
            .collect();
        Self { engine_ready, rows }
    }

    pub fn catalog_differs(&self, catalog: &Catalog, selected: &str) -> bool {
        MODELS.iter().zip(&self.rows).any(|(definition, row)| {
            // The catalog omits unavailable Orca copies unless their source is selected.
            let expected = (row.source.is_ok()
                || row.installation != Installation::Missing
                || definition.id == selected)
                .then_some(&row.source);
            catalog_source(catalog, definition.id) != expected
        })
    }
}

fn catalog_source<'a>(catalog: &'a Catalog, id: &str) -> Option<&'a Result<SessionSource, String>> {
    catalog
        .entries
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| &entry.source)
}

pub(super) struct ModelDisk {
    pub installation: Installation,
    source: Result<SessionSource, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrimaryAction {
    Use,
    Download,
    Reinstall,
    AwaitCatalog,
}

#[derive(PartialEq, Eq)]
pub(super) struct ModelView {
    pub installation: Installation,
    pub primary: PrimaryAction,
    pub reason: Cow<'static, str>,
}

impl ModelDisk {
    pub fn view(&self, catalog: &Catalog, id: &str) -> ModelView {
        let entry = catalog_source(catalog, id);
        let (primary, reason) = match &self.source {
            Ok(_) if entry == Some(&self.source) => (PrimaryAction::Use, "".into()),
            Ok(_) => (
                PrimaryAction::AwaitCatalog,
                "正在将磁盘上的模型同步到来源列表。".into(),
            ),
            Err(_) if self.installation == Installation::Missing && entry.is_none() => (
                PrimaryAction::Download,
                "所选转写来源不可用，请在设置中重新选择或刷新模型。".into(),
            ),
            Err(error) if self.installation == Installation::Missing => {
                (PrimaryAction::Download, error.clone().into())
            }
            Err(error) => (PrimaryAction::Reinstall, error.clone().into()),
        };
        ModelView {
            installation: self.installation,
            primary,
            reason,
        }
    }
}

impl ModelView {
    pub fn recommendation_status(&self, selected: bool) -> &'static str {
        match self.primary {
            PrimaryAction::Use if selected => "使用中",
            PrimaryAction::Use => "已安装",
            PrimaryAction::AwaitCatalog => "已安装 · 正在刷新",
            // The same repair action covers missing engines and damaged weights.
            PrimaryAction::Reinstall => "当前不可用",
            PrimaryAction::Download => "未安装",
        }
    }

    pub fn owned(&self) -> bool {
        self.installation != Installation::Missing
    }

    pub fn installed(&self) -> bool {
        self.owned()
            || matches!(
                self.primary,
                PrimaryAction::Use | PrimaryAction::AwaitCatalog
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
    pub(super) struct Counts {
        pub engine: usize,
        pub installation: usize,
        pub orca_root: usize,
    }

    thread_local! { static COUNTS: Cell<Counts> = Cell::new(Counts::default()); }

    pub(super) fn count(update: impl FnOnce(&mut Counts)) {
        COUNTS.with(|cell| {
            let mut counts = cell.get();
            update(&mut counts);
            cell.set(counts);
        });
    }

    fn fixture() -> (tempfile::TempDir, Paths) {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: temp.path().join("voiceport"),
        };
        std::fs::create_dir(&paths.root).unwrap();
        (temp, paths)
    }

    fn write_engine(paths: &Paths) {
        let runtime = paths.runtime();
        std::fs::create_dir(&runtime).unwrap();
        std::fs::write(runtime.join("package.json"), br#"{"version":"2.0.1"}"#).unwrap();
        for filename in ["libsherpa-onnx-c-api.dylib", "libonnxruntime.dylib"] {
            std::fs::write(
                runtime.join(filename),
                [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1],
            )
            .unwrap();
        }
    }

    fn write_model(root: &Path, definition: &crate::model_library::Definition) {
        let directory = root.join(definition.directory);
        std::fs::create_dir_all(&directory).unwrap();
        for asset in definition.assets {
            std::fs::File::create(directory.join(asset.name))
                .unwrap()
                .set_len(asset.bytes)
                .unwrap();
        }
    }

    fn sample(
        refresh: &mut LocalRefresh,
        now: Instant,
        visible: bool,
        reason: RefreshReason,
        paths: &Paths,
        orca: &Path,
    ) -> bool {
        refresh.sample(now, visible, reason, || LocalDisk::probe(paths, Some(orca)))
    }

    #[test]
    fn recommendation_does_not_prescribe_reinstall_when_only_engine_is_missing() {
        use crate::local_models::{CHATGPT, NEMOTRON, SENSEVOICE};
        use crate::model_library::recommend;
        use voiceport_control::ModelPreview;

        if std::env::consts::ARCH != "aarch64" {
            return;
        }
        let (_temp, paths) = fixture();
        assert_eq!(recommend("yue", ModelPreview::Any).unwrap().id, SENSEVOICE);
        // Nemotron requires the current engine at the explicit fixture path.
        let d = recommend("ja", ModelPreview::Live).unwrap();
        assert_eq!(d.id, NEMOTRON);
        let index = MODELS.iter().position(|model| model.id == d.id).unwrap();
        let probe =
            |catalog: &Catalog| LocalDisk::probe(&paths, None).rows[index].view(catalog, d.id);
        let pending = Catalog::pending(CHATGPT);
        write_model(&paths.root, d);

        let missing = probe(&pending);
        assert_eq!(missing.primary, PrimaryAction::Reinstall);
        assert_eq!(missing.recommendation_status(false), "当前不可用");
        assert_eq!(missing.recommendation_status(true), "当前不可用");

        write_engine(&paths);
        assert_eq!(
            probe(&pending).recommendation_status(false),
            "已安装 · 正在刷新"
        );
        let catalog = Catalog::discover(
            &paths.root,
            &paths.runtime(),
            CHATGPT,
            std::env::consts::ARCH,
        );
        let available = probe(&catalog);
        assert_eq!(available.recommendation_status(false), "已安装");
        assert_eq!(available.recommendation_status(true), "使用中");

        std::fs::remove_file(paths.model(d).join(d.assets[0].name)).unwrap();
        let damaged = probe(&catalog);
        assert_eq!(damaged.primary, PrimaryAction::Reinstall);
        assert_eq!(damaged.recommendation_status(false), "当前不可用");
    }

    #[test]
    fn nominal_ticks_measure_real_local_probe_calls() {
        if std::env::consts::ARCH != "aarch64" {
            return;
        }
        let (temp, paths) = fixture();
        let orca = temp.path().join("orca");
        let definition = &MODELS[1];
        for (name, visible, installed) in [
            ("empty", true, false),
            ("installed", true, true),
            ("cloud", false, true),
        ] {
            if name == "installed" {
                write_engine(&paths);
                write_model(&paths.root, definition);
            }
            COUNTS.with(|cell| cell.set(Counts::default()));
            let nominal_start = Instant::now();
            let mut refresh = LocalRefresh::default();
            let wall_start = Instant::now();
            for tick in 0..100 {
                assert_eq!(
                    sample(
                        &mut refresh,
                        nominal_start + Duration::from_millis(tick * 100),
                        visible,
                        RefreshReason::Tick,
                        &paths,
                        &orca
                    ),
                    visible && tick % 20 == 0
                );
            }
            let elapsed = wall_start.elapsed();
            let [engine, installation, orca_root] = if visible { [5, 40, 5] } else { [0, 0, 0] };
            let expected = Counts {
                engine,
                installation,
                orca_root,
            };
            let actual = COUNTS.with(Cell::get);
            assert_eq!(actual, expected);
            if let Some(disk) = &refresh.disk {
                assert_eq!(disk.engine_ready, installed);
                assert_eq!(
                    disk.rows[1].installation,
                    if installed {
                        Installation::Legacy
                    } else {
                        Installation::Missing
                    }
                );
            }
            println!("{name}: {actual:?}; 100 nominal ticks (0..9900 ms) in {elapsed:?}");
        }
    }

    #[test]
    fn selected_diagnosis_and_actions_converge_with_no_home() {
        use crate::local_models::{CHATGPT, orca_model_root};
        use crate::plugins::{HTTP, Source, Store, default_path};
        use std::{fs, process::Command};

        const CHILD: &str = "VOICEPORT_MODEL_RECONCILIATION_CHILD";
        const CONFIGURED: &str = "fixture-http";
        if std::env::consts::ARCH != "aarch64" {
            return;
        }
        if std::env::var_os(CHILD).is_none() {
            let (temp, paths) = fixture();
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "app::model_manager::local_state::tests::selected_diagnosis_and_actions_converge_with_no_home",
                    "--exact",
                    "--nocapture",
                ])
                .env_remove("HOME")
                .env("VOICEPORT_MODELS_PATH", &paths.root)
                .env("VOICEPORT_SOURCES_PATH", temp.path().join("sources.json"))
                .env(CHILD, "1")
                .status()
                .unwrap();
            assert!(status.success(), "isolated subprocess exited {status}");
            return;
        }

        let paths = Paths::system().unwrap();
        let sources = default_path().unwrap();
        let orca_root = orca_model_root();
        assert!(orca_root.is_none());
        assert!(!sources.exists());
        let definition = &MODELS[1];
        let id = definition.id;
        let mut catalog = Catalog::system(id);
        let disk = || LocalDisk::probe(&paths, orca_root.as_deref());
        let initial = disk();
        assert!(!initial.catalog_differs(&catalog, id));
        let missing = catalog.snapshot(id).unwrap_err();
        assert!(missing.contains("HOME"));
        assert_eq!(
            initial.rows[1].view(&catalog, id).primary,
            PrimaryAction::Download
        );
        assert_eq!(catalog.snapshot(CHATGPT), Ok(SessionSource::ChatGpt));

        let mut refresh = LocalRefresh::default();
        let mut now = Instant::now();
        let mut reconcile = |catalog: &mut Catalog, installation, before, after| {
            assert!(refresh.sample(now, true, RefreshReason::Tick, disk));
            now += DISK_REFRESH_INTERVAL;
            let observed = refresh.disk.as_ref().unwrap();
            assert_eq!(observed.rows[1].installation, installation);
            assert_eq!(observed.rows[1].view(catalog, id).primary, before);
            assert!(observed.catalog_differs(catalog, id));
            *catalog = Catalog::system(id);
            let row = observed.rows[1].view(catalog, id);
            assert_eq!(row.primary, after);
            if let Err(error) = catalog.snapshot(id) {
                assert_eq!(row.reason.as_ref(), error);
            }
            assert!(refresh.sample(now, true, RefreshReason::Tick, disk));
            now += DISK_REFRESH_INTERVAL;
            assert!(!refresh.disk.as_ref().unwrap().catalog_differs(catalog, id));
        };
        let directory = paths.model(definition);
        fs::create_dir(&directory).unwrap();
        for asset in definition.assets {
            fs::write(directory.join(asset.name), b"truncated").unwrap();
        }
        reconcile(
            &mut catalog,
            Installation::Incompatible,
            PrimaryAction::Reinstall,
            PrimaryAction::Reinstall,
        );
        assert!(
            catalog
                .snapshot(id)
                .unwrap_err()
                .contains("模型副本格式或版本不兼容")
        );

        write_model(&paths.root, definition);
        write_engine(&paths);
        reconcile(
            &mut catalog,
            Installation::Legacy,
            PrimaryAction::AwaitCatalog,
            PrimaryAction::Use,
        );
        assert!(
            matches!(catalog.snapshot(id), Ok(SessionSource::Local(model)) if model.directory == directory)
        );

        fs::remove_file(directory.join(definition.assets[0].name)).unwrap();
        reconcile(
            &mut catalog,
            Installation::Incompatible,
            PrimaryAction::Reinstall,
            PrimaryAction::Reinstall,
        );
        let cloud_with_incompatible_copy = Catalog::system(CHATGPT);
        assert!(!disk().catalog_differs(&cloud_with_incompatible_copy, CHATGPT));
        write_model(&paths.root, definition);
        reconcile(
            &mut catalog,
            Installation::Legacy,
            PrimaryAction::AwaitCatalog,
            PrimaryAction::Use,
        );

        fs::remove_dir_all(&directory).unwrap();
        reconcile(
            &mut catalog,
            Installation::Missing,
            PrimaryAction::Download,
            PrimaryAction::Download,
        );
        assert_eq!(catalog.snapshot(id).unwrap_err(), missing);
        assert!(disk().catalog_differs(&cloud_with_incompatible_copy, CHATGPT));
        assert!(!disk().catalog_differs(&Catalog::system(CHATGPT), CHATGPT));

        let mut store = Store::default();
        store.sources.push(Source {
            id: CONFIGURED.into(),
            name: "Fixture".into(),
            plugin_id: HTTP.into(),
            plugin_version: "1".into(),
            enabled: true,
            config: [
                ("endpoint".into(), "http://127.0.0.1:8/transcribe".into()),
                ("model".into(), "local".into()),
            ]
            .into(),
            credential_refs: Default::default(),
        });
        store.save(&sources).unwrap();
        assert!(matches!(
            Catalog::system(CONFIGURED).snapshot(CONFIGURED),
            Ok(SessionSource::Plugin(_))
        ));
        fs::write(sources, b"{").unwrap();
        let catalog = Catalog::system(CONFIGURED);
        assert!(catalog.snapshot(CONFIGURED).is_err());
        assert!(
            catalog
                .snapshot("configuration-error")
                .unwrap_err()
                .contains("JSON")
        );
        assert!(!disk().catalog_differs(&catalog, CONFIGURED));
    }

    #[test]
    fn lifecycle_refresh_sees_external_changes_and_reconciles_orca_precedence() {
        if std::env::consts::ARCH != "aarch64" {
            return;
        }
        let (temp, paths) = fixture();
        let orca = temp.path().join("orca");
        let definition = &MODELS[1];
        let mut refresh = LocalRefresh::default();
        let now = Instant::now();
        let pending = Catalog::pending(crate::local_models::CHATGPT);
        let read = |refresh: &LocalRefresh, catalog: &Catalog| {
            refresh.disk.as_ref().unwrap().rows[1].view(catalog, definition.id)
        };
        assert!(sample(
            &mut refresh,
            now,
            true,
            RefreshReason::Open,
            &paths,
            &orca
        ));
        assert_eq!(read(&refresh, &pending).primary, PrimaryAction::Download);

        write_engine(&paths);
        write_model(&orca, definition);
        let orca_catalog = Catalog::discover(&orca, &paths.runtime(), "", std::env::consts::ARCH);
        assert!(!sample(
            &mut refresh,
            now + Duration::from_millis(100),
            true,
            RefreshReason::Tick,
            &paths,
            &orca
        ));
        assert_eq!(
            read(&refresh, &orca_catalog).primary,
            PrimaryAction::Download
        );
        assert!(sample(
            &mut refresh,
            now + DISK_REFRESH_INTERVAL,
            true,
            RefreshReason::Tick,
            &paths,
            &orca
        ));
        assert!(
            refresh
                .disk
                .as_ref()
                .unwrap()
                .catalog_differs(&pending, crate::local_models::CHATGPT)
        );
        assert_eq!(
            read(&refresh, &pending).primary,
            PrimaryAction::AwaitCatalog
        );
        assert_eq!(read(&refresh, &orca_catalog).primary, PrimaryAction::Use);
        assert!(!read(&refresh, &orca_catalog).owned());
        assert!(read(&refresh, &orca_catalog).installed());
        assert!(
            !refresh
                .disk
                .as_ref()
                .unwrap()
                .catalog_differs(&orca_catalog, crate::local_models::CHATGPT)
        );

        // The already-rendered Use action becomes Reinstall if an incompatible
        // owned copy shadows the Orca one immediately before the button click.
        std::fs::create_dir(paths.model(definition)).unwrap();
        let at = now + DISK_REFRESH_INTERVAL + Duration::from_millis(100);
        assert!(sample(
            &mut refresh,
            at,
            true,
            RefreshReason::Action,
            &paths,
            &orca
        ));
        let shadowed = read(&refresh, &orca_catalog);
        assert_eq!(shadowed.primary, PrimaryAction::Reinstall);
        assert!(shadowed.owned() && shadowed.installed());
        assert!(
            refresh
                .disk
                .as_ref()
                .unwrap()
                .catalog_differs(&orca_catalog, crate::local_models::CHATGPT)
        );

        assert!(!sample(
            &mut refresh,
            at,
            false,
            RefreshReason::JobFinished,
            &paths,
            &orca
        ));
        assert!(refresh.disk.is_none());
        std::fs::remove_dir(paths.model(definition)).unwrap();
        assert!(sample(
            &mut refresh,
            at,
            true,
            RefreshReason::Tick,
            &paths,
            &orca
        ));
        assert_eq!(read(&refresh, &orca_catalog).primary, PrimaryAction::Use);

        write_model(&paths.root, definition);
        assert!(sample(
            &mut refresh,
            at,
            true,
            RefreshReason::JobFinished,
            &paths,
            &orca
        ));
        let owned = read(&refresh, &orca_catalog);
        assert_eq!(owned.installation, Installation::Legacy);
        assert_eq!(owned.primary, PrimaryAction::AwaitCatalog);
        assert!(owned.owned() && owned.installed());
        assert!(sample(
            &mut refresh,
            at,
            true,
            RefreshReason::Open,
            &paths,
            &orca
        ));
    }
}
